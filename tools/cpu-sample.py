#!/usr/bin/env python3
"""cpu-sample.py SOCK SECONDS [INTERVAL]: sample each AP vCPU's PC and exception level (HMP
`info registers -a`) every INTERVAL (default 0.05 s) for SECONDS. Prints one line per sample:
`t <per cpu: I idle | K<pc> kernel | U<pc> user>`. Idle is the most common EL1 PC (XNU's WFI
loop), passed as IDLE_PC or learned as the PC most CPUs share. TCG reads a running vCPU's PC as
of its last exit, so this is a coarse view."""
import json, os, re, socket, sys, time
sock, secs = sys.argv[1], float(sys.argv[2]); iv = float(sys.argv[3]) if len(sys.argv) > 3 else 0.05
idle = int(os.environ.get("IDLE_PC", "fffffe003f23c6ec"), 16)
s = socket.socket(socket.AF_UNIX); s.connect(sock); f = s.makefile("rw")
f.readline(); f.write('{"execute":"qmp_capabilities"}\n'); f.flush(); f.readline()
t0 = time.time()
while time.time() - t0 < secs:
    f.write(json.dumps({"execute": "human-monitor-command", "arguments": {"command-line": "info registers -a"}}) + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r: break
    out = []
    for cpu, pc, el in re.findall(r"CPU#(\d+)\r?\n PC=([0-9a-f]+).*?PSTATE=\S+ \S+ (EL\d)", r["return"], re.S):
        if int(cpu) > 7: continue
        pc = int(pc, 16)
        out.append("I" if pc == idle else ("K%x" % pc if el != "EL0" else "U%x" % pc))
    print(f"{time.time() - t0:6.2f} " + " ".join(out), flush=True)
    time.sleep(iv)
