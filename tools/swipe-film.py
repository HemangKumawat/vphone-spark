#!/usr/bin/env python3
"""swipe-film.py SOCK OUTDIR X0 Y0 X1 Y1 [SECONDS]: swipe (300 ms, guest pixels of 1290x2796)
and take a PPM screendump every ~50 ms for SECONDS (default 3) from the swipe's start. Saves
frames as OUTDIR/NNNN-<ms>.png (downscaled) and prints, per frame, the time and how much it
differs from the previous and from the last frame."""
import json, os, socket, sys, time, threading
from PIL import Image, ImageChops
sock, out = sys.argv[1], sys.argv[2]
x0, y0, x1, y1 = map(float, sys.argv[3:7]); secs = float(sys.argv[7]) if len(sys.argv) > 7 else 3
os.makedirs(out, exist_ok=True)
FILM = os.path.join(os.environ.get("TMPDIR", "/tmp"), "film.ppm")
def qmp():
    s = socket.socket(socket.AF_UNIX); s.connect(sock); f = s.makefile("rw")
    f.readline(); f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline(); return f
def cmd(f, name, **a):
    f.write(json.dumps({"execute": name, "arguments": a} if a else {"execute": name}) + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r: return r
def at(x, y):
    return [{"type": "abs", "data": {"axis": "x", "value": int(x / 1290 * 32767)}},
            {"type": "abs", "data": {"axis": "y", "value": int(y / 2796 * 32767)}}]
def swipe(f):
    print(f"send-start {time.clock_gettime_ns(time.CLOCK_BOOTTIME)}", flush=True)
    steps = 20
    cmd(f, "input-send-event", events=at(x0, y0)); cmd(f, "input-send-event", events=[{"type": "btn", "data": {"down": True, "button": "left"}}])
    for i in range(1, steps + 1):
        time.sleep(0.3 / steps)
        cmd(f, "input-send-event", events=at(x0 + (x1 - x0) * i / steps, y0 + (y1 - y0) * i / steps))
    cmd(f, "input-send-event", events=[{"type": "btn", "data": {"down": False, "button": "left"}}])
    print(f"send-end {time.clock_gettime_ns(time.CLOCK_BOOTTIME)}", flush=True)
f = qmp(); frames = []
t0 = time.time(); swipe(f)  # QMP serves one client at a time: same connection
while time.time() - t0 < secs:
    t = time.time() - t0
    cmd(f, "screendump", filename=FILM, format="ppm")
    im = Image.open(FILM).convert("RGB")
    frames.append((t, im.resize((im.width // 2, im.height // 2))))
    time.sleep(0.03)
last = frames[-1][1].convert("L")
prev = None
for i, (t, im) in enumerate(frames):
    g = im.convert("L")
    dprev = sum(ImageChops.difference(prev, g).histogram()[25:]) if prev else 0
    dlast = sum(ImageChops.difference(last, g).histogram()[25:])
    im.save(f"{out}/{i:04d}-{int(t * 1000)}.png")
    print(f"{t * 1000:6.0f} ms  vs prev {dprev:6d}  vs final {dlast:6d}")
    prev = g
