"""Exercise block input, predicate and persistence behavior on a disposable server."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import time
from collections.abc import Callable, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path

from tools.ironore.qualify import (
    Deadline,
    JavaBot,
    QualificationError,
    ServerProcess,
    block_placement_succeeded,
    free_loopback_port,
    load_packet_ids,
    write_pumpkin_config,
)


@dataclass(frozen=True)
class Observation:
    name: str
    passed: bool
    detail: str


class BlockQualification:
    def __init__(self, executable: Path, run_dir: Path, timeout: float) -> None:
        self.executable = executable
        self.run_dir = run_dir
        self.deadline = Deadline(timeout)
        self.port = free_loopback_port()
        self.ids = load_packet_ids(Path(__file__).resolve().parents[2])
        self.server = self._server("harness-server.log")
        self.bot: JavaBot | None = None
        self.observations: list[Observation] = []

    def _server(self, log_name: str) -> ServerProcess:
        return ServerProcess(
            (str(self.executable),),
            self.run_dir,
            self.run_dir / log_name,
            self.deadline,
        )

    def _start(self) -> None:
        self.server.start("Server is now running. Connect using port:")
        self.bot = JavaBot(self.port, self.ids, self.deadline, "BlockBot")
        self.bot.connect()
        self.bot.wait_ready()
        self.server.wait_for_text("BlockBot joined the game", "player join")
        self.server.run_command("forceload add 16 16")
        self.server.run_command("tp BlockBot 20.5 100 20.5")
        self.bot.assert_connected_for(0.5)
        end = time.monotonic() + self.deadline.slice("fixture chunk readiness", 15.0)
        while True:
            result = self.server.run_command("setblock 30 98 30 stone")
            if block_placement_succeeded(result, 30, 98, 30):
                self._place(30, 98, 30, "air")
                break
            if time.monotonic() >= end:
                raise QualificationError("fixture chunk not writable: " + result.text)
            time.sleep(0.1)

    def _stop(self) -> None:
        if self.bot is not None:
            self.bot.close()
            self.bot = None
        if not self.server.stop():
            raise QualificationError(f"unclean stop: {self.server.returncode}")

    def _case(self, name: str, probe: Callable[[], None]) -> None:
        try:
            probe()
            self.observations.append(
                Observation(name, True, "observed expected behavior")
            )
        except (OSError, QualificationError, ValueError) as error:
            self.observations.append(Observation(name, False, str(error)))

    def _place(self, x: int, y: int, z: int, block: str, mode: str = "") -> None:
        result = self.server.run_command(f"setblock {x} {y} {z} {block} {mode}".strip())
        if not block_placement_succeeded(result, x, y, z):
            raise QualificationError(result.command + ": " + result.text[-1200:])

    def _matches(self, position: str, predicate: str, expected: bool = True) -> None:
        result = self.server.run_command(
            f"execute if block {position} {predicate} run help setblock"
        )
        if result.looks_unsupported or ("/setblock" in result.text) != expected:
            raise QualificationError(result.command + ": " + result.text[-1200:])

    def _items(self, position: str, item: str, count: int) -> None:
        result = self.server.run_command(f"data get block {position}")
        pattern = rf'"?count"?\s*:\s*{count}(?:[bBsS])?(?=\s*[,}}])'
        if item not in result.text or re.search(pattern, result.text) is None:
            raise QualificationError(result.command + ": " + result.text[-1200:])

    def _input_nbt(self) -> None:
        self._place(
            17,
            100,
            17,
            'chest[facing=east]{id:"minecraft:furnace",x:0,y:0,z:0,Items:[{Slot:0b,id:"minecraft:diamond",count:3}]}',
        )
        self._items("17 100 17", "minecraft:diamond", 3)
        self._matches(
            "17 100 17", 'chest[facing=east]{id:"minecraft:chest",x:17,y:100,z:17}'
        )

    def _nbt_only_update(self) -> None:
        self._place(
            17,
            100,
            17,
            'chest[facing=east]{Items:[{Slot:0b,id:"minecraft:diamond",count:5}]}',
        )
        self._items("17 100 17", "minecraft:diamond", 5)

    def _strict_nbt(self) -> None:
        self._place(
            19,
            100,
            17,
            'chest{Items:[{Slot:0b,id:"minecraft:emerald",count:2}]}',
            "strict",
        )
        self._items("19 100 17", "minecraft:emerald", 2)

    def _property_predicates(self) -> None:
        self._place(21, 100, 17, "oak_stairs[facing=east,half=top]")
        self._matches("21 100 17", "oak_stairs[facing=east]")
        self._matches("21 100 17", "oak_stairs[facing=west]", False)
        self._matches("21 100 17", "oak_stairs[half=bottom]", False)

    def _tag_predicates(self) -> None:
        self._place(23, 100, 17, "oak_log[axis=x]")
        self._matches("23 100 17", "#minecraft:logs[axis=x]")
        self._matches("23 100 17", "#minecraft:logs[axis=y]", False)
        self._matches("23 100 17", "#minecraft:logs[unknown=value]", False)

    def _nbt_predicates(self) -> None:
        self._matches("17 100 17", 'chest{Items:[{id:"minecraft:diamond"}]}')
        self._matches("17 100 17", 'chest{Items:[{id:"minecraft:emerald"}]}', False)
        self._matches("17 100 17", "chest{Items:[]}", False)

    def _fill_property_filter(self) -> None:
        self._place(17, 100, 19, "oak_log[axis=x]")
        self._place(18, 100, 19, "oak_log[axis=y]")
        self.server.run_command(
            "fill 17 100 19 18 100 19 gold_block replace #minecraft:logs[axis=x]"
        )
        self._matches("17 100 19", "gold_block")
        self._matches("18 100 19", "oak_log[axis=y]")

    def _fill_nbt(self) -> None:
        self.server.run_command(
            'fill 20 100 19 21 100 19 chest{Items:[{Slot:0b,id:"minecraft:diamond",count:4}]}'
        )
        self._items("20 100 19", "minecraft:diamond", 4)
        self._items("21 100 19", "minecraft:diamond", 4)

    def _fill_nbt_filter(self) -> None:
        self._place(
            23, 100, 19, 'chest{Items:[{Slot:0b,id:"minecraft:diamond",count:1}]}'
        )
        self._place(
            24, 100, 19, 'chest{Items:[{Slot:0b,id:"minecraft:emerald",count:1}]}'
        )
        self.server.run_command(
            'fill 23 100 19 24 100 19 gold_block replace chest{Items:[{id:"minecraft:diamond"}]}'
        )
        self._matches("23 100 19", "gold_block")
        self._items("24 100 19", "minecraft:emerald", 1)

    def _clone_nbt_filter(self) -> None:
        self._place(
            17, 100, 21, 'chest{Items:[{Slot:0b,id:"minecraft:diamond",count:6}]}'
        )
        self._place(
            18, 100, 21, 'chest{Items:[{Slot:0b,id:"minecraft:emerald",count:7}]}'
        )
        self.server.run_command(
            'clone 17 100 21 18 100 21 21 100 21 filtered chest{Items:[{id:"minecraft:diamond"}]}'
        )
        self._items("21 100 21", "minecraft:diamond", 6)
        self._matches("22 100 21", "air")

    def _strict_neighbor_updates(self) -> None:
        self._place(25, 100, 23, "stone")
        self._place(25, 101, 23, "torch")
        self._place(25, 100, 23, "air", "strict")
        self._matches("25 101 23", "torch")
        self._place(27, 100, 23, "stone")
        self._place(27, 101, 23, "torch")
        self.server.run_command("fill 27 100 23 27 100 23 air strict")
        self._matches("27 101 23", "torch")

    def _fill_destroy(self) -> None:
        self._place(26, 100, 19, "stone")
        self.server.run_command("fill 26 100 19 26 100 19 gold_block destroy")
        self._matches("26 100 19", "gold_block")

    def _persisted(self) -> None:
        self._items("17 100 17", "minecraft:diamond", 5)
        self._items("19 100 17", "minecraft:emerald", 2)
        self._items("20 100 19", "minecraft:diamond", 4)
        self._items("21 100 21", "minecraft:diamond", 6)
        self._matches("18 100 19", "oak_log[axis=y]")

    def run(self) -> int:
        if not self.executable.is_file() or self.run_dir.exists():
            raise QualificationError(
                "executable must exist and run directory must be new"
            )
        self.run_dir.mkdir(parents=True)
        write_pumpkin_config(self.run_dir, self.port)
        try:
            self._start()
            for name, probe in (
                ("setblock_nbt_and_metadata", self._input_nbt),
                ("nbt_only_update", self._nbt_only_update),
                ("strict_container_nbt", self._strict_nbt),
                ("explicit_property_predicates", self._property_predicates),
                ("tag_property_predicates", self._tag_predicates),
                ("partial_nbt_predicates", self._nbt_predicates),
                ("fill_property_filter", self._fill_property_filter),
                ("fill_container_nbt", self._fill_nbt),
                ("fill_nbt_filter", self._fill_nbt_filter),
                ("clone_nbt_filter", self._clone_nbt_filter),
                ("strict_suppresses_neighbor_updates", self._strict_neighbor_updates),
                ("fill_destroy_completes", self._fill_destroy),
            ):
                self._case(name, probe)
            self._stop()
            self.observations.append(Observation("clean_first_stop", True, "exit=0"))
            self.server = self._server("harness-restart.log")
            self._start()
            self._case("block_entity_restart_persistence", self._persisted)
            self._stop()
            self.observations.append(Observation("clean_final_stop", True, "exit=0"))
        except (OSError, QualificationError, ValueError) as error:
            self.observations.append(Observation("server_lifecycle", False, str(error)))
        finally:
            if self.bot is not None:
                self.bot.close()
            self.server.kill_owned()
        report = {
            "protocol": 777,
            "executable_sha256": hashlib.sha256(
                self.executable.read_bytes()
            ).hexdigest(),
            "checks": [asdict(check) for check in self.observations],
        }
        (self.run_dir / "block-qualification.json").write_text(
            json.dumps(report, indent=2) + "\n", encoding="utf-8"
        )
        print(json.dumps(report, indent=2))
        return 0 if all(check.passed for check in self.observations) else 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=240.0)
    args = parser.parse_args(argv)
    return BlockQualification(
        args.executable.expanduser().resolve(),
        args.run_dir.expanduser().resolve(),
        args.timeout,
    ).run()


if __name__ == "__main__":
    raise SystemExit(main())
