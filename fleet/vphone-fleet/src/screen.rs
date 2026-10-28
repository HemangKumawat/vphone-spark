//! The host side of a phone's screen, where vphone-cli's Mac app has `/v1/host/screen`:
//! `GET /phones/<id>/screen` (a PNG from QMP `screendump`, else vphoned's `screen.screenshot`)
//! and `POST /phones/<id>/input` (touch, keys and text through QMP, as `tools/qmp.py` does).

use std::fs;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use crate::api::lock;
use crate::fleet::Fleet;
use crate::gateway::{Refused, wake_for_use};
use crate::host::{self, Qmp};

/// The display in pixels (`tools/qmp.py`): input coordinates are pixels of the screenshot.
const WIDTH: f64 = 1290.0;
const HEIGHT: f64 = 2796.0;
/// QEMU's absolute pointer axis runs 0 to this.
const AXIS_MAX: f64 = 32767.0;
/// `tools/qmp.py`'s timings: move, then press; hold a tap; a key's release (send-key holds
/// 100 ms) before the next key.
const BEFORE_PRESS: Duration = Duration::from_millis(50);
const TAP_HOLD: Duration = Duration::from_millis(120);
const KEY_GAP: Duration = Duration::from_millis(150);

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// `(content type, image, where it came from)`.
pub(crate) fn shot(
    fleet: &Mutex<Fleet>,
    id: &str,
) -> Result<(&'static str, Vec<u8>, &'static str), Refused> {
    static N: AtomicU64 = AtomicU64::new(0);
    let (run, running) = {
        let f = lock(fleet);
        (f.paths.run(id), f.status(id).is_some())
    };
    if !running {
        return Err((
            409,
            format!("phone {id} is not running: POST /phones/{id}/start"),
        ));
    }
    // QEMU writes the file itself; a paused (parked) phone still has its last frame.
    let file = run.join(format!("screen-{}.png", N.fetch_add(1, Ordering::Relaxed)));
    let dumped = Qmp::connect(&run.join("qmp.sock")).and_then(|mut q| {
        q.execute(
            "screendump",
            Some(&json!({ "filename": file, "format": "png" })),
        )
    });
    let png = dumped
        .and_then(|_| fs::read(&file).map_err(|e| format!("reading {}: {e}", file.display())));
    let _ = fs::remove_file(&file);
    let qmp_error = match png {
        Ok(png) if png.starts_with(PNG_MAGIC) => return Ok(("image/png", png, "qmp")),
        Ok(_) => "screendump wrote no PNG".to_owned(),
        Err(e) => e,
    };
    // No display (GFX=0) or no QMP: ask the guest.
    let upstream = wake_for_use(fleet, id)?;
    let rpc = r#"{"method":"screen.screenshot","params":{}}"#;
    let fail = |e: &str| {
        (
            502,
            format!("no screen: QMP: {qmp_error}; vphoned screen.screenshot: {e}"),
        )
    };
    let (code, text) = host::vphoned(&upstream, "POST", "/v1/rpc", rpc, Some(host::CALL_TIMEOUT))
        .map_err(|e| fail(&e))?;
    let answer: Value = serde_json::from_str(&text).map_err(|_| fail(&format!("{code} {text}")))?;
    let result = answer["result"]
        .as_object()
        .ok_or_else(|| fail(&format!("{code} {text}")))?;
    // ponytail: the image is the longest string in the result; its key name is IcliKit's.
    let image = result
        .values()
        .filter_map(Value::as_str)
        .max_by_key(|s| s.len())
        .and_then(|s| BASE64.decode(s.trim()).ok())
        .ok_or_else(|| fail("no base64 image in the result"))?;
    let jpeg = result.get("mime_type").and_then(Value::as_str) != Some("image/png");
    Ok((
        if jpeg { "image/jpeg" } else { "image/png" },
        image,
        "vphoned",
    ))
}

