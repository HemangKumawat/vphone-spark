//! The parking state machine against a fake QMP socket and a folder standing in for the cgroup.

use std::cell::Cell;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use super::*;

thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
}

fn now() -> u64 {
    NOW.with(Cell::get)
}

fn at(t: u64) {
    NOW.with(|n| n.set(t));
}

/// A QMP server recording each command but `qmp_capabilities`.
fn fake_qmp(socket: &Path) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Arc::clone(&log);
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut conn = conn.unwrap();
            conn.write_all(b"{\"QMP\": {\"version\": {}}}\n").unwrap();
            let lines = BufReader::new(conn.try_clone().unwrap()).lines();
            for line in lines {
                let line = line.unwrap();
                let cmd = line.split('"').nth(3).unwrap_or_default().to_owned();
                let answer = if cmd == "query-status" {
                    "{\"return\": {\"status\": \"running\", \"running\": true}}\n"
                } else {
                    "{\"return\": {}}\n"
                };
                if cmd != "qmp_capabilities" {
                    seen.lock().unwrap().push(cmd);
                }
                if conn.write_all(answer.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    log
}

struct Phone {
    qmp: Arc<Mutex<Vec<String>>>,
    cgroup: PathBuf,
}

impl Phone {
    fn weight(&self) -> String {
        fs::read_to_string(self.cgroup.join("cpu.weight")).unwrap_or_default()
    }

    fn commands(&self) -> Vec<String> {
        self.qmp.lock().unwrap().clone()
    }
}

/// A fleet in a fresh temp folder, on the injected clock.
fn fleet() -> Fleet {
    static N: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "vphone-fleet-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    let paths = Paths {
        fleet: root.join("vms"),
        runs: root.join("runs"),
        root,
    };
    let mut config = Config::default();
    config
        .features
        .set(Feature::MemoryReclaim, Switch::On, Source::Default);
    config.tunables.wake_secs = 1;
    let mut f = Fleet::new(paths, config);
    f.clock = now;
    f.vsock_dir = f.paths.root.join("vsock");
    f
}

/// A running phone `id` (this test process stands in for its QEMU), used at the current time.
fn phone(f: &mut Fleet, id: &str) -> Phone {
    let run = f.paths.run(id);
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("pid"), std::process::id().to_string()).unwrap();
    let qmp = fake_qmp(&run.join("qmp.sock"));
    let cgroup = f.paths.root.join(format!("cgroup-{id}"));
    fs::create_dir_all(&cgroup).unwrap();
    f.track(
        id,
        i32::try_from(std::process::id()).unwrap(),
        None,
        10,
        Power::Active,
    );
    f.phones.get_mut(id).unwrap().cgroup = Some(cgroup.clone());
    Phone { qmp, cgroup }
}

fn power(f: &Fleet, id: &str) -> Power {
    f.phones[id].power
}

