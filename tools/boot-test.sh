#!/bin/bash
# Boot a test run of the vphone (GPU on, window open), wait for vphoned, and unlock.
#
#   tools/boot-test.sh RUN CID [extra QEMU args...]
#
# RUN names the run directory, runs/RUN (serial.log, stderr.log, qemu.log, qmp.sock).
# VMDIR picks the VM bundle's parent (default: the migrated clone that boots to the home
# screen); never point it at vms/vphone-linux. WINDOW=0 keeps it headless. Only the VM a
# previous boot-test.sh started is stopped first (runs/*/qmp.sock in its command line);
# other sessions' QEMUs are left alone.
#
# QEMU links reims-vgpu statically: after `make` in reims-vgpu, relink QEMU
# (ninja -C qemu/build qemu-system-aarch64) before booting.
set -u
cd "$(dirname "$0")/.."
RUNS=$PWD/runs
R=$RUNS/$1; CID=$2; shift 2
VMDIR=${VMDIR:-vms/boot-lnx2}
mkdir -p "$R"
# Any QEMU binary (A/B copies are named otherwise), found by its runs/*/qmp.sock; never a phone
# tools/phone-up.sh started (-name phone-ID, docs/DENSITY.md).
mine() { for p in $(pgrep -f -- "-qmp unix:$RUNS/"); do
  case $(tr '\0' ' ' <"/proc/$p/cmdline" 2>/dev/null) in
    *"-name phone-"*) ;;
    *"$RUNS/"*/qmp.sock*) echo "$p" ;;
  esac; done; }
for p in $(mine); do kill "$p"; done
sleep 5
# A VM wedged after a GPU fault ignores SIGTERM.
for p in $(mine); do kill -9 "$p"; done
sleep 1
# Never two guests on one disk: the drives are opened with file.locking=off, so QEMU would not stop
# a second VM from writing the same APFS image (it corrupted the home-screen clone once).
for p in $(pgrep -f -- "$VMDIR/vm/Disk.img"); do
  if tr '\0' ' ' </proc/$p/cmdline 2>/dev/null | grep -q -- "-drive"; then
    echo "refusing to boot: pid $p already runs $VMDIR/vm/Disk.img" >&2
    exit 2
  fi
done
(WINDOW=${WINDOW:-1} REIMS_VGPU_LOG=${REIMS_VGPU_LOG:-info} GFX=1 CID=$CID SMP=${SMP:-8} \
  DFLAGS=guest_errors LOG=$R/qemu.log timeout 14400 \
  ./run-vphone.sh "$PWD/$VMDIR/vm" -qmp unix:$R/qmp.sock,server,nowait "$@" \
  >"$R/serial.log" 2>"$R/stderr.log" &)
start=$SECONDS
for i in $(seq 1 60); do
  CID=$CID VPHONED_TIMEOUT=8 timeout 10 tools/vphoned.sh GET /v1/health 2>/dev/null \
    | grep -q '"status":"ok"' && break
  grep -a -q 'fatal' "$R/stderr.log" && { echo "fatal t=$((SECONDS-start))"; exit 1; }
  sleep 10
done
echo "vphoned up t=$((SECONDS-start))"
for i in $(seq 1 20); do
  r=$(CID=$CID timeout 90 tools/vphoned.sh POST /v1/rpc \
    '{"method":"screen.unlock","params":{},"id":1}' 2>&1)
  echo "$r" | grep -q '"locked":false' && { echo "unlocked t=$((SECONDS-start))"; exit 0; }
  grep -a -q 'fatal' "$R/stderr.log" && { echo "fatal t=$((SECONDS-start))"; exit 1; }
  sleep 15
done
echo "not unlocked t=$((SECONDS-start))"
