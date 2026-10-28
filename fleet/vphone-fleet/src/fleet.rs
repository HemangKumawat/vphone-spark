//! Running phones: start and stop through `tools/phone-up.sh`, health, auto-restart, and the
//! parking loop (ACTIVE, IDLE, PARKED; docs/DENSITY.md "Parking and memory").
//!
//! A phone is managed while `runs/<id>/pid` exists; a vphone-fleet that restarts adopts every
//! phone it finds that way, so the phones never depend on this process staying up.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use vphone_api_proxy::{Upstream, log};

use crate::config::{Config, Feature, Paths, Source, Switch};
use crate::host;
use crate::journal::Journal;
use crate::phone::{self, PhoneToml};

pub(crate) use launch::{AfterWake, stop_phone};
use launch::{is_phone, launch, pid_alive, read_pid};

/// How often a running phone's `/v1/health` is checked. Each check is a request iOS answers, so
/// once a second for every phone would cost CPU on a host that is already CPU-bound.
pub(crate) const HEALTH_EVERY: u64 = 10;
/// Seconds a phone must have run before its death restarts it at once: a phone that dies while
/// booting is restarted at most this often.
const RESTART_BACKOFF: u64 = 60;
/// CPU weights: systemd's default for every phone, ten times that for the phone in use.
const WEIGHT_IDLE: &str = "100";
const WEIGHT_ACTIVE: &str = "1000";

/// Seconds on a monotonic clock that starts with this process.
pub(crate) fn monotonic() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Power {
    /// Used within `idle_secs`: high CPU weight, the fast cores.
    Active,
    /// Running at normal weight on every core.
    Idle,
    /// Paused with QMP `stop`: no CPU; its memory may be reclaimed.
    Parked,
}

impl Power {
    fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Parked => "parked",
        }
    }
}

#[derive(Debug)]
struct Running {
    pid: i32,
    /// Set when this process started it: kept to reap it.
    child: Option<Child>,
    cid: u32,
    cgroup: Option<PathBuf>,
    power: Power,
    last_used: u64,
    started: u64,
    last_ok: u64,
    last_check: u64,
    healthy: Option<bool>,
    /// The proxy's `runs/<id>/activity` time when last seen; a change is a use.
    activity: Option<SystemTime>,
    /// Its exit is recorded: recorded once, not at every tick until it restarts.
    exit_recorded: bool,
    /// Its failing health is recorded, once until it answers again.
    unhealthy_recorded: bool,
}

/// Every phone this process manages, and how it treats them.
#[derive(Debug)]
pub(crate) struct Fleet {
    pub(crate) paths: Paths,
    pub(crate) config: Config,
    /// Injected in tests; [`monotonic`] otherwise.
    pub(crate) clock: fn() -> u64,
    pub(crate) cgroup_root: PathBuf,
    pub(crate) vsock_dir: PathBuf,
    /// The cores an active phone gets (empty: no pinning), and all of them.
    pub(crate) fast: Vec<usize>,
    pub(crate) all: Vec<usize>,
    /// Starts, stops, parks, wakes and failures, for `/diag` and `/events`.
    pub(crate) journal: Journal,
    phones: BTreeMap<String, Running>,
}

impl Fleet {
    pub(crate) fn new(paths: Paths, config: Config) -> Self {
        let all = fs::read_to_string("/sys/devices/system/cpu/online")
            .ok()
            .and_then(|s| host::parse_cpus(&s).ok())
            .unwrap_or_default();
        Self {
            paths,
            config,
            clock: monotonic,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            vsock_dir: std::env::var_os("VSOCK_DIR")
                .map_or_else(|| PathBuf::from("/tmp/vphone-vsock"), PathBuf::from),
            fast: Vec::new(),
            all,
            journal: Journal::default(),
            phones: BTreeMap::new(),
        }
    }

    fn on(&self, f: Feature) -> bool {
        self.config.features.on(f)
    }

    fn upstream(&self, cid: u32) -> Upstream {
        Upstream::new(cid, &self.vsock_dir)
    }

