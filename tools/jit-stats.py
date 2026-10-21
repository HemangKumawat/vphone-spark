#!/usr/bin/env python3
# jit-stats.py SOCK SECONDS: QEMU's JIT counters (HMP "info jit") now and SECONDS later, and their rates.
import json, re, socket, sys, time
def info(sock):
    s = socket.socket(socket.AF_UNIX); s.connect(sock); f = s.makefile("rw")
    f.readline(); f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline()
    f.write(json.dumps({"execute": "human-monitor-command", "arguments": {"command-line": "info jit"}}) + "\n"); f.flush()
    t = json.loads(f.readline())["return"]
    num = lambda k: int(re.search(k + r"\s+(\d+)", t).group(1))
    return {"code": num("gen code size"), "tbs": num("TB count"), "flushes": num("TB flush count"),
            "invalidates": num("TB invalidate count"), "tlb_partial": num("TLB partial flushes")}
a = info(sys.argv[1]); time.sleep(float(sys.argv[2])); b = info(sys.argv[1]); dt = float(sys.argv[2])
for k in a:
    print(f"{k:12} {b[k]-a[k]:>14,} ({(b[k]-a[k])/dt:,.0f}/s)")
