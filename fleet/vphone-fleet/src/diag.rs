//! Diagnosis in one call: `GET /diag`, `GET /phones/<id>/diag`, `GET /phones/<id>/logs` and
//! `vphone-fleet doctor`. Each check is `{check, status: ok|fail|note|skipped, detail, hint}`;
//! a failed one always says, in plain words, what to do next.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use toml::Table;

use crate::api::lock;
use crate::config::Paths;
use crate::fleet::Fleet;
use crate::host;
use crate::phone::{self, PhoneToml};

mod probe;

/// How long CPU use is sampled; one 100 Hz tick is then a 2 % error.
const CPU_SAMPLE: Duration = Duration::from_millis(500);
/// A phones volume fuller than this fails: the 85 % line the rest of this host keeps.
const DISK_FULL_PERCENT: u64 = 85;
/// Restarts by this process that count as a restart loop.
const RESTART_LOOP: u32 = 3;
/// The most of each log file `/logs` reads, from its end.
const LOG_WINDOW: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// A check that passed, or (with the hint that fixes it) failed.
fn check(name: &str, detail: &str, failure: Option<&str>) -> Value {
    match failure {
        None => json!({ "check": name, "status": "ok", "detail": detail }),
        Some(hint) => json!({ "check": name, "status": "fail", "detail": detail, "hint": hint }),
    }
}

fn note(name: &str, detail: &str) -> Value {
    json!({ "check": name, "status": "note", "detail": detail })
}

fn failed(checks: &[Value]) -> bool {
    checks.iter().any(|c| c["status"] == "fail")
}

/// `tools/check-host.sh --runtime`, one check per line it prints (its `!` lines name the fix).
fn check_host(paths: &Paths) -> Vec<Value> {
    let script = paths.root.join("tools/check-host.sh");
    let out = match Command::new(&script).arg("--runtime").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(e) => {
            let hint =
                "the checkout is incomplete: restore tools/check-host.sh (git checkout tools/)";
            return vec![check(
                "check-host",
                &format!("{}: {e}", script.display()),
                Some(hint),
            )];
        }
    };
    out.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            if let Some(d) = l.strip_prefix("ok ") {
                Some(check("host", d, None))
            } else if let Some(d) = l.strip_prefix("! ") {
                Some(check("host", d, Some(d)))
            } else {
                l.strip_prefix("- ").map(|d| note("host", d))
            }
        })
        .collect()
}

/// The profile MEM of every phone folder, in bytes (run-vphone.sh's default is 8G).
fn planned_ram(paths: &Paths, phones: &[PhoneToml]) -> u64 {
    let profiles: Table = fs::read_to_string(paths.profiles())
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    phones
        .iter()
        .map(|p| {
            let name = p.profile.as_deref().unwrap_or("lite");
            let mem = profiles
                .get("profile")
                .and_then(|t| t.get(name))
                .and_then(|t| t.get("MEM"))
                .and_then(|v| v.as_str())
                .unwrap_or("8G");
            probe::size_bytes(mem).unwrap_or(8 * GIB)
        })
        .sum()
}

/// Every host check: check-host.sh, then what many phones need of the host.
fn host_checks(paths: &Paths, fast: &[usize]) -> Vec<Value> {
    let mut checks = check_host(paths);
    checks.push(note(
        "kvm",
        "not needed: phones run under QEMU TCG (run-vphone.sh -accel tcg), so /dev/kvm plays no part",
    ));
    checks.push(note(
        "fast_cores",
        &if fast.is_empty() {
            "none known: the phone in use is not pinned (core_pinning off, or tools/pin-threads.sh --fast-cpus found none)".to_owned()
        } else {
            format!("the phone in use runs on CPUs {fast:?}")
        },
    ));
    let phones = phone::list(paths);
    let planned = planned_ram(paths, &phones);
    checks.push(match probe::space(Path::new("/dev/shm")) {
        Ok((size, free)) => check(
            "shm",
            &format!(
                "{} phones plan {} GiB of guest RAM; /dev/shm is {} GiB ({} GiB free)",
                phones.len(),
                planned / GIB,
                size / GIB,
                free / GIB
            ),
            (planned > size).then_some("guest RAM lives in /dev/shm, parked phones' too: enlarge it (sudo mount -o remount,size=<N>G /dev/shm), use the lite profile, or keep fewer phones"),
        ),
        Err(e) => check("shm", &e, Some("no /dev/shm: QEMU cannot hold guest RAM there")),
    });
    let ksm = |f: &str| {
        fs::read_to_string(format!("/sys/kernel/mm/ksm/{f}")).map(|s| s.trim().to_owned())
    };
    checks.push(note(
        "ksm",
        &match (ksm("run"), ksm("pages_sharing")) {
            (Ok(run), Ok(sharing)) => format!(
                "run={run}, pages_sharing={sharing}: it merges memory only for GFX=0 phones (docs/DENSITY.md)"
            ),
            _ => "not in this kernel".to_owned(),
        },
    ));
    checks.extend(volume_checks(&paths.fleet));
    checks
}

