#!/usr/bin/env python3
"""thread-cpu.py PID SECONDS [INTERVAL]: each thread's CPU use (percent of one core) per INTERVAL
(default 0.5 s) over SECONDS, from /proc/PID/task/*/stat, grouped by thread name. Prints one line
per interval: the total, the vCPU threads ('CPU n/TCG') each, and the busiest other threads."""
import os
import sys
import time

pid, seconds = int(sys.argv[1]), float(sys.argv[2])
interval = float(sys.argv[3]) if len(sys.argv) > 3 else 0.5
hz = os.sysconf("SC_CLK_TCK")


def sample():
    out = {}
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            with open(f"/proc/{pid}/task/{tid}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            with open(f"/proc/{pid}/task/{tid}/comm") as f:
                name = f.read().strip()
        except OSError:
            continue
        out[tid] = (name, int(fields[11]) + int(fields[12]))
    return out


prev, t0 = sample(), time.time()
while time.time() - t0 < seconds:
    time.sleep(interval)
    cur = sample()
    use = {}
    for tid, (name, ticks) in cur.items():
        if tid in prev:
            use[(name, tid)] = (ticks - prev[tid][1]) / hz / interval * 100
    prev = cur
    vcpus = sorted(((n, u) for (n, _), u in use.items() if "TCG" in n), key=lambda x: x[0])
    others = sorted(((u, n) for (n, _), u in use.items() if "TCG" not in n and u >= 5), reverse=True)[:4]
    total = sum(use.values())
    print(f"{time.time() - t0:5.1f}s total {total:4.0f}%  vcpu "
          + " ".join(f"{u:3.0f}" for _, u in vcpus)
          + "  | " + " ".join(f"{n}:{u:.0f}" for u, n in others), flush=True)
