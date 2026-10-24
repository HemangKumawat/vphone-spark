#!/bin/bash
# Rebuild the SSH ramdisk from the (re-signed) SSH staging tree.
#
# The staging tree's Mach-Os were re-signed with
#   vphone-cli sign --apple-adhoc --merge
# so TXM selector 24 takes its ad-hoc branch instead of the CMS gate.
#
# Usage (as your normal user; it asks for sudo once, for the APFS mount):
#   bash tools/ssh-ramdisk-rebuild-signed.sh <library> <vm>
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
LIB=${1:?usage: ssh-ramdisk-rebuild-signed.sh <library> <vm>   (restore tree in full mode)}
VM=${2:?usage: ssh-ramdisk-rebuild-signed.sh <library> <vm>}
STOCK="$ROOT/runs/sshramdisk/stock-094-39914-031.dmg"
BASE="$ROOT/runs/sshramdisk/base/apfs.img"
STAGE="$ROOT/runs/sshramdisk/ssh-root"
WORK="$ROOT/runs/sshramdisk/base-signed"
PAYLOAD="$WORK/apfs.img"
OUT="$ROOT/runs/sshramdisk/out/094-39914-031.dmg"
MNT=/mnt/vphone-sshramdisk

# The stock restore ramdisk and its APFS payload come from the VM's restore tree, once.
if [ ! -f "$STOCK" ]; then
    shopt -s nullglob; trees=("$LIB/$VM"/*_Restore)
    [ ${#trees[@]} -eq 1 ] || { echo "expected one *_Restore tree in $LIB/$VM" >&2; exit 1; }
    [ -e "${trees[0]}/094-39914-031.dmg.full-restore" ] && { echo "tree is in ssh mode; run restore-tree-mode.sh ... full first" >&2; exit 1; }
    mkdir -p "$(dirname "$STOCK")"
    cp "${trees[0]}/094-39914-031.dmg" "$STOCK"
    cp "${trees[0]}/Firmware/094-39914-031.dmg.trustcache" "$STOCK.trustcache"
fi
if [ ! -f "$BASE" ]; then
    mkdir -p "$(dirname "$BASE")"
    tail -c +28 "$STOCK" > "$BASE"     # the APFS container starts at byte 27 of the IM4P
fi

for f in "$STOCK" "$BASE" "$STAGE/usr/local/bin/dropbear"; do
    [ -e "$f" ] || { echo "missing $f" >&2; exit 1; }
done

echo "=== fresh APFS copy (stock base is left untouched)"
rm -rf "$WORK"
mkdir -p "$WORK"
cp "$BASE" "$PAYLOAD"

echo "=== inject signed staging tree (needs sudo for losetup/mount)"
sudo "$ROOT/tools/ssh-ramdisk-inject.sh" "$PAYLOAD" "$STAGE" "$MNT"

echo "=== repack into the stock rdsk IM4P wrapper"
"$ROOT/tools/ssh-ramdisk-repack.sh" "$STOCK" "$PAYLOAD" "$OUT"

echo "=== built $OUT; install it with: tools/restore-tree-mode.sh $LIB $VM ssh"
echo "=== done"
sha1sum "$OUT"
