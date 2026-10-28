//! The control API's server: one token check for every route, then the phone gateway, the
//! event stream, the screen, or the JSON router; and the loop that ticks the fleet.

use std::collections::BTreeSet;
use std::env;
use std::io::{Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde_json::json;
use vphone_api_proxy::{Head, Token, admit, log, read_head};

use crate::api::{feature_off, lock, route, stop_one};
use crate::config::{Config, Feature, Paths};
use crate::fleet::Fleet;
use crate::{events, gateway, host, screen};

/// The loop's period.
const TICK: Duration = Duration::from_secs(1);
/// The largest request body the control API reads.
const BODY_MAX: usize = 64 * 1024;

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        207 => "Multi-Status",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Content Too Large",
        426 => "Upgrade Required",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

fn respond(client: &mut TcpStream, code: u16, content_type: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
        reason(code),
        body.len()
    );
    let _ = client
        .write_all(head.as_bytes())
        .and_then(|()| client.write_all(body));
}

fn respond_json(client: &mut TcpStream, code: u16, json: &str) {
    respond(
        client,
        code,
        "application/json",
        "",
        format!("{json}\n").as_bytes(),
    );
}

fn refuse(client: &mut TcpStream, code: u16, message: &str) {
    respond_json(client, code, &json!({ "error": message }).to_string());
}

/// One control API connection: token check (the same as each phone's API), then one request.
fn handle(mut client: TcpStream, fleet: &Mutex<Fleet>, token: &Token) {
    let parsed = read_head(&mut client).and_then(|(raw, early)| {
        let mut head = Head::parse(&raw)?;
        admit(&mut head, token)?;
        Ok((head, early))
    });
    let (head, mut body) = match parsed {
        Ok(x) => x,
        Err(refusal) => {
            if let Some(answer) = refusal.response() {
                let _ = client.write_all(&answer);
            }
            return;
        }
    };
    // vphoned refuses requests from web pages (an Origin header); so does every route here.
    if head.header("origin").is_some() {
        return refuse(
            &mut client,
            403,
            "requests with an Origin header (from a web page) are refused, as vphoned refuses them",
        );
    }
    let path = head.path().to_owned();
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (head.method(), segs.as_slice()) {
        (_, ["phones", id, "v1", ..]) => {
            if let Err((code, e)) = gateway::forward(&client, fleet, id, head.clone(), &body) {
                refuse(&mut client, code, &e);
            }
            return;
        }
        ("GET", ["events"]) => {
            let off = !lock(fleet).config.features.on(Feature::Events);
            let served = if off {
                Err(feature_off(Feature::Events))
            } else {
                events::serve(&client, fleet, &head)
            };
            if let Err((code, e)) = served {
                respond_json(&mut client, code, &e);
            }
            return;
        }
        ("GET", ["phones", id, "screen"]) => {
            match screen::shot(fleet, id) {
                Ok((kind, image, source)) => respond(
                    &mut client,
                    200,
                    kind,
                    &format!("X-Screen-Source: {source}\r\n"),
                    &image,
                ),
                Err((code, e)) => refuse(&mut client, code, &e),
            }
            return;
        }
        _ => {}
    }
    let want = head
        .header("content-length")
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if want > BODY_MAX {
        return refuse(&mut client, 413, "request body too large");
    }
    if body.len() < want {
        let mut more = vec![0u8; want - body.len()];
        // A client that announces a body and never sends it must not hold this thread.
        let _ = client.set_read_timeout(Some(host::CALL_TIMEOUT));
        if client.read_exact(&mut more).is_err() {
            return;
        }
        body.extend_from_slice(&more);
    }
    let query = head.target().split_once('?').map_or("", |(_, q)| q);
    let (code, json) = route(
        fleet,
        head.method(),
        &path,
        query,
        &String::from_utf8_lossy(&body),
    );
    respond_json(&mut client, code, &json);
}

/// The fast cores for an active phone: a list, or `auto` (`tools/pin-threads.sh --fast-cpus`).
fn fast_cpus(paths: &Paths, setting: &str) -> Vec<usize> {
    let list = if setting == "auto" {
        std::process::Command::new(paths.root.join("tools/pin-threads.sh"))
            .arg("--fast-cpus")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    } else {
        setting.to_owned()
    };
    host::parse_cpus(&list).unwrap_or_else(|e| {
        log!("[fleet] fast_cpus: {e}; core pinning is off");
        Vec::new()
    })
}

