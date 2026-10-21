#!/bin/bash
# scroll-bench.sh RUN CID [boot-test args...]: boot the home-screen clone with frame stats, open
# Settings, scroll it twice to load its icons, then scroll its list up and down for ~15 s, and print the guest frame rate per second
# (reims-vgpu stats: frames presented and the longest gap) and the vCPU load. Leaves the VM up.
# Env passes through (e.g. QEMU_TLB_UNSYNCED=2).
cd "$(dirname "$0")/.."
R=$1; CID=$2; shift 2
REIMS_VGPU_STATS=1 tools/boot-test.sh $R $CID -name vphone,debug-threads=on "$@" | tail -1
sleep 60
exec tools/scroll-measure.sh $R $CID
