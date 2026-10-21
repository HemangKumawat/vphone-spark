#!/bin/bash
# Replace a running guest's vphoned with a new build, through vphoned's own self-update:
# upload the binary to /var/root/Library/Caches/vphoned.next (PUT /v1/files/content), then
# `agent.apply_update` with its SHA-256, which renames it over the cached daemon (the launchd
# proxy runs the cached binary whose hash matches vphoned.api-v2) and restarts.
#
#   tools/vphoned-update.sh [-c CID] BINARY
#
# The binary must already be signed the way StageBundle.sh signs it (ad hoc, with
# VPhoneDaemon/Configuration/VPhoneDaemon.entitlements). This changes the VM's disk: use it on a
# reflink copy of a clone, not on a clone others test from.
set -euo pipefail
cd "$(dirname "$0")/.."
cid=${CID:-3}
if [ "${1:-}" = -c ]; then cid=$2; shift 2; fi
bin=${1:?usage: vphoned-update.sh [-c CID] BINARY}
sum=$(sha256sum "$bin" | cut -d' ' -f1)

# vphoned keeps the connection open after its reply, so read exactly one HTTP response.
python3 -I - "$cid" "$bin" <<'PY'
import os, socket, sys, time
cid, path = int(sys.argv[1]), sys.argv[2]
data = open(path, "rb").read()
uds = f"{os.environ.get('VSOCK_DIR', '/tmp/vphone-vsock')}/{cid}.sock"
if os.path.exists(uds):
    # VSOCK=uds VM (run-vphone.sh): hybrid vsock over a Unix socket
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(300)
    s.connect(uds)
    s.sendall(b"CONNECT 1339\n")
    line = b""
    while not line.endswith(b"\n"):
        c = s.recv(1)
        if not c:
            sys.exit("vsock: guest refused port 1339")
        line += c
    if not line.startswith(b"OK"):
        sys.exit(f"vsock: {line!r}")
else:
    s = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    # Linux caps the send credit at this socket's buffer (256 KiB); iOS sends a credit update
    # only when its 512 KiB window is nearly full: below that the upload stalls for good
    for opt in (socket.SO_VM_SOCKETS_BUFFER_MAX_SIZE, socket.SO_VM_SOCKETS_BUFFER_SIZE):
        s.setsockopt(socket.AF_VSOCK, opt, 4 << 20)
    s.connect((cid, 1339))
    s.settimeout(300)
t0 = time.time()
s.sendall((
    "PUT /v1/files/content?path=/var/root/Library/Caches/vphoned.next&mode=755 HTTP/1.1\r\n"
    "Host: localhost\r\nContent-Type: application/octet-stream\r\n"
    f"Content-Length: {len(data)}\r\n\r\n").encode() + data)
reply = b""
while b"\r\n\r\n" not in reply:
    reply += s.recv(65536)
head, body = reply.split(b"\r\n\r\n", 1)
length = next(int(l.split(b":")[1]) for l in head.split(b"\r\n") if l.lower().startswith(b"content-length"))
while len(body) < length:
    body += s.recv(65536)
s.close()
dt = time.time() - t0
print(f"uploaded {len(data)} bytes in {dt:.1f} s ({len(data) / dt / 1e6:.2f} MB/s): {body.decode()}")
if b'"error"' in body:
    sys.exit(1)
PY
echo "applying sha256 $sum"
CID=$cid VPHONED_TIMEOUT=20 timeout 30 tools/vphoned.sh POST /v1/rpc \
  "{\"method\":\"agent.apply_update\",\"params\":{\"sha256\":\"$sum\"},\"id\":1}" || true
