#!/bin/bash
# Change the iPhone's screen scale (1x, 2x or 3x) in one step. Run it while the VM is running.
#
#   tools/set-scale.sh vms/myphone 1        # 1x: 448x932 pixels, about 9 times fewer than 3x
#   tools/set-scale.sh vms/myphone 2        # 2x: 896x1864
#   tools/set-scale.sh vms/myphone 3        # back to iOS's default 3x (1290x2796)
#
# 1. Deletes iOS's MobileGestalt cache through vphoned. iOS keeps the screen scale it worked out at
#    an earlier boot there (ArtworkDeviceScaleFactor), and vphoned only clears that cache when the
#    device tree changes, which a scale change does not: without this step the next boot keeps the
#    old scale and a 1x screen shows a 3x interface, zoomed in.
# 2. Shuts the guest down and waits for QEMU to exit.
# 3. Patches LLB's display scale in nvram.bin (tools/display-scale.py) and writes the matching
#    display size to the VM's gfx-res (or removes it for 3x).
# Then start the VM again. At 1x, run `python3 tools/guest-1x-logo.py <CID>` once it is up: iOS has
# no 1x boot logo, so the boot screen stays black without it.
set -euo pipefail
cd "$(dirname "$0")/.."
VM=${1:?usage: set-scale.sh <vm-dir> 1|2|3}
SCALE=${2:?usage: set-scale.sh <vm-dir> 1|2|3}
case $SCALE in
  1) RES=448x932 ;;
  2) RES=896x1864 ;;
  3) RES= ;;
  *) echo "scale must be 1, 2 or 3" >&2; exit 1 ;;
esac
[ -f "$VM/nvram.bin" ] || { echo "$VM/nvram.bin not found" >&2; exit 1; }

# The QEMU that has this VM's disk open (compared by inode, so a symlinked path still matches).
want=$(stat -L -c %d:%i "$VM/Disk.img")
pid=
for p in $(pgrep -x qemu-system-aar || true); do
  for f in $(tr '\0' '\n' </proc/$p/cmdline 2>/dev/null | grep -oE 'file=[^,]*Disk\.img' | cut -d= -f2); do
    [ "$(stat -L -c %d:%i "$f" 2>/dev/null)" = "$want" ] && pid=$p
  done
done
[ -n "$pid" ] || { echo "start the VM first: this clears iOS's cached screen scale through vphoned, then shuts the VM down" >&2; exit 1; }
cid=$(tr '\0' '\n' </proc/$pid/cmdline | grep -oE 'guest-cid=[0-9]+' | head -1 | cut -d= -f2)
cid=${CID:-${cid:-3}}

CACHE=/private/var/containers/Shared/SystemGroup/systemgroup.com.apple.mobilegestaltcache/Library/Caches/com.apple.MobileGestalt.plist
echo "=== clearing iOS's cached screen scale (vsock CID $cid)"
CID=$cid timeout 60 tools/vphoned.sh POST /v1/rpc \
  "{\"method\":\"files.remove\",\"params\":{\"path\":\"$CACHE\",\"force\":true},\"id\":1}" >/dev/null 2>&1 \
  || echo "  (no cache to remove, or vphoned did not answer; continuing)"

echo "=== shutting the VM down"
CID=$cid timeout 60 tools/vphoned.sh POST /v1/rpc '{"method":"system.shutdown","params":{"force":true},"id":1}' >/dev/null 2>&1 || true
for _ in $(seq 1 120); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
if kill -0 "$pid" 2>/dev/null; then
  echo "the guest did not shut down within 2 minutes; stop QEMU (Ctrl-A X in its terminal) and run this again" >&2
  exit 1
fi

echo "=== setting ${SCALE}x"
python3 tools/display-scale.py "$VM/nvram.bin" "$([ "$SCALE" = 3 ] && echo 0 || echo "$SCALE")"
if [ -n "$RES" ]; then echo "$RES" > "$VM/gfx-res"; else rm -f "$VM/gfx-res"; fi
echo "=== done: start it again with  GFX=1 WINDOW=1 ./run-vphone.sh $VM"
[ "$SCALE" = 1 ] && echo "    then, once it is up:  python3 tools/guest-1x-logo.py $cid   (the boot logo at 1x)"
exit 0
