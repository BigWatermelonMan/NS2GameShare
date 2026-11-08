# SPDX-License-Identifier: GPL-3.0-only
"""Hardware-free tests for the client-facing bridge protocol."""

import struct
from contextlib import aclosing, asynccontextmanager, nullcontext
import sys
import types
import unittest
from unittest.mock import patch

import trio
from trio.testing import memory_stream_pair

# The real radio module imports Linux-only APIs.
ldn = types.ModuleType("ldn")
ldn.wlan = types.ModuleType("ldn.wlan")
ldn.wlan.nl80211 = types.SimpleNamespace()
with patch.dict(sys.modules, {"ldn": ldn, "ldn.wlan": ldn.wlan}):
    import bridge


class WireTests(unittest.TestCase):
    def test_fragmented_commands_and_binary(self):
        ethernet = bytes(range(14)) + b"\n\xa5\x5a"
        header = struct.pack("<QHB3xI", 0, len(ethernet), 0, 1)
        wire = b"\nhello\n" + bridge.encode(bridge.H2D, header + ethernet) + b"stop\n"

        async def exercise():
            async def chunks():
                for byte in wire:
                    yield bytes([byte])

            self.assertEqual(
                [message async for message in bridge.messages(chunks())],
                ["hello", ethernet, "stop"],
            )

        trio.run(exercise)

    def test_rejects_invalid_crc_and_ethernet_length(self):
        valid = bridge.encode(bridge.H2D, struct.pack("<QHB3xI", 0, 14, 0, 1) + bytes(14))
        bad_crc = valid[:-1] + bytes([valid[-1] ^ 1])
        bad_size = bridge.encode(bridge.H2D, bytes(32))

        async def exercise(packet, reason):
            async def chunks():
                yield packet

            async with aclosing(chunks()) as stream:
                with self.assertRaisesRegex(ValueError, reason):
                    await anext(bridge.messages(stream))

        trio.run(exercise, bad_crc, "CRC")
        trio.run(exercise, bad_size, "Ethernet length")

    def test_handshake_and_status(self):
        async def exercise():
            client, server = memory_stream_pair()
            async with trio.open_nursery() as nursery:
                nursery.start_soon(bridge.Bridge(server, nursery, "phy0").run)
                await client.send_all(b"\nhello\n")
                reply = await client.receive_some(4096)
                self.assertEqual(reply[:3], b"\xa5\x5a\x02")
                self.assertTrue(reply[5:-2].startswith(b"ok hello board=linux port=tcp t="))
                self.assertEqual(reply, bridge.encode(bridge.TEXT, reply[5:-2]))
                await client.aclose()

        trio.run(exercise)

    def test_advertisement_selector(self):
        frame = bytearray(92)
        frame[0] = 0xd0
        frame[24:29] = b"\x7f\x00\x22\xaa\x04"
        frame[30:32] = b"\x01\x01"
        frame[43] = 0x4d
        self.assertTrue(bridge.advertisement(frame))
        self.assertFalse(bridge.advertisement(frame[:-1]))
        frame[43] = 0x4e
        self.assertFalse(bridge.advertisement(frame))

    def test_probe_uses_radiotap_channel_and_strips_fcs(self):
        frame = bytearray(92)
        frame[0] = 0xd0
        frame[24:29] = b"\x7f\x00\x22\xaa\x04"
        frame[30:32] = b"\x01\x01"
        frame[43] = 0x4d

        class Monitor:
            _socket = nullcontext()

            def name(self):
                return "nsgsmon"

            def index(self):
                return 1

            async def set_channel(self, channel):
                if channel == 36:
                    raise OSError("channel unavailable")

            async def recv(self):
                return types.SimpleNamespace(frequency=2437, flags=0x10, data=frame + bytes(4))

        @asynccontextmanager
        async def create_monitor(_phy, _name):
            yield Monitor()

        @asynccontextmanager
        async def factory():
            yield types.SimpleNamespace(create_monitor=create_monitor)

        async def found(data, channel):
            self.assertEqual((data, channel), (frame, 6))
            return True

        with patch.object(bridge.wlan, "create_factory", factory, create=True), patch.object(
            bridge.wlan, "Frequencies", {2437: 6}, create=True
        ):
            trio.run(bridge.scan_radio, "phy0", (36, 1), 0.2, found, lambda *_: None)
