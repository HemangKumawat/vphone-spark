#!/usr/bin/env python3
"""screen-changed.py SOCK BEFORE.ppm: take a PPM screendump and exit 0 if it differs clearly from
BEFORE (an earlier screendump), else 1. Without BEFORE: just save SOCK's screen to stdout path
given as the only other argument? No: usage `screen-changed.py SOCK save PATH` saves one."""
import json, os, socket, sys
from PIL import Image, ImageChops

def shot(sock, path):
    s = socket.socket(socket.AF_UNIX); s.connect(sock); f = s.makefile("rw")
    f.readline(); f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline()
    f.write(json.dumps({"execute": "screendump", "arguments": {"filename": path, "format": "ppm"}}) + "\n"); f.flush()
    f.readline()
    return Image.open(path).convert("L").resize((86, 186))

sock = sys.argv[1]
if sys.argv[2] == "save":
    shot(sock, sys.argv[3]); sys.exit(0)
before = Image.open(sys.argv[2]).convert("L").resize((86, 186))
now = shot(sock, os.path.join(os.environ.get("TMPDIR", "/tmp"), "screen-now.ppm"))
d = sum(ImageChops.difference(before, now).histogram()[30:])
sys.exit(0 if d > 3000 else 1)
