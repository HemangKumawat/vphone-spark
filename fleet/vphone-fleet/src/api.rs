//! The control API: one router over the fleet, and the loop that ticks it.

use std::env;
use std::io::{Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use vphone_api_proxy::{Head, Token, admit, json_string, log, read_head};

use crate::config::{Config, Feature, Paths, Switch};
use crate::fleet::{Fleet, stop_phone};
use crate::phone::CreateSpec;
use crate::{host, phone};

/// The loop's period.
const TICK: Duration = Duration::from_secs(1);
/// The largest request body the control API reads.
const BODY_MAX: usize = 64 * 1024;

/// `%XX` and `+` decoding of one query value.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (b[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| decode(v))
}

fn ok(message: &str) -> (u16, String) {
    (
        200,
        format!("{{\"ok\":true,\"message\":{}}}", json_string(message)),
    )
}

fn refuse(code: u16, message: &str) -> (u16, String) {
    (code, format!("{{\"error\":{}}}", json_string(message)))
}

fn outcome(r: Result<String, String>) -> (u16, String) {
    r.map_or_else(|e| refuse(409, &e), |m| ok(&m))
}

fn bulk(results: &[(String, Result<String, String>)]) -> (u16, String) {
    let all = results.iter().all(|(_, r)| r.is_ok());
    let items: Vec<String> = results
        .iter()
        .map(|(id, r)| match r {
            Ok(m) => format!(
                "{{\"id\":{},\"ok\":true,\"message\":{}}}",
                json_string(id),
                json_string(m)
            ),
            Err(e) => format!(
                "{{\"id\":{},\"ok\":false,\"error\":{}}}",
                json_string(id),
                json_string(e)
            ),
        })
        .collect();
    (
        if all { 200 } else { 207 },
        format!("{{\"results\":[{}]}}", items.join(",")),
    )
}

/// Stops `id` outside the lock: the loop keeps running while it waits for QEMU to exit.
fn stop_one(fleet: &Mutex<Fleet>, id: &str) -> Result<String, String> {
    let (paths, child) = {
        let mut f = lock(fleet);
        (f.paths.clone(), f.forget(id))
    };
    let r = stop_phone(&paths, id);
    if let Some(mut c) = child {
        // Reaps it; it has exited or been killed by now.
        let _ = c.wait();
    }
    r
}

fn lock(fleet: &Mutex<Fleet>) -> std::sync::MutexGuard<'_, Fleet> {
    fleet.lock().expect("fleet lock poisoned")
}

/// The control API: one function from a request to a JSON answer.
pub(crate) fn route(
    fleet: &Mutex<Fleet>,
    method: &str,
    path: &str,
    query: &str,
    body: &str,
) -> (u16, String) {
    let (paths, config) = {
        let f = lock(fleet);
        (f.paths.clone(), f.config.clone())
    };
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, segs.as_slice()) {
        ("GET", ["phones"]) => (200, format!("[{}]", lock(fleet).list_json().join(","))),
        ("GET", ["phones", id]) => {
            let prefix = format!("{{\"id\":{},", json_string(id));
            let Some(item) = lock(fleet)
                .list_json()
                .into_iter()
                .find(|j| j.starts_with(&prefix))
            else {
                return refuse(404, &format!("no phone {id}"));
            };
            let token = std::fs::read_to_string(paths.run(id).join("api-token")).ok();
            let token = token
                .as_deref()
                .map_or("null".to_owned(), |t| json_string(t.trim()));
            (
                200,
                format!("{},\"api_token\":{token}}}", item.trim_end_matches('}')),
            )
        }
        ("POST", ["phones"]) => {
            let Some(golden) = query_value(query, "from") else {
                return refuse(400, "POST /phones?ids=A,B&from=GOLDEN[&profile=P]");
            };
            let profile = query_value(query, "profile");
            let spec = CreateSpec {
                golden: &golden,
                profile: profile.as_deref(),
            };
            let wanted = query_value(query, "ids").unwrap_or_default();
            let ids = wanted.split(',').filter(|i| !i.is_empty());
            bulk(
                &ids.map(|id| (id.to_owned(), phone::create(&paths, &config, id, &spec)))
                    .collect::<Vec<_>>(),
            )
        }
        ("POST", ["phones", verb @ ("start" | "stop")]) => {
            let ids: Vec<String> = query_value(query, "ids").map_or_else(
                || phone::list(&paths).into_iter().map(|p| p.id).collect(),
                |s| {
                    s.split(',')
                        .filter(|i| !i.is_empty())
                        .map(str::to_owned)
                        .collect()
                },
            );
            let act = |id: &str| {
                if *verb == "start" {
                    lock(fleet).start(id)
                } else {
                    stop_one(fleet, id)
                }
            };
            bulk(
                &ids.into_iter()
                    .map(|id| {
                        let r = act(&id);
                        (id, r)
                    })
                    .collect::<Vec<_>>(),
            )
        }
        ("POST", ["phones", id, action]) => phone_action(fleet, &paths, id, action, query),
        ("DELETE", ["phones", id]) => outcome(phone::delete(
            &paths,
            id,
            query_value(query, "confirm").as_deref(),
        )),
        ("POST", ["goldens"]) => match query_value(query, "dir") {
            Some(dir) => outcome(phone::golden_add(
                &paths,
                Path::new(&dir),
                query_value(query, "name").as_deref(),
            )),
            None => refuse(400, "POST /goldens?dir=DIR[&name=NAME]"),
        },
        ("GET", ["features"]) => (200, lock(fleet).config.features.json()),
        ("PUT", ["features", name]) => put_feature(fleet, name, body),
        _ => refuse(404, &format!("no route {method} {path}")),
    }
}

