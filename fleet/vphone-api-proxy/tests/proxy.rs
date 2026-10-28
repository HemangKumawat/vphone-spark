//! The proxy against a fake vphoned on a hybrid-vsock Unix socket, as QEMU's `VSOCK=uds` gives.
#![expect(
    clippy::unwrap_used,
    reason = "in these helpers a panic is the test failure"
)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

use vphone_api_proxy::{Head, Refusal, Token, Upstream, admit, serve};

const TOKEN: &str = "test-token-0123456789abcdef";
const CID: u32 = 7;

fn token() -> Token {
    TOKEN.parse().expect("a valid token")
}

fn head(text: &str) -> Head {
    Head::parse(text.as_bytes()).expect("a valid head")
}

/// A fresh, empty vsock dir for one test.
fn vsock_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "vphone-api-proxy-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fake vphoned behind QEMU's hybrid vsock: answers `CONNECT 1339` with `OK`, then echoes
/// the request head and the bytes after it (up to `body_len`) and closes.
fn fake_vphoned(dir: &Path, body_len: usize) -> thread::JoinHandle<()> {
    let listener = UnixListener::bind(dir.join(format!("{CID}.sock"))).unwrap();
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut line = Vec::new();
        let mut byte = [0u8];
        while line.last() != Some(&b'\n') {
            s.read_exact(&mut byte).unwrap();
            line.push(byte[0]);
        }
        assert_eq!(line, b"CONNECT 1339\n");
        s.write_all(b"OK 1073741824\n").unwrap();
        let mut got = Vec::new();
        let mut chunk = [0u8; 4096];
        while !got.windows(4).any(|w| w == b"\r\n\r\n")
            || got.len() < got.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4 + body_len
        {
            let n = s.read(&mut chunk).unwrap();
            assert_ne!(
                n,
                0,
                "the proxy closed early; got {:?}",
                String::from_utf8_lossy(&got)
            );
            got.extend_from_slice(&chunk[..n]);
        }
        s.write_all(&got).unwrap();
    })
}

/// Sends `request` through a proxy for one connection and returns everything it answered.
fn exchange(dir: &Path, request: &[u8]) -> (String, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = Upstream::new(CID, dir);
    let proxy = thread::spawn(move || serve(listener.accept().unwrap().0, &upstream, &token()));
    let mut client = TcpStream::connect(addr).unwrap();
    client.write_all(request).unwrap();
    let mut answer = String::new();
    client.read_to_string(&mut answer).unwrap();
    // Hang up as a client does once answered, which ends the proxy's drain after a refusal.
    drop(client);
    (answer, proxy.join().unwrap())
}

#[test]
fn token_rules() {
    assert!("short".parse::<Token>().is_err());
    assert!("a".repeat(257).parse::<Token>().is_err());
    assert!("has space in it 123".parse::<Token>().is_err());
    assert!("A-z0.9_~".repeat(2).parse::<Token>().is_ok());
    let t = token();
    assert!(t.matches(TOKEN));
    assert!(!t.matches("test-token-0123456789abcdeF"));
    assert!(!t.matches("test-token-0123456789abcde"));
    let random = Token::random().unwrap();
    assert_eq!(random.as_str().len(), 64);
    assert!(random.as_str().parse::<Token>().is_ok());
    assert!(
        !format!("{t:?}").contains(TOKEN),
        "Debug must not print the token"
    );
}

#[test]
fn malformed_heads_are_refused() {
    for bad in [
        "GET /v1/health HTTP/1.1\r\nHost: x\n\r\n\r\n",
        "GET /v1/health HTTP/1.1\r\nHost: x\rY: z\r\n\r\n",
        "GET /v1/health HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n",
        "GET /v1/health HTTP/1.1\r\nHost : x\r\n\r\n",
        "GET /v1/health HTTP/1.1\r\nHost: x\x01\r\n\r\n",
        "GET  /v1/health HTTP/1.1\r\n\r\n",
        "GET /v1/health\r\n\r\n",
        "GET /v1/health HTTP/1.1\r\n",
    ] {
        assert!(
            matches!(Head::parse(bad.as_bytes()), Err(Refusal::Unauthorized(_))),
            "accepted {bad:?}"
        );
    }
}