    pub(crate) fn upstream_of(&self, id: &str) -> Option<Upstream> {
        self.phones.get(id).map(|p| self.upstream(p.cid))
    }

    fn track(&mut self, id: &str, pid: i32, child: Option<Child>, cid: u32, power: Power) {
        let now = (self.clock)();
        self.phones.insert(
            id.to_owned(),
            Running {
                pid,
                child,
                cid,
                cgroup: None,
                power,
                last_used: now,
                started: now,
                last_ok: now,
                last_check: 0,
                healthy: None,
                activity: None,
                exit_recorded: false,
                unhealthy_recorded: false,
            },
        );
    }

    /// Starts phone `id` unless it already runs.
    pub(crate) fn start(&mut self, id: &str) -> Result<String, String> {
        let tracked = self.phones.get_mut(id).map(|p| (Self::alive(p), p.pid));
        if let Some((true, pid)) = tracked {
            return Ok(format!("phone {id} already runs (pid {pid})"));
        }
        // Started elsewhere (`vphone-fleet start` in another process) since this one last looked:
        // adopt it rather than launch a second QEMU and orphan the first. shortcut: a launcher
        // caught mid-execve (empty cmdline, microseconds) is not recognised; phone-up.sh's locks
        // still refuse the second QEMU, at the cost of its pid file. Retry next tick if seen.
        if let Some(pid) = read_pid(&self.paths.run(id))
            && tracked.map(|(_, p)| p) != Some(pid)
            && pid_alive(pid)
            && is_phone(pid, id)
        {
            self.forget(id);
            self.adopt();
            return Ok(format!("phone {id} already runs (pid {pid}, adopted)"));
        }
        let (child, cid) = launch(&self.paths, &self.vsock_dir, id)
            .inspect_err(|e| self.journal.record(id, "phone.start_failed", e))?;
        let pid = i32::try_from(child.id()).map_err(|e| e.to_string())?;
        self.track(id, pid, Some(child), cid, Power::Active);
        self.journal
            .record(id, "phone.started", &format!("pid {pid}"));
        Ok(format!("phone {id} started (pid {pid})"))
    }

    /// Stops managing `id`; the caller then stops it with [`stop_phone`], outside the lock.
    pub(crate) fn forget(&mut self, id: &str) -> Option<Child> {
        self.phones.remove(id).and_then(|p| p.child)
    }

