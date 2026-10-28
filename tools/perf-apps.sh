#!/bin/bash
# perf-apps.sh RUN CID [boot-test args...]: boot the home-screen clone (tools/boot-test.sh), then
# perf-record its QEMU for 40 s while opening Calendar, Settings, Maps and Photos (each followed
# by Home), then shut it down. Prints the guest-code share of vCPU time and the top symbols.
# Env passes through (e.g. VPHONE_RET_PREFETCH=1).
cd "$(dirname "$0")/.."
R=$1; CID=$2; shift 2
S=${PERF_OUT:-/tmp/claude-1000/perf-apps}; mkdir -p $S
tools/boot-test.sh $R $CID -name vphone,debug-threads=on "$@" | tail -1
P=
for p in $(pgrep -f -- "-qmp unix:$PWD/runs/$R/"); do
  [ "$(cat /proc/$p/comm 2>/dev/null)" = qemu-system-aar ] && P=$p
done
[ -n "$P" ] || { echo "no VM"; exit 1; }
sleep 20
perf record -F 999 -g -p $P -o $S/$R.perf -- sleep 40 >/dev/null 2>&1 & PERF=$!
sleep 1
for app in "497 1015" "1089 1953" "200 1953" "794 1015"; do
  tools/qmp.py runs/$R/qmp.sock tap $app; sleep 7; tools/qmp.py runs/$R/qmp.sock home; sleep 3
done
wait $PERF
CID=$CID VPHONED_TIMEOUT=30 timeout 40 tools/vphoned.sh POST /v1/rpc '{"method":"system.shutdown","params":{"force":true},"id":1}' >/dev/null 2>&1
for i in $(seq 1 40); do kill -0 $P 2>/dev/null || break; sleep 3; done
kill $P 2>/dev/null
C="CPU 0/TCG,CPU 1/TCG,CPU 2/TCG,CPU 3/TCG,CPU 4/TCG,CPU 5/TCG,CPU 6/TCG,CPU 7/TCG"
perf report -i $S/$R.perf --no-children --comms "$C" --sort dso --stdio -g none 2>/dev/null | grep "%" | head -3
perf report -i $S/$R.perf --no-children --comms "$C" --sort symbol --stdio -g none --percent-limit 1.2 2>/dev/null | grep "%" | awk '{print $1,$3}' | head -10
