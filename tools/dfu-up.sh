#!/bin/bash
# Start a VM bundle in DFU for a Linux restore: QEMU with the virtual USB device controller,
# and tools/vusb-usbip.py exporting it over USB/IP on 127.0.0.1.
#
#   tools/dfu-up.sh <vm-dir> <run-dir>
#
# With no firmware store in nvram.bin (a new VM), the boot ROM stays in DFU mode; with a restored one
# and auto-boot=false, iBoot stops at its recovery prompt. Both are what `vphone-cli restore` needs.
# tools/dfu-stop.sh <vm-dir> stops the VM again.
# Then, as root in other terminals: tools/usbmuxd-32k.sh and tools/usbip-autoattach.sh, and run
# `vphone-cli restore`. Logs go to <run-dir> (serial.log, stderr.log, qemu.log, usbip.log).
# A VM from another run on the same disk is refused; one this <run-dir> started is restarted.
#
# Env: QEMU (default: qemu/build),
#      GFX=1 WINDOW=1 (open the display window to watch), SMP (8), VUSB_DEBUG=1 (log every URB), VUSB_URB_TIMEOUT (seconds, default 300).
set -euo pipefail
VM=$(realpath "${1:?usage: dfu-up.sh <vm-dir> <run-dir>}")
mkdir -p "${2:?usage: dfu-up.sh <vm-dir> <run-dir>}"
R=$(realpath "$2")
cd "$(dirname "$0")/.."
SOCK=$R/vusb.sock
# Never two guests on one disk (the drives are opened without locking). Checked first, before
# anything is stopped: a VM from another run on this disk is refused; this run's own is restarted.
want=$(stat -L -c %d:%i "$VM/Disk.img")
for p in $(pgrep -x qemu-system-aar || true); do
  cmd=$(tr '\0' ' ' </proc/$p/cmdline 2>/dev/null) || continue
  # By inode: a VM started with a relative or symlinked path to this disk still counts.
  hit=
  for f in $(tr '\0' '\n' </proc/$p/cmdline | grep -oE 'file=[^,]*Disk\.img' | cut -d= -f2); do
    [ "$(cd "$(readlink /proc/$p/cwd 2>/dev/null || echo /)" && stat -L -c %d:%i "$f" 2>/dev/null)" = "$want" ] && hit=1
  done
  [ -n "$hit" ] || continue
  case $cmd in *"$SOCK"*) continue ;; esac
  echo "a VM is already running $VM (QEMU pid $p); stop it first: tools/dfu-stop.sh $VM" >&2; exit 2
done
# Any exporter, not only this run's: one left from an earlier run keeps 127.0.0.1:3240 and
# serves a VM that is gone, and the new one cannot bind.
for p in $(pgrep -x python3 || true); do
  tr '\0' ' ' </proc/$p/cmdline 2>/dev/null | grep -q 'tools/vusb-usbip.py ' && kill "$p"
done
for p in $(pgrep -x qemu-system-aar || true); do
  tr '\0' ' ' </proc/$p/cmdline | grep -q -- "$SOCK" && kill "$p"
done
sleep 2
QEMU_BIN=${QEMU:-$PWD/qemu/build/qemu-system-aarch64}
[ -x "$QEMU_BIN" ] || { echo "QEMU is not built: $QEMU_BIN is missing. Run ./build.sh first (docs/SETUP.md, step 3)." >&2; exit 1; }
rm -f "$SOCK"
(QEMU=${QEMU:-$PWD/qemu/build/qemu-system-aarch64} CID=0 SMP=${SMP:-8} \
  GFX=${GFX:-0} WINDOW=${WINDOW:-0} \
  DFLAGS=guest_errors LOG=$R/qemu.log timeout 14400 \
  ./run-vphone.sh "$VM" -chardev socket,id=vusb,path=$SOCK,server=on,wait=off \
  -global vresearch-vusb.chardev=vusb > "$R/serial.log" 2> "$R/stderr.log" &)
for _ in $(seq 1 30); do [ -S "$SOCK" ] && break; sleep 1; done
[ -S "$SOCK" ] || { echo "QEMU did not start; see $R/stderr.log" >&2; exit 1; }
sleep 20   # AVPBooter reaches DFU
(VUSB_DEBUG=${VUSB_EXPORTER_DEBUG:-} VUSB_URB_TIMEOUT=${VUSB_URB_TIMEOUT:-300} timeout 14400 \
  python3 -u tools/vusb-usbip.py "$SOCK" > "$R/usbip.log" 2>&1 &)
echo "DFU VM up; exporter log $R/usbip.log. Attach with: sudo tools/usbip-autoattach.sh"
