#!/bin/bash
# Build the libimobiledevice stack the restore tools link, when the distro's is missing or too old.
#
#   tools/build-imobiledevice-stack.sh   -> prints the prefix it built into (.build/imobiledevice),
#                                           or nothing when the system libraries are new enough
#
# vphone-cli's restore engine (linux-create MobileRestoreCore/MobileRecoveryCore) and the 32 KiB
# usbmuxd need libplist 2.x, libimobiledevice-glue, libusbmuxd 2.1, libtatsu and libimobiledevice
# 1.4. Fedora 44 ships them all; Ubuntu 24.04 ships libimobiledevice 1.3.0 and libusbmuxd 2.0.2,
# and no libtatsu or libimobiledevice-glue. The libraries are installed shared, with an rpath to
# the prefix, so every user loads this one copy. The system packages are left alone.
# Needs gcc, make, pkg-config, autoconf/automake/libtool, and the OpenSSL, libcurl and libusb
# development packages. All output except the prefix goes to stderr.
set -euo pipefail
cd "$(dirname "$0")/.."
if pkg-config --atleast-version=1.4.0 libimobiledevice-1.0 &&
   pkg-config --atleast-version=2.1.0 libusbmuxd-2.0 &&
   pkg-config --exists libtatsu-1.0 libimobiledevice-glue-1.0; then
  exit 0
fi
P=$PWD/.build/imobiledevice
SRC=$PWD/.build/imobiledevice-src
# name version, in dependency order
STACK="libplist 2.8.0
libimobiledevice-glue 1.3.3
libusbmuxd 2.1.1
libtatsu 1.0.5
libimobiledevice 1.4.0"
export PKG_CONFIG_PATH="$P/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export LDFLAGS="-Wl,-rpath,$P/lib${LDFLAGS:+ $LDFLAGS}"
mkdir -p "$SRC"
while read -r name ver; do
  [ -f "$P/.done-$name-$ver" ] && continue
  echo "building $name $ver into $P" >&2
  [ -d "$SRC/$name-$ver" ] || curl -fsSL \
    "https://github.com/libimobiledevice/$name/releases/download/$ver/$name-$ver.tar.bz2" | tar -xj -C "$SRC"
  ( cd "$SRC/$name-$ver" &&
    ./configure -q --prefix="$P" --libdir="$P/lib" --disable-static --without-cython >&2 &&
    make -s -j"$(nproc)" >&2 && make -s install >&2 )
  touch "$P/.done-$name-$ver"
done <<< "$STACK"
echo "$P"
