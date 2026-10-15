from __future__ import annotations

import struct
import tempfile
import unittest
import zlib
from pathlib import Path
from unittest.mock import patch

from tools.ironore.qualify import (
    COMMAND_WAIT_SECONDS,
    CommandResult,
    Deadline,
    NbtReader,
    NbtValue,
    Results,
    ServerProcess,
    block_placement_succeeded,
    block_state_from_chunk,
    command_check,
    command_completion_probe,
    datapack_enable_command,
    decode_varint_bytes,
    encode_varint,
    entity_data_command,
    parse_nbt_bool,
    parse_position,
    read_region_chunk,
    write_pumpkin_config,
    write_vanilla_config,
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

    def test_padded_anvil_palette_decodes_exact_block_properties(self) -> None:
        x, y, z = 21, 100, 21
        flattened = (y % 16) * 256 + (z % 16) * 16 + (x % 16)
        entries_per_word = 16
        word_index, entry_index = divmod(flattened, entries_per_word)
        packed: list[NbtValue] = [0] * (word_index + 1)
        packed[word_index] = 2 << (entry_index * 4)
        palettes: tuple[list[NbtValue], ...] = (
            [
                {"Name": "minecraft:air"},
                {"Name": "minecraft:stone"},
                {
                    "Name": "minecraft:oak_stairs",
                    "Properties": {
                        "facing": "east",
                        "half": "top",
                        "shape": "straight",
                    },
                },
            ],
            [
                {"": "minecraft:air"},
                {"": "minecraft:stone"},
                {
                    "id": "minecraft:oak_stairs",
                    "properties": {
                        "facing": "east",
                        "half": "top",
                        "shape": "straight",
                    },
                },
            ],
        )
        for palette in palettes:
            with self.subTest(palette=palette):
                chunk: dict[str, NbtValue] = {
                    "sections": [
                        {
                            "Y": y // 16,
                            "block_states": {"palette": palette, "data": packed},
                        }
                    ]
                }
                state = block_state_from_chunk(chunk, x, y, z)
                self.assertEqual(state.name, "minecraft:oak_stairs")
                self.assertEqual(state.properties["facing"], "east")
                self.assertEqual(state.properties["half"], "top")

    def test_position_parser_ignores_log_prefix_numbers(self) -> None:
        line = "[12:34:56 INFO]: IronoreBot has data: [20.5d, 100.0d, -2.25d]"
        self.assertEqual(parse_position(line), (20.5, 100.0, -2.25))

    def test_root_entity_nbt_parses_position_and_named_flags(self) -> None:
        output = (
            "IronoreBot has the following entity data: "
            "{Motion: [0.0d, 0.0d, 0.0d], OnGround: 1b, "
            "Pos: [20.5d, 100.0d, -2.25d], "
            "NoAI: 1b, NoGravity: 0b}"
        )
        self.assertEqual(parse_position(output), (20.5, 100.0, -2.25))
        self.assertTrue(parse_nbt_bool(output, "NoAI"))
        self.assertFalse(parse_nbt_bool(output, "NoGravity"))
        self.assertIsNone(parse_nbt_bool(output, "PersistenceRequired"))

    def test_syntax_error_echo_cannot_satisfy_fixture_marker(self) -> None:
        results = Results()
        command_check(
            results,
            "datapack_namespaced_function",
            CommandResult(
                "function ironore:namespaced",
                (
                    "Incorrect argument for command",
                    "function ironore:namespaced<--[HERE]",
                ),
            ),
            "ironore:namespaced",
        )
        results.finish_unrun("outside this test")
        observed = next(
            check
            for check in results.as_list()
            if check["name"] == "datapack_namespaced_function"
        )
        self.assertEqual(observed["status"], "unsupported")

    def test_entity_data_command_uses_vanilla_compatible_path(self) -> None:
        self.assertEqual(
            entity_data_command("pumpkin", "IronoreBot", "Pos"),
            "data get entity IronoreBot Pos",
        )
        self.assertEqual(
            entity_data_command("vanilla", "IronoreBot", "Pos"),
            "data get entity IronoreBot Pos",
        )

    def test_console_completion_probe_does_not_require_a_player(self) -> None:
        self.assertEqual(command_completion_probe(), ("list", "players online"))

    def test_pumpkin_datapack_is_enabled_from_world_datapacks(self) -> None:
        self.assertEqual(
            datapack_enable_command("pumpkin"),
            'datapack enable "file/ironore_e2e"',
        )
        self.assertIsNone(datapack_enable_command("vanilla"))

    def test_fixture_block_readiness_requires_success_feedback(self) -> None:
        unloaded = CommandResult(
            "setblock 20 98 20 minecraft:stone", ("That position is not loaded",)
        )
        placed = CommandResult(
            "setblock 20 98 20 minecraft:stone",
            ("Changed the block at 20, 98, 20",),
        )
        self.assertFalse(block_placement_succeeded(unloaded, 20, 98, 20))
        self.assertTrue(block_placement_succeeded(placed, 20, 98, 20))

    def test_pumpkin_fixture_selects_zlib_anvil_compression(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_path = Path(directory) / "pumpkin.toml"
            write_pumpkin_config(config_path.parent, 25566)
            config = config_path.read_text(encoding="utf-8")
        self.assertIn('[world.chunk.compression]\nalgorithm = "ZLib"', config)

    def test_server_command_caps_console_completion_wait(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = ServerProcess(
                ("unused",), root, root / "server.log", Deadline(60.0)
            )
            with (
                patch.object(server, "_write_stdin") as write_stdin,
                patch.object(server, "wait_for_text") as wait_for_text,
            ):
                result = server.run_command("data get entity IronoreBot Pos")

        self.assertEqual(result, CommandResult("data get entity IronoreBot Pos", ()))
        write_stdin.assert_called_once_with("data get entity IronoreBot Pos\nlist\n")
        wait_for_text.assert_called_once_with(
            "players online",
            "command completion: data get entity IronoreBot Pos",
            0,
            maximum=COMMAND_WAIT_SECONDS,
        )

    def test_pumpkin_syntax_error_is_classified_as_unsupported(self) -> None:
        result = CommandResult(
            "data get entity IronoreBot Pos",
            (
                "Incorrect argument for command",
                "data get entity IronoreBot Pos<--[HERE]",
            ),
        )
        self.assertTrue(result.looks_unsupported)

    def test_vanilla_fixture_disables_whitelist_and_empty_pause(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run_dir = Path(directory)
            write_vanilla_config(run_dir, 25566)
            properties = dict(
                line.split("=", 1)
                for line in (run_dir / "server.properties").read_text().splitlines()
            )
        self.assertEqual(properties["white-list"], "false")
        self.assertEqual(properties["enforce-whitelist"], "false")
        self.assertEqual(properties["pause-when-empty-seconds"], "-1")


if __name__ == "__main__":
    unittest.main()