/// Waits for a file a background thread writes.
fn eventually(path: &Path) -> String {
    for _ in 0..100 {
        if let Ok(s) = fs::read_to_string(path) {
            return s;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("{} was never written", path.display());
}

#[test]
fn active_idle_parked_and_awake_again() {
    let mut f = fleet();
    at(0);
    let a = phone(&mut f, "A");
    f.tick();
    assert_eq!(power(&f, "A"), Power::Active);

    at(59);
    f.tick();
    assert_eq!(power(&f, "A"), Power::Active, "idle_secs is 60");
    at(60);
    f.tick();
    assert_eq!(power(&f, "A"), Power::Idle);
    assert_eq!(a.weight(), "100");
    assert!(
        a.commands().is_empty(),
        "idle is not parked: {:?}",
        a.commands()
    );

    at(300);
    f.tick();
    assert_eq!(power(&f, "A"), Power::Parked);
    assert_eq!(a.commands(), ["stop"]);
    assert_eq!(eventually(&a.cgroup.join("memory.reclaim")), "3G");

    at(301);
    assert!(f.wake("A").unwrap().is_some());
    assert_eq!(power(&f, "A"), Power::Active);
    assert_eq!(a.commands(), ["stop", "cont"]);
    assert_eq!(a.weight(), "1000");
    f.tick();
    assert_eq!(power(&f, "A"), Power::Active, "a wake is a use");
}

#[test]
fn past_max_awake_the_least_recently_used_is_parked() {
    let mut f = fleet();
    f.config.tunables.max_awake = 1;
    at(0);
    let a = phone(&mut f, "A");
    at(5);
    let b = phone(&mut f, "B");
    at(10);
    f.tick();
    assert_eq!(power(&f, "A"), Power::Parked);
    assert_eq!(power(&f, "B"), Power::Active);
    assert_eq!(a.commands(), ["stop"]);
    assert_eq!(b.commands(), Vec::<String>::new());
}

#[test]
fn a_connection_to_its_api_wakes_a_parked_phone() {
    let mut f = fleet();
    at(0);
    let a = phone(&mut f, "A");
    let activity = f.paths.run("A").join("activity");
    let file = File::create(&activity).unwrap();
    f.tick();
    at(300);
    f.tick();
    assert_eq!(a.commands(), ["stop"]);
    // What vphone-api-proxy --activity does at each connection.
    file.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    at(301);
    f.tick();
    assert_eq!(a.commands(), ["stop", "cont"]);
    assert_eq!(power(&f, "A"), Power::Active);
}

#[test]
fn parking_off_through_the_api_resumes_parked_phones_and_parks_no_more() {
    let mut f = fleet();
    at(0);
    let a = phone(&mut f, "A");
    at(300);
    f.tick();
    assert_eq!(a.commands(), ["stop"]);
    let fleet = Mutex::new(f);
    let (code, body) = crate::api::route(&fleet, "PUT", "/features/parking", "", "{\"on\": false}");
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"persisted\":false"), "{body}");
    assert!(
        body.contains("\"parking\":{\"on\":false,\"source\":\"api (not persisted)\"}"),
        "{body}"
    );
    assert_eq!(a.commands(), ["stop", "cont"]);
    let mut f = fleet.into_inner().unwrap();
    at(10_000);
    f.tick();
    assert_eq!(a.commands(), ["stop", "cont"], "parking is off");
    assert_eq!(power(&f, "A"), Power::Idle);
}

#[test]
fn cpu_priority_off_puts_every_phone_back_to_the_default_weight() {
    let mut f = fleet();
    at(0);
    let a = phone(&mut f, "A");
    f.set_power("A", Power::Active);
    assert_eq!(a.weight(), "1000");
    f.set_feature(Feature::CpuPriority, Switch::Off);
    assert_eq!(a.weight(), "100");
    at(60);
    f.tick();
    at(61);
    f.wake("A").unwrap();
    assert_eq!(a.weight(), "100", "cpu_priority is off");
}

#[test]
fn a_phone_stopped_elsewhere_is_forgotten() {
    let mut f = fleet();
    at(0);
    phone(&mut f, "A");
    fs::remove_file(f.paths.run("A").join("pid")).unwrap();
    f.tick();
    assert!(!f.phones.contains_key("A"));
}

#[test]
fn a_phone_started_elsewhere_is_adopted_not_launched_again() {
    let mut f = fleet();
    at(0);
    phone(&mut f, "A");
    let dir = f.paths.phone("A");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("phone.toml"),
        "id = \"A\"\ngolden = \"g\"\ngolden_sha256 = \"x\"\ncid = 10\n",
    )
    .unwrap();
    // The tracked QEMU died, and `vphone-fleet start A` in another process has started a new one.
    let mut dead = Command::new("true").spawn().unwrap();
    dead.wait().unwrap();
    f.phones.get_mut("A").unwrap().pid = i32::try_from(dead.id()).unwrap();
    let mut other = Command::new("sh")
        .args(["-c", "sleep 30; :", "phone-A"]) // `; :` keeps sh: dash execs a lone command
        .spawn()
        .unwrap();
    let pid = i32::try_from(other.id()).unwrap();
    fs::write(f.paths.run("A").join("pid"), pid.to_string()).unwrap();
    // spawn() can return while the child is still in execve, its cmdline not yet set.
    for _ in 0..100 {
        if is_phone(pid, "A") {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    at(1000);
    f.tick();
    let tracked = f.phones["A"].pid;
    other.kill().unwrap();
    other.wait().unwrap();
    assert_eq!(tracked, pid, "the running phone is adopted, not restarted");
}
