#!/usr/bin/env python3
"""Boot the VM with -d int,unimp streamed through a FIFO and print the
exception log around the first N aborts/undefs after a trigger line.

  firstfault.py <vm-dir> [--after 'Guarded Execution Enter'] [--n 3] [--secs 150]
"""
import argparse, collections, os, re, subprocess, tempfile, signal

ap = argparse.ArgumentParser()
ap.add_argument("vm")
ap.add_argument("--after", default="Guarded Execution Enter")
ap.add_argument("--n", type=int, default=3)
ap.add_argument("--secs", type=int, default=150)
ap.add_argument("--ctx", type=int, default=40)
a = ap.parse_args()

d = tempfile.mkdtemp()
fifo = os.path.join(d, "int.fifo")
os.mkfifo(fifo)
env = dict(os.environ, LOG=fifo, DFLAGS=os.environ.get("DFLAGS", "int,unimp"),
           SMP=os.environ.get("SMP", "8"))
p = subprocess.Popen(["timeout", str(a.secs), os.path.expanduser("~/vphone-linux/run-vphone.sh"), a.vm],
                     env=env, stdout=open(os.path.join(a.vm, "ff.out"), "w"),
                     stderr=subprocess.STDOUT, start_new_session=True)
hist = collections.deque(maxlen=a.ctx)
started = False; count = 0; first = None
with open(fifo, errors="replace") as f:
    for line in f:
        if a.after in line:
            started = True
        if not started:
            continue
        hist.append(line.rstrip())
        if line.startswith("Taking exception") and re.search(r"Abort|Undefined", line):
            count += 1
            for _ in range(6):
                hist.append(next(f).rstrip())
            if count == 1:
                first = list(hist)
            if count >= a.n:
                break
os.killpg(p.pid, signal.SIGTERM)
print("\n".join(first or list(hist)))
