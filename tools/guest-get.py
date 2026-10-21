#!/usr/bin/env python3
"""guest-get.py CID GUEST_PATH LOCAL_PATH: download a guest file over vsock (vphoned's
GET /v1/files/content), streaming to LOCAL_PATH. Host socket buffer raised as for uploads."""
import socket, sys, urllib.parse
cid, src, dst = int(sys.argv[1]), sys.argv[2], sys.argv[3]
import os
uds = f"{os.environ.get('VSOCK_DIR', '/tmp/vphone-vsock')}/{cid}.sock"
if os.path.exists(uds):  # a VSOCK=uds VM (qemu branch vsock-uds)
    s = socket.socket(socket.AF_UNIX); s.settimeout(120); s.connect(uds)
    s.sendall(b"CONNECT 1339\n")
    line = b""
    while not line.endswith(b"\n"):
        line += s.recv(1)
else:
    s = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    for opt in (socket.SO_VM_SOCKETS_BUFFER_MAX_SIZE, socket.SO_VM_SOCKETS_BUFFER_SIZE):
        s.setsockopt(socket.AF_VSOCK, opt, 4 << 20)
    s.connect((cid, 1339)); s.settimeout(120)
s.sendall(f"GET /v1/files/content?path={urllib.parse.quote(src)} HTTP/1.1\r\nHost: localhost\r\n\r\n".encode())
buf = b""
while b"\r\n\r\n" not in buf:
    c = s.recv(65536)
    if not c: sys.exit("closed before headers")
    buf += c
head, _, rest = buf.partition(b"\r\n\r\n")
status = head.split(b"\r\n")[0]
length = None
for h in head.split(b"\r\n")[1:]:
    k, _, v = h.partition(b":")
    if k.strip().lower() == b"content-length": length = int(v)
if b" 200" not in status: sys.exit(f"{status.decode()} {rest[:200]!r}")
got = len(rest)
with open(dst, "wb") as f:
    f.write(rest)
    while length is None or got < length:
        c = s.recv(1 << 20)
        if not c: break
        f.write(c); got += len(c)
if length is not None and got != length: sys.exit(f"short: {got} of {length}")
print(f"{dst}: {got} bytes")
