//! The control API's JSON routes: one router over the fleet (server.rs serves it).

use std::path::Path;
use std::sync::Mutex;

use serde_json::json;
use vphone_api_proxy::json_string;

use crate::config::{Feature, Paths, Switch};
use crate::fleet::{Fleet, stop_phone};
use crate::phone::CreateSpec;
use crate::{diag, gateway, phone, screen};

/// Lines `GET /phones/<id>/logs` gives from each file unless `?tail=N` says otherwise.
const TAIL: usize = 200;

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
pub(crate) fn stop_one(fleet: &Mutex<Fleet>, id: &str) -> Result<String, String> {
    let (paths, child, journal) = {
        let mut f = lock(fleet);
        (f.paths.clone(), f.forget(id), f.journal.clone())
    };
    let r = stop_phone(&paths, id);
    if let Some(mut c) = child {
        // Reaps it; it has exited or been killed by now.
        let _ = c.wait();
    }
    if let Ok(m) = &r {
        journal.record(id, "phone.stopped", m);
    }
    r
}

pub(crate) fn lock(fleet: &Mutex<Fleet>) -> std::sync::MutexGuard<'_, Fleet> {
    fleet.lock().expect("fleet lock poisoned")
}

/// The answer of a route whose feature switch is off.
pub(crate) fn feature_off(f: Feature) -> (u16, String) {
    let name = f.name();
    (
        503,
        json!({ "error": format!("feature {name} is off"), "enable": format!("PUT /features/{name}") }).to_string(),
    )
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
                    .map(|id| (act(&id), id))
                    .map(|(r, id)| (id, r))
                    .collect::<Vec<_>>(),
            )
        }
        ("POST", ["phones", _, "input"]) if !config.features.on(Feature::QmpInput) => {
            feature_off(Feature::QmpInput)
        }
        ("POST", ["phones", id, "input"]) => screen::input(fleet, id, body),
        ("POST", ["phones", id, action]) => phone_action(fleet, &paths, id, action, query),
        ("GET", ["phones", id, "diag"]) => phone::phone_dir(&paths, id).map_or_else(
            |e| refuse(404, &e),
            |_| (200, diag::phone_report(fleet, id).to_string()),
        ),
        ("GET", ["phones", id, "logs"]) => {
            let tail = query_value(query, "tail").and_then(|t| t.parse().ok());
            diag::logs(&paths, id, tail.unwrap_or(TAIL))
                .map_or_else(|e| refuse(404, &e), |v| (200, v.to_string()))
        }
        ("GET", ["diag"]) => (200, diag::report(fleet).to_string()),
        ("POST", ["batch"]) if !config.features.on(Feature::Batch) => feature_off(Feature::Batch),
        ("POST", ["batch"]) => gateway::batch(fleet, body),
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
