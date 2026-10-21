#!/usr/bin/env python3
"""Export QEMU's vresearch-vusb chardev as a USB/IP device.

Example:

  run-vphone.sh <vm> -chardev socket,id=vusb,path=/tmp/vusb.sock,server=on,wait=off \\
      -global vresearch-vusb.chardev=vusb
  tools/vusb-usbip.py /tmp/vusb.sock
  usbip list -r 127.0.0.1
  sudo usbip attach -r 127.0.0.1 -b 1-1

The guest must be in recovery/DFU mode and have presented its USB device before
the bridge starts. USB/IP listens on 127.0.0.1:3240 by default.
"""

import argparse
import asyncio
import collections
import errno
import struct
import sys

USBIP_VERSION = 0x0111
OP_REQ_IMPORT = 0x8003
OP_REP_IMPORT = 0x0003
OP_REQ_DEVLIST = 0x8005
OP_REP_DEVLIST = 0x0005
CMD_SUBMIT, CMD_UNLINK = 1, 2
RET_SUBMIT, RET_UNLINK = 3, 4
USB_SPEED_HIGH = 3
BUSID = b"1-1"
DEVICE_PATH = b"/sys/devices/platform/vusb/usb1/1-1"

MSG_DATA, MSG_RESET, MSG_ABORT = 1, 2, 3
CODE_DATA, CODE_STALL, CODE_RESET, CODE_ABORT = 1, 2, 4, 5
MSG_PAYLOAD_MAX = 0x1000
CTRL_TIMEOUT = 5.0
URB_TIMEOUT = 30.0


def log(*args):
    print(*args, file=sys.stderr, flush=True)


class Stall(Exception):
    """The guest USB device returned a STALL response."""


class VUSB:
    """Framed chardev client and response queues for one guest USB device."""

    def __init__(self, reader, writer):
        self.reader = reader
        self.writer = writer
        self.ctrl = asyncio.Queue()
        self.reset_done = asyncio.Event()
        self.aborts = collections.defaultdict(asyncio.Queue)
        self.out_acks = collections.defaultdict(asyncio.Queue)
        self.in_data = collections.defaultdict(asyncio.Queue)
        self.ctrl_lock = asyncio.Lock()
        self.endpoint_locks = collections.defaultdict(asyncio.Lock)

    async def send(self, ep, typ, payload=b""):
        if len(payload) > MSG_PAYLOAD_MAX:
            raise ValueError(f"vusb payload too large: {len(payload)}")
        self.writer.write(struct.pack("<IBB", len(payload), ep, typ) + payload)
        await self.writer.drain()

    async def run(self):
        while True:
            hdr = await self.reader.readexactly(4)
            (length,) = struct.unpack("<I", hdr)
            if length < 2 or length > MSG_PAYLOAD_MAX + 2:
                raise ValueError(f"invalid vusb response length {length}")
            msg = await self.reader.readexactly(length)
            code, ep, data = msg[0], msg[1], msg[2:]
            if code == CODE_RESET:
                self.reset_done.set()
            elif code == CODE_ABORT:
                await self.aborts[ep].put((code, data))
            elif ep == 0:
                await self.ctrl.put((code, data))
            elif ep & 0x80:
                await self.in_data[ep].put((code, data))
            else:
                await self.out_acks[ep].put((code, data))

    async def _next(self, queue, timeout=URB_TIMEOUT):
        code, data = await asyncio.wait_for(queue.get(), timeout)
        if code == CODE_STALL:
            raise Stall()
        if code != CODE_DATA:
            raise OSError(errno.EPROTO, f"unexpected vusb response code {code}")
        return data

    async def reset(self):
        self.reset_done.clear()
        await self.send(0, MSG_RESET)
        await asyncio.wait_for(self.reset_done.wait(), CTRL_TIMEOUT)

    async def control(self, setup, out_data=b""):
        if len(setup) != 8:
            raise ValueError("USB SETUP packet must be 8 bytes")
        async with self.ctrl_lock:
            await self.send(0, MSG_DATA, setup + out_data)
            return await self._next(self.ctrl, CTRL_TIMEOUT)

    async def transfer(self, ep, direction_in, payload=b""):
        address = ep | (0x80 if direction_in else 0)
        async with self.endpoint_locks[address]:
            if direction_in:
                await self.send(address, MSG_DATA)
                return await self._next(self.in_data[address])
            await self.send(address, MSG_DATA, payload)
            await self._next(self.out_acks[address])
            return b""

    async def abort(self, ep, direction_in):
        address = ep | (0x80 if direction_in else 0)
        await self.send(address, MSG_ABORT)
        try:
            await asyncio.wait_for(self.aborts[address].get(), CTRL_TIMEOUT)
        except asyncio.TimeoutError:
            pass


