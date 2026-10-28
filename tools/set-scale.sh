#!/bin/bash
# Change the iPhone's screen scale (1x, 2x or 3x) in one step. Run it while the VM is running.
#
#   tools/set-scale.sh vms/myphone 2        # 2x: 2.25 times fewer pixels than 3x, same layout
#   tools/set-scale.sh vms/myphone 1        # 1x: about 9 times fewer (iOS 26 only, see below)
#   tools/set-scale.sh vms/myphone 3        # back to iOS's default 3x
#
# The display size follows the VM's own 3x size (its gfx-res, else 1290x2796), kept the first
# time as gfx-res.3x: exactly the same points at the new scale. An iPhone 17 Pro Max is 430x932 at
# 1x and 860x1864 at 2x; the iPhone Duo (1398x2034) is 932x1356 at 2x. Keep the points exact: iOS
# picks a device's own layout (the Duo's status bar and navigation) by its screen size in points,
# and a wider screen gets the generic iPhone one. ALIGN32=1 rounds the width up to 32 pixels
# instead (NVIDIA's 128-byte rows, fewer GPU copies), at the cost of a few points of width.
#
# 1. Sets the scale in the guest's own device tree through vphoned (tools/guest-dt-scale.py):
#    `cfw install` copies the board's /product artwork-scale-factor into it, and iOS 27 takes the
#    screen scale from there too, so without this a 2x screen shows a 3x interface, zoomed in.
# 2. Deletes iOS's MobileGestalt cache through vphoned. iOS keeps the screen scale it worked out at
#    an earlier boot there (ArtworkDeviceScaleFactor), and vphoned only clears that cache when the
#    device tree changes in a way it notices.
# 3. Shuts the guest down and waits for QEMU to exit.
# 4. Patches LLB's display scale in nvram.bin (tools/display-scale.py) and writes the matching
#    display size to the VM's gfx-res.
# Then start the VM again. At 1x, run `python3 tools/guest-1x-logo.py <CID>` once it is up: iOS has
# no 1x boot logo, so the boot screen stays black without it.
#
# iOS 27 does not go below 2x: with everything set to 1 the iPhone Duo on 27.1 still reported
# UIScreen.scale 2 (a 1x screen then shows a 2x interface). Use 2 there.
set -euo pipefail
cd "$(dirname "$0")/.."
VM=${1:?usage: set-scale.sh <vm-dir> 1|2|3}
SCALE=${2:?usage: set-scale.sh <vm-dir> 1|2|3}
case $SCALE in 1|2|3) ;; *) echo "scale must be 1, 2 or 3" >&2; exit 1 ;; esac
[ -f "$VM/nvram.bin" ] || { echo "$VM/nvram.bin not found" >&2; exit 1; }
# The VM's 3x display size: kept in gfx-res.3x once; until then gfx-res when LLB is still at 3x.
if [ ! -f "$VM/gfx-res.3x" ]; then
  if python3 tools/display-scale.py "$VM/nvram.bin" | grep -q 'boot video (3x)'; then
    cat "$VM/gfx-res" 2>/dev/null || echo 1290x2796
  else
    echo "the VM is not at 3x and has no gfx-res.3x: write its 3x size (e.g. 1290x2796) to $VM/gfx-res.3x" >&2
    exit 1
  fi > "$VM/gfx-res.3x.new" && mv "$VM/gfx-res.3x.new" "$VM/gfx-res.3x"
fi
IFS=x read -r W3 H3 < "$VM/gfx-res.3x"
if [ "$SCALE" = 3 ]; then
  RES=${W3}x${H3}
else
  if [ "${ALIGN32:-0}" = 1 ]; then
    RES=$(( (W3 / 3 + 31) / 32 * 32 * SCALE ))x$(( H3 / 3 * SCALE ))
  else
    RES=$(( W3 / 3 * SCALE ))x$(( H3 / 3 * SCALE ))
  fi
fi

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
echo "=== setting ${SCALE}x in the guest's device tree (vsock CID $cid)"
python3 tools/guest-dt-scale.py "$cid" "$SCALE" \
  || echo "  (the device tree was not changed; on iOS 26 the LLB patch alone sets the scale)"
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
echo "$RES" > "$VM/gfx-res"
echo "    display $RES"
echo "=== done: start it again with  GFX=1 WINDOW=1 ./run-vphone.sh $VM"
[ "$SCALE" = 1 ] && echo "    then, once it is up:  python3 tools/guest-1x-logo.py $cid   (the boot logo at 1x)"
exit 0
