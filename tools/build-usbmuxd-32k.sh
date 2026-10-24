#!/bin/bash
# Build the usbmuxd a Linux restore needs: stock usbmuxd with a 32 KiB USB MTU.
#
#   tools/build-usbmuxd-32k.sh        -> .build/usbmuxd-32k/usbmuxd (then: sudo tools/usbmuxd-32k.sh)
#
# The RestoreOS's AppleUSBDeviceMux reads 32 KiB buffers and drops any transfer shorter than its
# mux packet; stock usbmuxd sends 48 KiB packets, which reset the guest during NORData.
# Needs git, gcc, pkg-config and the libusb-1.0, libplist-2.0 and libimobiledevice-glue-1.0
# development packages (Fedora: libusb1-devel libplist-devel libimobiledevice-glue-devel).
# No autotools: the sources are compiled directly.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$PWD/.build/usbmuxd-32k
SRC=$OUT/src
COMMIT=3ded00c9985a5108cfc7591a309f9a23d57a8cba
mkdir -p "$OUT"
[ -d "$SRC/.git" ] || git clone -q https://github.com/libimobiledevice/usbmuxd "$SRC"
git -C "$SRC" checkout -q -f "$COMMIT"
sed -i 's/#define USB_MTU (3 \* 16384)/#define USB_MTU (2 * 16384)/' "$SRC/src/usb.h"
grep -q '#define USB_MTU (2 \* 16384)' "$SRC/src/usb.h"
cd "$SRC"
# HAVE_LIBUSB_HOTPLUG_API follows the device across DFU/recovery/RestoreOS re-enumerations;
# HAVE_LIBIMOBILEDEVICE stays undefined (no lockdownd preflight).
gcc -O2 -Wall -Isrc $(pkg-config --cflags libusb-1.0 libplist-2.0 libimobiledevice-glue-1.0) \
  -DPACKAGE_NAME='"usbmuxd"' -DPACKAGE_VERSION='"1.1.1-32k"' -DPACKAGE_STRING='"usbmuxd 1.1.1-32k"' \
  -DPACKAGE_URL='"https://libimobiledevice.org"' \
  -DPACKAGE_BUGREPORT='"https://github.com/libimobiledevice/usbmuxd/issues"' \
  -DHAVE_CLOCK_GETTIME=1 -DHAVE_LOCALTIME_R=1 -DHAVE_PPOLL=1 -DHAVE_SYS_TYPES_H=1 \
  -DHAVE_LIBUSB_HOTPLUG_API=1 \
  src/client.c src/device.c src/preflight.c src/log.c src/usb.c src/utils.c src/conf.c src/main.c \
  -o "$OUT/usbmuxd" $(pkg-config --libs libusb-1.0 libplist-2.0 libimobiledevice-glue-1.0) -lpthread -lm
echo "built $OUT/usbmuxd ($("$OUT/usbmuxd" --version))"