/// Free space and reflinks on the phones volume.
fn volume_checks(fleet: &Path) -> Vec<Value> {
    let hint = "phones are reflinks of their golden: put FLEET on btrfs or XFS with reflink=1 (tools/phones-volume.sh)";
    if !fleet.is_dir() {
        return vec![check(
            "phones_volume",
            &format!("FLEET {} does not exist", fleet.display()),
            Some(hint),
        )];
    }
    let disk = match probe::space(fleet) {
        Ok((size, free)) => {
            let used = 100 - free.saturating_mul(100) / size.max(1);
            check(
                "disk",
                &format!("{}: {} GiB free of {} GiB ({used} % used)", fleet.display(), free / GIB, size / GIB),
                (used > DISK_FULL_PERCENT).then_some("a full volume stops phones writing their disks: delete old snapshots (vphone-fleet delete / their snapshots folder) or move FLEET"),
            )
        }
        Err(e) => check("disk", &e, Some("FLEET is not readable by this user")),
    };
    let (src, dst) = (
        fleet.join(".reflink-probe"),
        fleet.join(".reflink-probe-copy"),
    );
    let _ = fs::remove_file(&dst);
    let cloned = fs::write(&src, b"probe")
        .map_err(|e| e.to_string())
        .and_then(|()| phone::reflink(&src, &dst));
    let _ = fs::remove_file(&src);
    let _ = fs::remove_file(&dst);
    let reflink = match cloned {
        Ok(()) => check(
            "reflink",
            &format!("{} shares blocks (FICLONE)", fleet.display()),
            None,
        ),
        Err(e) => check("reflink", &e, Some(hint)),
    };
    vec![disk, reflink]
}

/// Searches a JSON value for the first key `name`.
fn find_key<'v>(v: &'v Value, name: &str) -> Option<&'v Value> {
    match v {
        Value::Object(m) => m
            .get(name)
            .or_else(|| m.values().find_map(|x| find_key(x, name))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, name)),
        _ => None,
    }
}

/// One phone's checks and facts.
pub(crate) fn phone_report(fleet: &Mutex<Fleet>, id: &str) -> Value {
    let (status, paths, journal) = {
        let f = lock(fleet);
        (f.status(id), f.paths.clone(), f.journal.clone())
    };
    let toml = PhoneToml::read(&paths.phone(id)).ok();
    let (restarts, last_error) = journal.phone(id);
    let logs = format!("GET /phones/{id}/logs (console.log has QEMU's last words)");
    let mut checks = Vec::new();
    let mut facts = json!({
        "golden": toml.as_ref().map(|t| t.golden.clone()),
        "profile": toml.as_ref().map(|t| t.profile.clone().unwrap_or_else(|| "lite".to_owned())),
        "restarts": restarts,
        "last_error": last_error,
    });
    let alive = status
        .as_ref()
        .is_some_and(|s| probe::cpu_ticks(s.pid).is_some());
    match &status {
        None => checks.push(check(
            "qemu",
            "not running",
            Some(format!("POST /phones/{id}/start; if it stops again, {logs}").as_str()),
        )),
        Some(s) if !alive => checks.push(check(
            "qemu",
            &format!("pid {} is gone", s.pid),
            Some(logs.as_str()),
        )),
        Some(s) => {
            let before = probe::cpu_ticks(s.pid).unwrap_or(0);
            thread::sleep(CPU_SAMPLE);
            let used = probe::cpu_ticks(s.pid).unwrap_or(before) - before;
            let ms = u64::try_from(CPU_SAMPLE.as_millis()).unwrap_or(500);
            facts["pid"] = json!(s.pid);
            facts["state"] = json!(s.power);
            facts["healthy"] = json!(s.healthy);
            facts["uptime_secs"] = json!(probe::uptime_secs(s.pid));
            facts["last_used_secs_ago"] = json!(s.last_used_secs_ago);
            facts["qemu_cpu_percent"] = json!(used * 100_000 / (probe::ticks_per_sec() * ms));
            facts["rss_bytes"] = json!(probe::mem_bytes(s.pid, "Rss"));
            facts["pss_bytes"] = json!(probe::mem_bytes(s.pid, "Pss"));
            facts["cpus"] = json!(probe::proc_field(s.pid, "status", "Cpus_allowed_list"));
            facts["cgroup"] = json!(s.cgroup.as_ref().map(|c| c.display().to_string()));
            checks.push(check("qemu", &format!("pid {}, {}", s.pid, s.power), None));
            checks.extend(guest_checks(&paths, id, s.power, &s.upstream));
        }
    }
    let (mut allocated, mut shared) = (0, 0);
    for f in phone::STATE {
        if let Ok((a, s)) = probe::extents(&paths.phone(id).join(f)) {
            allocated += a;
            shared += s;
        }
    }
    facts["disk"] = json!({ "allocated_bytes": allocated, "shared_bytes": shared, "own_bytes": allocated - shared });
    checks.push(if restarts >= RESTART_LOOP {
        check(
            "restarts",
            &format!("restarted {restarts} times by this vphone-fleet"),
            Some(
                format!("a restart loop: PUT /features/auto_restart {{\"on\":false}}, then {logs}")
                    .as_str(),
            ),
        )
    } else {
        check("restarts", &format!("{restarts} restarts"), None)
    });
    json!({ "phone": id, "ok": !failed(&checks), "checks": checks, "facts": facts })
}

