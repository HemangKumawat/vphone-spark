#!/usr/bin/env python3
"""Talk to QEMU's vresearch-vusb device (the guest's USB device side) over
its chardev, as the USB host would.

  run-vphone.sh <vm> -chardev socket,id=vusb,path=/tmp/vusb.sock,server=on,wait=off \
                     -global vresearch-vusb.chardev=vusb
  vusb-host.py /tmp/vusb.sock script...

Host to device: { u32 len; u8 ep; u8 type; payload } (type 1 transfer,
2 bus reset, 3 abort); a SETUP packet is a type 1 transfer on ep 0. Device to host arrives as { u32 len; u8 code; u8 ep; data }.

Script steps (each an argument):
  reset                    bus reset (type 2)
  setup:<8 hex bytes>      SETUP on EP0 (type 1 on ep 0), e.g. setup:8006000100001200
  xfer:<ep>[:<hex data>]   transfer message on an endpoint, e.g. xfer:80
  abort:<ep>
  wait:<seconds>           print device messages for that long
"""
import socket
import struct
import sys
import time


def send(s, ep, typ, payload=b""):
    msg = struct.pack("<IBB", len(payload), ep, typ) + payload
    print(f"-> ep 0x{ep:02x} type {typ} {payload.hex()}")
    s.sendall(msg)


def recv_for(s, secs, buf):
    end = time.monotonic() + secs
    while True:
        while len(buf) >= 4:
            (n,) = struct.unpack_from("<I", buf)
            if len(buf) < 4 + n:
                break
            m = bytes(buf[4:4 + n])
            del buf[:4 + n]
            code, ep = (m[0], m[1]) if n >= 2 else (m[0] if n else -1, -1)
            print(f"<- code {code} ep 0x{ep:02x} {m[2:].hex()}  ({n} bytes)")
        left = end - time.monotonic()
        if left <= 0:
            return
        s.settimeout(left)
        try:
            d = s.recv(65536)
        except socket.timeout:
            return
        if not d:
            print("closed")
            sys.exit(0)
        buf += d


def main():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sys.argv[1])
    buf = bytearray()
    for step in sys.argv[2:]:
        kind, _, arg = step.partition(":")
        if kind == "reset":
            send(s, 0, 2)
        elif kind == "setup":
            send(s, 0, 1, bytes.fromhex(arg))
        elif kind == "xfer":
            ep, _, data = arg.partition(":")
            send(s, int(ep, 16), 1, bytes.fromhex(data))
        elif kind == "abort":
            send(s, int(arg, 16), 3)
        elif kind == "wait":
            recv_for(s, float(arg), buf)
            continue
        else:
            sys.exit(f"bad step {step}")
        recv_for(s, 0.5, buf)
    recv_for(s, 1, buf)


if __name__ == "__main__":
    main()