/// Answers connections on `listener` for ever, one thread each.
pub(crate) fn accept(listener: &TcpListener, fleet: &'static Mutex<Fleet>, token: &'static Token) {
    for conn in listener.incoming() {
        match conn {
            Ok(client) => {
                if let Err(e) = thread::Builder::new().spawn(move || handle(client, fleet, token)) {
                    log!("[fleet] no thread for a connection: {e}");
                }
            }
            Err(e) => {
                log!("[fleet] accept: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

pub(crate) fn serve(paths: &Paths, config: Config, listen: &str) -> Result<(), String> {
    let token = match env::var("VPHONE_FLEET_TOKEN") {
        Ok(t) if !t.is_empty() => t
            .parse::<Token>()
            .map_err(|e| format!("VPHONE_FLEET_TOKEN: {e}"))?,
        _ => Token::random().map_err(|e| format!("making a token: {e}"))?,
    };
    std::fs::create_dir_all(&paths.runs).map_err(|e| format!("{}: {e}", paths.runs.display()))?;
    // One loop per checkout: two would both restart a phone that dies, from runs/ they share.
    let lock_file = paths.runs.join("fleet.lock");
    let lock =
        std::fs::File::create(&lock_file).map_err(|e| format!("{}: {e}", lock_file.display()))?;
    lock.try_lock().map_err(|e| {
        format!(
            "another vphone-fleet serve holds {} ({e})",
            lock_file.display()
        )
    })?;
    let token_file = paths.runs.join("fleet-token");
    let _ = std::fs::remove_file(&token_file);
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&token_file)
            .and_then(|mut f| f.write_all(token.as_str().as_bytes()))
            .map_err(|e| format!("writing {}: {e}", token_file.display()))?;
    }
    let listener = TcpListener::bind(listen).map_err(|e| format!("listening on {listen}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    if !addr.ip().is_loopback() {
        log!(
            "[fleet] warning: {addr} is not loopback: anyone who can reach it needs only the token, which travels in clear text"
        );
    }
    let mut fleet = Fleet::new(paths.clone(), config);
    fleet.fast = fast_cpus(paths, &fleet.config.tunables.fast_cpus);
    log!(
        "[fleet] control API http://{addr} (token in {}), fast cores {:?}",
        token_file.display(),
        fleet.fast
    );
    log!(
        "[fleet] features:\n{}",
        fleet.config.features.table().trim_end()
    );
    // ponytail: leaked, as serve runs until the process ends; every thread can borrow them.
    let fleet: &'static Mutex<Fleet> = Box::leak(Box::new(Mutex::new(fleet)));
    let token: &'static Token = Box::leak(Box::new(token));
    thread::spawn(move || run_loop(fleet));
    accept(&listener, fleet, token);
    Ok(())
}

/// Once a second: the state machine under the lock, then due health checks outside it, then a
/// `/v1/events` watcher for each phone that needs one.
fn run_loop(fleet: &'static Mutex<Fleet>) {
    let watching: &'static Mutex<BTreeSet<String>> = Box::leak(Box::default());
    loop {
        let due = {
            let mut f = lock(fleet);
            f.tick();
            f.due_checks()
        };
        let results: Vec<(String, bool)> = thread::scope(|s| {
            let checks: Vec<_> = due
                .into_iter()
                .map(|(id, up)| s.spawn(move || (id, host::healthy(&up))))
                .collect();
            checks.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        let (restart, journal) = {
            let mut f = lock(fleet);
            (f.record_health(&results), f.journal.clone())
        };
        for id in restart {
            match stop_one(fleet, &id).and_then(|_| lock(fleet).start(&id)) {
                Ok(_) => journal.record(
                    &id,
                    "phone.restarted",
                    "/v1/health failing past [fleet] unhealthy_secs (auto_restart)",
                ),
                Err(e) => journal.record(&id, "phone.restart_failed", &e),
            }
        }
        if lock(fleet).config.features.on(Feature::Events) {
            events::start_watchers(fleet, watching);
        }
        thread::sleep(TICK);
    }
}
