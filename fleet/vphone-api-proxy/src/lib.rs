//! Token-checked HTTP/WebSocket gateway to one phone's guest daemon, the Linux `--api-listen`.
//!
//! vphoned serves HTTP/1.1 and WebSocket on guest vsock port 1339 (vphone-cli
//! `Research/vphoned_http_api.md`, "Transport and ownership" and "Access control"). A client's
//! first request head must carry the API token; the proxy strips it, opens one guest connection
//! and copies bytes both ways unchanged. `vphone-fleet` routes its per-phone API through
//! [`read_head`], [`Head`], [`admit`], [`Upstream`] and [`forward`]; [`serve`] is the whole
//! exchange for a listener in front of one phone.

mod head;
mod upstream;

use std::fmt::{self, Write as _};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

pub use head::{Head, admit, read_head};

/// Writes one line to stderr and ignores a failed write.
///
/// `eprintln!` panics when stderr is a closed pipe, and the release profile aborts on panic, so
/// one log line after a reader went away would end a phone's API (or the fleet manager). Every
/// long-running path in these binaries logs through this instead.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stderr(), $($arg)*);
    }};
}
pub use upstream::Upstream;

/// The guest vsock port vphoned listens on.
pub const VPHONED_PORT: u32 = 1339;
/// The largest first request head read before refusing it (the Mac proxy's limit).
const HEAD_MAX: usize = 16 * 1024;
/// How long a client has to send its first request head, and QEMU to answer `CONNECT`.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// The route vphone-cli's Mac app serves itself from the VM display; vphoned has no such route.
const HOST_SCREEN: &str = "/v1/host/screen";
const TOKEN_HINT: &str = "send Authorization: Bearer <token>, a token=<token> query item or the WebSocket subprotocol vphone-token.<token>";

/// The API token: 16 to 256 characters of `A-Z a-z 0-9 - . _ ~`.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A new token: 32 random bytes in hex, as vphone-cli makes one.
    ///
    /// # Errors
    ///
    /// Reading `/dev/urandom` fails.
    pub fn random() -> io::Result<Self> {
        let mut bytes = [0u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let mut hex = String::with_capacity(64);
        for b in bytes {
            write!(hex, "{b:02x}").expect("writing to a String cannot fail");
        }
        Ok(Self(hex))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `presented` is this token, in a time that depends only on the two lengths.
    pub fn matches(&self, presented: &str) -> bool {
        let (ours, theirs) = (self.0.as_bytes(), presented.as_bytes());
        let diff = ours
            .iter()
            .zip(theirs)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        ours.len() == theirs.len() && std::hint::black_box(diff) == 0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

impl FromStr for Token {
    type Err = InvalidTokenError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || "-._~".contains(c);
        if (16..=256).contains(&s.len()) && s.chars().all(allowed) {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidTokenError)
        }
    }
}

/// A token that is not 16 to 256 characters of `A-Z a-z 0-9 - . _ ~`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTokenError;

impl fmt::Display for InvalidTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the API token must be 16 to 256 characters of A-Z a-z 0-9 - . _ ~")
    }
}

impl std::error::Error for InvalidTokenError {}

/// Why a connection was not forwarded; each answer names what failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Refusal {
    /// 401: no token, a wrong one, or a first request head that is too large or malformed.
    Unauthorized(String),
    /// 408: no complete request head within 10 s.
    Timeout,
    /// 501: `/v1/host/screen`, which only vphone-cli's Mac app serves.
    HostScreen,
    /// 502: the phone's vphoned could not be reached.
    Unreachable(String),
    /// The client hung up or failed before a whole head arrived; there is no one to answer.
    ClientGone(io::Error),
}

