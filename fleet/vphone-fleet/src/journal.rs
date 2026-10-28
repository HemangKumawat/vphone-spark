//! What happened to the fleet: every phone start, stop, park, wake and failure, kept for `/diag`
//! and sent to every `/events` listener, with the phones' own events beside them.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use vphone_api_proxy::log;

/// The newest records `/diag` shows; older ones are only in the log.
const KEEP: usize = 256;

/// Fleet events that mean something went wrong; `/diag` lists them as failures.
const FAILURES: [&str; 6] = [
    "phone.start_failed",
    "phone.exited",
    "phone.unhealthy",
    "phone.restart_failed",
    "phone.park_failed",
    "phone.wake_failed",
];

#[derive(Debug, Default)]
struct State {
    records: VecDeque<Value>,
    listeners: Vec<Sender<String>>,
    restarts: BTreeMap<String, u32>,
    last_error: BTreeMap<String, Value>,
}

/// The fleet's event log and broadcaster; clones share it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Journal(Arc<Mutex<State>>);

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

impl Journal {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().expect("journal lock poisoned")
    }

    /// Records a fleet event for `phone` (`event` is `phone.<what>`), logs it and sends it to
    /// every listener.
    pub(crate) fn record(&self, phone: &str, event: &str, reason: &str) {
        log!("[fleet] {phone}: {event}: {reason}");
        let record = json!({
            "type": "event",
            "source": "fleet",
            "event": event,
            "phone": phone,
            "time": unix_now(),
            "reason": reason,
            "failure": FAILURES.contains(&event),
        });
        let mut s = self.state();
        if event == "phone.restarted" {
            *s.restarts.entry(phone.to_owned()).or_default() += 1;
        }
        if FAILURES.contains(&event) {
            s.last_error.insert(phone.to_owned(), record.clone());
        }
        if s.records.len() == KEEP {
            s.records.pop_front();
        }
        s.records.push_back(record.clone());
        Self::send(&mut s, &record.to_string());
    }

    /// Sends one message to every listener, dropping the ones that left.
    pub(crate) fn publish(&self, message: &str) {
        Self::send(&mut self.state(), message);
    }

    fn send(s: &mut State, message: &str) {
        s.listeners.retain(|l| l.send(message.to_owned()).is_ok());
    }

    /// A new listener: every message from now on.
    pub(crate) fn subscribe(&self) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        self.state().listeners.push(tx);
        rx
    }

    /// Whether anyone listens (a closed listener counts until the next message).
    pub(crate) fn listening(&self) -> bool {
        !self.state().listeners.is_empty()
    }

    /// The failures among the kept records, newest last.
    pub(crate) fn failures(&self) -> Vec<Value> {
        self.state()
            .records
            .iter()
            .filter(|r| r["failure"] == true)
            .cloned()
            .collect()
    }

    /// How often `phone` was restarted by this process, and its newest failure.
    pub(crate) fn phone(&self, phone: &str) -> (u32, Option<Value>) {
        let s = self.state();
        (
            s.restarts.get(phone).copied().unwrap_or(0),
            s.last_error.get(phone).cloned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_reach_listeners_and_failures_are_kept() {
        let j = Journal::default();
        let rx = j.subscribe();
        j.record("A", "phone.started", "pid 7");
        j.record("A", "phone.exited", "pid 7 exited");
        j.record("A", "phone.restarted", "QEMU exited");
        let first: Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
        assert_eq!(first["event"], "phone.started");
        assert_eq!(first["phone"], "A");
        assert_eq!(first["failure"], false);
        assert_eq!(j.failures().len(), 1);
        let (restarts, last) = j.phone("A");
        assert_eq!(restarts, 1);
        assert_eq!(last.unwrap()["reason"], "pid 7 exited");
        drop(rx);
        j.publish("x");
        assert!(!j.listening(), "a listener that left is dropped");
    }
}