/// QMP, vsock, vphoned and the lock screen of a running phone.
fn guest_checks(
    paths: &Paths,
    id: &str,
    power: &str,
    upstream: &vphone_api_proxy::Upstream,
) -> Vec<Value> {
    let qmp_hint = format!(
        "QMP does not answer: QEMU is hung or still starting; POST /phones/{id}/stop, then start"
    );
    let mut checks = vec![
        match host::qmp(&paths.run(id).join("qmp.sock"), "query-status") {
            Ok(line) => {
                let status = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v["return"]["status"].as_str().map(str::to_owned));
                check(
                    "qmp",
                    &format!("status {}", status.as_deref().unwrap_or("?")),
                    None,
                )
            }
            Err(e) => check("qmp", &e, Some(qmp_hint.as_str())),
        },
    ];
    if power == "parked" {
        let skip = |name: &str| json!({ "check": name, "status": "skipped", "detail": "parked: the next call wakes it" });
        checks.extend(["vsock", "vphoned", "locked"].map(skip));
        return checks;
    }
    let boot = "the guest may still be booting ([fleet] boot_secs), or vphoned crashed: GET /phones/{id}/logs; the guest's own log is GET /phones/{id}/v1/files/content?path=/var/log/vphoned.log";
    let boot = boot.replace("{id}", id);
    match upstream.connect() {
        Ok(_) => checks.push(check("vsock", "guest port 1339 accepts", None)),
        Err(e) => {
            checks.push(check("vsock", &e.to_string(), Some(boot.as_str())));
            return checks;
        }
    }
    match host::vphoned(upstream, "GET", "/v1/health", "", Some(host::CALL_TIMEOUT)) {
        Ok((200, body)) => {
            let v: Value = serde_json::from_str(&body).unwrap_or_default();
            checks.push(check(
                "vphoned",
                &format!("iOS {}, instance {}", v["ios"], v["instance"]),
                None,
            ));
        }
        Ok((code, body)) => checks.push(check(
            "vphoned",
            &format!("/v1/health {code}: {}", body.trim()),
            Some(boot.as_str()),
        )),
        Err(e) => checks.push(check("vphoned", &e, Some(boot.as_str()))),
    }
    let snapshot = host::vphoned(
        upstream,
        "POST",
        "/v1/rpc",
        r#"{"method":"device.snapshot","params":{}}"#,
        Some(host::CALL_TIMEOUT),
    );
    let locked = snapshot
        .ok()
        .and_then(|(_, b)| serde_json::from_str::<Value>(&b).ok())
        .and_then(|v| find_key(&v, "locked").cloned());
    checks.push(note(
        "locked",
        &match locked {
            Some(Value::Bool(true)) => format!(
                "locked: unlock with POST /phones/{id}/v1/rpc {{\"method\":\"screen.unlock\"}}"
            ),
            Some(Value::Bool(false)) => "unlocked".to_owned(),
            _ => "unknown: device.snapshot has no locked field on this vphoned".to_owned(),
        },
    ));
    checks
}

