//! `vphone-fleet mcp`: the fleet as tools for AI agents, MCP over stdio (newline-delimited
//! JSON-RPC 2.0). Each tool is one call to the running `vphone-fleet serve`.
//!
//! Hand-written rather than the official rmcp crate: that one needs tokio and proc macros, and
//! this server is four methods over a line-by-line loop.

use std::fmt::Write as _;
use std::io::{BufRead, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use vphone_api_proxy::log;

use crate::client::{Client, encode};
use crate::gateway::vphoned_path;
use crate::phone::check_name;

/// The protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Where `install_app` puts the archive in the guest; vphoned deletes it after the install.
const GUEST_DROP: &str = "/var/tmp";

/// Every tool with its JSON Schema.
#[expect(clippy::too_many_lines, reason = "a table: one entry per tool")]
fn tools() -> Value {
    let phone = json!({ "type": "string", "description": "phone id, as list_phones shows it" });
    let ids = json!({ "type": "array", "items": { "type": "string" }, "description": "phone ids; omit for every phone" });
    let num = json!({ "type": "number" });
    let s = json!({ "type": "string" });
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": props, "required": required } });
    json!([
        tool(
            "list_phones",
            "Every phone: id, state (active, idle, parked, stopped), health, API port, golden.",
            json!({}),
            &[]
        ),
        tool(
            "create_phones",
            "Make phones from a golden.",
            json!({ "ids": ids, "from": s, "profile": s }),
            &["ids", "from"]
        ),
        tool(
            "start_phones",
            "Start phones (all when ids is omitted).",
            json!({ "ids": ids }),
            &[]
        ),
        tool(
            "stop_phones",
            "Stop phones (all when ids is omitted).",
            json!({ "ids": ids }),
            &[]
        ),
        tool(
            "screenshot",
            "The phone's screen as a PNG (1290x2796 pixels).",
            json!({ "phone": phone }),
            &["phone"]
        ),
        tool(
            "tap",
            "Tap at screen pixel x,y (of the 1290x2796 screenshot).",
            json!({ "phone": phone, "x": num, "y": num }),
            &["phone", "x", "y"]
        ),
        tool(
            "swipe",
            "Press at x0,y0, drag to x1,y1 over ms milliseconds, release.",
            json!({ "phone": phone, "x0": num, "y0": num, "x1": num, "y1": num, "ms": num }),
            &["phone", "x0", "y0", "x1", "y1"]
        ),
        tool(
            "type_text",
            "Type text on the phone's keyboard (US layout keys).",
            json!({ "phone": phone, "text": s }),
            &["phone", "text"]
        ),
        tool(
            "press_home",
            "Go to the Home Screen.",
            json!({ "phone": phone }),
            &["phone"]
        ),
        tool(
            "install_app",
            "Install an IPA or TIPA from a path on this host.",
            json!({ "phone": phone, "ipa": s }),
            &["phone", "ipa"]
        ),
        tool(
            "launch_app",
            "Launch an app by bundle id.",
            json!({ "phone": phone, "bundle_id": s }),
            &["phone", "bundle_id"]
        ),
        tool(
            "read_file",
            "Read a file in the phone (absolute guest path).",
            json!({ "phone": phone, "path": s }),
            &["phone", "path"]
        ),
        tool(
            "write_file",
            "Write text to a file in the phone (absolute guest path).",
            json!({ "phone": phone, "path": s, "content": s }),
            &["phone", "path", "content"]
        ),
        tool(
            "call_phone",
            "Any vphoned API call (Research/vphoned_http_api.md), e.g. POST /v1/rpc {\"method\":\"apps.list\"}.",
            json!({ "phone": phone, "method": s, "path": s, "body": {} }),
            &["phone", "method", "path"]
        ),
        tool(
            "batch",
            "One vphoned call on many phones at once; phones is a list or \"all\".",
            json!({ "phones": {}, "method": s, "path": s, "body": {} }),
            &["phones", "path"]
        ),
        tool(
            "diagnose",
            "Health checks with a plain-language hint per failure: the host and every phone, or one phone.",
            json!({ "phone": phone }),
            &[]
        ),
        tool(
            "logs",
            "The end of a phone's logs (QEMU console and others).",
            json!({ "phone": phone, "tail": num }),
            &["phone"]
        ),
    ])
}

fn text(t: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": t }], "isError": false })
}

fn error(t: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": t }], "isError": true })
}

/// An HTTP answer as a tool result: any status of 400 or more is an error.
fn answer(r: Result<(u16, String), String>) -> Value {
    match r {
        Ok((code, body)) if code >= 400 => error(&body),
        Ok((_, body)) => text(&body),
        Err(e) => error(&e),
    }
}

fn ids_query(args: &Value) -> String {
    match args["ids"].as_array() {
        Some(ids) => format!(
            "?ids={}",
            encode(
                &ids.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        ),
        None => String::new(),
    }
}

/// Runs one tool.
#[expect(clippy::too_many_lines, reason = "one match arm per tool")]
fn call(client: &Client, name: &str, args: &Value) -> Value {
    let arg = |k: &str| args[k].as_str().unwrap_or_default();
    let phone = arg("phone");
    let needs_phone = !(matches!(
        name,
        "list_phones" | "create_phones" | "start_phones" | "stop_phones" | "batch"
    ) || (name == "diagnose" && phone.is_empty()));
    if needs_phone && let Err(e) = check_name("phone", phone) {
        return error(&e);
    }
    let input = |body: Value| {
        answer(client.json("POST", &format!("/phones/{phone}/input"), &body.to_string()))
    };
    match name {
        "list_phones" => answer(client.json("GET", "/phones", "")),
        "create_phones" => {
            let mut q = format!("{}&from={}", ids_query(args), encode(arg("from")));
            if !arg("profile").is_empty() {
                let _ = write!(q, "&profile={}", encode(arg("profile")));
            }
            answer(client.json("POST", &format!("/phones{q}"), ""))
        }
        "start_phones" | "stop_phones" => {
            let verb = name.trim_end_matches("_phones");
            answer(client.json("POST", &format!("/phones/{verb}{}", ids_query(args)), ""))
        }
        "screenshot" => match client.call(
            "GET",
            &format!("/phones/{phone}/screen"),
            "application/json",
            b"",
        ) {
            Ok((200, png)) => {
                json!({ "content": [{ "type": "image", "data": BASE64.encode(&png), "mimeType": "image/png" }], "isError": false })
            }
            Ok((code, body)) => error(&format!("{code}: {}", String::from_utf8_lossy(&body))),
            Err(e) => error(&e),
        },
        "tap" => input(json!({ "action": "tap", "x": args["x"], "y": args["y"] })),
        "swipe" => input(
            json!({ "action": "swipe", "x0": args["x0"], "y0": args["y0"], "x1": args["x1"], "y1": args["y1"], "ms": args["ms"] }),
        ),
        "type_text" => input(json!({ "action": "text", "text": arg("text") })),
        "press_home" => input(json!({ "action": "home" })),
        "install_app" => {
            // Only an app archive: the tool must not carry any other host file into a phone,
            // where read_file would hand it to the agent. Checked after symlinks resolve.
            let ipa = match std::fs::canonicalize(arg("ipa")) {
                Ok(p) => p,
                Err(e) => return error(&format!("ipa {}: {e}", arg("ipa"))),
            };
            let is_app = ipa.extension().is_some_and(|e| e == "ipa" || e == "tipa");
            let Some(file) = ipa.file_name().and_then(|f| f.to_str()).filter(|_| is_app) else {
                return error("ipa: a path to an .ipa or .tipa file on this host");
            };
            let bytes = match std::fs::read(&ipa) {
                Ok(b) if b.starts_with(b"PK\x03\x04") => b,
                Ok(_) => return error(&format!("{}: not an app archive (a zip)", ipa.display())),
                Err(e) => return error(&format!("{}: {e}", ipa.display())),
            };
            let guest = format!("{GUEST_DROP}/vphone-fleet-{file}");
            let up = client.call(
                "PUT",
                &format!("/phones/{phone}/v1/files/content?path={}", encode(&guest)),
                "application/octet-stream",
                &bytes,
            );
            match up {
                Ok((code, _)) if code < 400 => answer(client.json(
                    "POST",
                    &format!("/phones/{phone}/v1/apps/install"),
                    &json!({ "path": guest }).to_string(),
                )),
                Ok((code, body)) => error(&format!(
                    "upload {code}: {}",
                    String::from_utf8_lossy(&body)
                )),
                Err(e) => error(&e),
            }
        }
        "launch_app" => answer(client.json(
            "POST",
            &format!("/phones/{phone}/v1/apps/launch"),
            &json!({ "bundle_id": arg("bundle_id") }).to_string(),
        )),
        "read_file" => answer(client.json(
            "GET",
            &format!(
                "/phones/{phone}/v1/files/content?path={}",
                encode(arg("path"))
            ),
            "",
        )),
        "write_file" => {
            let r = client.call(
                "PUT",
                &format!(
                    "/phones/{phone}/v1/files/content?path={}",
                    encode(arg("path"))
                ),
                "application/octet-stream",
                arg("content").as_bytes(),
            );
            answer(r.map(|(c, b)| (c, String::from_utf8_lossy(&b).into_owned())))
        }
        "call_phone" => {
            let path = arg("path");
            if !vphoned_path(path) {
                return error("path: a vphoned route, /v1/... with no spaces");
            }
            let body = if args["body"].is_null() {
                String::new()
            } else {
                args["body"].to_string()
            };
            let method = if arg("method").is_empty() {
                "GET"
            } else {
                arg("method")
            };
            if !method.chars().all(|c| c.is_ascii_uppercase()) {
                return error("method: A-Z only, e.g. GET or POST");
            }
            answer(client.json(method, &format!("/phones/{phone}{path}"), &body))
        }
        "batch" => {
            let body = json!({ "phones": args["phones"], "method": args["method"].as_str().unwrap_or("GET"), "path": arg("path"), "body": args["body"] });
            answer(client.json("POST", "/batch", &body.to_string()))
        }
        "diagnose" if phone.is_empty() => answer(client.json("GET", "/diag", "")),
        "diagnose" => answer(client.json("GET", &format!("/phones/{phone}/diag"), "")),
        "logs" => {
            let tail = args["tail"].as_u64().unwrap_or(200);
            answer(client.json("GET", &format!("/phones/{phone}/logs?tail={tail}"), ""))
        }
        _ => error(&format!("no tool {name}")),
    }
}

/// The answer to one JSON-RPC message, or `None` for a notification.
fn reply(client: &Client, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let params = &msg["params"];
    let result = match msg["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "vphone-fleet", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Phones of one host. list_phones first; diagnose when something fails.",
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => call(
            client,
            params["name"].as_str().unwrap_or_default(),
            &params["arguments"],
        ),
        other => {
            return Some(
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("no method {other}") } }),
            );
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Serves MCP on `input` and `output` until `input` ends.
pub(crate) fn run(
    input: impl BufRead,
    mut output: impl Write,
    client: &Client,
) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(msg) if msg.is_object() => reply(client, &msg),
            _ => Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "one JSON-RPC object per line" } }),
            ),
        };
        if let Some(a) = answer {
            writeln!(output, "{a}")
                .and_then(|()| output.flush())
                .map_err(|e| e.to_string())?;
        }
    }
    log!("vphone-fleet mcp: input closed");
    Ok(())
}