impl Refusal {
    /// The HTTP response for the client, or `None` when the client is gone.
    pub fn response(&self) -> Option<Vec<u8>> {
        let (status, extra) = match self {
            Self::Unauthorized(_) => ("401 Unauthorized", "WWW-Authenticate: Bearer\r\n"),
            Self::Timeout => ("408 Request Timeout", ""),
            Self::HostScreen => ("501 Not Implemented", ""),
            Self::Unreachable(_) => ("502 Bad Gateway", ""),
            Self::ClientGone(_) => return None,
        };
        let body = format!("{{\"error\":{}}}\n", json_string(&self.to_string()));
        let length = body.len();
        Some(
            format!(
                "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
            )
            .into_bytes(),
        )
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized(why) => write!(f, "401 unauthorized: {why}"),
            Self::Timeout => write!(
                f,
                "408 no complete request head within {} s",
                HEAD_TIMEOUT.as_secs()
            ),
            Self::HostScreen => write!(
                f,
                "501 {HOST_SCREEN} is served only by vphone-cli's Mac app; on Linux take a frame with \
                 POST /v1/rpc {{\"method\":\"screen.screenshot\"}} through this proxy, or use vphone-fleet's screen route"
            ),
            Self::Unreachable(why) => write!(f, "502 phone unreachable: {why}"),
            Self::ClientGone(e) => write!(f, "client gone before a whole request head: {e}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// `s` as a JSON string literal, quotes included.
pub fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => {
                write!(out, "\\u{:04x}", u32::from(c)).expect("writing to a String cannot fail");
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Sends `head` and `early` (bytes that came with the head) to the guest, then copies bytes both
/// ways until the guest closes. Returns the bytes sent (to the guest, to the client).
///
/// A plain request's client may half-close: the guest side stays open, because vphoned drops a
/// half-closed connection before it answers. An upgraded (WebSocket) client's end of input ends
/// the connection, so a tunnel whose client left does not stay open in the guest.
///
/// # Errors
///
/// The first I/O error of either direction; the connection is closed either way.
///
/// # Panics
///
/// If the client-to-guest copy thread panics, which `io::copy` does not.
pub fn forward(
    client: &TcpStream,
    guest: &UnixStream,
    head: &Head,
    early: &[u8],
) -> io::Result<(u64, u64)> {
    // ponytail: one thread per direction, so two per connection; epoll if thousands of clients.
    let upgraded = head.is_upgrade();
    let head = head.to_bytes();
    client.set_nodelay(true)?;
    (&*guest).write_all(&head)?;
    (&*guest).write_all(early)?;
    thread::scope(|scope| {
        let up = thread::Builder::new().spawn_scoped(scope, || {
            let copied = io::copy(&mut &*client, &mut &*guest);
            if copied.is_err() || upgraded {
                // The client is gone: end the guest side so the other direction returns. An error
                // here means the guest is already closed.
                guest.shutdown(Shutdown::Both).ok();
            }
            copied
        })?;
        let down = io::copy(&mut &*guest, &mut &*client);
        // The guest is done, or the client stopped reading. Errors here mean already closed.
        guest.shutdown(Shutdown::Both).ok();
        client.shutdown(Shutdown::Write).ok();
        drain(client);
        client.shutdown(Shutdown::Both).ok();
        let up = up.join().expect("the copy thread does not panic")?;
        let sent = u64::try_from(head.len() + early.len()).expect("a head fits in u64");
        Ok((sent + up, down?))
    })
}

/// Reads and drops what the client still sends (an upload, the rest of a large head) until it
/// closes or 10 s pass: closing a socket with unread bytes resets the connection, and the reset
/// can destroy the answer before the client reads it.
fn drain(client: &TcpStream) {
    let deadline = Instant::now() + HEAD_TIMEOUT;
    let mut sink = [0u8; 16 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || client.set_read_timeout(Some(left)).is_err() {
            return;
        }
        match (&*client).read(&mut sink) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Serves one client of the phone behind `upstream` and returns a log line saying what happened.
pub fn serve(mut client: TcpStream, upstream: &Upstream, token: &Token) -> String {
    let started = Instant::now();
    let mut request = String::from("-");
    let outcome = match open(&mut client, upstream, token, &mut request) {
        Ok((head, early, guest)) => match forward(&client, &guest, &head, &early) {
            Ok((up, down)) => format!("forwarded: {up} B to the phone, {down} B back"),
            Err(e) => format!("forwarded, then failed: {e}"),
        },
        Err(refusal) => {
            if let Some(response) = refusal.response() {
                // A client that already left misses its answer; the log line still has it.
                client.write_all(&response).ok();
                client.shutdown(Shutdown::Write).ok();
                drain(&client);
            }
            refusal.to_string()
        }
    };
    format!(
        "{request} -> {outcome} ({:.2} s)",
        started.elapsed().as_secs_f64()
    )
}

fn open(
    client: &mut TcpStream,
    upstream: &Upstream,
    token: &Token,
    request: &mut String,
) -> Result<(Head, Vec<u8>, UnixStream), Refusal> {
    let (raw, early) = read_head(client)?;
    let mut head = Head::parse(&raw)?;
    // The path alone until the token is out of the query.
    *request = format!("{} {}", head.method(), head.path());
    admit(&mut head, token)?;
    *request = format!("{} {}", head.method(), head.target());
    let guest = upstream.connect()?;
    Ok((head, early, guest))
}
