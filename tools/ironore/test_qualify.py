from __future__ import annotations

import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from tools.ironore.qualify import (
    NbtReader,
    decode_varint_bytes,
    encode_varint,
    parse_position,
    read_region_chunk,
)


def nbt_string(value: str) -> bytes:
    raw = value.encode("utf-8")
    return struct.pack(">H", len(raw)) + raw


def named_long(name: str, value: int) -> bytes:
    return b"\x04" + nbt_string(name) + struct.pack(">q", value)


def named_compound(name: str, children: bytes) -> bytes:
    return b"\x0a" + nbt_string(name) + children + b"\x00"


class CodecTests(unittest.TestCase):
    def test_signed_varints_round_trip(self) -> None:
        for value in (0, 1, 127, 128, 255, 2**31 - 1, -1, -(2**31)):
            encoded = encode_varint(value)
            decoded, offset = decode_varint_bytes(encoded)
            self.assertEqual(decoded, value)
            self.assertEqual(offset, len(encoded))

    def test_world_clock_shape_decodes(self) -> None:
        payload = named_compound(
            "data",
            named_compound("minecraft:overworld", named_long("total_ticks", 6000)),
        )
        root = NbtReader(b"\x0a\x00\x00" + payload + b"\x00").read_root()
        data = root["data"]
        self.assertIsInstance(data, dict)
        assert isinstance(data, dict)
        overworld = data["minecraft:overworld"]
        self.assertIsInstance(overworld, dict)
        assert isinstance(overworld, dict)
        self.assertEqual(overworld["total_ticks"], 6000)

    def test_zlib_region_chunk_decodes(self) -> None:
        chunk_nbt = (
            b"\x0a\x00\x00"
            + b"\x09"
            + nbt_string("block_entities")
            + b"\x0a"
            + struct.pack(">i", 0)
            + b"\x00"
        )
        compressed = zlib.compress(chunk_nbt)
        record = struct.pack(">i", len(compressed) + 1) + b"\x02" + compressed
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "r.0.0.mca"
            header = bytearray(8192)
            header[:4] = (2).to_bytes(3, "big") + b"\x01"
            path.write_bytes(bytes(header) + record + bytes(4096 - len(record)))
            root = read_region_chunk(path, 0, 0)
        self.assertEqual(root["block_entities"], [])

    def test_position_parser_ignores_log_prefix_numbers(self) -> None:
        line = "[12:34:56 INFO]: IronoreBot has data: [20.5d, 100.0d, -2.25d]"
        self.assertEqual(parse_position(line), (20.5, 100.0, -2.25))


if __name__ == "__main__":
    unittest.main()
