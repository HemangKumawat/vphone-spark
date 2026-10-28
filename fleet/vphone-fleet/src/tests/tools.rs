//! The screen, input, MCP and `/diag` end to end, on the same fakes.

use super::*;

#[test]
fn screen_is_a_png_from_qmp_and_input_sends_qmp_events() {
    let env = Env::new(&["A", "B"]);
    let (code, png) = env
        .client()
        .call("GET", "/phones/A/screen", "application/json", b"")
        .unwrap();
    assert_eq!((code, png.as_slice()), (200, PNG));
    let answer = env.raw(&format!(
        "GET /phones/A/screen HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
    ));
    assert!(
        answer.contains("Content-Type: image/png") && answer.contains("X-Screen-Source: qmp"),
        "{answer}"
    );
    // No QMP: the guest's own screenshot.
    fs::remove_file(env.root.join("runs/B/qmp.sock")).unwrap();
    let (code, jpeg) = env
        .client()
        .call("GET", "/phones/B/screen", "application/json", b"")
        .unwrap();
    assert_eq!((code, jpeg.as_slice()), (200, JPEG));

    let (code, body) = env.json(
        "POST",
        "/phones/A/input",
        r#"{"action":"tap","x":645,"y":1398}"#,
    );
    assert_eq!(code, 200, "{body}");
    let sent: Vec<Value> = env.phones[0]
        .qmp
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.contains("input-send-event"))
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(sent.len(), 3);
    assert_eq!(
        sent[0]["arguments"]["events"][0]["data"],
        json!({"axis": "x", "value": 16384.0})
    );
    assert_eq!(
        sent[0]["arguments"]["events"][1]["data"],
        json!({"axis": "y", "value": 16384.0})
    );
    assert_eq!(
        sent[1]["arguments"]["events"][0]["data"],
        json!({"button": "left", "down": true})
    );
    assert_eq!(
        sent[2]["arguments"]["events"][0]["data"],
        json!({"button": "left", "down": false})
    );
    let (code, _) = env.json(
        "POST",
        "/phones/A/input",
        r#"{"action":"text","text":"Hi"}"#,
    );
    assert_eq!(code, 200);
    let last = env.phones[0].qmp.lock().unwrap().last().cloned().unwrap();
    assert!(
        last.contains("\"send-key\"") && last.contains("\"i\""),
        "{last}"
    );
    assert_eq!(
        names(&env.phones[0].qmp)
            .iter()
            .filter(|n| *n == "send-key")
            .count(),
        2
    );
    assert_eq!(
        env.json("PUT", "/features/qmp_input", r#"{"on": false}"#).0,
        200
    );
    let (code, body) = env.json("POST", "/phones/A/input", r#"{"action":"home"}"#);
    assert_eq!(
        (code, body["enable"].as_str()),
        (503, Some("PUT /features/qmp_input"))
    );
}

#[test]
fn mcp_initialize_list_and_call_over_stdio() {
    let env = Env::new(&["A", "B", "C"]);
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "list_phones", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "install_app", "arguments": {"phone": "A", "ipa": env.root.join("config/profiles.toml")}}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "install_app", "arguments": {"phone": "A", "ipa": env.root.join("secret.ipa")}}}),
    ]
    .map(|m| m.to_string())
    .join("\n");
    fs::write(env.root.join("secret.ipa"), "not a zip").unwrap();
    let mut out = Vec::new();
    mcp::run(input.as_bytes(), &mut out, &env.client()).unwrap();
    let replies: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        replies.len(),
        5,
        "a notification gets no reply: {replies:?}"
    );
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "vphone-fleet");
    let tools: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for t in [
        "list_phones",
        "screenshot",
        "tap",
        "type_text",
        "install_app",
        "launch_app",
        "read_file",
        "write_file",
        "batch",
    ] {
        assert!(tools.contains(&t), "no tool {t}: {tools:?}");
    }
    let result = &replies[2]["result"];
    assert_eq!(result["isError"], false, "{result}");
    let phones: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    let ids: Vec<&str> = phones
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["A", "B", "C"]);
    for r in &replies[3..] {
        assert_eq!(
            r["result"]["isError"], true,
            "only an app archive is uploaded: {r}"
        );
    }
    assert!(
        env.phones[0]
            .heads
            .lock()
            .unwrap()
            .iter()
            .all(|h| !h.starts_with("PUT")),
        "nothing was uploaded"
    );
}

#[test]
fn diag_runs_every_check_and_a_stopped_phone_fails_with_a_hint() {
    let env = Env::new(&["A"]);
    make_phone_dir(&lock(env.fleet).paths, "D", 20);
    lock(env.fleet)
        .journal
        .record("A", "phone.unhealthy", "test failure");
    fs::write(env.root.join("runs/A/console.log"), "one\ntwo\nthree\n").unwrap();
    let (code, report) = env.json("GET", "/diag", "");
    assert_eq!(code, 200);
    let host: Vec<&str> = report["host"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["check"].as_str().unwrap())
        .collect();
    for c in ["host", "kvm", "fast_cores", "shm", "ksm", "disk", "reflink"] {
        assert!(host.contains(&c), "no host check {c}: {host:?}");
    }
    let vsock = report["host"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["detail"].as_str().unwrap().contains("vhost-vsock"))
        .unwrap();
    assert_eq!(
        (vsock["status"].as_str(), vsock["hint"].is_string()),
        (Some("fail"), true)
    );
    let phone = |id: &str| {
        report["phones"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["phone"] == id)
            .unwrap()
            .clone()
    };
    let a = phone("A");
    let status = |p: &Value, c: &str| {
        p["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["check"] == c)
            .map(|x| x["status"].as_str().unwrap().to_owned())
    };
    for c in ["qemu", "qmp", "vsock", "vphoned", "restarts"] {
        assert_eq!(status(&a, c).as_deref(), Some("ok"), "{c}: {a}");
    }
    assert_eq!(status(&a, "locked").as_deref(), Some("note"));
    assert!(a["facts"]["rss_bytes"].as_u64().unwrap() > 0);
    assert_eq!(a["facts"]["golden"], "g1");
    assert_eq!(a["facts"]["last_error"]["reason"], "test failure");
    let d = phone("D");
    assert_eq!(d["ok"], false);
    let qemu = &d["checks"][0];
    assert_eq!(
        (qemu["check"].as_str(), qemu["status"].as_str()),
        (Some("qemu"), Some("fail"))
    );
    assert!(
        qemu["hint"]
            .as_str()
            .unwrap()
            .contains("POST /phones/D/start"),
        "{qemu}"
    );
    assert_eq!(report["failures"][0]["event"], "phone.unhealthy");

    let (code, logs) = env.json("GET", "/phones/A/logs?tail=2", "");
    assert_eq!(code, 200);
    assert_eq!(
        logs["files"][0],
        json!({"file": "console.log", "text": "two\nthree"})
    );
    assert_eq!(
        env.json("GET", "/phones/D/diag", "").1["checks"][0]["status"],
        "fail"
    );
}