/// `POST /phones/<id>/<action>`.
fn phone_action(
    fleet: &Mutex<Fleet>,
    paths: &Paths,
    id: &str,
    action: &str,
    query: &str,
) -> (u16, String) {
    let name = || query_value(query, "name").ok_or("needs ?name=NAME".to_owned());
    match action {
        "start" => outcome(lock(fleet).start(id)),
        "stop" => outcome(stop_one(fleet, id)),
        "park" => outcome(lock(fleet).park(id)),
        "wake" => {
            let after = lock(fleet).wake(id);
            match after {
                Ok(Some(after)) => outcome(after.finish()),
                Ok(None) => ok(&format!("phone {id} is awake")),
                Err(e) => refuse(409, &e),
            }
        }
        "snapshots" => outcome(name().and_then(|n| phone::snapshot(paths, id, &n))),
        "restore" => outcome(name().and_then(|n| phone::restore(paths, id, &n))),
        "reset" => outcome(phone::reset(
            paths,
            id,
            query_value(query, "confirm").as_deref(),
        )),
        _ => refuse(
            404,
            &format!("no action {action}: start, stop, park, wake, snapshots, restore, reset"),
        ),
    }
}

/// `PUT /features/<name>` with `{"on": true|false}`: live, never written to the file.
fn put_feature(fleet: &Mutex<Fleet>, name: &str, body: &str) -> (u16, String) {
    let Some(f) = Feature::from_name(name) else {
        return refuse(404, &format!("no feature {name}"));
    };
    let on = match body.split_whitespace().collect::<String>().as_str() {
        "{\"on\":true}" => true,
        "{\"on\":false}" => false,
        _ => return refuse(400, "body: {\"on\": true} or {\"on\": false}"),
    };
    let mut fl = lock(fleet);
    fl.set_feature(f, Switch::from(on));
    (
        200,
        format!(
            "{{\"feature\":\"{name}\",\"on\":{on},\"persisted\":false,\"note\":\"live until vphone-fleet restarts; set config/profiles.toml [features] to keep it\",\"features\":{}}}",
            fl.config.features.json()
        ),
    )
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        207 => "Multi-Status",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Content Too Large",
        _ => "Error",
    }
}

/// One control API connection: token check (the same as each phone's API), then one request.
fn handle(mut client: TcpStream, fleet: &Mutex<Fleet>, token: &Token) {
    let (raw, mut body) = match read_head(&mut client) {
        Ok(x) => x,
        Err(refusal) => {
            if let Some(answer) = refusal.response() {
                let _ = client.write_all(&answer);
            }
            return;
        }
    };
    let mut head = match Head::parse(&raw) {
        Ok(h) => h,
        Err(refusal) => {
            if let Some(answer) = refusal.response() {
                let _ = client.write_all(&answer);
            }
            return;
        }
    };
    if let Err(refusal) = admit(&mut head, token) {
        if let Some(answer) = refusal.response() {
            let _ = client.write_all(&answer);
        }
        return;
    }
    let text = String::from_utf8_lossy(&head.to_bytes()).to_ascii_lowercase();
    let want = text
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let (code, json) = if want > BODY_MAX {
        refuse(413, "request body too large")
    } else {
        if body.len() < want {
            let mut more = vec![0u8; want - body.len()];
            if client.read_exact(&mut more).is_err() {
                return;
            }
            body.extend_from_slice(&more);
        }
        let target = head.target().to_owned();
        let query = target.split_once('?').map_or("", |(_, q)| q);
        route(
            fleet,
            head.method(),
            head.path(),
            query,
            &String::from_utf8_lossy(&body),
        )
    };
    let answer = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}\n",
        reason(code),
        json.len() + 1
    );
    let _ = client.write_all(answer.as_bytes());
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
    let mut fleet = Fleet::new(paths.clone(), config);
    fleet.fast = fast_cpus(paths, &fleet.config.tunables.fast_cpus);
    log!(
        "[fleet] control API http://{} (token in {}), fast cores {:?}",
        listener.local_addr().map_err(|e| e.to_string())?,
        token_file.display(),
        fleet.fast
    );
    log!(
        "[fleet] features:\n{}",
        fleet.config.features.table().trim_end()
    );
    let fleet = Mutex::new(fleet);
    thread::scope(|scope| {
        scope.spawn(|| run_loop(&fleet));
        for conn in listener.incoming() {
            match conn {
                Ok(client) => {
                    let (fleet, token) = (&fleet, &token);
                    if let Err(e) = thread::Builder::new()
                        .spawn_scoped(scope, move || handle(client, fleet, token))
                    {
                        log!("[fleet] no thread for a connection: {e}");
                    }
                }
                Err(e) => {
                    log!("[fleet] accept: {e}");
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    });
    Ok(())
}

/// Once a second: the state machine under the lock, then due health checks outside it.
fn run_loop(fleet: &Mutex<Fleet>) {
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
        let restart = fleet
            .lock()
            .expect("fleet lock poisoned")
            .record_health(&results);
        for id in restart {
            log!("[fleet] {id}: /v1/health failing: restarting");
            if let Err(e) = stop_one(fleet, &id).and_then(|_| lock(fleet).start(&id)) {
                log!("[fleet] restarting {id}: {e}");
            }
        }
        thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_decoded() {
        assert_eq!(
            query_value("ids=a,b&from=g%20x", "from").as_deref(),
            Some("g x")
        );
        assert_eq!(query_value("a=1", "b"), None);
        assert_eq!(decode("%2Fsrv%2"), "/srv%2");
    }
}
