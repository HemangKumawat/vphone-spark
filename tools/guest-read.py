#!/usr/bin/env python3
"""guest-read.py CID GUEST_PATH LOCAL_PATH [SIZE]: fetch a guest file through vphoned's files.read
RPC (base64 in JSON), for files GET /v1/files/content cannot stream (past its first 64 KiB chunk
it closes without a reply). SIZE defaults to 512 MiB, the most read."""
import base64, json, socket, sys
cid, src, dst = int(sys.argv[1]), sys.argv[2], sys.argv[3]
size = int(sys.argv[4]) if len(sys.argv) > 4 else 512 << 20
body = json.dumps({"method": "files.read", "params": {"path": src, "binary": True, "limit": size}, "id": 1}).encode()
s = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
for opt in (socket.SO_VM_SOCKETS_BUFFER_MAX_SIZE, socket.SO_VM_SOCKETS_BUFFER_SIZE):
    s.setsockopt(socket.AF_VSOCK, opt, 4 << 20)
s.connect((cid, 1339)); s.settimeout(600)
s.sendall(b"POST /v1/rpc HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: %d\r\n\r\n" % len(body) + body)
chunks = []
while True:
    c = s.recv(1 << 22)
    if not c: break
    chunks.append(c)
raw = b"".join(chunks)
r = json.loads(raw.partition(b"\r\n\r\n")[2])
if "error" in r: sys.exit(r["error"]["message"])
data = base64.b64decode(r["result"]["content"])
open(dst, "wb").write(data)
print(f"{dst}: {len(data)} of {r['result']['size']} bytes{' (truncated)' if r['result'].get('truncated') else ''}")
