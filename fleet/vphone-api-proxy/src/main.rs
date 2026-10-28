//! vphone-api-proxy: the Linux `--api-listen` for one phone (see the crate docs).

use std::env::{self, VarError};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use vphone_api_proxy::{Token, Upstream, serve};

/// How long to wait after a failed accept before the next.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

const USAGE: &str = "\
usage: vphone-api-proxy --listen HOST:PORT --cid CID [--vsock-dir DIR] [--verbose] [--exit-with-parent PID]

Token-checked HTTP/WebSocket gateway to one phone's vphoned (guest vsock port 1339), the Linux
equivalent of vphone-cli's --api-listen. One listener per phone; PORT 0 picks a free port.

The token is VPHONE_API_TOKEN (16 to 256 characters of A-Z a-z 0-9 - . _ ~), else a new random one,
printed at start. Send it as `Authorization: Bearer <token>`, a token=<token> query item, or the
WebSocket subprotocol vphone-token.<token>. GET /v1/host/screen answers 501 (Mac app only).

The phone is reached through DIR/CID.sock when that socket exists (VSOCK=uds), else AF_VSOCK CID:1339.
DIR defaults to $VSOCK_DIR, else /tmp/vphone-vsock (run-vphone.sh's default).

  --verbose           one log line per connection: client, request, outcome, bytes, time
  --exit-with-parent PID  exit when the parent process PID does (tools/phone-up.sh: QEMU)";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vphone-api-proxy: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let mut listen = None;
    let mut cid = None;
    let mut vsock_dir =
        env::var_os("VSOCK_DIR").map_or_else(|| PathBuf::from("/tmp/vphone-vsock"), PathBuf::from);
    let mut verbose = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--listen" => listen = Some(args.next().ok_or("--listen needs HOST:PORT")?),
            "--cid" => {
                cid = Some(
                    args.next()
                        .and_then(|c| c.parse::<u32>().ok())
                        .ok_or("--cid needs an integer")?,
                );
            }
            "--vsock-dir" => vsock_dir = args.next().ok_or("--vsock-dir needs a directory")?.into(),
            "-v" | "--verbose" => verbose = true,
            "--exit-with-parent" => {
                let parent: libc::pid_t = args
                    .next()
                    .and_then(|p| p.parse().ok())
                    .ok_or("--exit-with-parent needs the parent's pid")?;
                // SAFETY: prctl(PR_SET_PDEATHSIG) reads only its integer arguments.
                if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } != 0 {
                    return Err(format!("prctl: {}", std::io::Error::last_os_error()));
                }
                // A parent that died before the prctl sends no signal: check it is still ours.
                // SAFETY: getppid(2) takes no arguments and cannot fail.
                if unsafe { libc::getppid() } != parent {
                    return Err(format!("parent {parent} is gone before the API started"));
                }
            }
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }
    let (Some(listen), Some(cid)) = (listen, cid) else {
        return Err(format!("--listen and --cid are required\n\n{USAGE}"));
    };
    let token = match env::var("VPHONE_API_TOKEN") {
        Ok(t) if !t.is_empty() => t
            .parse::<Token>()
            .map_err(|e| format!("VPHONE_API_TOKEN: {e}"))?,
        Ok(_) | Err(VarError::NotPresent) => {
            Token::random().map_err(|e| format!("making a token: {e}"))?
        }
        Err(VarError::NotUnicode(_)) => return Err("VPHONE_API_TOKEN is not UTF-8".to_owned()),
    };
    let listener = TcpListener::bind(&listen).map_err(|e| format!("listening on {listen}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let upstream = Upstream::new(cid, &vsock_dir);
    eprintln!("[api] HTTP/WebSocket API: http://{addr} -> phone cid {cid} (vphoned port 1339)");
    eprintln!("[api] token: {}", token.as_str());
    eprintln!("[api] send it as: Authorization: Bearer {}", token.as_str());
    if !addr.ip().is_loopback() {
        eprintln!(
            "[api] warning: {addr} is not loopback: whoever reaches it with the token controls the phone as root"
        );
    }
    thread::scope(|scope| {
        for conn in listener.incoming() {
            let client = match conn {
                Ok(client) => client,
                Err(e) => {
                    // Out of fds (EMFILE) fails at once on every call: pause rather than spin.
                    eprintln!("[api] accept: {e}");
                    thread::sleep(ACCEPT_PAUSE);
                    continue;
                }
            };
            let peer = client
                .peer_addr()
                .map_or_else(|_| "?".to_owned(), |a| a.to_string());
            let (upstream, token) = (&upstream, &token);
            // ponytail: no cap on connections; one listener serves one phone, on loopback by default.
            let spawned = thread::Builder::new().spawn_scoped(scope, move || {
                let line = serve(client, upstream, token);
                if verbose {
                    eprintln!("[api] {peer} {line}");
                }
            });
            if let Err(e) = spawned {
                // The client is closed with the closure that held it.
                eprintln!("[api] no thread for a new connection: {e}");
            }
        }
    });
    Ok(())
}
