//! The first request head: reading it, parsing it, and checking and stripping its token.

use std::fmt::{self, Write as _};
use std::io::{ErrorKind, Read};
use std::net::TcpStream;
use std::time::Instant;

use crate::{HEAD_MAX, HEAD_TIMEOUT, HOST_SCREEN, Refusal, TOKEN_HINT, Token};

/// Reads a connection's first request head: through its blank line, at most 16 KiB, within 10 s.
///
/// Returns the head, ending in `\r\n\r\n`, and any bytes the client sent after it. The read
/// timeout is cleared again on success.
///
/// # Errors
///
/// [`Refusal::Unauthorized`] for a head over 16 KiB, [`Refusal::Timeout`], or
/// [`Refusal::ClientGone`].
pub fn read_head(client: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), Refusal> {
    let deadline = Instant::now() + HEAD_TIMEOUT;
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > HEAD_MAX {
                break;
            }
            let early = buf.split_off(end + 4);
            client.set_read_timeout(None).map_err(Refusal::ClientGone)?;
            return Ok((buf, early));
        }
        if buf.len() >= HEAD_MAX {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Refusal::Timeout);
        }
        client
            .set_read_timeout(Some(left))
            .map_err(Refusal::ClientGone)?;
        match client.read(&mut chunk) {
            Ok(0) => return Err(Refusal::ClientGone(ErrorKind::UnexpectedEof.into())),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(Refusal::Timeout);
            }
            Err(e) => return Err(Refusal::ClientGone(e)),
        }
    }
    Err(Refusal::Unauthorized(format!(
        "request head over {} KiB",
        HEAD_MAX / 1024
    )))
}

/// A parsed HTTP/1.1 request head. Its `Debug` shows header names only: values can hold the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Head {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
}

impl fmt::Debug for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Head")
            .field("method", &self.method)
            .field("path", &self.path())
            .field("headers", &names)
            .finish_non_exhaustive()
    }
}

impl Head {
    /// Parses a head as [`read_head`] returns it, refusing what the Mac proxy refuses: bare CR or
    /// LF, control bytes, folded lines and a space before a header colon. Bytes that are not
    /// UTF-8 are refused too.
    ///
    /// # Errors
    ///
    /// [`Refusal::Unauthorized`] naming the first malformed part.
    pub fn parse(raw: &[u8]) -> Result<Self, Refusal> {
        let bad = |why: &str| Refusal::Unauthorized(format!("malformed request head: {why}"));
        let text = raw
            .strip_suffix(b"\r\n\r\n")
            .ok_or_else(|| bad("no blank line at the end"))?;
        let text = std::str::from_utf8(text).map_err(|_| bad("bytes that are not UTF-8"))?;
        let mut lines = text.split("\r\n");
        let request = lines.next().unwrap_or_default();
        let ctl = |c: char| c.is_ascii_control() && c != '\t';
        if request.chars().any(|c| c.is_ascii_control()) {
            return Err(bad(
                "a bare CR or LF, or a control byte, in the request line",
            ));
        }
        let token_char = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        let parts: Vec<&str> = request.split(' ').collect();
        let [method, target, version] = parts[..] else {
            return Err(bad("the request line is not METHOD TARGET HTTP/1.x"));
        };
        if method.is_empty() || !method.chars().all(token_char) {
            return Err(bad("the method"));
        }
        if target.is_empty() {
            return Err(bad("the request target"));
        }
        if !version.starts_with("HTTP/1.") {
            return Err(bad("not HTTP/1.x"));
        }
        let mut headers = Vec::new();
        for line in lines {
            if line.contains(['\r', '\n']) {
                return Err(bad("a bare CR or LF"));
            }
            if line.starts_with([' ', '\t']) {
                return Err(bad("a folded header line"));
            }
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| bad("a header line without a colon"))?;
            if name.is_empty() || !name.chars().all(token_char) {
                return Err(bad("a header name (a space before the colon?)"));
            }
            if value.chars().any(ctl) {
                return Err(bad("a control byte in a header value"));
            }
            headers.push((name.to_owned(), value.trim_matches([' ', '\t']).to_owned()));
        }
        Ok(Self {
            method: method.to_owned(),
            target: target.to_owned(),
            version: version.to_owned(),
            headers,
        })
    }

    /// Whether the client asks to switch protocols (a WebSocket).
    pub fn is_upgrade(&self) -> bool {
        self.headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("upgrade"))
    }

    /// The first header named `name` (any case), if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    /// The request target, with its query.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Replaces the request target, as a router does when it strips its own prefix.
    /// Drops every `vphone-token.*` subprotocol, and the header when none is left: for a gateway
    /// whose token must not reach the guest (`admit` leaves them for vphoned to select).
    pub fn strip_token_protocols(&mut self) {
        for (name, value) in &mut self.headers {
            if name.eq_ignore_ascii_case("sec-websocket-protocol") {
                let kept: Vec<&str> = value
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty() && !p.starts_with("vphone-token."))
                    .collect();
                *value = kept.join(", ");
            }
        }
        self.headers.retain(|(name, value)| {
            !(name.eq_ignore_ascii_case("sec-websocket-protocol") && value.is_empty())
        });
    }

    pub fn set_target(&mut self, target: String) {
        self.target = target;
    }

    /// The request target without its query.
    pub fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(&self.target, |(path, _)| path)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!("{} {} {}\r\n", self.method, self.target, self.version);
        for (name, value) in &self.headers {
            write!(out, "{name}: {value}\r\n").expect("writing to a String cannot fail");
        }
        out.push_str("\r\n");
        out.into_bytes()
    }
}

