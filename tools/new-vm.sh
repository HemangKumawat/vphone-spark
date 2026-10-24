#!/bin/bash
# Make an empty VM bundle for creating the VM on Linux (docs/CREATE-ON-LINUX.md, step 3).
#
#   tools/new-vm.sh vms/myphone [ECID]
#
# Writes what vphone-cli's `vm new` would: config.plist (vresearch101, 8 CPUs, 8 GiB, the iPhone
# 1290x2796 screen, OpenAVB/OpenSEP as the ROMs, NAT with a random locally administered MAC, and the
# machine identifier holding the ECID), ecid, udid-prediction.txt (so `restore` needs no --ecid),
# copies of OpenAVB.bin and OpenSEP.bin, and empty sparse Disk.img, nvram.bin and SEPStorage.
# The ECID is random unless given (hex, with or without 0x); Apple's signing server accepted a random
# one in testing. Refuses an existing directory.
#
# Env: OPENAVB, OPENSEP (default: OpenAVB/OpenAVB.bin and OpenSEP/OpenSEP.bin in the checkout,
#      unzipped from vphone-cli's release), MEM_GIB (8), CPUS (8).
set -euo pipefail
cd "$(dirname "$0")/.."
VM=${1:?usage: new-vm.sh <vm-dir> [ECID]}
ECID=${2:-}
OPENAVB=${OPENAVB:-OpenAVB/OpenAVB.bin}
OPENSEP=${OPENSEP:-OpenSEP/OpenSEP.bin}

[ -e "$VM" ] && { echo "$VM already exists" >&2; exit 1; }
case $(basename "$VM") in
  .*|"") echo "the VM's directory name must not start with '.' (vphone-cli refuses it)" >&2; exit 1 ;;
esac
for f in "$OPENAVB" "$OPENSEP"; do
  [ -f "$f" ] || { echo "missing $f (unzip vphone-cli's release, see docs/CREATE-ON-LINUX.md)" >&2; exit 1; }
done
grep -aq 'OpenBootROM:' "$OPENAVB" || { echo "$OPENAVB is not OpenAVB" >&2; exit 1; }

mkdir -p "$VM"
cp "$OPENAVB" "$VM/OpenAVB.bin"
cp "$OPENSEP" "$VM/OpenSEP.bin"
truncate -s 64G "$VM/Disk.img"
truncate -s 33570816 "$VM/nvram.bin"     # 16 KiB aux-storage header + 32 MiB firmware store/NVRAM
truncate -s 512K "$VM/SEPStorage"

python3 - "$VM" "$ECID" "${MEM_GIB:-8}" "${CPUS:-8}" <<'PY'
import os, plistlib, secrets, sys
vm, ecid_arg, mem_gib, cpus = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
if ecid_arg:
    ecid = int(ecid_arg, 16)
else:
    ecid = secrets.randbits(63) | (1 << 40)          # nonzero, well inside 64 bits
mac = bytearray(secrets.token_bytes(6))
mac[0] = (mac[0] & 0xFC) | 0x02                      # unicast, locally administered
config = {
    "schemaVersion": 2,
    "platformType": "vresearch101",
    "cpuCount": cpus,
    "memorySize": mem_gib << 30,
    "diskImage": "Disk.img",
    "nvramStorage": "nvram.bin",
    "sepStorage": "SEPStorage",
    "romImages": {"avpBooter": "OpenAVB.bin", "avpSEPBooter": "OpenSEP.bin"},
    "screenConfig": {"width": 1290, "height": 2796, "pixelsPerInch": 460, "scale": 3.0},
    "networkConfig": {"mode": "nat", "macAddress": ":".join(f"{b:02x}" for b in mac)},
    # Virtualization.framework's VZMacMachineIdentifier: a binary plist holding the ECID.
    "machineIdentifier": plistlib.dumps({"ECID": ecid}, fmt=plistlib.FMT_BINARY),
}
with open(os.path.join(vm, "config.plist"), "wb") as f:
    plistlib.dump(config, f, fmt=plistlib.FMT_XML)
with open(os.path.join(vm, "ecid"), "w") as f:
    f.write(f"0x{ecid:016x}\n")
with open(os.path.join(vm, "udid-prediction.txt"), "w") as f:
    f.write(f"ECID={ecid:016X}\n")
print(f"{vm}: ECID 0x{ecid:016x}")
PY
echo "next: fw prepare -l $(dirname "$VM") $(basename "$VM") ... (docs/CREATE-ON-LINUX.md, step 4)"
