# SPDX-License-Identifier: GPL-3.0-only
"""Experimental Linux Wi-Fi backend for the NS2GameShare wire protocol."""

import argparse
from binascii import crc_hqx
import socket
import struct
import sys
import time

import trio
from ldn import wlan
from ldn.wlan import nl80211

SYNC = b"\xa5\x5a"
PKT, TEXT, H2D, D2H, EVENT = range(1, 6)
OUI = b"\x88\xb7"
VENDOR_IE = bytes.fromhex("dd060022aa100101")


def encode(kind, payload):
    body = struct.pack("<BH", kind, len(payload)) + payload
    return SYNC + body + struct.pack("<H", crc_hqx(body, 0xffff))


def advertisement(frame):
    return (
        92 <= len(frame) <= 4176
        and frame[0] == 0xd0
        and frame[24:29] == b"\x7f\x00\x22\xaa\x04"
        and frame[30:32] == b"\x01\x01"
        and frame[36:44] == (0x4d).to_bytes(8, "big")
    )


async def messages(stream):
    """Split the client's mixed ASCII and binary stream across TCP reads."""
    buffer = bytearray()
    async for chunk in stream:
        buffer.extend(chunk)
        while buffer:
            if buffer[0] == SYNC[0]:
                if len(buffer) < 5:
                    break
                if buffer[:2] != SYNC or buffer[2] != H2D:
                    raise ValueError("invalid binary header")
                size = int.from_bytes(buffer[3:5], "little")
                if not 32 <= size <= 4096:
                    raise ValueError("invalid Ethernet payload size")
                end = size + 7
                if len(buffer) < end:
                    break
                if crc_hqx(buffer[2:end - 2], 0xffff) != int.from_bytes(buffer[end - 2:end], "little"):
                    raise ValueError("invalid frame CRC")
                if int.from_bytes(buffer[13:15], "little") != size - 18:
                    raise ValueError("invalid Ethernet length")
                frame = bytes(buffer[23:end - 2])
                del buffer[:end]
                yield frame
            else:
                end = buffer.find(b"\n")
                if end > 256 or (end < 0 and len(buffer) > 256):
                    raise ValueError("command too long")
                if end < 0:
                    break
                line = buffer[:end].decode("ascii").strip()
                del buffer[:end + 1]
                if line:
                    yield line


class TargetedStationSocket:
    """Supply GameShare association attributes while reusing LDN's station logic."""

    def __init__(self, netlink, bssid):
        self.netlink = netlink
        self.bssid = bssid

    def __getattr__(self, name):
        return getattr(self.netlink, name)

    async def request(self, command, attrs):
        if command == nl80211.NL80211_CMD_CONNECT:
            attrs[nl80211.NL80211_ATTR_MAC] = self.bssid
            attrs[nl80211.NL80211_ATTR_IE] += VENDOR_IE
        return await self.netlink.request(command, attrs)


