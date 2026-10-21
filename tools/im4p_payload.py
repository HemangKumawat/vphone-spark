#!/usr/bin/env python3
"""Extract (and LZFSE-decompress) the payload of an IM4P file: im4p_payload.py in.im4p out.bin"""
import sys, ctypes, ctypes.util

def der(buf, off):
    tag = buf[off]; ln = buf[off + 1]; off += 2
    if ln & 0x80:
        n = ln & 0x7F; ln = int.from_bytes(buf[off:off + n], "big"); off += n
    return tag, off, ln

buf = open(sys.argv[1], "rb").read()
_, off, ln = der(buf, 0); end = off + ln
payload = None
while off < end:
    tag, o, l = der(buf, off)
    if tag == 0x04:
        payload = buf[o:o + l]; break
    off = o + l
if payload[:3] == b"bvx":
    lib = ctypes.CDLL(ctypes.util.find_library("lzfse") or "liblzfse.so.1")
    lib.lzfse_decode_buffer.restype = ctypes.c_size_t
    lib.lzfse_decode_buffer.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_void_p]
    out = ctypes.create_string_buffer(len(payload) * 16)
    n = lib.lzfse_decode_buffer(out, len(out), payload, len(payload), None)
    payload = out.raw[:n]
open(sys.argv[2], "wb").write(payload)
print(len(payload), "bytes", payload[:4])
