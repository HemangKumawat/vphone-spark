#!/usr/bin/env bash
# vphone-setup, step 1: turn a bare Linux machine into a working vphone-linux
# build (dependencies, sources, release bundle, all binaries).
#
#   ./setup.sh [<phase>] [-y] [--src DIR] [--workdir DIR]
#
# Phases: deps, sources, build, release, all (default). Re-runnable; each
# phase skips work that is already done.
#
#   ./setup.sh                       # everything, asking before sudo apt/dnf
#   ./setup.sh -y                    # everything, no prompts
#
# This script never runs a VM or QEMU; it only builds binaries.
set -Eeuo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/common.sh"

VSETUP_ASSUME_YES=${VSETUP_ASSUME_YES:-0}
VSETUP_PHASE=all
while [ $# -gt 0 ]; do
  case $1 in
    -y|--yes)        VSETUP_ASSUME_YES=1 ;;
    --src)           VSETUP_SRC=$2; shift ;;
    --workdir)       mkdir -p "$2"; VSETUP_SRC=$2/src; VSETUP_RELEASE=$2/vphone-release; shift ;;
    deps|sources|build|release|all) VSETUP_PHASE=$1 ;;
    *) die "unknown option: $1" ;;
  esac; shift
done

do_phase() { [ "$VSETUP_PHASE" = "$1" ] || [ "$VSETUP_PHASE" = all ]; }

# ---------------------------------------------------------------- 1. deps ---
pkg_feeds() {
  local distro; distro=${1%%:*}
  log " Installing splash-screen packages for $distro"
  case $distro in
    fedora)
      sudo dnf install -y git gcc gcc-c++ clang binutils make cmake ninja-build meson \
        python3 python3-pillow glib2-devel pixman-devel lzfse-devel libslirp-devel \
        libssh-devel libzstd-devel nettle-devel gnutls-devel gtk3-devel \
        sdl2-compat-devel bzip2-devel libX11-devel wayland-devel libxkbcommon-devel \
        vulkan-loader vulkan-loader-devel vulkan-tools \
        socat gdb perf \
        usbip fuse-sshfs fuse3 7zip kernel-devel \
        libusb1-devel libplist-devel libimobiledevice-glue-devel openssl-devel libcurl-devel ;;
    ubuntu|debian|linuxmint)
      sudo apt-get update
      sudo apt-get install -y git gcc g++ clang binutils make cmake ninja-build meson \
        python3 python3-pil python3-setuptools pkg-config \
        libglib2.0-dev libpixman-1-dev liblzfse-dev libslirp-dev libssh-dev \
        libzstd-dev libnettle-dev libgnutls28-dev libgtk-3-dev libsdl2-dev \
        libbzip2-dev libx11-dev libxkbcommon-dev wayland libwayland-dev \
        libvulkan-dev mesa-vulkan-drivers vulkan-tools \
        socat gdb linux-tools-generic \
        usbip sshfs fuse3 p7zip-full linux-headers-generic \
        libusb-1.0-0-dev libplist-dev libimobiledevice-glue-dev libssl-dev libcurl4-openssl-dev ;;
    arch|cachyos|manjaro)
      sudo pacman -S --needed --noconfirm git gcc clang binutils make cmake ninja meson \
        python python-pillow glib2 pixman lzfse libslirp libssh zstd nettle gnutls gtk3 \
        sdl2 bzip2 libx11 libxkbcommon wayland vulkan-icd-loader vulkan-tools \
        socat gdb perf usbip sshfs fuse3 p7zip linux-headers \
        libusb libplist openssl curl ;;
    *) die "unsupported distro: $1 — install the packages from docs/SETUP.md §2/§5.1 manually." ;;
  esac
}