class Bridge:
    def __init__(self, stream, nursery, phy):
        self.stream = stream
        self.nursery = nursery
        self.phy = phy
        self.write_lock = trio.Lock()
        self.channels = [1, 6, 11]
        self.dwell = 0.2
        self.mac = None
        self.target = None
        self.worker = None
        self.finished = None
        self.station = None
        self.ethernet = None
        self.sequence = 0
        self.last_attempt = None

    async def send(self, kind, payload):
        async with self.write_lock:
            # Once a frame starts, complete it or close the connection on timeout.
            with trio.CancelScope(shield=True), trio.fail_after(2):
                await self.stream.send_all(encode(kind, payload))

    async def report_advertisement(self, frame, channel):
        if channel and advertisement(frame):
            header = struct.pack("<QHB13x", time.monotonic_ns() // 1000, len(frame), channel)
            await self.send(PKT, header + frame)

    async def report_ethernet(self, frame, channel):
        if 14 <= len(frame) <= 4078:
            self.sequence = (self.sequence + 1) & 0xffffffff
            header = struct.pack("<QHB3xI", time.monotonic_ns() // 1000, len(frame), channel, self.sequence)
            await self.send(D2H, header + frame)

    async def scan(self):
        async with wlan.create_factory() as factory:
            async with factory.create_monitor(self.phy, "nsgsmon") as monitor:
                self.interface = (monitor.name(), monitor.index())
                with monitor._socket:
                    while True:
                        tuned = False
                        for channel in self.channels:
                            try:
                                await monitor.set_channel(channel)
                            except OSError:
                                continue
                            tuned = True
                            with trio.move_on_after(self.dwell):
                                while True:
                                    radio = await monitor.recv()
                                    if (radio.flags or 0) & 0x40:  # Bad FCS
                                        continue
                                    actual = wlan.Frequencies.get(radio.frequency)
                                    if radio.frequency is None:
                                        actual = channel
                                    frame = radio.data[:-4] if (radio.flags or 0) & 0x10 else radio.data
                                    await self.report_advertisement(frame, actual)
                        if not tuned:
                            raise OSError("no usable scan channel")

    async def join(self):
        ssid, bssid, channel, key = self.target
        async with wlan.create_factory() as factory:
            async with factory._create_interface(
                self.phy, "nsgssta", nl80211.NL80211_IFTYPE_STATION
            ) as attrs:
                index = attrs[nl80211.NL80211_ATTR_IFINDEX]
                self.interface = ("nsgssta", index)
                netlink = TargetedStationSocket(factory._wlan, bssid)
                mac = wlan.MACAddress(self.mac)
                station = wlan.Station(netlink, factory._router, "nsgssta", index, mac, ssid, channel, key)
                await station.update_link(mac)
                async with station.connect():
                    with trio.socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3)) as ethernet:
                        await ethernet.bind(("nsgssta", 0))
                        await station.set_authorized()
                        self.station, self.ethernet = station, ethernet
                        await self.send(EVENT, b"sta linked")
                        async with trio.open_nursery() as incoming:
                            incoming.start_soon(self.read_ethernet, ethernet, channel)
                            await self.station_events(station, channel)
                            incoming.cancel_scope.cancel()

    async def station_events(self, station, channel):
        while True:
            event = await station.next_event()
            if isinstance(event, wlan.CustomFrameEvent):
                frame = self.mac + bytes(event.address) + OUI + event.data
                await self.report_ethernet(frame, channel)
            elif isinstance(event, wlan.ActionFrameEvent):
                channel_seen = wlan.Frequencies.get(event.frequency, 0)
                await self.report_advertisement(event.frame.encode(), channel_seen)
            elif isinstance(event, wlan.DisassociationEvent):
                return

    async def read_ethernet(self, ethernet, channel):
        while True:
            frame, address = await ethernet.recvfrom(4096)
            if address[2] != socket.PACKET_OUTGOING:
                await self.report_ethernet(frame, channel)

    async def run_worker(self, operation, *, task_status=trio.TASK_STATUS_IGNORED):
        with trio.CancelScope() as scope:
            self.worker = scope
            self.finished = trio.Event()
            task_status.started()
            self.interface = None
            try:
                await operation()
            except Exception as error:
                print(f"Wi-Fi worker failed ({type(error).__name__})", file=sys.stderr)
            finally:
                was_linked = self.station is not None
                self.station = self.ethernet = None
                try:
                    if not scope.cancel_called:
                        if operation == self.scan:
                            event = b"scan failed linux radio"
                        elif was_linked:
                            event = b"sta disconnected"
                        else:
                            event = b"sta join fail linux radio"
                        await self.send(EVENT, event)
                finally:
                    # The LDN context managers may be cancelled before they can
                    # delete their temporary interface. Retry after they unwind.
                    try:
                        with trio.move_on_after(3, shield=True):
                            await self.cleanup_interface()
                    finally:
                        self.worker = None
                        self.finished.set()

    async def cleanup_interface(self):
        if self.interface is None:
            return
        name, index = self.interface
        try:
            current_index = socket.if_nametoindex(name)
        except OSError:
            return
        if current_index == index:
            await trio.run_process(
                ["iw", "dev", name, "del"],
                capture_stdout=True, capture_stderr=True, check=False,
            )

    async def stop(self):
        if self.worker is not None:
            self.worker.cancel()
        if self.finished is not None:
            await self.finished.wait()

    async def command(self, line):
        fields = line.split()
        if line == "hello":
            return f"ok hello board=linux port=tcp t={time.monotonic_ns() // 1000}"
        if line in ("stop", "sta leave"):
            await self.stop()
            if line == "sta leave":
                self.target = self.mac = None
        elif line == "start":
            await self.stop()
            await self.nursery.start(self.run_worker, self.scan)
        elif line == "sta join":
            if self.target is None or self.mac is None or self.mac == self.last_attempt:
                raise ValueError("station configuration missing")
            await self.stop()
            self.last_attempt = self.mac
            await self.nursery.start(self.run_worker, self.join)
        elif line == "sta mac":
            if self.mac is None:
                raise ValueError("station MAC missing")
            return "ok sta mac " + self.mac.hex(":")
        elif len(fields) == 3 and fields[:2] == ["sta", "macset"]:
            mac = bytes.fromhex(fields[2].replace(":", ""))
            if len(mac) != 6 or mac[0] & 1 or mac == bytes(6):
                raise ValueError("invalid station MAC")
            if self.station is not None:
                raise ValueError("station is already linked")
            self.mac = mac
        elif len(fields) == 6 and fields[:2] == ["sta", "set"]:
            ssid, bssid, channel, key = fields[2:]
            channel = int(channel)
            bssid = bytes.fromhex(bssid.replace(":", ""))
            key = bytes.fromhex(key)
            if len(bytes.fromhex(ssid)) != 16 or len(bssid) != 6 or len(key) != 16 or channel not in wlan.Channels:
                raise ValueError("invalid station parameters")
            self.target = (ssid.lower(), bssid, channel, key)
        elif fields[0] in ("ch", "hop"):
            if len(fields) != (2 if fields[0] == "ch" else 3):
                raise ValueError("invalid scan command")
            channels = [int(value) for value in fields[-1].split(",")]
            dwell = 200 if fields[0] == "ch" else int(fields[1])
            if not 1 <= len(channels) <= 32 or not 50 <= dwell <= 5000:
                raise ValueError("invalid scan parameters")
            if any(channel not in wlan.Channels for channel in channels):
                raise ValueError("unsupported scan channel")
            self.channels, self.dwell = channels, dwell / 1000
        elif line not in ("mode monitor", "filter 5", "filter 1", "rssi -90", "mac clear", "sta stats"):
            raise ValueError("unsupported command")
        return "ok"

    async def run(self):
        async for message in messages(self.stream):
            if isinstance(message, str):
                try:
                    reply = await self.command(message)
                except ValueError:
                    reply = "err invalid command"
                await self.send(TEXT, reply.encode())
            elif self.station is not None and message[6:12] == self.mac:
                if message[12:14] == OUI:
                    await self.station.send_custom_frame(wlan.MACAddress(message[:6]), message[14:])
                else:
                    await self.ethernet.send(message)


async def serve(phy, port):
    async def handle(stream):
        # Only one client may own the radio at a time.
        if owner.locked():
            await stream.aclose()
            return
        async with owner, stream:
            try:
                async with trio.open_nursery() as nursery:
                    bridge = Bridge(stream, nursery, phy)
                    try:
                        await bridge.run()
                    finally:
                        nursery.cancel_scope.cancel()
            except Exception as error:
                print(f"Bridge connection ended ({type(error).__name__})", file=sys.stderr)

    owner = trio.Lock()
    await trio.serve_tcp(handle, port, host="127.0.0.1")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phy", required=True, help="Dedicated Linux wireless PHY, e.g. phy1")
    parser.add_argument("--port", type=int, default=32990)
    args = parser.parse_args()
    trio.run(serve, args.phy, args.port)
