#!/usr/bin/env python3
"""Rebuild the restore ramdisk trust cache (rtsc) for the SSH ramdisk.

The stock RestoreTrustCache admits the stock ramdisk's Apple userland.  The
SSHRD staging tree replaces that userland, so each executable/dylib it injects
needs its cdhash in the trust cache or TXM's selector-24 ladder has nothing to
admit it against (the ladder then exits 0x32202).

This merges the stock entries with the SHA-1 cdhashes of every Mach-O in the
staging tree, sorts/dedupes, and rewraps the payload in the stock IM4P shape.

Usage:
    tools/ssh-ramdisk-trustcache.py <stock.rtsc> <staging-tree> <out.rtsc>
"""

import hashlib
import os
import struct
import sys

LC_CODE_SIGNATURE = 0x1D
CSMAGIC_SUPERBLOB = 0xFADE0CC0
CSMAGIC_CODEDIRECTORY = 0xFADE0C02


def read_der(data):
    """Return (outer_sequence_bytes, [(tag, value)...]) for the stock IM4P."""
    tag, body, _ = der_tlv(data, 0)
    if tag != 0x30:
        raise ValueError("not a DER SEQUENCE")
    items = []
    off = 0
    while off < len(body):
        t, v, off = der_tlv(body, off)
        items.append((t, v))
    return items


def der_tlv(data, off):
    tag = data[off]
    off += 1
    length = data[off]
    off += 1
    if length & 0x80:
        count = length & 0x7F
        length = int.from_bytes(data[off:off + count], "big")
        off += count
    return tag, data[off:off + length], off + length


def der_len(length):
    if length < 0x80:
        return bytes([length])
    if length < 0x100:
        return b"\x81" + bytes([length])
    if length < 0x10000:
        return b"\x82" + length.to_bytes(2, "big")
    return b"\x83" + length.to_bytes(3, "big")


def der(tag, value):
    return bytes([tag]) + der_len(len(value)) + value


def macho_cdhashes(path):
    """Yield SHA-1 cdhashes for every signed thin slice, kind of."""
    data = open(path, "rb").read()
    if len(data) < 8:
        return
    magic_be = struct.unpack(">I", data[:4])[0]
    slices = []
    if magic_be in (0xCAFEBABE, 0xBEBAFECA):
        # Fat Mach-O (network byte order).  Only the 32-bit fat header is
        # expected here; skip malformed files instead of guessing.
        n = struct.unpack(">I", data[4:8])[0]
        if n == 0 or n > 64:
            return
        for i in range(n):
            base = 8 + i * 20
            if base + 20 > len(data):
                return
            off, size = struct.unpack(">II", data[base + 8:base + 16])
            if off + size <= len(data):
                slices.append((off, size))
    else:
        slices.append((0, len(data)))

    for off, size in slices:
        chunk = data[off:off + size]
        if len(chunk) < 32:
            continue
        magic = struct.unpack("<I", chunk[:4])[0]
        if magic == 0xFEEDFACF:
            header, cmd_off = 32, 32
        elif magic == 0xFEEDFACE:
            header, cmd_off = 28, 28
        else:
            continue
        ncmds = struct.unpack("<I", chunk[16:20])[0]
        p = cmd_off
        sig = None
        for _ in range(ncmds):
            if p + 8 > len(chunk):
                break
            cmd, cmdsize = struct.unpack("<II", chunk[p:p + 8])
            if cmdsize < 8 or p + cmdsize > len(chunk):
                break
            if cmd == LC_CODE_SIGNATURE:
                dataoff, datasize = struct.unpack("<II", chunk[p + 8:p + 16])
                sig = (dataoff, datasize)
            p += cmdsize
        if sig is None:
            continue
        dataoff, datasize = sig
        sb = chunk[dataoff:dataoff + datasize]
        if len(sb) < 12 or struct.unpack(">I", sb[:4])[0] != CSMAGIC_SUPERBLOB:
            continue
        count = struct.unpack(">I", sb[8:12])[0]
        q = 12
        primary = None
        for _ in range(count):
            if q + 8 > len(sb):
                break
            blob_type, blob_off = struct.unpack(">II", sb[q:q + 8])
            q += 8
            if blob_type != 0:
                continue
            if blob_off + 8 > len(sb):
                continue
            if struct.unpack(">I", sb[blob_off:blob_off + 4])[0] != CSMAGIC_CODEDIRECTORY:
                continue
            length = struct.unpack(">I", sb[blob_off + 4:blob_off + 8])[0]
            if blob_off + length > len(sb):
                continue
            primary = sb[blob_off:blob_off + length]
        if primary is None:
            continue
        # CodeDirectory.hashType at +37: 1 = SHA-1 (the cdhash is the SHA-1 of the
        # CodeDirectory), 2 = SHA-256 (the cdhash is its SHA-256, cut to 20 bytes,
        # as in the stock entries). Trust-cache entries hold 20 bytes either way.
        if primary[37] == 1:
            yield hashlib.sha1(primary).digest()
        elif primary[37] == 2:
            yield hashlib.sha256(primary).digest()[:20]
        else:
            print(f"warn: {path}: CodeDirectory hashType {primary[37]} not supported, skipped",
                  file=sys.stderr)