/// A character's QEMU key code, and whether it needs shift (a US layout).
fn qcode(c: char) -> Option<(bool, String)> {
    const PLAIN: [(char, &str); 13] = [
        (' ', "spc"),
        ('\n', "ret"),
        ('\t', "tab"),
        ('-', "minus"),
        ('=', "equal"),
        (',', "comma"),
        ('.', "dot"),
        ('/', "slash"),
        (';', "semicolon"),
        ('\'', "apostrophe"),
        ('[', "bracket_left"),
        (']', "bracket_right"),
        ('\\', "backslash"),
    ];
    const SHIFTED: &str = "!@#$%^&*()_+<>?:\"{}|~";
    const UNSHIFTED: &str = "1234567890-=,./;'[]\\`";
    match c {
        'a'..='z' | '0'..='9' => Some((false, c.to_string())),
        'A'..='Z' => Some((true, c.to_ascii_lowercase().to_string())),
        '`' => Some((false, "grave_accent".to_owned())),
        _ => {
            if let Some((_, q)) = PLAIN.iter().find(|(p, _)| *p == c) {
                return Some((false, (*q).to_owned()));
            }
            let i = SHIFTED.chars().position(|s| s == c)?;
            qcode(UNSHIFTED.chars().nth(i)?).map(|(_, q)| (true, q))
        }
    }
}

