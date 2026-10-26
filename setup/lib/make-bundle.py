#!/usr/bin/env python3
"""Write an empty vphone CLI VM bundle: config.plist (schemaVersion 2, a fresh
ECID), ecid, an empty 64G Disk.img, a blank nvram.bin and SEPStorage, and the
OpenAVB/OpenSEP boot ROMs. With a blank nvram, OpenAVB drops into DFU — the
state a restore starts from.

Usage: make-bundle.py <vm-dir> [DEVICE_ID]  — the device's own screen comes
from data/compatibility.json (devices[].screen = [w, h, ppi, scale]);
the default stays the iPhone17,3's 1290x2796.

Value ZERO_DISK=1 skips truncating Disk.img (dry-run validation)."""
import json
import os
import plistlib
import sys
import uuid
from pathlib import Path

vm = sys.argv[1]
device = sys.argv[2] if len(sys.argv) > 2 else "iPhone17,3"
ecid = uuid.uuid4().int >> 17 & (1 << 63) - 1     # a fresh 63-bit ECID

screen = [1290, 2796, 460, 3.0]
data_path = Path(__file__).resolve().parent.parent / "data" / "compatibility.json"
try:
    devices = json.load(open(data_path))["devices"]
    for dev in devices:
        if dev["id"] == device and dev.get("screen"):
            screen = dev["screen"]
            break
except (OSError, KeyError, json.JSONDecodeError):
    pass

os.makedirs(vm, exist_ok=True)
plistlib.dump({
    "schemaVersion": 2,
    "platformType": "vresearch101",
    "cpuCount": 8,
    "memorySize": 8 * 2**30,
    "screenConfig": {
        "width": screen[0], "height": screen[1],
        "pixelsPerInch": screen[2], "scale": screen[3],
    },
    "romImages": {"avpBooter": "OpenAVB.bin", "avpSEPBooter": "OpenSEP.bin"},
    "machineIdentifier": plistlib.dumps({"ECID": ecid}),
}, open(os.path.join(vm, "config.plist"), "wb"))
open(os.path.join(vm, "ecid"), "w").write("0x%x\n" % ecid)

if os.environ.get("ZERO_DISK", "") != "0":
    # sparse placeholders (truncate costs nothing but needs the fs though)
    for name, size in (("Disk.img", 64 * 2**30),
                       ("nvram.bin", 33570816),
                       ("SEPStorage", 512 * 1024)):
        with open(os.path.join(vm, name), "wb") as f:
            f.truncate(size)

# The display scale (1 | 2 | 3) lowers the guest's render scale below the
# panel's native one (tools/display-scale.py patches the LLB's display-scale
# to it); the host's window then shows the same UI points at fewer pixels.
# The device's screen is the native panel [w, h, ppi, native-scale]; the
# render size is the UI points (native / native scale) times the flag.
flag = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0
native_w, native_h, _ppi, native_scale = screen[0], screen[1], screen[2], screen[3]
render_w = round(native_w / native_scale * flag / 2) * 2   # even; NVIDIA rows
render_h = round(native_h / native_scale * flag / 2) * 2   # want padding
with open(os.path.join(vm, "gfx-res"), "w") as f:
    f.write(f"{render_w}x{render_h}\n")

print(f"bundle ready: {vm} (ECID 0x{ecid:x}, screen {native_w}x{native_h} "
      f"native @{native_scale}x, flag {flag}x -> render {render_w}x{render_h})")