def main():
    if len(sys.argv) != 4:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    stock_path, tree, out_path = sys.argv[1:4]

    stock = open(stock_path, "rb").read()
    items = read_der(stock)
    strings = [v for t, v in items if t == 0x16]
    octets = [v for t, v in items if t == 0x04]
    if len(strings) < 2 or not octets:
        raise SystemExit("unexpected stock rtsc DER shape")
    fourcc, ptype, desc = strings[0], strings[1], strings[2]
    if fourcc != b"IM4P" or ptype != b"rtsc":
        raise SystemExit(f"unexpected stock rtsc type {fourcc!r}/{ptype!r}")

    payload = octets[0]
    version = struct.unpack("<I", payload[:4])[0]
    uuid = payload[4:20]
    count = struct.unpack("<I", payload[20:24])[0]
    if version != 1 or 24 + count * 22 != len(payload):
        raise SystemExit("unsupported stock rtsc payload")

    entries = {}
    for i in range(count):
        entry = payload[24 + i * 22:24 + (i + 1) * 22]
        entries[entry[:20]] = entry[20:22]
    stock_count = len(entries)

    # Every regular Mach-O in the staging tree (the injector copies exactly
    # these files over the stock APFS).
    added = {}
    for dirpath, dirnames, filenames in os.walk(tree):
        for name in filenames:
            path = os.path.join(dirpath, name)
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            try:
                for cdhash in macho_cdhashes(path):
                    if cdhash not in entries:
                        added[cdhash] = b"\x02\x00"
            except Exception as exc:  # noqa: BLE001 - a stray file must not abort
                print(f"warn: {path}: {exc}", file=sys.stderr)

    for cdhash, tail in added.items():
        entries[cdhash] = tail

    ordered = sorted(entries.items(), key=lambda kv: kv[0])
    body = b"".join(cdhash + tail for cdhash, tail in ordered)
    new_payload = (struct.pack("<I", 1) + uuid + struct.pack("<I", len(ordered)) + body)

    out = der(0x30, der(0x16, b"IM4P") + der(0x16, b"rtsc")
              + der(0x16, desc) + der(0x04, new_payload))
    with open(out_path, "wb") as fh:
        fh.write(out)

    for label, path in (("stock", stock_path), ("output", out_path)):
        d = open(path, "rb").read()
        print(f"{label}: {path} ({len(d)} bytes) sha384={hashlib.sha384(d).hexdigest()}")
    print(f"entries: stock {stock_count}, added {len(added)}, total {len(ordered)}")
    print(f"dropbear cdhash present: "
          f"{any(cdhash in entries for cdhash in macho_cdhashes(os.path.join(tree, 'usr/local/bin/dropbear')))}")


if __name__ == "__main__":
    sys.exit(main())