do_phase deps && {
  if [ "$(id -u)" = 0 ]; then
    pkg_feeds
  else
    confirm "Install the system packages with sudo? (chooser: $(detect_distro))" &&
      pkg_feeds "$(detect_distro)"
  fi

  # rustup (reims-vgpu) — its own pins come from rust-toolchain.toml
  if ! have cargo; then
    log " Installing rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    # shellcheck disable=SC1091  # rustup's env file, outside this checkout
    . "$HOME/.cargo/env"
  fi
  if ! have rcodesign; then
    log " Installing rcodesign (apple-codesign)"
    cargo install apple-codesign --locked
  fi
  # swiftly (Swift for the vphone-cli Linux build)
  if ! have swift; then
    log " Installing swiftly + Swift 6.4.1"
    curl -O https://download.swift.org/swiftly/linux/swiftly.tar.gz
    tar -zxf swiftly.tar.gz && rm swiftly.tar.gz
    ./swiftly install -y 6.4.1  # swiftly 6.4.1; ask: 'swiftly install 6.4.1' if script name changes
    rm -f ./swiftly
    # shellcheck disable=SC1091  # swiftly's env file, outside this checkout
    if [ -r "$HOME/.local/share/swiftly/env.sh" ]; then
      . "$HOME/.local/share/swiftly/env.sh"
    else
      # shellcheck disable=SC1091
      . "$HOME/.swiftly/env.sh"
    fi
  fi
  # sysctl for reims-vgpu's guest-memory mappings
  if [ "$(cat /proc/sys/vm/max_map_count)" -lt 1048576 ]; then
    warn "vm.max_map_count is $(cat /proc/sys/vm/max_map_count); add"
    warn "  'vm.max_map_count = 1048576' to /etc/sysctl.d/vphone.conf  (sudo)"
  fi
  # vhost-vsock is the default control channel
  if ! [ -e /dev/vhost-vsock ] && [ "$(id -u)" = 0 ]; then
    modprobe vhost_vsock || true
  fi
  [ -e /dev/vhost-vsock ] || warn "/dev/vhost-vsock missing: boot with VSOCK=uds, or load vhost_vsock"
}

# ------------------------------------------------------------- 2. sources ---
# The setup lives IN the checkout it builds (VSETUP_SRC defaults to it); a
# --src DIR still points the build at a second checkout.
do_phase sources && {
  if [ ! -d "$VSETUP_SRC/.git" ]; then
    log " Cloning vphone-linux ($VPHONE_LINUX_BRANCH) into $VSETUP_SRC"
    mkdir -p "$(dirname "$VSETUP_SRC")"
    git clone --branch "$VPHONE_LINUX_BRANCH" \
      https://github.com/YahiaAngelo/vphone-linux.git "$VSETUP_SRC"
  fi
  log " Fetching submodules (single level — NOT --recursive)"
  git -C "$VSETUP_SRC" submodule update --init
  git -C "$VSETUP_SRC/qemu" submodule update --init util/mlib
}

# --------------------------------------------------------------- 3. build ---
build_reims() {
  if [ ! -f "$VSETUP_SRC/reims-vgpu/target/dist-vulkan/lib/libreims_vgpu.a" ]; then
    log " Building reims-vgpu (~the long first build: its static lib is ~800 MB)"
    make -C "$VSETUP_SRC/reims-vgpu"
  fi
}
build_qemu() {
  local pkgcfg="$VSETUP_SRC/reims-vgpu/target/dist-vulkan/lib/pkgconfig"
  if [ ! -x "$VSETUP_SRC/qemu/build/qemu-system-aarch64" ] ||
     [ "$VSETUP_SRC/reims-vgpu/target/dist-vulkan/lib/libreims_vgpu.a" -nt \
       "$VSETUP_SRC/qemu/build/qemu-system-aarch64" ]; then
    log " Building QEMU (-M vresearch101) against reims-vgpu"
    mkdir -p "$VSETUP_SRC/qemu/build"
    ( export PKG_CONFIG_PATH=$pkgcfg
      cd "$VSETUP_SRC/qemu/build"
      [ -f config.status ] || ../configure --target-list=aarch64-softmmu --enable-pvg \
        --enable-lzfse --enable-slirp --enable-libssh --enable-zstd --enable-nettle \
        --enable-gnutls --enable-gtk --enable-sdl --disable-werror --disable-qom-cast-debug
      ninja qemu-system-aarch64 )
  fi
}
build_creator() {
  log " Building vphone-cli's firmware tooling for Linux (vphone-cli $VPHONE_CLI_PUBLIC)"
  local src="$VSETUP_SRC/cpu-tree/vphone-cli-$VPHONE_CLI_PUBLIC"
  mkdir -p "$(dirname "$src")"
  if [ ! -d "$src/.git" ]; then
    git clone --depth 1 --branch "$VPHONE_CLI_PUBLIC" "$VPHONE_CLI_REPO" "$src"
  fi
  ( cd "$VSETUP_SRC/linux-create"
    VPHONE_CLI="$src" ./sync-sources.sh
    swift build --build-system swiftbuild --scratch-path .build/out )
}
build_usbmuxd() { "$VSETUP_SRC/tools/build-usbmuxd-32k.sh"; }