def get_device_record(device, config):
    """Return the USB/IP fixed-size device record and interface records."""
    if len(device) < 18 or device[1] != 1:
        raise ValueError("guest returned an invalid USB device descriptor")
    if len(config) < 9 or config[1] != 2:
        raise ValueError("guest returned an invalid USB configuration descriptor")

    interfaces = []
    endpoint_sizes = {}
    offset = 0
    while offset + 2 <= len(config):
        size, dtype = config[offset], config[offset + 1]
        if size < 2 or offset + size > len(config):
            raise ValueError("malformed USB configuration descriptor")
        if dtype == 4 and size >= 9:
            interfaces.append(config[offset + 5:offset + 8])
        elif dtype == 5 and size >= 7:
            address = config[offset + 2]
            endpoint_sizes[address] = int.from_bytes(config[offset + 4:offset + 6], "little") & 0x7ff
        offset += size

    record = struct.pack(
        "!256s32sIIIHHHBBBBBB",
        DEVICE_PATH, BUSID, 1, 1, USB_SPEED_HIGH,
        int.from_bytes(device[8:10], "little"),
        int.from_bytes(device[10:12], "little"),
        int.from_bytes(device[12:14], "little"),
        device[4], device[5], device[6], config[5], device[17], config[4],
    )
    if len(record) != 312:
        raise AssertionError(f"USB/IP device record is {len(record)} bytes")
    return record, interfaces, endpoint_sizes


