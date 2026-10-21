#!/usr/bin/env python3
"""display-scale.py NVRAM [SCALE]: set the screen scale iOS uses (UIScreen.scale, MobileGestalt's
main-screen-scale) by patching LLB in a VM's nvram.bin firmware store. LLB fills
/chosen/display-scale from the boot video depth word's scale byte (`ubfx wN, wM, #16, #8;
add wN, wN, #1; str wN, [..]`), which the paravirtual display always sets to 3. This replaces the
`add` with `mov wN, #SCALE` (vphone-cli's IBootPatchDisplayScale, which it applies for iPads).
Without SCALE, prints the current state. SCALE 0 restores the original instruction.
Pair 2x with a 2/3-size display (run-vphone.sh GFX_RES=860x1864): same 430x932-point screen,
2.25x fewer pixels to draw."""
import struct
import sys

path = sys.argv[1]
scale = int(sys.argv[2]) if len(sys.argv) > 2 else None
d = bytearray(open(path, "rb").read())
hits = []
for off in range(0, len(d) - 12, 4):
    a, b, c = struct.unpack_from("<III", d, off)
    if a & 0xFFFFFC00 != (0x53000000 | (16 << 16) | (23 << 10)):
        continue
    rd = a & 31
    add = 0x11000000 | (1 << 10) | (rd << 5) | rd
    is_mov = b & 0xFFE0001F == (0x52800000 | rd)
    is_str = (c & 0xFFC0001F) == (0xB9000000 | rd) or (c & 0xFFE00C1F) == (0xB8000000 | rd)
    if is_str and (b == add or is_mov):
        hits.append((off, rd, b, add))
if len(hits) != 1:
    sys.exit(f"expected one display-scale computation, found {len(hits)}")
off, rd, b, add = hits[0]
cur = "boot video (3x)" if b == add else f"{(b >> 5) & 0xFFFF}x (patched)"
if scale is None:
    print(f"{path}: LLB +{off + 4:#x}: display-scale {cur}")
    sys.exit()
new = add if scale == 0 else 0x52800000 | (scale << 5) | rd
struct.pack_into("<I", d, off + 4, new)
open(path, "r+b").write(d)
print(f"{path}: display-scale {cur} -> {'boot video (3x)' if scale == 0 else f'{scale}x'}")
