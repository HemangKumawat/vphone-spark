//! One address for every phone: `/phones/<id>/v1/...` reaches that phone's vphoned, and
//! `POST /batch` sends one call to many phones.

use std::net::TcpStream;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use vphone_api_proxy::{Head, Upstream, forward as copy_both_ways, log};

use crate::api::lock;
use crate::fleet::Fleet;
use crate::host;

/// A refusal: an HTTP status and its message.
pub(crate) type Refused = (u16, String);

/// Phone `id`'s vphoned, woken first if it is parked (QMP `cont`, then `/v1/health`); the call
/// counts as a use, so the phone is not parked again until it has been quiet.
pub(crate) fn wake_for_use(fleet: &Mutex<Fleet>, id: &str) -> Result<Upstream, Refused> {
    let (upstream, after) = {
        let mut f = lock(fleet);
        let upstream = f.upstream_of(id).ok_or_else(|| {
            (
                409,
                format!(
                    "phone {id} is not running: POST /phones/{id}/start, then GET /phones/{id}/diag"
                ),
            )
        })?;
        (upstream, f.wake(id).map_err(|e| (502, e))?)
    };
    if let Some(after) = after {
        after.finish().map_err(|e| (504, e))?;
    }
    Ok(upstream)
}

/// Forwards one connection to phone `id`: `/phones/<id>/v1/x` reaches vphoned as `/v1/x`, every
/// byte after the head (a body, a WebSocket) unchanged. The head is admitted already: its token
/// is gone and its `Host` is `localhost`.
pub(crate) fn forward(
    client: &TcpStream,
    fleet: &Mutex<Fleet>,
    id: &str,
    mut head: Head,
    early: &[u8],
) -> Result<(), Refused> {
    let rest = head
        .target()
        .strip_prefix(&format!("/phones/{id}"))
        .filter(|r| r.starts_with("/v1/"))
        .ok_or_else(|| (404, format!("no route {}", head.path())))?
        .to_owned();
    if rest.split('?').next() == Some("/v1/host/screen") {
        return Err((
            501,
            format!(
                "/v1/host/screen is served only by vphone-cli's Mac app: use GET /phones/{id}/screen (PNG)"
            ),
        ));
    }
    let upstream = wake_for_use(fleet, id)?;
    head.set_target(rest);
    // The fleet token opens every phone: a guest never sees it, not even as a subprotocol.
    head.strip_token_protocols();
    let guest = upstream.connect().map_err(|r| (502, r.to_string()))?;
    if let Err(e) = copy_both_ways(client, &guest, &head, early) {
        log!(
            "[fleet] {id} {} {}: forwarded, then failed: {e}",
            head.method(),
            head.path()
        );
    }
    Ok(())
}

/// Whether `path` is a vphoned route that is safe to put in a request line.
pub(crate) fn vphoned_path(path: &str) -> bool {
    path.starts_with("/v1/")
        && !path
            .chars()
            .any(|c| c.is_ascii_whitespace() || c.is_control())
}

/// `POST /batch` `{"phones": [ids] | "all", "method", "path", "body"?, "timeout_secs"?}`: the
/// call on each phone, at most `max_awake` at a time (so `all` wakes no more phones at once than
/// may run), and each phone's answer.
pub(crate) fn batch(fleet: &Mutex<Fleet>, body: &str) -> Refused {
    let usage = "body: {\"phones\": [\"A\", \"B\"] or \"all\", \"method\": \"GET\", \"path\": \"/v1/health\", \"body\": {...}, \"timeout_secs\": 60}";
    let Ok(req) = serde_json::from_str::<Value>(body) else {
        return (400, json!({ "error": usage }).to_string());
    };
    let method = req["method"].as_str().unwrap_or("GET");
    let path = req["path"].as_str().unwrap_or_default();
    if !method.chars().all(|c| c.is_ascii_uppercase()) || !vphoned_path(path) {
        return (
            400,
            json!({ "error": format!("{usage}; method A-Z, path /v1/... with no spaces") })
                .to_string(),
        );
    }
    let ids: Vec<String> = match &req["phones"] {
        Value::String(all) if all == "all" => lock(fleet).running(),
        Value::Array(ids) => ids
            .iter()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect(),
        _ => return (400, json!({ "error": usage }).to_string()),
    };
    let call_body = match &req["body"] {
        Value::Null => String::new(),
        b => b.to_string(),
    };
    let timeout = req["timeout_secs"].as_u64().map(Duration::from_secs);
    let at_once = lock(fleet).config.tunables.max_awake.max(1);
    let mut results = Vec::new();
    for group in ids.chunks(at_once) {
        let answers: Vec<Value> = thread::scope(|s| {
            let calls: Vec<_> = group
                .iter()
                .map(|id| {
                    let call_body = &call_body;
                    s.spawn(move || {
                        let r = wake_for_use(fleet, id).and_then(|up| {
                            host::vphoned(&up, method, path, call_body, timeout).map_err(|e| (502, e))
                        });
                        match r {
                            Ok((status, text)) => json!({
                                "phone": id,
                                "ok": status < 400,
                                "status": status,
                                "body": serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)),
                            }),
                            Err((status, e)) => json!({ "phone": id, "ok": false, "status": status, "error": e }),
                        }
                    })
                })
                .collect();
            calls
                .into_iter()
                .zip(group)
                .map(|(c, id)| {
                    c.join().unwrap_or_else(
                        |_| json!({ "phone": id, "ok": false, "error": "the call panicked" }),
                    )
                })
                .collect()
        });
        results.extend(answers);
    }
    let all_ok = results.iter().all(|r| r["ok"] == true);
    (
        if all_ok { 200 } else { 207 },
        json!({ "results": results }).to_string(),
    )
}
