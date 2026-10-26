#!/usr/bin/env bash
# vphone-setup, step 2: pick a device and iOS version and create the VM.
#
#   ./create-vm.sh                                 # interactive menus
#   ./create-vm.sh --device "iPhone17,3" --ios 26.6.2
#   ./create-vm.sh --list                          # devices + tested pairings
#   ./create-vm.sh --dry-run --device ... --ios ... # print the pipeline, run nothing
#
# The pipeline is vphone-cli's Linux firmware flow (docs/SETUP.md §5) with one
# state file per VM, so a re-run resumes after an interrupted step. QEMU is
# only ever STARTED here (the last step, once); this script never stops a VM.
set -Eeuo pipefail
# shellcheck disable=SC1091  # this checkout's own lib/common.sh, resolved at run time
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/common.sh"

DEVICE=${DEVICE:-} IOS=${IOS:-} CLOUDOS_PICK=${CLOUDOS_PICK:-}
SCALE=${SCALE:-1} NAME=${NAME:-} DRY=0 LIST=0
while [ $# -gt 0 ]; do
  case $1 in
    --device)  DEVICE=$2; shift ;;
    --ios)     IOS=$2; shift ;;
    --cloudos) CLOUDOS_PICK=$2; shift ;;
    --name)    NAME=$2; shift ;;
    --scale)   SCALE=$2; shift ;;   # display scale: 1 (default) | 2 | 3
    --dry-run) DRY=1 ;;
    --list)    LIST=1 ;;
    -y)        export VSETUP_ASSUME_YES=1 ;;
    *) die "unknown option: $1" ;;
  esac; shift
done

J="$VSETUP_ROOT/lib/json_query.py"

if [ "$LIST" = 1 ]; then
  echo "Available devices (tested pairings, from vphone-cli's compatibility list):"
  while IFS=$'\t' read -r id nm; do
    printf '  %-20s %s\n' "$id" "$nm"
    while IFS=$'\t' read -r v b c pr flags; do
      printf '      iOS %-10s build %-9s cloudOS %-14s preset %-12s %s\n' \
        "$v" "$b" "$c" "$pr" "$flags"
    done < <(python3 "$J" choices "$id")
  done < <(python3 "$J" devices)
  exit 0
fi