do_phase build && for step in reims qemu creator usbmuxd; do
  "build_$step"
done

# ------------------------------------------------------------- 4. release ---
do_phase release && {
  rel="$VSETUP_RELEASE/app"
  if [ ! -d "$rel/VPhone.bundle/Contents/Resources/OpenAVB" ]; then
    log " Fetching the vphone-cli $VPHONE_CLI_PUBLIC release bundle (ROMs + guest resources)"
    zip="$VSETUP_RELEASE/VPhone-$VPHONE_CLI_PUBLIC.zip"
    mkdir -p "$rel"
    curl -L --retry 3 -o "$zip" \
      "https://github.com/Lakr233/vphone-cli/releases/download/$VPHONE_CLI_PUBLIC/VPhone-$VPHONE_CLI_PUBLIC.zip"
    unzip -q "$zip" -d "$rel"
    # Release zips have carried both layouts: app/VPhone.bundle (2.12) and
    # VPhone.bundle at the root (2.13+). Normalize to app/VPhone.bundle.
    [ -d "$rel/VPhone.bundle" ] && [ ! -d "$rel/app" ] && {
      mkdir -p "$rel/app" && mv "$rel/VPhone.bundle" "$rel/app/"
    }
  fi
  log " Setting up OpenAVB/OpenSEP ROMs and guest-resources symlinks"
  r="$rel/VPhone.bundle/Contents/Resources"
  ( cd "$VSETUP_SRC"
    ln -sfn "$r/guest-resources" guest-resources
    mkdir -p OpenAVB OpenSEP
    cp "$r/OpenAVB/OpenAVB.bin" OpenAVB/OpenAVB.bin 2>/dev/null || cp "$r/OpenAVB.bin" OpenAVB/OpenAVB.bin
    cp "$r/OpenSEP/OpenSEP.bin" OpenSEP/OpenSEP.bin 2>/dev/null || cp "$r/OpenSEP.bin" OpenSEP/OpenSEP.bin )
}

# ---- GPU driver: needs a temporary stock cloudOS restore; interactive. ----
if [ "$VSETUP_PHASE" = all ] || [ "$VSETUP_PHASE" = release ]; then
  [ -d "$VSETUP_RELEASE/AppleParavirtGPUMetalIOGPUFamily.bundle" ] || {
    log "The paravirt GPU driver bundle is not extracted yet."
    log "Run tools/gpu-driver-from-cloudos.sh (restores a stock cloudOS into a scratch"
    log "bundle once, then extracts it). Headless and safe; ~1 h. Or skip it: a VM works"
    log "headless without it, but no graphics. Run:  sudo $VSETUP_SRC/tools/gpu-driver-from-cloudos.sh"
  }
fi

log "Setup complete: binaries under $VSETUP_SRC, ROMs under $VSETUP_RELEASE."
log "Next: ./create-vm.sh to pick the device and the iOS version and build the VM."