/// `GET /diag`: the host, every phone, and the fleet's recent failures.
pub(crate) fn report(fleet: &Mutex<Fleet>) -> Value {
    let (paths, fast, journal, features) = {
        let f = lock(fleet);
        (
            f.paths.clone(),
            f.fast.clone(),
            f.journal.clone(),
            f.config.features.json(),
        )
    };
    let ids: Vec<String> = phone::list(&paths).into_iter().map(|p| p.id).collect();
    let (host, phones) = thread::scope(|s| {
        let host = s.spawn(|| host_checks(&paths, &fast));
        let phones: Vec<_> = ids
            .iter()
            .map(|id| s.spawn(|| phone_report(fleet, id)))
            .collect();
        (
            host.join().unwrap_or_default(),
            phones
                .into_iter()
                .filter_map(|h| h.join().ok())
                .collect::<Vec<Value>>(),
        )
    });
    let ok = !failed(&host) && phones.iter().all(|p| p["ok"] == true);
    json!({
        "ok": ok,
        "host": host,
        "phones": phones,
        "failures": journal.failures(),
        "features": serde_json::from_str::<Value>(&features).unwrap_or_default(),
    })
}

/// The last `tail` lines of `file`, read from at most its last [`LOG_WINDOW`] bytes.
fn tail_lines(file: &Path, tail: usize) -> std::io::Result<String> {
    let mut f = File::open(file)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(LOG_WINDOW)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    Ok(lines[lines.len().saturating_sub(tail)..].join("\n"))
}

/// `GET /phones/<id>/logs?tail=N`: the end of every `*.log` in `runs/<id>/` and one folder
/// below (QEMU's console, the GPU census).
pub(crate) fn logs(paths: &Paths, id: &str, tail: usize) -> Result<Value, String> {
    phone::check_name("phone id", id)?;
    let run = paths.run(id);
    let mut files: Vec<_> = fs::read_dir(&run)
        .map_err(|e| format!("{}: {e} (the phone never ran here)", run.display()))?
        .flatten()
        .flat_map(|e| {
            if e.path().is_dir() {
                fs::read_dir(e.path())
                    .map(|d| d.flatten().map(|e| e.path()).collect())
                    .unwrap_or_default()
            } else {
                vec![e.path()]
            }
        })
        .filter(|p| p.extension().is_some_and(|x| x == "log") && p.is_file())
        .collect();
    files.sort();
    let out: Vec<Value> = files
        .iter()
        .map(|f| {
            let name = f.strip_prefix(&run).unwrap_or(f).display().to_string();
            match tail_lines(f, tail) {
                Ok(text) => json!({ "file": name, "text": text }),
                Err(e) => json!({ "file": name, "error": e.to_string() }),
            }
        })
        .collect();
    Ok(json!({ "phone": id, "tail": tail, "files": out }))
}

/// Prints a report for a person; whether nothing failed.
pub(crate) fn print(report: &Value, say: impl Fn(&str)) -> bool {
    let line = |c: &Value| {
        let mark = match c["status"].as_str() {
            Some("ok") => "ok  ",
            Some("fail") => "FAIL",
            Some("skipped") => "skip",
            _ => "note",
        };
        let mut l = format!(
            "  {mark} {:<14} {}",
            c["check"].as_str().unwrap_or("?"),
            c["detail"].as_str().unwrap_or("")
        );
        if let Some(h) = c["hint"]
            .as_str()
            .filter(|h| *h != c["detail"].as_str().unwrap_or_default())
        {
            let _ = write!(l, "\n         -> {h}");
        }
        l
    };
    let phone = |p: &Value| {
        say(&format!(
            "phone {} ({})",
            p["phone"].as_str().unwrap_or("?"),
            if p["ok"] == true { "ok" } else { "FAILING" }
        ));
        for c in p["checks"].as_array().into_iter().flatten() {
            say(&line(c));
        }
        say(&format!("  facts: {}", p["facts"]));
    };
    if let Some(host) = report["host"].as_array() {
        say("host");
        for c in host {
            say(&line(c));
        }
        for p in report["phones"].as_array().into_iter().flatten() {
            phone(p);
        }
        for f in report["failures"].as_array().into_iter().flatten() {
            say(&format!(
                "failure: {} {} {}",
                f["phone"], f["event"], f["reason"]
            ));
        }
    } else {
        phone(report);
    }
    report["ok"] == true
}
