//! `GET /events`: one WebSocket with every phone's `/v1/events` messages and the fleet's own
//! events (started, stopped, parked, woken, failures), each tagged with its phone id.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write as _};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use vphone_api_proxy::{Head, Upstream, log};

use crate::api::lock;
use crate::fleet::Fleet;
use crate::journal::Journal;
use crate::ws;

/// How often a quiet `/events` connection is pinged, and a quiet watcher checks whether anyone
/// still listens.
const QUIET: Duration = Duration::from_secs(15);

/// Serves one `/events` client until it leaves. `Err` is a refusal to send as plain HTTP.
pub(crate) fn serve(
    client: &TcpStream,
    fleet: &Mutex<Fleet>,
    head: &Head,
) -> Result<(), (u16, String)> {
    let Some(key) = head
        .header("sec-websocket-key")
        .filter(|_| head.is_upgrade())
    else {
        return Err((426, "GET /events is a WebSocket: connect with a WebSocket client (websocat, Python websockets)".to_owned()));
    };
    // A browser fails the handshake unless the token subprotocol it offered is selected.
    let protocol = head
        .header("sec-websocket-protocol")
        .and_then(|p| {
            p.split(',')
                .map(str::trim)
                .find(|p| p.starts_with("vphone-token."))
        })
        .map(|p| format!("Sec-WebSocket-Protocol: {p}\r\n"))
        .unwrap_or_default();
    let journal = lock(fleet).journal.clone();
    let rx = journal.subscribe();
    let writer = Mutex::new(client.try_clone().map_err(|e| (500, e.to_string()))?);
    let send = |op: u8, payload: &[u8]| {
        ws::write_frame(
            &*writer.lock().expect("writer lock poisoned"),
            op,
            payload,
            None,
        )
    };
    write!(
        &*writer.lock().expect("writer lock poisoned"),
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n{protocol}\r\n",
        ws::accept(key)
    )
    .map_err(|e| (500, e.to_string()))?;
    let hello = json!({"type": "event", "source": "fleet", "event": "connected", "phones": lock(fleet).running()});
    if send(ws::TEXT, hello.to_string().as_bytes()).is_err() {
        return Ok(());
    }
    thread::scope(|s| {
        // The client's frames: answer pings; a close or a lost connection ends the stream.
        s.spawn(|| {
            let mut reader = client;
            while let Ok((op, payload)) = ws::read_message(&mut reader) {
                if op == ws::CLOSE {
                    break;
                }
                if op == ws::PING && send(ws::PONG, &payload).is_err() {
                    break;
                }
            }
            let _ = send(ws::CLOSE, &[]);
            let _ = client.shutdown(std::net::Shutdown::Both);
        });
        loop {
            let written = match rx.recv_timeout(QUIET) {
                Ok(message) => send(ws::TEXT, message.as_bytes()),
                Err(RecvTimeoutError::Timeout) => send(ws::PING, &[]),
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if written.is_err() {
                break;
            }
        }
        let _ = client.shutdown(std::net::Shutdown::Both);
    });
    Ok(())
}

/// Opens phone's `/v1/events` WebSocket; returns the stream past the 101 answer.
fn connect(upstream: &Upstream) -> Result<UnixStream, String> {
    let mut nonce = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut nonce))
        .map_err(|e| e.to_string())?;
    let mut s = upstream.connect().map_err(|e| e.to_string())?;
    write!(
        s,
        "GET /v1/events HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        ws::key(nonce)
    )
    .map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(QUIET)).map_err(|e| e.to_string())?;
    // Byte by byte so no frame after the head is read into a buffer and lost.
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 || s.read(&mut byte).map_err(|e| e.to_string())? == 0 {
            return Err(format!(
                "no WebSocket answer: {:?}",
                String::from_utf8_lossy(&head)
            ));
        }
        head.push(byte[0]);
    }
    if !head.starts_with(b"HTTP/1.1 101") {
        return Err(format!("refused: {:?}", String::from_utf8_lossy(&head)));
    }
    Ok(s)
}

/// One phone's `/v1/events` messages into the journal, tagged with its id, until the phone
/// closes the stream or nobody listens.
fn watch(id: &str, upstream: &Upstream, journal: &Journal) -> Result<(), String> {
    let mut s = connect(upstream)?;
    // shortcut: one mask per connection, not per frame; vphoned checks only that frames are masked.
    let mut mask = [0u8; 4];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut mask))
        .map_err(|e| e.to_string())?;
    loop {
        match ws::read_message(&mut s) {
            Ok((ws::TEXT, payload)) => {
                let text = String::from_utf8_lossy(&payload);
                let mut msg = serde_json::from_str::<Value>(&text)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({ "raw": text }));
                msg["phone"] = json!(id);
                msg["source"] = json!("phone");
                journal.publish(&msg.to_string());
            }
            Ok((ws::PING, payload)) => {
                ws::write_frame(&mut s, ws::PONG, &payload, Some(mask))
                    .map_err(|e| e.to_string())?;
            }
            Ok((ws::CLOSE, _)) => return Ok(()),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
        if !journal.listening() {
            let _ = ws::write_frame(&mut s, ws::CLOSE, &[], Some(mask));
            return Ok(());
        }
    }
}

/// Starts a watcher for every healthy, unparked phone that has none, while anyone listens on
/// `/events`; `watching` holds the ids that have one.
pub(crate) fn start_watchers(
    fleet: &'static Mutex<Fleet>,
    watching: &'static Mutex<BTreeSet<String>>,
) {
    let (journal, phones) = {
        let f = lock(fleet);
        (f.journal.clone(), f.watchable())
    };
    if !journal.listening() {
        return;
    }
    for (id, upstream) in phones {
        if !watching
            .lock()
            .expect("watching lock poisoned")
            .insert(id.clone())
        {
            continue;
        }
        let journal = journal.clone();
        let name = id.clone();
        let spawned = thread::Builder::new().spawn(move || {
            if let Err(e) = watch(&id, &upstream, &journal) {
                // Seen by every listener; a tick after QUIET tries again, not every second.
                journal.publish(&json!({"type": "event", "source": "fleet", "event": "phone.events_lost", "phone": id, "reason": e}).to_string());
                thread::sleep(QUIET);
            }
            watching.lock().expect("watching lock poisoned").remove(&id);
        });
        if let Err(e) = spawned {
            log!("[fleet] {name}: no thread to watch its events: {e}");
            watching
                .lock()
                .expect("watching lock poisoned")
                .remove(&name);
        }
    }
}
