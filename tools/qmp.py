#!/usr/bin/env python3
"""Drive a test VM over its QMP socket.

    qmp.py SOCK shot OUT.png              screenshot of the display (PNG)
    qmp.py SOCK tap X Y                   click at display pixel X,Y (1290x2796)
    qmp.py SOCK swipe X0 Y0 X1 Y1         press, drag and release
    qmp.py SOCK key QCODE...              press and release keys (e.g. ret, spc)

Pointer events go through QEMU's input layer, as the reims window's do.
"""
import json
import socket
import sys
import time

W, H = 1290, 2796


class Qmp:
    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(path)
        self.f = self.s.makefile("rw")
        self.f.readline()
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        self.f.write(json.dumps(msg) + "\n")
        self.f.flush()
        while True:
            r = json.loads(self.f.readline())
            if "return" in r or "error" in r:
                return r

    def events(self, *events):
        return self.cmd("input-send-event", events=list(events))


def at(x, y):
    return [
        {"type": "abs", "data": {"axis": "x", "value": int(x / W * 32767)}},
        {"type": "abs", "data": {"axis": "y", "value": int(y / H * 32767)}},
    ]


def btn(down):
    return {"type": "btn", "data": {"down": down, "button": "left"}}


def main():
    q = Qmp(sys.argv[1])
    op, args = sys.argv[2], sys.argv[3:]
    if op == "shot":
        r = q.cmd("screendump", filename=args[0], format="png")
        sys.exit(0 if "return" in r else 1)
    if op == "tap":
        x, y = map(float, args)
        q.events(*at(x, y))
        time.sleep(0.05)
        q.events(btn(True))
        time.sleep(0.12)
        q.events(btn(False))
    elif op == "swipe":
        # swipe X0 Y0 X1 Y1 [MS [STEPS]]: STEPS moves spread over MS milliseconds.
        x0, y0, x1, y1 = map(float, args[:4])
        ms = float(args[4]) if len(args) > 4 else 300
        steps = int(args[5]) if len(args) > 5 else 20
        q.events(*at(x0, y0))
        time.sleep(0.05)
        q.events(btn(True))
        for i in range(1, steps + 1):
            t = i / steps
            q.events(*at(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t))
            time.sleep(ms / 1000 / steps)
        q.events(btn(False))
    elif op == "home":
        # A middle click: usb-touchscreen turns it into Cmd+H, iOS's Home shortcut.
        q.events({"type": "btn", "data": {"down": True, "button": "middle"}})
        time.sleep(0.1)
        q.events({"type": "btn", "data": {"down": False, "button": "middle"}})
    elif op == "key":
        for k in args:
            q.cmd("send-key", keys=[{"type": "qcode", "data": k}])
            time.sleep(0.3)
    else:
        sys.exit(f"unknown op {op}")


if __name__ == "__main__":
    main()
