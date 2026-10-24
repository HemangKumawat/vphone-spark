#!/bin/bash
# Run as root while restoring a VM over USB/IP (tools/vusb-usbip.py):
#
#   sudo tools/usbip-autoattach.sh
#
# A restore moves the guest through several boot stages (DFU, iBoot, the restore OS), and each
# presents a new USB device. The exporter unplugs the old one and offers the new one; this
# attaches whatever 127.0.0.1 offers whenever nothing from it is attached. Ctrl-C to stop.
# It only ever talks to the local exporter.
set -u
HOST=127.0.0.1
[ "$(id -u)" = 0 ] || { echo "run as root: sudo $0" >&2; exit 1; }
modprobe vhci-hcd
echo "attaching devices offered by $HOST (Ctrl-C to stop)"
while true; do
  # usbip port often cannot name the remote host ("unknown host"), so any import counts.
  if ! usbip port 2>/dev/null | grep -q "Port in Use"; then
    busid=$(usbip list -r "$HOST" 2>/dev/null | sed -nE 's/^ *([0-9]+-[0-9.]+): .*/\1/p' | head -1)
    if [ -n "$busid" ]; then
      if usbip attach -r "$HOST" -b "$busid" 2>/dev/null; then
        echo "$(date +%T) attached $(usbip list -r "$HOST" 2>/dev/null | grep -m1 "$busid:" | sed 's/^ *//')"
      fi
    fi
  fi
  sleep 0.5
done