/// Checks the head's token and strips it so the guest never sees it.
///
/// The token may come as `Authorization: Bearer`, a `token` query item, or a
/// `Sec-WebSocket-Protocol` value `vphone-token.<token>` (left in place: vphoned selects it in
/// its 101 reply). Every `Authorization` header and `token` query item is removed, and `Host`
/// becomes `localhost`, which vphoned's Host check accepts whatever address the client used.
///
/// # Errors
///
/// [`Refusal::Unauthorized`] for no token or a wrong one, then [`Refusal::HostScreen`].
pub fn admit(head: &mut Head, token: &Token) -> Result<(), Refusal> {
    let mut presented = Vec::new();
    head.headers.retain(|(name, value)| {
        if !name.eq_ignore_ascii_case("authorization") {
            return true;
        }
        match value.split_once(' ') {
            Some((scheme, t)) if scheme.eq_ignore_ascii_case("bearer") => {
                presented.push(t.trim().to_owned());
            }
            _ => {}
        }
        false
    });
    for (name, value) in &mut head.headers {
        if name.eq_ignore_ascii_case("sec-websocket-protocol") {
            let offered = value
                .split(',')
                .filter_map(|p| p.trim().strip_prefix("vphone-token."));
            presented.extend(offered.map(str::to_owned));
        } else if name.eq_ignore_ascii_case("host") {
            "localhost".clone_into(value);
        }
    }
    if let Some((path, query)) = head.target.split_once('?') {
        let mut kept = Vec::new();
        for item in query.split('&') {
            match item.strip_prefix("token=") {
                Some(t) => presented.push(t.to_owned()),
                None => kept.push(item),
            }
        }
        head.target = if kept.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{}", kept.join("&"))
        };
    }
    if presented.is_empty() {
        return Err(Refusal::Unauthorized(format!("no token ({TOKEN_HINT})")));
    }
    if !presented.iter().any(|t| token.matches(t)) {
        return Err(Refusal::Unauthorized(
            "wrong token (it is printed at start, or set with VPHONE_API_TOKEN)".to_owned(),
        ));
    }
    if head.path() == HOST_SCREEN {
        return Err(Refusal::HostScreen);
    }
    Ok(())
}
