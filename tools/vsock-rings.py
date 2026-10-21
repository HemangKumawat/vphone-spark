#!/usr/bin/env python3
"""vsock-rings.py SOCK: the vhost-vsock device's virtqueue layout (QEMU's view) and, from guest
memory, each ring's avail and used index. Diagnoses a vsock that times out on every port."""
import json, socket, sys

s = socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f = s.makefile("rw")
f.readline(); f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline()


def cmd(c, **a):
    f.write(json.dumps({"execute": c, "arguments": a} if a else {"execute": c}) + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            return r


def hmp(c):
    return cmd("human-monitor-command", **{"command-line": c})["return"]


path = next(d["path"] for d in cmd("x-query-virtio")["return"] if "vsock" in d["name"])
for q in range(3):
    st = cmd("x-query-virtio-queue-status", path=path, queue=q)["return"]
    av, us = st["vring-avail"], st["vring-used"]
    a = hmp(f"xp /2hx {av}").split(":", 1)[1].split()
    u = hmp(f"xp /2hx {us}").split(":", 1)[1].split()
    print(f"q{q}: desc {st['vring-desc']} avail {av} (+{av - st['vring-desc']}) used {us} (+{us - av}) "
          f"num {st['vring-num']}; avail idx {a[1]} used flags {u[0]} idx {u[1]}")