fn point(body: &Value, x: &str, y: &str) -> Result<Value, String> {
    let (Some(px), Some(py)) = (body[x].as_f64(), body[y].as_f64()) else {
        return Err(format!(
            "needs numbers {x} and {y}: screen pixels, 0-{WIDTH} by 0-{HEIGHT}"
        ));
    };
    if !(0.0..=WIDTH).contains(&px) || !(0.0..=HEIGHT).contains(&py) {
        return Err(format!(
            "{x},{y} = {px},{py} is off the {WIDTH}x{HEIGHT} screen"
        ));
    }
    let axis = |v: f64, max: f64| (v / max * AXIS_MAX).round();
    Ok(json!({ "events": [
        { "type": "abs", "data": { "axis": "x", "value": axis(px, WIDTH) } },
        { "type": "abs", "data": { "axis": "y", "value": axis(py, HEIGHT) } },
    ] }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Down,
    Up,
}

fn button(name: &str, press: Press) -> Value {
    json!({ "events": [{ "type": "btn", "data": { "down": press == Press::Down, "button": name } }] })
}

/// The QMP commands for one input action, each with the pause after it.
fn plan(body: &Value) -> Result<Vec<(&'static str, Value, Duration)>, String> {
    let none = Duration::ZERO;
    let ev = "input-send-event";
    match body["action"].as_str() {
        Some("tap") => Ok(vec![
            (ev, point(body, "x", "y")?, BEFORE_PRESS),
            (ev, button("left", Press::Down), TAP_HOLD),
            (ev, button("left", Press::Up), none),
        ]),
        Some("swipe") => {
            let start = point(body, "x0", "y0")?;
            point(body, "x1", "y1")?;
            let ms = body["ms"].as_u64().unwrap_or(300);
            let steps: u16 = body["steps"].as_u64().map_or(20, |n| u16::try_from(n.clamp(1, 1000)).unwrap_or(20));
            let mut out = vec![(ev, start, BEFORE_PRESS), (ev, button("left", Press::Down), none)];
            let f = |k: &str| body[k].as_f64().unwrap_or(0.0);
            for i in 1..=steps {
                let t = f64::from(i) / f64::from(steps);
                let at = json!({ "x": f("x0") + (f("x1") - f("x0")) * t, "y": f("y0") + (f("y1") - f("y0")) * t });
                out.push((ev, point(&at, "x", "y")?, Duration::from_millis(ms / u64::from(steps))));
            }
            out.push((ev, button("left", Press::Up), none));
            Ok(out)
        }
        // A middle click: usb-touchscreen turns it into Cmd+H, iOS's Home shortcut.
        Some("home") => Ok(vec![(ev, button("middle", Press::Down), Duration::from_millis(100)), (ev, button("middle", Press::Up), none)]),
        Some("key") => body["keys"]
            .as_array()
            .filter(|k| !k.is_empty())
            .ok_or("key needs keys: [\"ret\", ...] (QEMU qcodes)")?
            .iter()
            .map(|k| {
                let k = k.as_str().ok_or("each key is a QEMU qcode string")?;
                Ok(("send-key", json!({ "keys": [{ "type": "qcode", "data": k }] }), KEY_GAP))
            })
            .collect(),
        Some("text") => body["text"]
            .as_str()
            .ok_or("text needs text: \"...\"")?
            .chars()
            .map(|c| {
                let (shift, q) = qcode(c).ok_or_else(|| {
                    format!("{c:?} has no US key: send it with vphoned input.type (POST /phones/<id>/v1/rpc)")
                })?;
                let mut keys = vec![json!({ "type": "qcode", "data": q })];
                if shift {
                    keys.insert(0, json!({ "type": "qcode", "data": "shift" }));
                }
                Ok(("send-key", json!({ "keys": keys }), KEY_GAP))
            })
            .collect(),
        _ => Err("body: {\"action\": \"tap\"|\"swipe\"|\"key\"|\"text\"|\"home\", ...} (docs/AUTOMATION.md)".to_owned()),
    }
}

/// `POST /phones/<id>/input`: wakes a parked phone, then sends the action's QMP commands.
pub(crate) fn input(fleet: &Mutex<Fleet>, id: &str, body: &str) -> Refused {
    let refuse = |code: u16, e: &str| (code, json!({ "error": e }).to_string());
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return refuse(
            400,
            "body must be JSON: {\"action\": \"tap\", \"x\": 645, \"y\": 1398}",
        );
    };
    let commands = match plan(&body) {
        Ok(c) => c,
        Err(e) => return refuse(400, &e),
    };
    if let Err((code, e)) = wake_for_use(fleet, id) {
        return refuse(code, &e);
    }
    let socket = lock(fleet).paths.run(id).join("qmp.sock");
    let mut qmp = match Qmp::connect(&socket) {
        Ok(q) => q,
        Err(e) => return refuse(502, &e),
    };
    let mut sent = Vec::new();
    for (command, args, pause) in commands {
        if let Err(e) = qmp.execute(command, Some(&args)) {
            return refuse(502, &e);
        }
        sent.push(json!({ "execute": command, "arguments": args }));
        thread::sleep(pause);
    }
    (
        200,
        json!({ "ok": true, "phone": id, "qmp": sent }).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_maps_to_us_keys() {
        assert_eq!(qcode('a'), Some((false, "a".to_owned())));
        assert_eq!(qcode('Q'), Some((true, "q".to_owned())));
        assert_eq!(qcode('!'), Some((true, "1".to_owned())));
        assert_eq!(qcode('~'), Some((true, "grave_accent".to_owned())));
        assert_eq!(qcode('?'), Some((true, "slash".to_owned())));
        assert_eq!(qcode('é'), None);
        assert!(plan(&json!({"action": "text", "text": "é"})).is_err());
    }

    #[test]
    fn taps_are_scaled_to_the_axis_and_checked() {
        let p = plan(&json!({"action": "tap", "x": 1290, "y": 0})).unwrap();
        assert_eq!(p[0].1["events"][0]["data"]["value"], 32767.0);
        assert_eq!(p[0].1["events"][1]["data"]["value"], 0.0);
        assert!(plan(&json!({"action": "tap", "x": 2000, "y": 5})).is_err());
        let swipe =
            plan(&json!({"action": "swipe", "x0": 0, "y0": 0, "x1": 10, "y1": 10, "steps": 4}));
        assert_eq!(swipe.unwrap().len(), 7);
    }
}
