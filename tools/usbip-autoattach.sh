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
# A device is attached only once it has been offered for ATTACH_SETTLE seconds (default 3):
# attaching the instant iBoot's recovery-mode device appears made iBoot panic during enumeration
# (DGX Spark: "Entering recovery mode" then "iBoot Panic" in the same second as the attach).
SETTLE=${ATTACH_SETTLE:-3}
modprobe vhci-hcd
echo "attaching devices offered by $HOST (Ctrl-C to stop; settle ${SETTLE}s)"
seen= since=0
while true; do
  # usbip port often cannot name the remote host ("unknown host"), so any import counts.
  if ! usbip port 2>/dev/null | grep -q "Port in Use"; then
    offer=$(usbip list -r "$HOST" 2>/dev/null | grep -m1 -E '^ *[0-9]+-[0-9.]+: ')
    busid=$(printf '%s\n' "$offer" | sed -nE 's/^ *([0-9]+-[0-9.]+): .*/\1/p')
    if [ -z "$busid" ]; then
      seen=
    elif [ "$offer" != "$seen" ]; then
      seen=$offer since=$(date +%s)
    elif [ $(( $(date +%s) - since )) -ge "$SETTLE" ]; then
      if usbip attach -r "$HOST" -b "$busid" 2>/dev/null; then
        echo "$(date +%T) attached $(printf '%s' "$offer" | sed 's/^ *//')"
      fi
      seen=
    fi
  else
    seen=
  fi
  sleep 0.5
done
