#!/bin/bash
# Stop the VM that tools/dfu-up.sh (or run-vphone.sh) runs from a VM directory, and its USB/IP exporter.
#
#   tools/dfu-stop.sh vms/myphone
#
# Used between the steps of creating a VM on Linux (docs/CREATE-ON-LINUX.md): after the restore, and
# after the SSH ramdisk. Only the QEMU that has this VM's Disk.img open is stopped; other VMs are left
# alone.
set -euo pipefail
VM=$(realpath "${1:?usage: dfu-stop.sh <vm-dir>}")
want=$(stat -L -c %d:%i "$VM/Disk.img")
stopped=0
for p in $(pgrep -x qemu-system-aar || true); do
  cmd=$(tr '\0' ' ' </proc/$p/cmdline 2>/dev/null) || continue
  # Compared by inode, so a VM started through a symlinked path (vms/boot-x/vm) still matches.
  hit=
  for f in $(tr '\0' '\n' </proc/$p/cmdline | grep -oE 'file=[^,]*Disk\.img' | cut -d= -f2); do
    [ "$(cd "$(readlink /proc/$p/cwd 2>/dev/null || echo /)" && stat -L -c %d:%i "$f" 2>/dev/null)" = "$want" ] && hit=1
  done
  [ -n "$hit" ] || continue
  sock=$(printf '%s' "$cmd" | grep -oE 'path=[^, ]*vusb\.sock' | head -1 | cut -d= -f2 || true)
  kill "$p" && echo "stopped QEMU $p"
  for _ in $(seq 1 20); do kill -0 "$p" 2>/dev/null || break; sleep 0.5; done
  kill -0 "$p" 2>/dev/null && { kill -9 "$p"; echo "QEMU $p did not exit; killed"; }
  stopped=$((stopped + 1))
  if [ -n "$sock" ]; then
    for e in $(pgrep -x python3 || true); do
      tr '\0' ' ' </proc/$e/cmdline 2>/dev/null | grep -q -- "vusb-usbip.py $sock" && kill "$e" && echo "stopped exporter $e"
    done
  fi
done
[ "$stopped" -gt 0 ] || echo "no VM is running $VM"