class USBIPBridge:
    def __init__(self, usb):
        self.usb = usb
        self.device_record = None
        self.interfaces = []
        self.endpoint_sizes = {}
        self.write_lock = asyncio.Lock()
        self.pending = {}

    async def discover(self):
        """Read the descriptors needed by OP_REP_DEVLIST and OP_REP_IMPORT."""
        await self.usb.reset()
        setup = lambda value, length: struct.pack("<BBHHH", 0x80, 6, value, 0, length)
        device = await self.usb.control(setup(0x0100, 18))
        device = device[:18]
        config_head = await self.usb.control(setup(0x0200, 9))
        if len(config_head) < 9:
            raise ValueError("short USB configuration descriptor")
        total = int.from_bytes(config_head[2:4], "little")
        if not 9 <= total <= 4096:
            raise ValueError(f"invalid USB configuration length {total}")
        config = await self.usb.control(setup(0x0200, total))
        if len(config) < total:
            raise ValueError(f"short USB configuration: got {len(config)}, expected {total}")
        self.device_record, self.interfaces, self.endpoint_sizes = get_device_record(
            device, config[:total])
        log("USB device discovered: %04x:%04x, %d interface(s)" % (
            int.from_bytes(device[8:10], "little"),
            int.from_bytes(device[10:12], "little"), len(self.interfaces)))

    async def write(self, writer, data):
        async with self.write_lock:
            writer.write(data)
            await writer.drain()

    async def send_op_header(self, writer, code, status=0):
        await self.write(writer, struct.pack("!HHI", USBIP_VERSION, code, status))

    async def handle_devlist(self, writer):
        await self.send_op_header(writer, OP_REP_DEVLIST)
        await self.write(writer, struct.pack("!I", 1) + self.device_record)
        if self.interfaces:
            records = b"".join(bytes((i[0], i[1], i[2], 0)) for i in self.interfaces)
            await self.write(writer, records)

    async def handle_import(self, reader, writer):
        busid = (await reader.readexactly(32)).split(b"\0", 1)[0]
        if busid != BUSID:
            await self.send_op_header(writer, OP_REP_IMPORT, 1)
            return False
        await self.send_op_header(writer, OP_REP_IMPORT)
        await self.write(writer, self.device_record)
        return True

    async def handle_connection(self, reader, writer):
        tasks = set()
        try:
            header = await reader.readexactly(8)
            version, code, status = struct.unpack("!HHI", header)
            if version != USBIP_VERSION or status:
                raise ValueError(f"bad USB/IP operation header: {version:#x}/{status}")
            if code == OP_REQ_DEVLIST:
                await self.handle_devlist(writer)
                return
            if code == OP_REQ_IMPORT:
                if not await self.handle_import(reader, writer):
                    return
            else:
                raise ValueError(f"unsupported USB/IP operation {code:#x}")

            while True:
                command = struct.unpack("!I", await reader.readexactly(4))[0]
                if command == CMD_SUBMIT:
                    rest = await reader.readexactly(44)
                    fields = struct.unpack("!IIIIIIiiii8s", struct.pack("!I", command) + rest)
                    _, seqnum, _, direction, _, _, length, _, _, _, _ = fields
                    if length < 0 or length > 16 * 1024 * 1024:
                        raise ValueError(f"invalid USB/IP transfer length {length}")
                    payload = await reader.readexactly(length) if direction == 0 else b""
                    task = asyncio.create_task(self.handle_submit(fields, payload, writer))
                    self.pending[seqnum] = (task, fields[4], direction)
                    tasks.add(task)
                    task.add_done_callback(lambda done, seq=seqnum:
                                           self._submit_done(done, seq, tasks))
                elif command == CMD_UNLINK:
                    rest = await reader.readexactly(20)
                    await self.handle_unlink(rest, writer)
                else:
                    raise ValueError(f"unsupported USB/IP command {command}")
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        except Exception as e:
            log(f"USB/IP client error: {e}")
        finally:
            for task in tasks:
                task.cancel()
            if tasks:
                await asyncio.gather(*tasks, return_exceptions=True)
            writer.close()
            try:
                await writer.wait_closed()
            except (ConnectionError, OSError):
                pass

    async def handle_submit(self, fields, payload, writer):
        (_, seqnum, devid, direction, ep, _flags, length, start_frame,
         packets, _interval, setup) = fields
        status, data = 0, b""
        try:
            if ep == 0:
                if direction != (1 if setup[0] & 0x80 else 0):
                    raise OSError(errno.EINVAL, "URB direction disagrees with SETUP packet")
                data = await self.usb.control(setup, payload if direction == 0 else b"")
                data = data[:length] if direction == 1 else b""
            elif direction == 1:
                packet_size = self.endpoint_sizes.get(ep | 0x80, 512)
                result = bytearray()
                while len(result) < length:
                    packet = await self.usb.transfer(ep, True)
                    room = length - len(result)
                    result.extend(packet[:room])
                    if len(packet) < packet_size or len(result) >= length:
                        break
                data = bytes(result)
            else:
                packet_size = self.endpoint_sizes.get(ep, 512)
                if length == 0:
                    await self.usb.transfer(ep, False)
                else:
                    for offset in range(0, length, packet_size):
                        await self.usb.transfer(ep, False, payload[offset:offset + packet_size])
        except OSError as e:
            status = -(e.errno or errno.EIO)
        except Stall:
            status = -errno.EPIPE
        except asyncio.TimeoutError:
            status = -errno.ETIMEDOUT
        except asyncio.CancelledError:
            status = -errno.ECONNRESET
        except Exception as e:
            log(f"USB/IP URB {seqnum} failed: {e}")
            status = -errno.EIO
        await self._send_submit_reply(writer, seqnum, devid, direction, ep,
                                      status, data, start_frame, packets)

    async def _send_submit_reply(self, writer, seqnum, devid, direction, ep,
                                 status, data, start_frame, packets):
        hdr = struct.pack("!IIIIIiiiii8s", RET_SUBMIT, seqnum, devid,
                          direction, ep, status, len(data), start_frame,
                          packets, 0, b"\0" * 8)
        await self.write(writer, hdr + (data if direction == 1 else b""))

    async def handle_unlink(self, rest, writer):
        seqnum, devid, direction, ep, target = struct.unpack("!IIIII", rest)
        pending = self.pending.get(target)
        task = pending[0] if pending else None
        status = 0 if task and not task.done() else -errno.ENOENT
        await self.write(writer, struct.pack("!IIIIIi", RET_UNLINK, seqnum,
                                            devid, direction, ep, status))
        if status == 0:
            _, target_ep, target_direction = pending
            await self.usb.abort(target_ep, target_direction == 1)
            task.cancel()

    def _submit_done(self, task, seqnum, tasks):
        tasks.discard(task)
        pending = self.pending.get(seqnum)
        if pending and pending[0] is task:
            self.pending.pop(seqnum, None)


async def main_async(args):
    reader, writer = await asyncio.open_unix_connection(args.chardev)
    usb = VUSB(reader, writer)
    pump = asyncio.create_task(usb.run())
    try:
        bridge = USBIPBridge(usb)
        await bridge.discover()
        server = await asyncio.start_server(bridge.handle_connection,
                                            args.host, args.port)
        log(f"USB/IP listening on {args.host}:{args.port} (busid {BUSID.decode()})")
        async with server:
            await server.serve_forever()
    finally:
        pump.cancel()
        writer.close()
        await writer.wait_closed()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("chardev", help="QEMU vusb Unix socket")
    parser.add_argument("--host", default="127.0.0.1", help="listen address (default: localhost)")
    parser.add_argument("--port", type=int, default=3240)
    args = parser.parse_args()
    try:
        asyncio.run(main_async(args))
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