#[test]
fn admit_strips_every_token_form() {
    let mut h = head(&format!(
        "GET /v1/health HTTP/1.1\r\nHost: 10.0.0.2:8765\r\nauthorization: bearer {TOKEN}\r\n\r\n"
    ));
    admit(&mut h, &token()).unwrap();
    assert_eq!(
        h.to_bytes(),
        b"GET /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n"
    );

    let mut h = head(&format!(
        "GET /v1/events?a=1&token={TOKEN}&b=2 HTTP/1.1\r\n\r\n"
    ));
    admit(&mut h, &token()).unwrap();
    assert_eq!(h.target(), "/v1/events?a=1&b=2");
    let mut h = head(&format!("GET /v1/events?token={TOKEN} HTTP/1.1\r\n\r\n"));
    admit(&mut h, &token()).unwrap();
    assert_eq!(h.target(), "/v1/events");

    let ws = format!(
        "GET /v1/events HTTP/1.1\r\nSec-WebSocket-Protocol: chat, vphone-token.{TOKEN}\r\n\r\n"
    );
    let mut h = head(&ws);
    admit(&mut h, &token()).unwrap();
    assert_eq!(
        h.to_bytes(),
        ws.as_bytes(),
        "vphoned selects the subprotocol, so it stays"
    );
}

#[test]
fn admit_refuses_missing_and_wrong_tokens() {
    let mut h = head("GET /v1/health HTTP/1.1\r\nAuthorization: Basic Zm9v\r\n\r\n");
    assert!(
        matches!(admit(&mut h, &token()), Err(Refusal::Unauthorized(m)) if m.starts_with("no token"))
    );
    let mut h = head("GET /v1/health?token=nope-nope-nope-nope HTTP/1.1\r\n\r\n");
    assert!(
        matches!(admit(&mut h, &token()), Err(Refusal::Unauthorized(m)) if m.starts_with("wrong token"))
    );
    let mut h = head("GET /v1/host/screen HTTP/1.1\r\n\r\n");
    assert!(
        matches!(admit(&mut h, &token()), Err(Refusal::Unauthorized(_))),
        "the token comes before 501"
    );
    let mut h = head(&format!(
        "GET /v1/host/screen?token={TOKEN} HTTP/1.1\r\n\r\n"
    ));
    assert!(matches!(admit(&mut h, &token()), Err(Refusal::HostScreen)));
}

#[test]
fn without_the_token_the_guest_is_never_opened() {
    let dir = vsock_dir();
    // No fake vphoned: a forwarded request would fail with 502, not 401.
    let (answer, log) = exchange(&dir, b"GET /v1/health HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert!(
        answer.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "{answer}"
    );
    assert!(answer.contains("WWW-Authenticate: Bearer"), "{answer}");
    assert!(answer.ends_with("\r\n\r\n{\"error\":\"401 unauthorized: no token (send Authorization: Bearer <token>, a token=<token> query item or the WebSocket subprotocol vphone-token.<token>)\"}\n"), "{answer}");
    assert!(
        log.starts_with("GET /v1/health -> 401 unauthorized: no token"),
        "{log}"
    );
}

#[test]
fn with_the_token_bytes_reach_the_guest_and_come_back() {
    let dir = vsock_dir();
    let guest = fake_vphoned(&dir, 7);
    let request = format!(
        "PUT /v1/files/content?token={TOKEN}&path=/tmp/x HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: 7\r\n\r\npayload"
    );
    let (answer, log) = exchange(&dir, request.as_bytes());
    guest.join().unwrap();
    assert_eq!(
        answer,
        "PUT /v1/files/content?path=/tmp/x HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\n\r\npayload"
    );
    assert!(
        log.starts_with("PUT /v1/files/content?path=/tmp/x -> forwarded"),
        "{log}"
    );
    assert!(
        !log.contains(TOKEN),
        "the log must not carry the token: {log}"
    );
}

#[test]
fn a_body_sent_after_the_head_is_forwarded_too() {
    let dir = vsock_dir();
    let guest = fake_vphoned(&dir, 5000);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = Upstream::new(CID, &dir);
    let proxy = thread::spawn(move || serve(listener.accept().unwrap().0, &upstream, &token()));
    let mut client = TcpStream::connect(addr).unwrap();
    client.write_all(format!("POST /v1/rpc HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 5000\r\n\r\n").as_bytes()).unwrap();
    thread::sleep(std::time::Duration::from_millis(50));
    client.write_all(&[b'x'; 5000]).unwrap();
    // A client that half-closes still gets its answer: the guest side stays open.
    client.shutdown(Shutdown::Write).unwrap();
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).unwrap();
    guest.join().unwrap();
    assert!(
        answer.ends_with(&[b'x'; 5000]),
        "{} bytes back",
        answer.len()
    );
    assert!(proxy.join().unwrap().contains("forwarded"));
}

#[test]
fn host_screen_is_501_with_a_hint() {
    let (answer, _) = exchange(
        &vsock_dir(),
        format!("GET /v1/host/screen HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n").as_bytes(),
    );
    assert!(
        answer.starts_with("HTTP/1.1 501 Not Implemented\r\n"),
        "{answer}"
    );
    assert!(answer.contains("screen.screenshot"), "{answer}");
}

