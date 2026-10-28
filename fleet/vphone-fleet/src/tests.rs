//! The control API end to end: a real server on a loopback port, phones that are fake vphoneds
//! on QEMU's hybrid-vsock Unix sockets and fake QMP sockets; this test process stands in for
//! each phone's QEMU (its `/proc` entry is what `/diag` reads).

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use vphone_api_proxy::Token;

use crate::api::lock;
use crate::client::Client;
use crate::config::{Config, Paths};
use crate::fleet::Fleet;
use crate::{events, mcp, ws};

mod tools;

const TOKEN: &str = "test-token-0123456789abcdef";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-frame";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0fake-jpeg";

/// A QMP server recording each command line but `qmp_capabilities`; `screendump` writes a PNG.
pub(crate) fn fake_qmp(socket: &Path) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Arc::clone(&log);
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { return };
            conn.write_all(b"{\"QMP\": {\"version\": {}}}\n").unwrap();
            let lines = BufReader::new(conn.try_clone().unwrap()).lines();
            for line in lines.map_while(Result::ok) {
                let msg: Value = serde_json::from_str(&line).unwrap();
                let answer = match msg["execute"].as_str().unwrap_or_default() {
                    "query-status" => {
                        "{\"return\": {\"status\": \"running\", \"running\": true}}\n"
                    }
                    "screendump" => {
                        fs::write(msg["arguments"]["filename"].as_str().unwrap(), PNG).unwrap();
                        "{\"return\": {}}\n"
                    }
                    _ => "{\"return\": {}}\n",
                };
                if msg["execute"] != "qmp_capabilities" {
                    seen.lock().unwrap().push(line);
                }
                if conn.write_all(answer.as_bytes()).is_err() {
                    break;
                }
            }
        }
    });
    log
}

/// The `execute` name of each recorded QMP line.
pub(crate) fn names(log: &Mutex<Vec<String>>) -> Vec<String> {
    let lines = log.lock().unwrap();
    lines
        .iter()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["execute"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// A fake vphoned behind QEMU's hybrid vsock: answers each request with what it saw, and
/// `/v1/events` with a WebSocket that sends one `connected` event. Records each request head.
fn fake_vphoned(socket: &Path, cid: u32) -> Arc<Mutex<Vec<String>>> {
    let heads = Arc::new(Mutex::new(Vec::new()));
    let listener = UnixListener::bind(socket).unwrap();
    let seen = Arc::clone(&heads);
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { return };
            let seen = Arc::clone(&seen);
            thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8];
                while !head.ends_with(b"\n") {
                    s.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                assert_eq!(head, b"CONNECT 1339\n");
                s.write_all(b"OK 1073741824\n").unwrap();
                head.clear();
                while !head.ends_with(b"\r\n\r\n") {
                    if s.read(&mut byte).unwrap() == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let text = String::from_utf8(head).unwrap();
                seen.lock().unwrap().push(text.clone());
                let lower = text.to_ascii_lowercase();
                let header = |n: &str| {
                    lower
                        .lines()
                        .find_map(|l| l.strip_prefix(n))
                        .map(|v| v.trim().to_owned())
                };
                let path = text.split(' ').nth(1).unwrap().to_owned();
                if path == "/v1/events" {
                    let key = text
                        .lines()
                        .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
                        .unwrap();
                    write!(s, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept(key.trim())).unwrap();
                    let hello =
                        json!({"type": "event", "event": "connected", "data": {"cid": cid}});
                    ws::write_frame(&s, ws::TEXT, hello.to_string().as_bytes(), None).unwrap();
                    while let Ok((op, _)) = ws::read_message(&s) {
                        if op == ws::CLOSE {
                            break;
                        }
                    }
                    return;
                }
                let n = header("content-length:")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; n];
                s.read_exact(&mut body).unwrap();
                let body = String::from_utf8_lossy(&body).into_owned();
                let answer = if body.contains("device.snapshot") {
                    json!({"type": "response", "result": {"screen": {"locked": false}}})
                } else if body.contains("screen.screenshot") {
                    json!({"type": "response", "result": {"image": BASE64.encode(JPEG), "mime_type": "image/jpeg", "width": 1290}})
                } else {
                    json!({"status": "ok", "cid": cid, "path": path, "host": header("host:"), "authorization": header("authorization:"), "origin": header("origin:"), "body": body})
                };
                let answer = answer.to_string();
                write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).unwrap();
            });
        }
    });
    heads
}

