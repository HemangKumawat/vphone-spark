#!/usr/bin/env python3
"""tap-latency.py SOCK NAME X Y [NAME X Y ...]: from the home screen, tap each icon at (X, Y)
(guest pixels), report the seconds until the screen first changes and until it settles (two
screenshots 0.5 s apart that match), then press Home (a middle click, which usb-touchscreen
turns into Cmd+H) and wait for the home screen to settle again. Driven over QMP only, so it
needs no vphoned."""
import json
import os
import socket
import sys
import time

from PIL import Image, ImageChops

SOCK = sys.argv[1]


class Qmp:
    def __init__(self):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(SOCK)
        self.f = self.s.makefile("rw")
        self.f.readline()
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        self.f.write(json.dumps({"execute": name, "arguments": args} if args else {"execute": name}) + "\n")
        self.f.flush()
        while True:
            r = json.loads(self.f.readline())
            if "return" in r or "error" in r:
                return r

    def shot(self, path=os.path.join(os.environ.get("TMPDIR", "/tmp"), "tap-latency.ppm")):
        # PPM: no compression on QEMU's main thread, which holds the lock the vCPUs need.
        self.cmd("screendump", filename=path, format="ppm")
        return Image.open(path).convert("L").resize((129, 280))

    def events(self, *ev):
        self.cmd("input-send-event", events=list(ev))


def at(x, y):
    return [{"type": "abs", "data": {"axis": "x", "value": int(x / 1290 * 32767)}},
            {"type": "abs", "data": {"axis": "y", "value": int(y / 2796 * 32767)}}]


def btn(button, down):
    return {"type": "btn", "data": {"down": down, "button": button}}


def diff(a, b):
    return sum(ImageChops.difference(a, b).histogram()[20:])


def settle(q, start, limit=60):
    prev = q.shot()
    while time.time() - start < limit:
        time.sleep(0.5)
        cur = q.shot()
        if diff(prev, cur) < 300:
            return time.time() - start
        prev = cur
    return None


def main():
    q = Qmp()
    items = sys.argv[2:]
    for i in range(0, len(items), 3):
        name, x, y = items[i], float(items[i + 1]), float(items[i + 2])
        base = q.shot()
        q.events(*at(x, y))
        time.sleep(0.05)
        start = time.time()
        q.events(btn("left", True))
        time.sleep(0.1)
        q.events(btn("left", False))
        first = None
        while time.time() - start < 60:
            if diff(base, q.shot()) > 2000:
                first = time.time() - start
                break
            time.sleep(0.25)
        settled = settle(q, start)
        print(f"{name:10} first change {first if first is None else round(first, 2)} s, "
              f"settled {settled if settled is None else round(settled, 2)} s", flush=True)
        q.events(btn("middle", True))
        time.sleep(0.1)
        q.events(btn("middle", False))
        settle(q, time.time())
        time.sleep(1)


if __name__ == "__main__":
    main()
