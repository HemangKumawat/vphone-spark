#!/bin/bash
# Run as root while restoring a VM.
#
# Replaces the stock usbmuxd (48 KiB mux packets) with a build that uses a
# 32 KiB MTU.  The RestoreOS device mux (AppleUSBDeviceMux) allocates 32 KiB
# read/write buffers and drops any USB transfer shorter than the mux packet
# length, so 48 KiB host packets reset the RestoreOS during NORData.
#
# The patched instance uses its own pidfile and the standard socket, and the
# system unit is masked for this boot so udev cannot restart it.  Undo with:
#   sudo systemctl unmask --runtime usbmuxd.service
#   sudo systemctl start usbmuxd.service   # after killing the patched one
set -eu
# Built by tools/build-usbmuxd-32k.sh. The pidfile is not usbmuxd's default, so udev's
# device-remove "usbmuxd -x" cannot stop it between the restore's USB stages.
BIN=${USBMUXD_BIN:-$(cd "$(dirname "$0")/.." && pwd)/.build/usbmuxd-32k/usbmuxd}
PIDFILE=/run/usbmuxd-32k.pid
LOGFILE=/tmp/usbmuxd-32k.log
[ "$(id -u)" = 0 ] || { echo "run as root: sudo $0" >&2; exit 1; }
[ -x "$BIN" ] || { echo "missing $BIN: run tools/build-usbmuxd-32k.sh first" >&2; exit 1; }

systemctl stop usbmuxd.service 2>/dev/null || true
systemctl mask --runtime usbmuxd.service 2>/dev/null || true
pkill -x usbmuxd 2>/dev/null || true
sleep 1
rm -f /var/run/usbmuxd /var/run/usbmuxd.pid "$PIDFILE"

nohup "$BIN" -f -v \
  -S /var/run/usbmuxd \
  -P "$PIDFILE" \
  -l "$LOGFILE" \
  >/dev/null 2>&1 &
sleep 1
if [ -S /var/run/usbmuxd ]; then
  echo "patched usbmuxd running on /var/run/usbmuxd (pid $(cat "$PIDFILE" 2>/dev/null))"
  echo "log: $LOGFILE"
else
  echo "patched usbmuxd failed; see $LOGFILE" >&2
  exit 1
fi