#[test]
fn an_unreachable_phone_is_502_saying_why() {
    let dir = vsock_dir();
    // A socket file nobody listens on: a phone that stopped.
    drop(UnixListener::bind(dir.join(format!("{CID}.sock"))).unwrap());
    let ok = format!("GET /v1/health HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n");
    let (answer, log) = exchange(&dir, ok.as_bytes());
    assert!(
        answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{answer}"
    );
    assert!(
        answer.contains("phone unreachable") && answer.contains("7.sock"),
        "{answer}"
    );
    assert!(log.contains("502"), "{log}");

    // QEMU accepts, but the guest port refuses: the hybrid socket closes without "OK".
    let dir = vsock_dir();
    let listener = UnixListener::bind(dir.join(format!("{CID}.sock"))).unwrap();
    let refuser = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut line = [0u8; 13];
        s.read_exact(&mut line).unwrap();
    });
    let (answer, _) = exchange(&dir, ok.as_bytes());
    refuser.join().unwrap();
    assert!(answer.contains("vsock refused"), "{answer}");
}

#[test]
fn an_oversized_head_is_401() {
    let big = format!(
        "GET /v1/health HTTP/1.1\r\nX: {}\r\n\r\n",
        "a".repeat(17 * 1024)
    );
    let (answer, _) = exchange(&vsock_dir(), big.as_bytes());
    assert!(
        answer.starts_with("HTTP/1.1 401 Unauthorized\r\n") && answer.contains("over 16 KiB"),
        "{answer}"
    );
}

#[test]
fn utf8_passes_and_other_bytes_do_not() {
    let mut h = head(&format!(
        "GET /v1/files/content?path=/F\u{f6}to.jpg&token={TOKEN} HTTP/1.1\r\nX-Name: caf\u{e9}\r\n\r\n"
    ));
    admit(&mut h, &token()).unwrap();
    assert_eq!(h.target(), "/v1/files/content?path=/F\u{f6}to.jpg");
    let raw = b"GET /v1/health HTTP/1.1\r\nX: \xff\r\n\r\n";
    assert!(matches!(Head::parse(raw), Err(Refusal::Unauthorized(_))));
}

#[test]
fn head_debug_hides_header_values() {
    let h = head(&format!(
        "GET /v1/events?token={TOKEN} HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
    ));
    let shown = format!("{h:?}");
    assert!(
        !shown.contains(TOKEN) && shown.contains("Authorization"),
        "{shown}"
    );
}

#[test]
fn a_websocket_client_that_leaves_closes_the_guest_side() {
    let dir = vsock_dir();
    let listener = UnixListener::bind(dir.join(format!("{CID}.sock"))).unwrap();
    // A guest that never writes after OK, like an idle tunnel; it returns when the proxy closes it.
    let guest = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"OK 1\n").unwrap();
        let mut sink = Vec::new();
        s.read_to_end(&mut sink).ok();
    });
    let (answer, log) = {
        let ws = format!(
            "GET /v1/events HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let upstream = Upstream::new(CID, &dir);
        let proxy = thread::spawn(move || serve(listener.accept().unwrap().0, &upstream, &token()));
        let mut client = TcpStream::connect(addr).unwrap();
        client.write_all(ws.as_bytes()).unwrap();
        thread::sleep(Duration::from_millis(100));
        client.shutdown(Shutdown::Write).unwrap();
        let mut answer = String::new();
        client.read_to_string(&mut answer).unwrap();
        drop(client);
        (answer, proxy.join().unwrap())
    };
    guest.join().unwrap();
    assert!(answer.is_empty(), "{answer}");
    assert!(log.contains("forwarded"), "{log}");
}

#[test]
fn a_refused_client_that_keeps_trickling_is_cut_off() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = Upstream::new(CID, &vsock_dir());
    let proxy = thread::spawn(move || serve(listener.accept().unwrap().0, &upstream, &token()));
    let mut client = TcpStream::connect(addr).unwrap();
    let started = Instant::now();
    client
        .write_all(b"GET /v1/health HTTP/1.1\r\n\r\n")
        .unwrap();
    // One byte every 2 s after the 401; the drain must end on its own 10 s clock.
    let trickle = thread::spawn(move || {
        for _ in 0..10 {
            thread::sleep(Duration::from_secs(2));
            if client.write_all(b"x").is_err() {
                return;
            }
        }
    });
    let log = proxy.join().unwrap();
    let took = started.elapsed();
    assert!(log.contains("401"), "{log}");
    assert!(
        took < Duration::from_secs(13),
        "the proxy held a refused client for {took:?}"
    );
    trickle.join().unwrap();
}

#[test]
fn a_socket_another_user_could_have_planted_is_not_used() {
    use std::os::unix::fs::PermissionsExt;
    let dir = vsock_dir();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    let _listener = UnixListener::bind(dir.join(format!("{CID}.sock"))).unwrap();
    let ok = format!("GET /v1/health HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n");
    let (answer, _) = exchange(&dir, ok.as_bytes());
    assert!(
        answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n") && answer.contains("not used"),
        "{answer}"
    );
}