struct Phone {
    qmp: Arc<Mutex<Vec<String>>>,
    heads: Arc<Mutex<Vec<String>>>,
}

struct Env {
    root: PathBuf,
    fleet: &'static Mutex<Fleet>,
    addr: String,
    phones: Vec<Phone>,
}

impl Env {
    /// A server over phones `ids` (cids 10, 11, ...), each running and healthy.
    fn new(ids: &[&str]) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "vphone-e2e-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths {
            fleet: root.join("vms"),
            runs: root.join("runs"),
            root: root.clone(),
        };
        for dir in ["tools", "config", "vsock"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::set_permissions(root.join("vsock"), fs::Permissions::from_mode(0o700)).unwrap();
        // check-host.sh's three kinds of line.
        let script = root.join("tools/check-host.sh");
        fs::write(&script, "#!/bin/sh\necho '  ok aarch64'\necho '  ! /dev/vhost-vsock missing (sudo modprobe vhost_vsock)'\necho '  - zswap N'\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            root.join("config/profiles.toml"),
            "[profile.lite]\nMEM = \"4G\"\n",
        )
        .unwrap();
        let mut fleet = Fleet::new(paths.clone(), Config::default());
        fleet.vsock_dir = root.join("vsock");
        let mut phones = Vec::new();
        for (cid, id) in (10..).zip(ids) {
            make_phone_dir(&paths, id, cid);
            let run = paths.run(id);
            fs::create_dir_all(&run).unwrap();
            fs::write(run.join("pid"), std::process::id().to_string()).unwrap();
            phones.push(Phone {
                qmp: fake_qmp(&run.join("qmp.sock")),
                heads: fake_vphoned(&root.join(format!("vsock/{cid}.sock")), cid),
            });
            fleet.track_for_test(id, cid, Some(true));
        }
        let fleet: &'static Mutex<Fleet> = Box::leak(Box::new(Mutex::new(fleet)));
        let token: &'static Token = Box::leak(Box::new(TOKEN.parse().unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        thread::spawn(move || crate::server::accept(&listener, fleet, token));
        Self {
            root,
            fleet,
            addr,
            phones,
        }
    }

    fn client(&self) -> Client {
        Client::new(&self.addr, TOKEN)
    }

    fn json(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
        let (code, text) = self.client().json(method, path, body).unwrap();
        (
            code,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// One raw request; the whole answer as text.
    fn raw(&self, request: &str) -> String {
        let mut s = TcpStream::connect(&self.addr).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }
}

fn make_phone_dir(paths: &Paths, id: &str, cid: u32) {
    let dir = paths.phone(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("phone.toml"),
        format!(
            "id = \"{id}\"\ngolden = \"g1\"\ngolden_sha256 = \"x\"\ncid = {cid}\napi_port = 0\n"
        ),
    )
    .unwrap();
    for f in crate::phone::STATE {
        fs::write(dir.join(f), b"state").unwrap();
    }
}

#[test]
fn the_gateway_reaches_each_phone_by_id_without_the_token() {
    let env = Env::new(&["A", "B", "C"]);
    for (cid, id) in (10..).zip(["A", "B", "C"]) {
        let (code, body) = env.json("GET", &format!("/phones/{id}/v1/health?x=1"), "");
        assert_eq!(code, 200, "{body}");
        assert_eq!(body["cid"], cid, "{id} reached the wrong phone: {body}");
        assert_eq!(body["path"], "/v1/health?x=1");
        assert_eq!(body["host"], "localhost");
        assert_eq!(body["authorization"], Value::Null, "the token is stripped");
    }
    let (code, body) = env.json("POST", "/phones/A/v1/rpc", "{\"method\":\"apps.list\"}");
    assert_eq!(
        (code, body["body"].as_str()),
        (200, Some("{\"method\":\"apps.list\"}"))
    );
    assert_eq!(
        env.json("GET", "/phones/Z/v1/health", "").0,
        409,
        "not running"
    );
    let (code, body) = env.json("GET", "/phones/A/v1/host/screen", "");
    assert_eq!(code, 501);
    assert!(
        body["error"].as_str().unwrap().contains("/phones/A/screen"),
        "{body}"
    );
}

#[test]
fn no_token_is_401_and_an_origin_is_never_forwarded() {
    let env = Env::new(&["A"]);
    for path in [
        "/phones/A/v1/health",
        "/phones",
        "/diag",
        "/events",
        "/phones/A/screen",
    ] {
        let answer = env.raw(&format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(answer.starts_with("HTTP/1.1 401"), "{path}: {answer}");
    }
    let before = env.phones[0].heads.lock().unwrap().len();
    let answer = env.raw(&format!("GET /phones/A/v1/health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nOrigin: http://evil.example\r\n\r\n"));
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    assert_eq!(
        env.phones[0].heads.lock().unwrap().len(),
        before,
        "nothing reached the phone"
    );
    assert!(
        env.phones[0]
            .heads
            .lock()
            .unwrap()
            .iter()
            .all(|h| !h.to_ascii_lowercase().contains("origin:"))
    );
}

#[test]
fn batch_on_all_answers_for_every_phone_and_can_be_switched_off() {
    let env = Env::new(&["A", "B", "C"]);
    let (code, body) = env.json(
        "POST",
        "/batch",
        r#"{"phones":"all","method":"GET","path":"/v1/health"}"#,
    );
    assert_eq!(code, 200, "{body}");
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    let cids: BTreeSet<u64> = results
        .iter()
        .map(|r| r["body"]["cid"].as_u64().unwrap())
        .collect();
    assert_eq!(cids, BTreeSet::from([10, 11, 12]));
    let (code, body) = env.json(
        "POST",
        "/batch",
        r#"{"phones":["A","Z"],"path":"/v1/health"}"#,
    );
    assert_eq!(code, 207, "{body}");
    assert_eq!(body["results"][1]["status"], 409);
    assert_eq!(
        env.json(
            "POST",
            "/batch",
            r#"{"phones":"all","path":"/v1/x HTTP/1.1\r\nX: y"}"#
        )
        .0,
        400
    );
    assert_eq!(
        env.json("PUT", "/features/batch", r#"{"on": false}"#).0,
        200
    );
    let (code, body) = env.json("POST", "/batch", r#"{"phones":"all","path":"/v1/health"}"#);
    assert_eq!(code, 503);
    assert_eq!(body["enable"], "PUT /features/batch");
}

#[test]
fn events_carry_tagged_messages_from_every_phone_and_the_fleet() {
    let env = Env::new(&["A", "B"]);
    let mut s = TcpStream::connect(&env.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET /events?token={TOKEN} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(
        head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "{head}"
    );
    let next = |s: &mut TcpStream| -> Value {
        let (op, payload) = ws::read_message(s).unwrap();
        assert_eq!(op, ws::TEXT);
        serde_json::from_slice(&payload).unwrap()
    };
    assert_eq!(next(&mut s)["event"], "connected");
    // What the loop does each tick; the listener above is subscribed by now.
    let watching: &'static Mutex<BTreeSet<String>> = Box::leak(Box::default());
    events::start_watchers(env.fleet, watching);
    let mut from = BTreeSet::new();
    while from.len() < 2 {
        let m = next(&mut s);
        assert_eq!(m["source"], "phone", "{m}");
        assert_eq!(m["event"], "connected");
        from.insert(m["phone"].as_str().unwrap().to_owned());
    }
    assert_eq!(from, BTreeSet::from(["A".to_owned(), "B".to_owned()]));
    lock(env.fleet).journal.record("B", "phone.parked", "test");
    let m = next(&mut s);
    assert_eq!(
        (
            m["source"].as_str(),
            m["phone"].as_str(),
            m["event"].as_str()
        ),
        (Some("fleet"), Some("B"), Some("phone.parked"))
    );
}
