#!/bin/bash
# Start a VM bundle in DFU for a Linux restore: QEMU with the virtual USB device controller,
# and tools/vusb-usbip.py exporting it over USB/IP on 127.0.0.1.
#
#   tools/dfu-up.sh <vm-dir> <run-dir>
#
# The VM's Disk.img must hold no bootable system (a fresh, empty one), so AVPBooter stays in DFU.
# Then, as root in other terminals: tools/usbmuxd-32k.sh and tools/usbip-autoattach.sh, and run
# `vphone-cli restore`. Logs go to <run-dir> (serial.log, stderr.log, qemu.log, usbip.log).
# Only the QEMU and exporter that a previous dfu-up.sh started for <run-dir> are stopped first.
#
# Env: QEMU (default: qemu-create/build, the build with the RestoreOS's USB device controller),
#      GFX=1 WINDOW=1 (open the display window to watch), SMP (8), VUSB_DEBUG=1 (log every URB), VUSB_URB_TIMEOUT (seconds, default 300).
set -euo pipefail
VM=$(realpath "${1:?usage: dfu-up.sh <vm-dir> <run-dir>}")
mkdir -p "${2:?usage: dfu-up.sh <vm-dir> <run-dir>}"
R=$(realpath "$2")
cd "$(dirname "$0")/.."
SOCK=$R/vusb.sock
# Any exporter, not only this run's: one left from an earlier run keeps 127.0.0.1:3240 and
# serves a VM that is gone, and the new one cannot bind.
for p in $(pgrep -x python3 || true); do
  tr '\0' ' ' </proc/$p/cmdline 2>/dev/null | grep -q 'tools/vusb-usbip.py ' && kill "$p"
done
for p in $(pgrep -x qemu-system-aar || true); do
  tr '\0' ' ' </proc/$p/cmdline | grep -q -- "$SOCK" && kill "$p"
done
sleep 2
# Never two guests on one disk (the drives are opened without locking).
for p in $(pgrep -x qemu-system-aar || true); do
  if tr '\0' ' ' </proc/$p/cmdline | grep -q -- "$VM/Disk.img"; then
    echo "refusing: pid $p already runs $VM/Disk.img" >&2; exit 2
  fi
done
rm -f "$SOCK"
(QEMU=${QEMU:-$PWD/qemu-create/build/qemu-system-aarch64} CID=0 SMP=${SMP:-8} \
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
