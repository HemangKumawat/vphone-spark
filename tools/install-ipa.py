#!/usr/bin/env python3
"""Install an .ipa into the running guest through vphoned, and optionally launch it.

    tools/install-ipa.py APP.ipa [--launch BUNDLE_ID] [--cid CID]

The archive is uploaded with `PUT /v1/files/content` to the guest's cache directory, then
`apps.install` signs it inside the guest (vphone's signer), registers it and deletes the upload.
No Apple ID or host-side signing is needed, so an unsigned `xtool dev build --ipa` works.
CID defaults to $CID or 3 (run-vphone.sh's default).
"""
import argparse
import json
import os
import socket
import sys
import time

PORT = 1339


def connect(cid):
    s = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    # Linux caps the send credit at this socket's buffer (256 KiB by default) and iOS sends a
    # credit update only when its 512 KiB window is nearly full: a bigger body would stall.
    for opt in (socket.SO_VM_SOCKETS_BUFFER_MAX_SIZE, socket.SO_VM_SOCKETS_BUFFER_SIZE):
        s.setsockopt(socket.AF_VSOCK, opt, 4 << 20)
    s.connect((cid, PORT))
    s.settimeout(600)
    return s


def request(cid, head, body=b""):
    s = connect(cid)
    s.sendall(head.encode() + body)
    reply = b""
    while b"\r\n\r\n" not in reply:
        chunk = s.recv(65536)
        if not chunk:
            sys.exit("vphoned closed the connection")
        reply += chunk
    header, data = reply.split(b"\r\n\r\n", 1)
    length = next((int(l.split(b":")[1]) for l in header.split(b"\r\n")
                   if l.lower().startswith(b"content-length")), None)
    while length is not None and len(data) < length:
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    return header.split(b"\r\n", 1)[0].decode(), data


def rpc(cid, method, params):
    body = json.dumps({"method": method, "params": params, "id": 1}).encode()
    status, data = request(cid, "POST /v1/rpc HTTP/1.1\r\nHost: localhost\r\n"
                           "Content-Type: application/json\r\n"
                           f"Content-Length: {len(body)}\r\n\r\n", body)
    reply = json.loads(data)
    if "error" in reply:
        sys.exit(f"{method} failed: {reply['error']}")
    return reply.get("result", reply)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ipa")
    ap.add_argument("--launch", metavar="BUNDLE_ID", help="launch this app after installing it")
    ap.add_argument("--cid", type=int, default=int(os.environ.get("CID", "3")))
    args = ap.parse_args()

    data = open(args.ipa, "rb").read()
    guest_path = f"/var/root/Library/Caches/{os.path.basename(args.ipa)}"
    t0 = time.time()
    status, body = request(args.cid,
                           f"PUT /v1/files/content?path={guest_path} HTTP/1.1\r\nHost: localhost\r\n"
                           "Content-Type: application/octet-stream\r\n"
                           f"Content-Length: {len(data)}\r\n\r\n", data)
    if " 200" not in status:
        sys.exit(f"upload failed: {status} {body[:200]!r}")
    print(f"uploaded {len(data)} bytes in {time.time() - t0:.1f} s")

    t0 = time.time()
    result = rpc(args.cid, "apps.install", {"path": guest_path})
    print(f"installed in {time.time() - t0:.1f} s: {json.dumps(result)[:300]}")

    if args.launch:
        result = rpc(args.cid, "apps.launch", {"bundle_id": args.launch})
        print(f"launched {args.launch}: {json.dumps(result)[:200]}")


if __name__ == "__main__":
    main()