# ------------------------------------------------------------- pick device --
device_menu() {
  local i choice
  mapfile -t devs < <(python3 "$J" devices)
  echo >&2; echo "Choose the device:" >&2
  for ((i = 0; i < ${#devs[@]}; i++)); do
    printf ' %2d) %s\n' $((i + 1)) "${devs[$i]//$'\t'/' — '}" >&2
  done
  read -rp "Choice: " choice
  [[ $choice =~ ^[0-9]+$ ]] || die "not a number"
  if [ "$choice" -lt 1 ] || [ "$choice" -gt "${#devs[@]}" ]; then die "no such choice"; fi
  printf '%s\n' "${devs[choice - 1]%%	*}"
}
[ -n "$DEVICE" ] || DEVICE=$(device_menu)
python3 "$J" devices | grep -q "^$DEVICE"$'\t' || die "unknown device: $DEVICE"

# ------------------------------------------------------------ pick iOS ver --
choices() { python3 "$J" choices "$DEVICE"; }
ios_menu() {
  local i choice
  mapfile -t opts < <(choices)
  echo >&2; echo "Choose the iOS version for $DEVICE:" >&2
  for ((i = 0; i < ${#opts[@]}; i++)); do
    printf '  %2d) %s\n' $((i + 1)) "${opts[$i]//$'\t'/'  |  '}" >&2
  done
  read -rp "Choice: " choice
  [[ $choice =~ ^[0-9]+$ ]] || die "not a number"
  if [ "$choice" -lt 1 ] || [ "$choice" -gt "${#opts[@]}" ]; then die "no such choice"; fi
  printf '%s\n' "${opts[choice - 1]%%	*}"
}
[ -n "$IOS" ] || IOS=$(ios_menu)

PAIR_JSON=$(python3 "$J" find "$DEVICE" "$IOS") ||
  die "no tested pairing for $DEVICE + $IOS — try --list (source: Lakr233/vphone-cli compatibility.md)"

DEVICE_NAME=$(python3 "$J" devname "$DEVICE")
IFS=$'\t' read -r PAIR_IPSW PAIR_CLOUDOS PAIR_PRESET PAIR_BUILD PAIR_LINUX \
  PAIR_MAC PAIR_MACBUILDER PAIR_NOTE <<< "$PAIR_JSON"
export PAIR_IPSW PAIR_MAC   # both flags render in the menus

if [ "$PAIR_MACBUILDER" = 1 ]; then
  die "$DEVICE $IOS (cloudOS $PAIR_CLOUDOS) is kernel-first iOS 18: the kernelcache mix
     must be linked by a macOS kmutil (VPHONE_KMUTIL_SSH) before Linux can boot it.
     Run that step on a macOS host per vphone-cli's create-and-run.md."
fi
if [ "$PAIR_LINUX" = 0 ] && [ "$DRY" = 0 ]; then
  warn "This pairing is macOS-tested but NOT yet verified on Linux: it is under test."
  warn "The only Linux-verified pairing is iPhone17,3 + iOS 26.6.2 (cloudOS 26.4-23E5207q)."
  warn "Expect a first run to need manual attention (extra patches, CPU/RAM, restore retries)."
  confirm "Continue anyway?" || { echo "aborted."; exit 1; }
fi
[ -n "$CLOUDOS_PICK" ] && PAIR_CLOUDOS=$CLOUDOS_PICK

# ------------------------------------------------------------ the pipeline --
NAME=${NAME:-vphone-${DEVICE//,/-}-${IOS//./}}
VM=$VSETUP_DEVICES/$NAME
STATE=$VM/.create-state
[ "$DRY" = 1 ] || mkdir -p "$VM"
done_step() { grep -qxs "^$1$" "$STATE" 2>/dev/null; }
dry() { if [ "$DRY" = 1 ]; then printf '   \033[2m[dry-run]\033[0m %s\n' "$*"; else
        # shellcheck disable=SC2016  # the point is to show the raw command
        echo '+ '"$*"' (not executed in a dry run)' >&2; "$@"; fi; }
mark() { # record a finished step; the VM dir may not exist yet at mark time
  [ "$DRY" = 1 ] || { mkdir -p "$VM"; printf '%s\n' "$1" >> "$STATE"; }
}
step() { local key=$1; shift
  if done_step "$key"; then log "  • $key  (already done)"; return 0; fi
  log "▶ $key"
  if "$@"; then mark "$key"; else die "step $key failed (fix it, then re-run to resume)"; fi
}

log "Device:  $DEVICE ($DEVICE_NAME)"
log "iOS:     $IOS (build $PAIR_BUILD)   cloudOS: $PAIR_CLOUDOS   preset: $PAIR_PRESET"
[ -n "${PAIR_NOTE:-}" ] && log "Note:    $PAIR_NOTE"
log "VM:      $VM"
[ "$DRY" = 1 ] && log "DRY RUN: nothing below executes."

VC=$VSETUP_SRC/linux-create/.build/out/out/Products/Debug-linux-x86_64/vphone-cli
QEMU_RUN=$VSETUP_SRC/run-vphone.sh

step sanity && {
  if [ "$DRY" = 1 ]; then
    log "   (dry-run: binaries appear when ./setup.sh has run — skipping checks)"
  else
    local_missing=0
    for one in "$VC" "$QEMU_RUN" \
               "$VSETUP_SRC/OpenAVB/OpenAVB.bin" "$VSETUP_SRC/OpenSEP/OpenSEP.bin"; do
      [ -e "$one" ] || { warn "missing: $one — run ./setup.sh first"; local_missing=1; }
    done
    [ "$local_missing" = 0 ]
  fi
}

step bundle && {
  dry python3 "$VSETUP_ROOT/lib/make-bundle.py" "$VM" "$DEVICE" "$SCALE"
  dry cp "$VSETUP_SRC/OpenAVB/OpenAVB.bin" "$VM/OpenAVB.bin"
  dry cp "$VSETUP_SRC/OpenSEP/OpenSEP.bin" "$VM/OpenSEP.bin"
}

# Download the firmware the pairing names: iOS restore + the cloudOS image.
# vphone-cli's fw catalog resolves the CDN URLs (and its own cloudOS for a
# research-board pairing); files land in ~/.cache/vphone and are reused.
WANT_IOS=${IPSW_URL:-}   # a URL given on the command line skips the resolver
step firmware && {
  cache="$HOME/.cache/vphone"
  [ "$DRY" = 1 ] || mkdir -p "$cache"
  if [ -n "$WANT_IOS" ]; then
    IOS_URL=$WANT_IOS IOS_FILE=${IPSW_PATH:-$(basename "$WANT_IOS")}
  elif [ ! -x "$VC" ]; then
    # ./setup.sh has not built vphone-cli yet, so the fw catalog is not
    # available. Skip cleanly; IPSW_URL=/CLOUDOS_URL= can still fetch.
    log "  (vphone-cli not built: firmware resolution skipped)"
    IOS_FILE=$PAIR_IPSW IOS_URL='' CLOUD_FILE='' CLOUDOS_LOCAL=''
  else
    resolved=$(python3 "$VSETUP_ROOT/lib/get-firmware.py" "$VC" "$DEVICE" \
               "$IOS" "$PAIR_CLOUDOS") ||
      die "fw catalog has no pairing for $DEVICE $IOS — pass IPSW_URL= and CLOUDOS_URL="
    IFS=$'\t' read -r IOS_FILE IOS_URL <<< "$(echo "$resolved" | head -1)"
    IFS=$'\t' read -r CLOUD_FILE CLOUD_URL <<< "$(echo "$resolved" | sed -n 2p)"
    [ "${CLOUDOS_IPSW_URL:-}" ] && CLOUD_URL=$CLOUDOS_IPSW_URL CLOUD_FILE=$(basename "$CLOUD_URL")
    [ -s "$cache/$CLOUD_FILE" ] || dry curl -L --retry 3 -o "$cache/$CLOUD_FILE" "$CLOUD_URL"
  fi
  IOS_LOCAL=$cache/$IOS_FILE
  if [ -n "${IOS_URL:-}" ]; then
    [ -s "$IOS_LOCAL" ] || dry curl -L --retry 3 -o "$IOS_LOCAL" "$IOS_URL"
  fi
  # cloudOS for `-c` — the resolver's file (or the caller's URL)
  CLOUDOS_LOCAL=${CLOUD_FILE:+$cache/$CLOUD_FILE}
  log "  firmware: $IOS_FILE + $CLOUD_FILE (or the given URLs) under $cache"
}

step prepare && {
  # The GPU driver's cryptex keys are not public (Apple answers 403 for the
  # 23E5207q), so it has to come from a stock cloudOS restore. Look in the
  # usual places before giving up; GPU_DRIVER_BUNDLE= wins always.
  gpu_bundle=${GPU_DRIVER_BUNDLE:-}
  if [ -z "$gpu_bundle" ]; then
    for cand in "$VSETUP_RELEASE/AppleParavirtGPUMetalIOGPUFamily.bundle" \
                "$HOME/.cache/vphone/AppleParavirtGPUMetalIOGPUFamily.bundle"; do
      [ -d "$cand" ] && { gpu_bundle=$cand; break; }
    done
    if [ -z "$gpu_bundle" ]; then
      # last resort: one extracted tree under $HOME (not a guest dir)
      gpu_bundle=$(find "$HOME" -maxdepth 4 -type d \
        -name AppleParavirtGPUMetalIOGPUFamily.bundle -print -quit 2>/dev/null)
      [ -n "$gpu_bundle" ] && log "Found the GPU driver bundle at $gpu_bundle"
    fi
  fi
  [ "$DRY" = 1 ] || [ -d "$gpu_bundle" ] || die "GPU driver bundle not found (SETUP.md §5.4). Do one of:
     • restore the stock cloudOS $PAIR_CLOUDOS into a scratch bundle once and
       7zip-extract AppleParavirtGPUMetalIOGPUFamily.bundle from its System volume,
     • or fetch one from this release's GPU-driver note and copy it to
       $VSETUP_RELEASE/AppleParavirtGPUMetalIOGPUFamily.bundle,
     • or set GPU_DRIVER_BUNDLE=/path/to/AppleParavirtGPUMetalIOGPUFamily.bundle"
  local_ipsw="${IPSW_PATH:-$IOS_LOCAL}"   # from the firmware step
  cloud_arg="${CLOUDOS_LOCAL:-${CLOUDOS_IPSW_URL:-$PAIR_CLOUDOS}}"
  dry "$VC" fw prepare -l "$VSETUP_DEVICES" "$NAME" \
      -i "${IPSW_URL:-$local_ipsw}" -c "$cloud_arg" \
      --gpu-driver-bundle "$gpu_bundle"
  # iOS 27 pairings need the seal patcher, which mounts the restore ramdisk
  # read-write through linux-apfs-rw (root), and the txm-boot-precheck. The
  # patch command runs under sudo for them; a 26.x pairing patches as the user.
  case $IOS in
    27.*)
      dry sudo sh -c "modprobe apfs 2>/dev/null || insmod $VSETUP_SRC/.build/linux-apfs-rw/apfs.ko" || true
      dry sudo "$VC" fw patch --restore-txm-precheck -l "$VSETUP_DEVICES" "$NAME" ;;
    *)
      dry "$VC" fw patch -l "$VSETUP_DEVICES" "$NAME" ;;
  esac
}

step restore && {
  dry sudo "$VSETUP_SRC/tools/usbmuxd-32k.sh"      # stops the system usbmuxd only
  dry sudo "$VSETUP_SRC/tools/usbip-autoattach.sh"
  dry "$VSETUP_SRC/tools/dfu-up.sh" "$VM" "$VM/.runs-dfu"
  dry "$VC" recovery-probe
  ECID_SHOWN=$(cat "$VM/ecid" 2>/dev/null || echo 0x0)
  dry env VPHONE_RESTORE_TIMEOUT_SCALE=10 "$VC" restore \
      -l "$VSETUP_DEVICES" "$NAME" --ecid "$ECID_SHOWN" -v
}

step cfw && {
  dry bash "$VSETUP_SRC/tools/ssh-ramdisk-rebuild-signed.sh"
  dry "$VSETUP_SRC/tools/restore-tree-mode.sh" "$VM" ssh
  dry python3 "$VSETUP_SRC/tools/nvram-autoboot.py" "$VM/nvram.bin" --auto-boot false
  dry "$VSETUP_SRC/tools/dfu-up.sh" "$VM" "$VM/.runs-sshrd"
  dry env VPHONE_RESTORE_TIMEOUT_SCALE=10 VPHONE_RESTORE_BOOT_ARGS_EXTRA="serial=3" \
      "$VC" restore -l "$VSETUP_DEVICES" "$NAME" \
      --ecid "${ECID_SHOWN:-0x0}" --boot-ramdisk -v
  dry "$VSETUP_SRC/tools/ssh-ramdisk.sh" 'id'
  # A previous install (or update) renamed the gigalocker to AA.gl; the
  # ramdisk's own seputil looks for the UUID-named one and its failure panics
  # the guest seconds into mount_filesystems. Rename it back first.
  # (The single quotes are the point: the whole script runs in the guest.)
  # shellcheck disable=SC2016
  dry "$VSETUP_SRC/tools/ssh-ramdisk.sh" \
      'for n in 3 4 5 6 7; do t=$(/System/Library/Filesystems/apfs.fs/apfs.util -p /dev/disk1s$n 2>/dev/null); [ "$t" = "xART" ] && /sbin/mount_apfs /dev/disk1s$n /mnt7 2>/dev/null; done; [ -f /mnt7/AA.gl ] && mv /mnt7/AA.gl /mnt7/512E2CD6-BF8C-5D47-86CC-013714874FB4.gl; true'
  dry "$VSETUP_SRC/tools/ssh-ramdisk.sh" \
      '/bin/bash /usr/bin/mount_filesystems; /sbin/mount'
  dry sudo "$VC" cfw install -l "$VSETUP_DEVICES" "$NAME" \
      --work-parent /var/tmp --keep-artifacts
  dry "$VSETUP_SRC/tools/ssh-ramdisk.sh" \
      'sync; /sbin/umount /mnt2; /sbin/umount /mnt6; /sbin/umount /mnt1; sync'
  # 2.13 moved the snapshot flip under cfw and it takes the Disk.img path.
  dry "$VC" cfw flip-snapshot "$VM/Disk.img"
  dry python3 "$VSETUP_SRC/tools/nvram-autoboot.py" "$VM/nvram.bin" --auto-boot true
  dry "$VSETUP_SRC/tools/restore-tree-mode.sh" "$VM" full
  dry python3 "$VSETUP_SRC/tools/display-scale.py" "$VM/nvram.bin" "$SCALE"
}

# Last step — the only place QEMU is started: once, in the background, for THIS
# VM alone. Nothing ever kills or signals a VM.
if done_step cfw && ! done_step booted; then
  log "▶ first boot (vphoned answers in 1-2 min; unlock a bit later)"
  if [ "$DRY" = 1 ]; then
    # shellcheck disable=SC2086  # the env assignments are the point
    dry env GFX=${GFX:-1} WINDOW=${WINDOW:-1} "$QEMU_RUN" "$VM"
    mark booted
  else
    log "QEMU output → $VM/serial.log (stop it later via"
    log "    tools/vphoned.sh POST /v1/rpc '{\"method\":\"system.shutdown\",\"params\":{\"force\":true},\"id\":1}'"
    log "  — or Ctrl-A X in the window/serial; never stop it from the shell)."
    GFX=${GFX:-1} WINDOW=${WINDOW:-1} "$QEMU_RUN" "$VM" > "$VM/serial.log" 2>&1 &
    mark booted
    until "$VSETUP_SRC/tools/vphoned.sh" GET "/v1/health" 2>/dev/null |
        grep -q '"status"'; do sleep 10; printf '.' >&2; done
    "$VSETUP_SRC/tools/vphoned.sh" POST /v1/rpc \
      '{"method":"setup.skip","params":{"force":true},"id":1}'
    "$VSETUP_SRC/tools/vphoned.sh" POST /v1/rpc \
      '{"method":"screen.unlock","params":{},"id":1}'
    echo
    log "Unlocked — set Auto-Lock to Never and you are set."
  fi
fi

if [ "$DRY" = 1 ]; then
  log "Dry run complete — no command executed."
else
  log "VM '$NAME' ready at $VM. Re-launch any time:"
  log "  GFX=1 WINDOW=1 $QEMU_RUN $VM"
fi
