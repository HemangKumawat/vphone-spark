# Automating the phone fleet

`vphone-fleet serve` gives every phone of one host a single authenticated address. This page shows
how a script, a bot or an AI agent drives the phones through it. The full contract is
[openapi.yaml](openapi.yaml) (OpenAPI 3.1), and the rest of `vphone-fleet` is in
[DENSITY.md](DENSITY.md#vphone-fleet).

**Status: untested on Spark.** Every route was checked end to end against fake vphoneds, a fake QMP
and a fake `/proc` (`cargo test` in `fleet/`). Nothing here has run against a real iOS guest yet.
Three parts are guesses until a real guest confirms them: QMP input reaching iOS, the field
`screen.screenshot` returns its image in, and the `locked` field of `device.snapshot`.

| Address | What it is |
|---|---|
| `ANY /phones/{id}/v1/...` | that phone's own vphoned API (vphone-cli `Research/vphoned_http_api.md`), unchanged |
| `POST /batch` | one vphoned call on many phones, at most `max_awake` at a time |
| `GET /events` | one WebSocket: every phone's events plus the fleet's, each tagged with its phone |
| `GET /phones/{id}/screen` | a PNG (QMP `screendump`), else a JPEG from vphoned |
| `POST /phones/{id}/input` | tap, swipe, home, keys and text through QMP |
| `GET /diag`, `GET /phones/{id}/diag`, `GET /phones/{id}/logs?tail=N` | what is wrong and what to do about it |
| `vphone-fleet mcp` | all of the above as MCP tools, over stdin/stdout |

## Security

- The token is required on every route. It comes from `VPHONE_FLEET_TOKEN`, or the serve makes one
  and writes it to `runs/fleet-token` (mode 0600). Send it as `Authorization: Bearer <token>`, as a
  `token=<token>` query item, or on a WebSocket as the subprotocol `vphone-token.<token>`.
- The serve binds `127.0.0.1:8760` by default. It logs a warning for any other address, because the
  token is then the only thing between the network and every phone.
- The token is stripped before anything reaches a phone. Each phone's vphoned gets `Host: localhost`
  and never an `Origin` header. A request that carries an `Origin` header (a web page) is refused
  with 403, just as vphoned refuses one.

## Set up

```sh
cd fleet && cargo spark            # static aarch64 musl binary (cargo build --release elsewhere)
vphone-fleet serve &               # the API and the 1 s loop
export FLEET=http://127.0.0.1:8760
export TOKEN=$(cat runs/fleet-token)       # or set VPHONE_FLEET_TOKEN before serve
```

## curl: five phones, one app, a screenshot each

```sh
auth=(-H "Authorization: Bearer $TOKEN")

# 1. Create five phones from a golden, then start them.
curl -sS "${auth[@]}" -X POST "$FLEET/phones?ids=p1,p2,p3,p4,p5&from=ios26-base"
curl -sS "${auth[@]}" -X POST "$FLEET/phones/start?ids=p1,p2,p3,p4,p5"
curl -sS "${auth[@]}" -X POST "$FLEET/batch" -H 'Content-Type: application/json' \
  -d '{"phones":"all","method":"GET","path":"/v1/health"}'   # 200 once every phone answers

# 2. Install an IPA on every phone. The upload carries bytes, so it is one PUT per phone; the
#    install is one batch. vphoned deletes the file after installing it.
for p in p1 p2 p3 p4 p5; do
  curl -sS "${auth[@]}" -X PUT -H 'Content-Type: application/octet-stream' \
    --data-binary @MyApp.ipa "$FLEET/phones/$p/v1/files/content?path=/var/tmp/MyApp.ipa"
done
curl -sS "${auth[@]}" -X POST "$FLEET/batch" -H 'Content-Type: application/json' \
  -d '{"phones":"all","method":"POST","path":"/v1/apps/install","body":{"path":"/var/tmp/MyApp.ipa"},"timeout_secs":300}'

# 3. Launch it everywhere.
curl -sS "${auth[@]}" -X POST "$FLEET/batch" -H 'Content-Type: application/json' \
  -d '{"phones":"all","method":"POST","path":"/v1/apps/launch","body":{"bundle_id":"com.example.myapp"}}'

# 4. A screenshot of each phone, and one tap.
for p in p1 p2 p3 p4 p5; do
  curl -sS "${auth[@]}" -D - -o "$p.png" "$FLEET/phones/$p/screen" | grep -i x-screen-source
done
curl -sS "${auth[@]}" -X POST "$FLEET/phones/p1/input" -H 'Content-Type: application/json' \
  -d '{"action":"tap","x":645,"y":1400}'

# 5. Watch events (any WebSocket client; websocat shown).
websocat -H "Authorization: Bearer $TOKEN" ws://127.0.0.1:8760/events
```

`/batch` answers 200 when every phone succeeded and 207 when any failed. Each result is
`{phone, ok, status, body}`, or `{phone, ok: false, status, error}` when the phone could not be
reached. Any vphoned call works through the gateway as well, for example
`POST /phones/p1/v1/rpc {"method":"apps.list"}`.

## Python: the same, standard library plus one WebSocket package

```python
import json, os, urllib.error, urllib.request

FLEET = os.environ.get("FLEET", "http://127.0.0.1:8760")
TOKEN = os.environ["TOKEN"]
PHONES = ["p1", "p2", "p3", "p4", "p5"]

def call(method, path, body=None, raw=None, ctype="application/json"):
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    req = urllib.request.Request(FLEET + path, data=data, method=method,
                                 headers={"Authorization": f"Bearer {TOKEN}", "Content-Type": ctype})
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            return r.status, r.read(), r.headers
    except urllib.error.HTTPError as e:   # 207, 4xx and 5xx still carry a JSON answer
        return e.code, e.read(), e.headers

def batch(method, path, body=None, **extra):
    status, out, _ = call("POST", "/batch", {"phones": "all", "method": method, "path": path,
                                             "body": body, **extra})
    results = json.loads(out)["results"]
    for r in results:
        if not r["ok"]:
            print(r["phone"], "failed:", r.get("error") or r.get("body"))
    return results

ids = ",".join(PHONES)
call("POST", f"/phones?ids={ids}&from=ios26-base")
call("POST", f"/phones/start?ids={ids}")
batch("GET", "/v1/health")

ipa = open("MyApp.ipa", "rb").read()
for p in PHONES:
    call("PUT", f"/phones/{p}/v1/files/content?path=/var/tmp/MyApp.ipa", raw=ipa,
         ctype="application/octet-stream")
batch("POST", "/v1/apps/install", {"path": "/var/tmp/MyApp.ipa"}, timeout_secs=300)
batch("POST", "/v1/apps/launch", {"bundle_id": "com.example.myapp"})

for p in PHONES:
    status, image, headers = call("GET", f"/phones/{p}/screen")
    ext = "png" if headers["Content-Type"] == "image/png" else "jpg"
    open(f"{p}.{ext}", "wb").write(image)

call("POST", "/phones/p1/input", {"action": "swipe", "x0": 645, "y0": 2400, "x1": 645, "y1": 800})
call("POST", "/phones/p1/input", {"action": "text", "text": "hello\n"})
```

Watching events (`pip install websocket-client`):

```python
import json, websocket   # websocket-client

ws = websocket.create_connection("ws://127.0.0.1:8760/events",
                                 header=[f"Authorization: Bearer {TOKEN}"])
while True:
    m = json.loads(ws.recv())
    if m.get("source") == "fleet":
        print(m["time"], m["phone"], m["event"], m["reason"], "FAILURE" if m["failure"] else "")
    else:
        print(m.get("phone"), m.get("event"), m.get("data"))
```

The first message is `{"event":"connected","phones":[...]}`. Phone messages are each phone's own
`/v1/events` messages plus `phone` and `source: "phone"`. Fleet messages have
`source: "fleet"`, `phone`, `event`, `reason`, `time` and `failure`. Their events are
`phone.started`, `phone.stopped`, `phone.unhealthy`, `phone.restarted`, `phone.parked` and
`phone.woken`, and the failures `phone.start_failed`, `phone.exited`, `phone.restart_failed`,
`phone.park_failed` and `phone.wake_failed`. `phone.events_lost` means the fleet lost a phone's own
event stream and retries every 15 s. It goes out on the stream only, because a phone that stops
also ends its stream. The server pings every 15 s when nothing happens.

## Input

| Body | Does |
|---|---|
| `{"action":"tap","x":645,"y":1400}` | press and release at a screen pixel (1290 x 2796) |
| `{"action":"swipe","x0":..,"y0":..,"x1":..,"y1":..,"ms":300,"steps":20}` | drag from one point to another |
| `{"action":"home"}` | the home button (QEMU's middle button) |
| `{"action":"key","keys":["ret","esc"]}` | QEMU qcodes, one after another |
| `{"action":"text","text":"Hello, 1!"}` | types it on a US layout (shift where needed) |

The answer lists the QMP commands that were sent. Untested on Spark: whether iOS takes these events
as touches has only been checked against a fake QMP.

## Parked phones

A call to a parked phone wakes it first: the gateway, `/screen`, `/input` and every phone in a
batch. `/batch` with `"all"` works through the phones `max_awake` at a time, so it never wakes more
phones at once than may run. The event stream shows `phone.parked` and `phone.woken`.

## MCP: the fleet as tools for an AI agent

`vphone-fleet mcp` is an MCP server on stdin/stdout (JSON-RPC 2.0, protocol versions 2025-06-18,
2025-03-26 and 2024-11-05). It talks to the running serve through `VPHONE_FLEET_URL` (default
`http://127.0.0.1:8760`) and `VPHONE_FLEET_TOKEN`, else the token in `runs/fleet-token`.

```sh
claude mcp add vphone-fleet -e VPHONE_FLEET_TOKEN="$TOKEN" -- /path/to/vphone-fleet mcp
```

Any other MCP client takes the same command:

```json
{ "mcpServers": { "vphone-fleet": { "command": "/path/to/vphone-fleet", "args": ["mcp"],
  "env": { "VPHONE_FLEET_TOKEN": "..." } } } }
```

Tools: `list_phones`, `create_phones`, `start_phones`, `stop_phones`, `screenshot` (returned as an
image), `tap`, `swipe`, `type_text`, `press_home`, `install_app` (a host IPA path: uploads it, then
installs it), `launch_app`, `read_file`, `write_file`, `call_phone` (any vphoned call), `batch`,
`diagnose` and `logs`.

The server is hand-written: newline-delimited JSON-RPC over stdio is a small loop on
`serde_json` (`fleet/vphone-fleet/src/mcp.rs`, with the 17 tool schemas). The official Rust SDK (`rmcp`, modelcontextprotocol/rust-sdk) was the alternative. It
needs tokio and proc macros in a binary that is otherwise synchronous and static; a hand-written
stdio server keeps the binary small.

## Feature switches

`batch`, `events`, `qmp_input` and `mcp` are switches like the rest (default on;
[DENSITY.md](DENSITY.md#feature-switches)). A route whose switch is off answers 503:

```json
{"error":"feature batch is off","enable":"PUT /features/batch"}
```

To turn one back on live: `curl -X PUT "${auth[@]}" "$FLEET/features/batch" -d '{"on":true}'`. The
token check is never a switch.

## When something is wrong

Start with `vphone-fleet doctor`, which prints the same report as `GET /diag`. It asks the running
serve first and checks from the local machine when no serve answers. Each check is `ok`, `fail`,
`note` or `skipped`. Every `fail` has a `hint`: what to do, in plain words.
`vphone-fleet doctor ID --logs` adds the end of each of that phone's logs.

The report has five parts:

- **host**: arch, page size, cores, vhost-vsock and Vulkan (from `tools/check-host.sh --runtime`),
  a note that KVM is not needed (iOS needs TCG), the fast cores, `/dev/shm` against the RAM the
  phones plan to use, KSM, free disk on the phones volume, and whether it can reflink.
- **phones**: per phone, `qemu` (is the process alive), `qmp` (`query-status`), `vsock` (does guest
  port 1339 accept), `vphoned` (`/v1/health`), `locked` (from `device.snapshot`) and `restarts`
  (three or more by this serve is a restart loop). A parked phone skips the guest checks, because
  waking it to check it would change it.
- **facts**: pid, state, uptime, CPU % (sampled over half a second), RSS and PSS, pinned CPUs,
  cgroup, the disk split into bytes shared with the golden and bytes of its own, golden, profile,
  restarts and the last failure.
- **failures**: the last 256 fleet-level failures since the serve started, each with its time and reason. The
  same records go out on `/events`.
- **features**: each switch and where its value came from.

| Symptom | Ask | Look at | Usual fix |
|---|---|---|---|
| A phone won't boot | `GET /phones/{id}/diag` | `qemu` fail means QEMU died: read the hint and `GET /phones/{id}/logs?tail=200` (`console.log`). `qemu` ok but `vsock` failing means iOS has not got far enough yet, or is stuck | a `start_failed` reason in `failures`; `VPHONE_SAFE_TCG=1` in the profile when the guest misbehaves on an aarch64 host |
| It keeps restarting | `GET /diag` | `restarts` fail, `phone.exited` and `phone.restarted` in `failures` | `PUT /features/auto_restart {"on":false}`, then read the logs while it stays down |
| It is slow | `GET /phones/{id}/diag` | `facts.qemu_cpu_percent` (100 per busy core), `facts.cpus` (where it may run), and the host `shm` and `disk` checks | fewer running phones (`max_awake`), the lite profile, more `/dev/shm` |
| The API cannot reach it | `GET /phones/{id}/diag` | which of `qmp`, `vsock` and `vphoned` fails first: QMP down means QEMU, vsock down means the guest side, vphoned down means the daemon in iOS | the hint on the first failing check |
| An app crashes | `POST /phones/{id}/v1/rpc` | the app's own state through vphoned; the daemon's log via `GET /phones/{id}/v1/files/content?path=/var/log/vphoned.log` | reinstall through `/batch`; `restore` a cold snapshot |
| Screenshots are JPEG, not PNG | the `X-Screen-Source` header | `vphoned` means QMP `screendump` failed: check `qmp` in `/diag` | — |

Nothing fails silently. Each fleet failure is logged, kept in `/diag` `failures` and the phone's
`last_error`, and sent on `/events`.
