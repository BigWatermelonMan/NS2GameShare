# SPDX-License-Identifier: GPL-3.0-only
"""Hardware-free tests for the client-facing bridge protocol."""

import struct
from contextlib import aclosing
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
