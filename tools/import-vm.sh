#!/bin/bash
# Copy a VM that vphone-cli created on the Mac into vms/NAME and prepare it for Linux.
#
#   tools/import-vm.sh SOURCE NAME
#
# SOURCE is the VM's folder, local or over ssh, e.g.
#   me@my-mac.local:.vphone/machines/myphone     (vphone-cli's default library)
#   /run/media/me/usb/myphone
# The 64 GB disk is copied sparse. A second copy, vms/NAME.pristine, is kept so you
# can start over; on btrfs/XFS it costs no space.
set -euo pipefail
cd "$(dirname "$0")/.."
SRC=${1:?usage: tools/import-vm.sh SOURCE NAME}; NAME=${2:?usage: tools/import-vm.sh SOURCE NAME}
DST=vms/$NAME
[ -e "$DST" ] && { echo "$DST already exists" >&2; exit 1; }
mkdir -p "$DST"
rsync -aS "${SRC%/}/config.plist" "$DST/"
# The boot ROMs config.plist names: OpenAVB.bin/OpenSEP.bin on vphone-cli 2.10+, else Apple's.
roms=$(python3 -I - "$DST/config.plist" <<'PY'
import plistlib, sys
r = plistlib.load(open(sys.argv[1], "rb")).get("romImages") or {}
print(r.get("avpBooter") or "AVPBooter.vresearch1.bin", r.get("avpSEPBooter") or "AVPSEPBooter.vresearch1.bin")
PY
)
for f in Disk.img nvram.bin SEPStorage $roms; do
  rsync -aS --info=progress2 "${SRC%/}/$f" "$DST/"
done
# The launcher needs the machine's ECID, which config.plist stores in machineIdentifier.
python3 -I - "$DST/config.plist" > "$DST/ecid" <<'PY'
import plistlib, sys
d = plistlib.load(open(sys.argv[1], "rb"))
print(hex(plistlib.loads(d["machineIdentifier"])["ECID"]))
PY
cp -a --reflink=auto "$DST" "$DST.pristine"
echo "Imported $DST (ECID $(cat "$DST/ecid")). Start it with: GFX=1 WINDOW=1 ./run-vphone.sh $DST"
