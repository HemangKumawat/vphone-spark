//! A client of a running `vphone-fleet serve`, for `vphone-fleet mcp` and `doctor`.

use std::env;
use std::fmt::Write as _;
use std::io::Write as _;
use std::net::{TcpStream, ToSocketAddrs};

use crate::config::Paths;
use crate::host;

/// The serve's address and token.
pub(crate) struct Client {
    addr: String,
    token: String,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// `%XX`-encodes a query value.
pub(crate) fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

impl Client {
    /// `VPHONE_FLEET_URL` (default `http://127.0.0.1:8760`) and `VPHONE_FLEET_TOKEN`, else the
    /// token the serve wrote to `runs/fleet-token`.
    pub(crate) fn find(paths: &Paths, listen: &str) -> Self {
        let url = env::var("VPHONE_FLEET_URL").unwrap_or_else(|_| format!("http://{listen}"));
        let addr = url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_owned();
        let token = env::var("VPHONE_FLEET_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .or_else(|| std::fs::read_to_string(paths.runs.join("fleet-token")).ok())
            .unwrap_or_default()
            .trim()
            .to_owned();
        Self { addr, token }
    }

    #[cfg(test)]
    pub(crate) fn new(addr: &str, token: &str) -> Self {
        Self {
            addr: addr.to_owned(),
            token: token.to_owned(),
        }
    }

    /// One request; `(status, body)`. The answer may take as long as the call does (an app
    /// install takes minutes); only connecting is bounded.
    pub(crate) fn call(
        &self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        let down = |e: &dyn std::fmt::Display| {
            format!(
                "vphone-fleet serve at {}: {e} (is it running? vphone-fleet serve)",
                self.addr
            )
        };
        let addr = self
            .addr
            .to_socket_addrs()
            .map_err(|e| down(&e))?
            .next()
            .ok_or_else(|| down(&"no address"))?;
        let mut s = TcpStream::connect_timeout(&addr, host::CALL_TIMEOUT).map_err(|e| down(&e))?;
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.token,
            body.len()
        );
        s.write_all(head.as_bytes())
            .and_then(|()| s.write_all(body))
            .map_err(|e| down(&e))?;
        host::read_response(&mut s).map_err(|e| down(&e))
    }

    /// [`Self::call`] with a JSON body (or none), the answer as text.
    pub(crate) fn json(
        &self,
        method: &str,
        path: &str,
        body: &str,
    ) -> Result<(u16, String), String> {
        let (code, b) = self.call(method, path, "application/json", body.as_bytes())?;
        Ok((code, String::from_utf8_lossy(&b).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(encode("/var/mobile/a b&c=d"), "/var/mobile/a%20b%26c%3Dd");
    }
}
