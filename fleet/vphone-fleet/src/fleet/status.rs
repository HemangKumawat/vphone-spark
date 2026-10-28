//! What each running phone is doing: health checks, the phone list, and the views `/diag` and
//! `/events` read.

use std::path::PathBuf;

use vphone_api_proxy::{Upstream, json_string};

use super::{Fleet, HEALTH_EVERY, Power};
use crate::config::Feature;
use crate::phone;

/// A running phone as `/diag` reports it.
#[derive(Debug, Clone)]
pub(crate) struct Status {
    pub(crate) pid: i32,
    pub(crate) power: &'static str,
    pub(crate) healthy: Option<bool>,
    pub(crate) last_used_secs_ago: u64,
    pub(crate) upstream: Upstream,
    pub(crate) cgroup: Option<PathBuf>,
}

impl Fleet {
    /// Phone `id` if this process runs it.
    pub(crate) fn status(&self, id: &str) -> Option<Status> {
        let now = (self.clock)();
        self.phones.get(id).map(|p| Status {
            pid: p.pid,
            power: p.power.name(),
            healthy: p.healthy,
            last_used_secs_ago: now.saturating_sub(p.last_used),
            upstream: self.upstream(p.cid),
            cgroup: p.cgroup.clone(),
        })
    }

    /// The ids of every running phone.
    pub(crate) fn running(&self) -> Vec<String> {
        self.phones.keys().cloned().collect()
    }

    /// Unparked phones whose vphoned answered its last health check: those whose `/v1/events`
    /// can be watched without waiting on a guest that is booting or paused.
    pub(crate) fn watchable(&self) -> Vec<(String, Upstream)> {
        self.phones
            .iter()
            .filter(|(_, p)| p.power != Power::Parked && p.healthy == Some(true))
            .map(|(id, p)| (id.clone(), self.upstream(p.cid)))
            .collect()
    }

    /// Tracks a phone this test process stands in for, with its last health check.
    #[cfg(test)]
    pub(crate) fn track_for_test(&mut self, id: &str, cid: u32, healthy: Option<bool>) {
        let pid = i32::try_from(std::process::id()).expect("a pid fits in i32");
        self.track(id, pid, None, cid, Power::Active);
        if let Some(p) = self.phones.get_mut(id) {
            p.healthy = healthy;
        }
    }

    /// Running, unparked phones whose health check is due: `(id, upstream)`.
    pub(crate) fn due_checks(&mut self) -> Vec<(String, Upstream)> {
        let now = (self.clock)();
        let vsock = self.vsock_dir.clone();
        self.phones
            .iter_mut()
            .filter(|(_, p)| p.power != Power::Parked && now >= p.last_check + HEALTH_EVERY)
            .map(|(id, p)| {
                p.last_check = now;
                (id.clone(), Upstream::new(p.cid, &vsock))
            })
            .collect()
    }

    /// Records health results; returns the phones to restart (unhealthy past their boot window).
    pub(crate) fn record_health(&mut self, results: &[(String, bool)]) -> Vec<String> {
        let now = (self.clock)();
        let t = &self.config.tunables;
        let restart = self.config.features.on(Feature::AutoRestart);
        let mut out = Vec::new();
        for (id, ok) in results {
            let Some(p) = self.phones.get_mut(id) else {
                continue;
            };
            if p.power == Power::Parked {
                continue;
            }
            p.healthy = Some(*ok);
            let booted = now >= p.started + t.boot_secs;
            if *ok {
                p.last_ok = now;
                p.unhealthy_recorded = false;
            } else if booted && !p.unhealthy_recorded {
                p.unhealthy_recorded = true;
                self.journal.record(
                    id,
                    "phone.unhealthy",
                    &format!("vphoned stopped answering /v1/health: GET /phones/{id}/diag"),
                );
            }
            if !*ok && restart && booted && now >= p.last_ok + t.unhealthy_secs {
                out.push(id.clone());
            }
        }
        out
    }

    /// Every phone folder with its run-time state, as JSON objects.
    pub(crate) fn list_json(&self) -> Vec<String> {
        let now = (self.clock)();
        phone::list(&self.paths)
            .iter()
            .map(|t| {
                let (state, pid, healthy, used) = match self.phones.get(&t.id) {
                    Some(p) => (
                        p.power.name(),
                        p.pid.to_string(),
                        p.healthy.map_or("null".to_owned(), |h| h.to_string()),
                        now.saturating_sub(p.last_used).to_string(),
                    ),
                    None => ("stopped", "null".into(), "null".into(), "null".into()),
                };
                format!(
                    "{{\"id\":{},\"state\":\"{state}\",\"pid\":{pid},\"healthy\":{healthy},\"last_used_secs_ago\":{used},\"cid\":{},\"api_port\":{},\"golden\":{},\"profile\":{}}}",
                    json_string(&t.id),
                    t.cid,
                    t.api_port,
                    json_string(&t.golden),
                    t.profile.as_deref().map_or("null".to_owned(), json_string)
                )
            })
            .collect()
    }
}
