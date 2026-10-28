//! Starting and stopping one phone's processes, and what a wake leaves to do.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vphone_api_proxy::{Token, Upstream, log};

use crate::config::Paths;
use crate::host;
use crate::phone::{self, PhoneToml};

/// How long a stopped phone gets to exit after SIGTERM before SIGKILL; `tools/boot-test.sh`
/// waits 5 s, and a VM wedged after a GPU fault ignores SIGTERM.
const STOP_GRACE: Duration = Duration::from_secs(10);

pub(super) fn pid_alive(pid: i32) -> bool {
    // The state follows the command name in parentheses; Z is a zombie, already dead.
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(") ").map(|(_, rest)| !rest.starts_with('Z')))
        .unwrap_or(false)
}

/// Whether `pid` is phone `id`'s launcher or QEMU, not a recycled pid.
pub(super) fn is_phone(pid: i32, id: &str) -> bool {
    let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let args: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    let name = format!("phone-{id}");
    args.contains(&name.as_bytes())
        || args
            .windows(2)
            .any(|w| w[0].ends_with(b"phone-up.sh") && w[1] == id.as_bytes())
}

pub(super) fn read_pid(run: &Path) -> Option<i32> {
    // 0 and -1 mean "my group" and "everyone" to kill(2): never a phone's pid.
    fs::read_to_string(run.join("pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|&p: &i32| p > 1)
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Writes a new file only this user can read.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let _ = fs::remove_file(path);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Starts phone `id` with `tools/phone-up.sh`, detached into its own process group: QEMU's
/// console goes to `runs/<id>/console.log`, its API to 127.0.0.1:`api_port` with a new token in
/// `runs/<id>/api-token`, and its pid to `runs/<id>/pid`.
pub(super) fn launch(paths: &Paths, vsock_dir: &Path, id: &str) -> Result<(Child, u32), String> {
    let phone = PhoneToml::read(&phone::phone_dir(paths, id)?)?;
    if phone.api_port == 0 {
        return Err(format!(
            "{id}'s phone.toml has no api_port: add one (a free port), or make the phone with vphone-fleet create"
        ));
    }
    let run = paths.run(id);
    fs::create_dir_all(&run).map_err(|e| format!("creating {}: {e}", run.display()))?;
    let token = Token::random().map_err(|e| format!("making a token: {e}"))?;
    write_private(&run.join("api-token"), token.as_str())?;
    let console = File::options()
        .create(true)
        .append(true)
        .open(run.join("console.log"))
        .map_err(|e| format!("opening {}/console.log: {e}", run.display()))?;
    let console2 = console.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(paths.root.join("tools/phone-up.sh"));
    cmd.arg(id)
        .current_dir(&paths.root)
        .env("FLEET", &paths.fleet)
        .env("VSOCK_DIR", vsock_dir)
        .env("API_LISTEN", format!("127.0.0.1:{}", phone.api_port))
        .env("VPHONE_API_TOKEN", token.as_str())
        .stdin(Stdio::null())
        .stdout(console)
        .stderr(console2)
        // Its own process group: a Ctrl-C meant for vphone-fleet does not reach the phone.
        .process_group(0);
    let proxy = std::env::current_exe()
        .ok()
        .and_then(|e| Some(e.parent()?.join("vphone-api-proxy")))
        .filter(|p| p.is_file());
    if let Some(p) = proxy {
        cmd.env("VPHONE_API_PROXY", p);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("starting tools/phone-up.sh {id}: {e}"))?;
    write_private(&run.join("pid"), &format!("{}\n", child.id()))?;
    Ok((child, phone.cid))
}

/// Stops phone `id` (by its pid file) and waits for it: SIGTERM, then SIGKILL after
/// [`STOP_GRACE`]. Removing the pid file first tells a running vphone-fleet not to restart it.
pub(crate) fn stop_phone(paths: &Paths, id: &str) -> Result<String, String> {
    phone::check_name("phone id", id)?;
    let run = paths.run(id);
    let pid = read_pid(&run);
    let _ = fs::remove_file(run.join("pid"));
    let _ = fs::remove_file(run.join("api-token"));
    let Some(pid) = pid.filter(|p| pid_alive(*p) && is_phone(*p, id)) else {
        return Ok(format!("phone {id} was not running"));
    };
    // SAFETY: kill(2) takes two integers; the pid was just checked to be this phone's.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + STOP_GRACE;
    while pid_alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if pid_alive(pid) {
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        return Ok(format!("phone {id} stopped (SIGKILL after {STOP_GRACE:?})"));
    }
    Ok(format!("phone {id} stopped"))
}

/// What a wake leaves to do outside the fleet's lock: wait for vphoned and set its clock.
#[derive(Debug)]
pub(crate) struct AfterWake {
    pub(super) id: String,
    pub(super) upstream: Upstream,
    pub(super) wait: Duration,
    pub(super) clock_resync: bool,
}

impl AfterWake {
    /// Waits for `/v1/health`, then (with `clock_resync`) sets the guest clock: `stop` froze it,
    /// so iOS's time lags by the pause. vphoned's `time.clock` (`PUT /v1/clock {time}`).
    pub(crate) fn finish(self) -> Result<String, String> {
        let deadline = Instant::now() + self.wait;
        while !host::healthy(&self.upstream) {
            if Instant::now() >= deadline {
                return Err(format!(
                    "{} woke but /v1/health did not answer within {:?}",
                    self.id, self.wait
                ));
            }
            thread::sleep(Duration::from_millis(250));
        }
        if !self.clock_resync {
            return Ok(format!("{} awake", self.id));
        }
        let body = format!("{{\"time\":{:.3}}}", unix_now());
        match host::vphoned(&self.upstream, "PUT", "/v1/clock", &body) {
            Ok((200, answer)) => Ok(format!("{} awake, clock: {}", self.id, answer.trim())),
            Ok((code, answer)) => Err(format!(
                "{} awake, clock not set: {code} {}",
                self.id,
                answer.trim()
            )),
            Err(e) => Err(format!("{} awake, clock not set: {e}", self.id)),
        }
    }

    /// [`Self::finish`] on its own thread, logging the outcome.
    pub(super) fn spawn(self) {
        thread::spawn(move || match self.finish() {
            Ok(m) => log!("[fleet] {m}"),
            Err(e) => log!("[fleet] {e}"),
        });
    }
}
