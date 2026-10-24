#!/bin/bash
# Repack the modified APFS payload into the original rdsk IM4P wrapper.
#
# The stock 094-39914-031.dmg is a DER SEQUENCE containing the IA5 strings
# "IM4P"/"rdsk"/"0" and one OCTET STRING holding the whole APFS container at
# offset 27.  The payload length is unchanged by the injection, so the stock
# header can be reused verbatim; only the payload bytes are replaced.
#
#   tools/ssh-ramdisk-repack.sh [stock.dmg] [payload.apfs] [out.dmg]
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC=${1:-$ROOT/runs/sshramdisk/stock-094-39914-031.dmg}
PAYLOAD=${2:-$ROOT/runs/sshramdisk/base/apfs.img}
OUT=${3:-$ROOT/runs/sshramdisk/out/094-39914-031.dmg}

[ -f "$SRC" ] || { echo "missing stock ramdisk: $SRC" >&2; exit 1; }
[ -f "$PAYLOAD" ] || { echo "missing modified APFS payload: $PAYLOAD" >&2; exit 1; }

mkdir -p "$(dirname "$OUT")"

python3 - "$SRC" "$PAYLOAD" "$OUT" <<'PY'
import hashlib
import os
import sys

src, payload_path, out = sys.argv[1:4]
data = bytearray(open(src, "rb").read())

assert data[0] == 0x30 and data[1] == 0x84, "not a DER SEQUENCE"
seqlen = int.from_bytes(data[2:6], "big")
assert data[6] == 0x16 and data[7] == 4 and data[8:12] == b"IM4P", "not IM4P"
assert data[12] == 0x16 and data[13] == 4 and data[14:18] == b"rdsk", "not rdsk"
assert data[18] == 0x16 and data[19] == 1 and data[20:21] == b"0", "bad description"
assert data[21] == 0x04 and data[22] == 0x84, "not OCTET STRING"
plen = int.from_bytes(data[23:27], "big")
assert 27 + plen == len(data), "payload length does not match file"

payload = open(payload_path, "rb").read()
assert len(payload) == plen, f"payload size changed: {len(payload)} != {plen}"
assert payload[32:36] == b"NXSB", "payload does not look like an APFS container"

data[27:27 + plen] = payload
open(out, "wb").write(data)

for label, path in (("stock", src), ("output", out)):
    digest = hashlib.sha1(open(path, "rb").read()).hexdigest()
    print(f"{label}: {path} ({os.path.getsize(path)} bytes) sha1={digest}")
print(f"payload: {payload_path} ({plen} bytes, APFS NXSB at +32)")
PY