    fn alive(p: &mut Running) -> bool {
        match &mut p.child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => pid_alive(p.pid),
        }
    }

    /// Adopts every phone with a live `runs/<id>/pid` that is not managed yet; a paused one comes
    /// back parked.
    pub(crate) fn adopt(&mut self) {
        let Ok(runs) = fs::read_dir(&self.paths.runs) else {
            return;
        };
        for entry in runs.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if self.phones.contains_key(&id) || phone::check_name("phone id", &id).is_err() {
                continue;
            }
            let Some(pid) = read_pid(&entry.path()) else {
                continue;
            };
            let Ok(toml) = PhoneToml::read(&self.paths.phone(&id)) else {
                continue;
            };
            if !pid_alive(pid) || !is_phone(pid, &id) {
                continue;
            }
            let paused = host::qmp(&entry.path().join("qmp.sock"), "query-status")
                .is_ok_and(|s| s.contains("\"paused\""));
            let power = if paused { Power::Parked } else { Power::Idle };
            self.track(&id, pid, None, toml.cid, power);
        }
    }

    fn apply_weight(&self, p: &Running) {
        if !self.on(Feature::CpuPriority) {
            return;
        }
        if let Some(cg) = &p.cgroup {
            let w = if p.power == Power::Active {
                WEIGHT_ACTIVE
            } else {
                WEIGHT_IDLE
            };
            if let Err(e) = host::cgroup_write(cg, "cpu.weight", w) {
                log!("[fleet] {e}");
            }
        }
    }

    fn apply_cpus(&self, p: &Running) {
        if !self.on(Feature::CorePinning) || self.fast.is_empty() {
            return;
        }
        let cpus = if p.power == Power::Active {
            &self.fast
        } else {
            &self.all
        };
        if let Err(e) = host::pin(p.cgroup.as_deref(), p.pid, cpus) {
            log!("[fleet] pid {}: {e}", p.pid);
        }
    }

    fn set_power(&mut self, id: &str, power: Power) {
        if let Some(p) = self.phones.get_mut(id) {
            p.power = power;
        }
        if let Some(p) = self.phones.get(id) {
            self.apply_weight(p);
            self.apply_cpus(p);
        }
    }

    /// Pauses phone `id` (QMP `stop`) and, with `memory_reclaim`, reclaims its memory.
    pub(crate) fn park(&mut self, id: &str) -> Result<String, String> {
        let p = self
            .phones
            .get(id)
            .ok_or(format!("phone {id} is not running"))?;
        if p.power == Power::Parked {
            return Ok(format!("phone {id} is already parked"));
        }
        host::qmp(&self.paths.run(id).join("qmp.sock"), "stop")
            .inspect_err(|e| self.journal.record(id, "phone.park_failed", e))?;
        self.set_power(id, Power::Parked);
        let cgroup = self.phones.get(id).and_then(|p| p.cgroup.clone());
        if self.on(Feature::MemoryReclaim)
            && let Some(cg) = cgroup
        {
            // memory.reclaim blocks while it reclaims: never under the fleet's lock.
            let amount = self.config.tunables.reclaim_on_park.clone();
            let id = id.to_owned();
            thread::spawn(move || {
                // EAGAIN means less than asked could be reclaimed (the rest stays resident), so
                // the outcome is what is left, not the error.
                let result = host::cgroup_write(&cg, "memory.reclaim", &amount)
                    .map_or_else(|e| e, |()| format!("reclaimed {amount}"));
                let left = fs::read_to_string(cg.join("memory.current")).unwrap_or_default();
                log!(
                    "[fleet] {id} parked: {result}; {} MiB resident",
                    left.trim().parse::<u64>().unwrap_or(0) >> 20
                );
            });
        }
        self.journal
            .record(id, "phone.parked", "idle, or past max_awake");
        Ok(format!("phone {id} parked"))
    }

    /// Resumes phone `id` (QMP `cont`) and marks it used; the caller finishes the wake
    /// ([`AfterWake::finish`]) outside the lock. A phone that is not parked is only marked used.
    pub(crate) fn wake(&mut self, id: &str) -> Result<Option<AfterWake>, String> {
        let now = (self.clock)();
        let p = self
            .phones
            .get_mut(id)
            .ok_or(format!("phone {id} is not running"))?;
        p.last_used = now;
        if p.power != Power::Parked {
            if p.power == Power::Idle {
                self.set_power(id, Power::Active);
            }
            return Ok(None);
        }
        p.last_ok = now;
        let cid = p.cid;
        host::qmp(&self.paths.run(id).join("qmp.sock"), "cont")
            .inspect_err(|e| self.journal.record(id, "phone.wake_failed", e))?;
        self.set_power(id, Power::Active);
        self.journal.record(id, "phone.woken", "used while parked");
        Ok(Some(AfterWake {
            id: id.to_owned(),
            upstream: self.upstream(cid),
            wait: Duration::from_secs(self.config.tunables.wake_secs),
            clock_resync: self.on(Feature::ClockResync),
            journal: self.journal.clone(),
        }))
    }

    /// Turns a feature on or off for the running loop, undoing what it did where that is safe.
    pub(crate) fn set_feature(&mut self, f: Feature, value: Switch) {
        self.config.features.set(f, value, Source::Api);
        let ids: Vec<String> = self.phones.keys().cloned().collect();
        for id in ids {
            let Some(p) = self.phones.get(&id) else {
                continue;
            };
            match (f, value) {
                (Feature::Parking, Switch::Off) if p.power == Power::Parked => match self.wake(&id)
                {
                    Ok(after) => after.into_iter().for_each(AfterWake::spawn),
                    Err(e) => log!("[fleet] {e}"),
                },
                (Feature::CpuPriority, Switch::Off) => {
                    if let Some(cg) = &p.cgroup
                        && let Err(e) = host::cgroup_write(cg, "cpu.weight", WEIGHT_IDLE)
                    {
                        log!("[fleet] {e}");
                    }
                }
                (Feature::CorePinning, Switch::Off) if !self.fast.is_empty() => {
                    if let Err(e) = host::pin(p.cgroup.as_deref(), p.pid, &self.all) {
                        log!("[fleet] {e}");
                    }
                }
                (Feature::CpuPriority, Switch::On) => self.apply_weight(p),
                (Feature::CorePinning, Switch::On) => self.apply_cpus(p),
                _ => {}
            }
        }
    }

    /// One pass of the loop: adopt, notice uses, restart the dead, and move phones between
    /// ACTIVE, IDLE and PARKED. Health checks are separate ([`Self::due_checks`]): they wait on
    /// the network and run outside the lock.
    pub(crate) fn tick(&mut self) {
        let now = (self.clock)();
        let t = self.config.tunables.clone();
        self.adopt();
        let ids: Vec<String> = self.phones.keys().cloned().collect();
        for id in &ids {
            let run = self.paths.run(id);
            if !run.join("pid").exists() {
                // Stopped by `vphone-fleet stop` in another process.
                self.forget(id);
                continue;
            }
            let Some(p) = self.phones.get_mut(id) else {
                continue;
            };
            if !Self::alive(p) {
                let pid = p.pid;
                if !p.exit_recorded {
                    p.exit_recorded = true;
                    self.journal.record(
                        id,
                        "phone.exited",
                        &format!(
                            "QEMU (pid {pid}) exited; its last words are in GET /phones/{id}/logs"
                        ),
                    );
                }
                let Some(p) = self.phones.get(id) else {
                    continue;
                };
                if self.config.features.on(Feature::AutoRestart)
                    && now >= p.started + RESTART_BACKOFF
                {
                    match self.start(id) {
                        Ok(_) => self.journal.record(
                            id,
                            "phone.restarted",
                            &format!("QEMU (pid {pid}) exited (auto_restart)"),
                        ),
                        Err(e) => self.journal.record(id, "phone.restart_failed", &e),
                    }
                }
                continue;
            }
            if p.cgroup.is_none() {
                p.cgroup = host::scope_cgroup(&self.cgroup_root, &run, p.pid);
                if p.cgroup.is_some() {
                    let p = &self.phones[id];
                    self.apply_weight(p);
                    self.apply_cpus(p);
                }
            }
            let Some(p) = self.phones.get_mut(id) else {
                continue;
            };
            let seen = fs::metadata(run.join("activity"))
                .and_then(|m| m.modified())
                .ok();
            if seen != p.activity {
                let first = p.activity.is_none();
                p.activity = seen;
                if !first {
                    match self.wake(id) {
                        Ok(after) => after.into_iter().for_each(AfterWake::spawn),
                        Err(e) => log!("[fleet] waking {id} on use: {e}"),
                    }
                }
            }
            let (power, quiet) = {
                let p = &self.phones[id];
                (p.power, now.saturating_sub(p.last_used))
            };
            if power == Power::Active && quiet >= t.idle_secs {
                self.set_power(id, Power::Idle);
            }
            if self.on(Feature::Parking) && power != Power::Parked && quiet >= t.park_secs {
                // A failure is in the journal (phone.park_failed).
                let _ = self.park(id);
            }
        }
        while self.on(Feature::Parking) {
            let awake: Vec<(&String, u64)> = self
                .phones
                .iter()
                .filter(|(_, p)| p.power != Power::Parked)
                .map(|(id, p)| (id, p.last_used))
                .collect();
            if awake.len() <= t.max_awake {
                break;
            }
            let Some(lru) = awake
                .iter()
                .min_by_key(|(_, used)| *used)
                .map(|(id, _)| (*id).clone())
            else {
                break;
            };
            if self.park(&lru).is_err() {
                break;
            }
        }
    }
}

mod launch;
mod status;
#[cfg(test)]
mod tests;
