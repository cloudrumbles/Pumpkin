"""Live Java 26.3 entity behavior qualification in a disposable world.

Uses real protocol login, console commands, ticks and saved NBT. A registration
or a passing compilation is deliberately not treated as a behavior check.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Callable

from tools.ironore.qualify import (
    Deadline, JavaBot, QualificationError, ServerProcess, free_loopback_port,
    load_packet_ids, parse_nbt_bool, parse_position, write_pumpkin_config,
    write_vanilla_config,
)


@dataclass
class Observation:
    name: str
    passed: bool
    detail: str


class EntityQualification:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.repo = Path(__file__).resolve().parents[2]
        self.run_dir = args.run_dir.resolve()
        self.deadline = Deadline(args.timeout)
        self.port = free_loopback_port()
        self.bot: JavaBot | None = None
        self.observations: list[Observation] = []
        self.commands: list[dict[str, str]] = []
        command = [str(args.executable.resolve())] if args.backend == "pumpkin" else [
            "java", "-Xms256M", "-Xmx768M", "-jar", str(args.vanilla_jar.resolve()), "nogui"
        ]
        self.server = ServerProcess(command, self.run_dir, self.run_dir / "server.log", self.deadline)

    def command(self, text: str) -> str:
        result = self.server.run_command(text).text
        self.commands.append({"command": text, "result": result})
        return result

    def wait(self, seconds: float) -> None:
        assert self.bot is not None
        self.bot.assert_connected_for(seconds)

    def nbt(self, tag: str) -> str:
        return self.command(f"data get entity @e[tag={tag},limit=1]")

    def summon(self, kind: str, tag: str, x: float = 24.5, y: float = 202.0,
               z: float = 24.5, extra: str = "") -> None:
        extra = ("," + extra) if extra else ""
        result = self.command(f'summon minecraft:{kind} {x} {y} {z} {{Tags:["{tag}","ai_fixture"],PersistenceRequired:1b{extra}}}')
        if "summon" not in result.lower() or "unable" in result.lower() or "failed" in result.lower():
            raise QualificationError(result)

    def cleanup(self) -> None:
        self.command("kill @e[type=!minecraft:player]")
        self.wait(1.1)

    @staticmethod
    def number(nbt: str, key: str) -> float:
        found = re.search(rf'(?<![\w])"?{re.escape(key)}"?\s*:\s*(-?\d+(?:\.\d+)?(?:[Ee][+-]?\d+)?)', nbt)
        if found is None:
            raise QualificationError(f"missing {key}: {nbt[-1200:]}")
        return float(found.group(1))

    @staticmethod
    def require(ok: bool, detail: str) -> None:
        if not ok:
            raise QualificationError(detail)

    def case(self, name: str, probe: Callable[[], None]) -> None:
        try:
            self.cleanup()
            probe()
            self.observations.append(Observation(name, True, "observed expected behavior"))
        except (OSError, ValueError, QualificationError) as error:
            self.observations.append(Observation(name, False, str(error)))
        print(json.dumps(asdict(self.observations[-1])), flush=True)

    def no_ai(self, kind: str) -> None:
        self.summon(kind, "frozen", extra="NoAI:1b,NoGravity:1b,Invulnerable:1b")
        self.wait(0.15)
        before = parse_position(self.nbt("frozen"))
        self.wait(1.2)
        after = parse_position(self.nbt("frozen"))
        self.require(before is not None and after is not None and math.dist(before, after) < 0.001,
                     f"NoAI position changed: {before} -> {after}")

    def veto_creative(self, kind: str) -> None:
        self.command("gamemode creative AIBot")
        self.command("tp AIBot 24.5 202 30.5")
        self.summon(kind, "neutral_to_creative", extra="NoGravity:1b")
        self.wait(4.0)
        nbt = self.nbt("neutral_to_creative")
        self.require("AIBot" not in nbt and "AttackTarget" not in nbt, "creative player acquired as target: " + nbt)
        if kind == "warden":
            self.require("suspects: []" in nbt or "suspects:[]" in nbt or 'suspects: []' in nbt,
                         "warden gained anger from creative player: " + nbt)

    def pufferfish(self) -> None:
        self.command("fill 17 199 17 31 205 31 glass hollow")
        self.command("fill 18 200 18 30 204 30 water")
        self.command("tp AIBot 44.5 202 44.5")
        self.summon("pufferfish", "puffer", 24.5, 202, 24.5)
        self.summon("axolotl", "threat", 25.5, 202, 24.5, "NoAI:1b,NoGravity:1b")
        self.wait(3.0)
        observed = self.nbt("puffer")
        self.require(self.number(observed, "PuffState") == 2, "puffer did not fully inflate: " + observed)
        self.command("kill @e[tag=threat]")
        self.wait(6.0)
        observed = self.nbt("puffer")
        self.require(self.number(observed, "PuffState") == 0, "puffer did not deflate: " + observed)
        self.command("fill 17 199 17 31 205 31 air")
        self.command("fill 16 199 16 47 199 47 stone")

    def vex_lifetime(self) -> None:
        self.summon("vex", "short_lived", extra="NoAI:1b,life_ticks:1")
        self.wait(1.3)
        health = self.number(self.nbt("short_lived"), "Health")
        self.require(health < 14, f"limited-life vex health remained {health}")

    def tadpole_growth(self) -> None:
        self.command("fill 17 199 17 31 205 31 glass hollow")
        self.command("fill 18 200 18 30 204 30 water")
        self.summon("tadpole", "growing", extra="Age:23990,AgeLocked:0b")
        self.wait(1.0)
        nbt = self.command("data get entity @e[type=minecraft:frog,limit=1]")
        self.require("Health" in nbt and "variant" in nbt, "tadpole failed to become frog: " + nbt)
        self.command("fill 17 199 17 31 205 31 air")
        self.command("fill 16 199 16 47 199 47 stone")

    def tadpole_locked(self) -> None:
        self.summon("tadpole", "locked", extra="Age:23990,AgeLocked:1b,NoAI:1b,NoGravity:1b")
        self.wait(1.0)
        self.require(self.number(self.nbt("locked"), "Age") == 23990, "locked tadpole aged")

    def turtle_eggs(self) -> None:
        self.command("fill 18 199 18 30 199 30 sand")
        self.summon("turtle", "expecting", 24.5, 200, 24.5, 'has_egg:1b,home_pos:[I;24,200,24]')
        end = time.monotonic() + 23.0
        last = ""
        while time.monotonic() < end:
            self.wait(1.0)
            last = self.nbt("expecting")
            if parse_nbt_bool(last, "has_egg") is False:
                break
        self.require(parse_nbt_bool(last, "has_egg") is False, "turtle did not lay its eggs: " + last)

    def hive_release(self) -> None:
        self.command("time set 6000")
        self.command("weather clear")
        self.command("setblock 24 200 24 beehive[facing=south,honey_level=0]")
        self.command('data merge block 24 200 24 {bees:[{entity_data:{id:"minecraft:bee",HasNectar:1b,NoAI:1b,Age:-100},ticks_in_hive:2400,min_ticks_in_hive:2400}]}')
        self.wait(1.5)
        bee = self.command("data get entity @e[type=minecraft:bee,limit=1]")
        self.require("Health" in bee and parse_nbt_bool(bee, "HasNectar") is False, "hive did not release delivered bee: " + bee)
        result = self.command("execute if block 24 200 24 beehive[honey_level=1]")
        self.require("passed" in result.lower() or "successful" in result.lower(), "honey did not increase: " + result)

    def axolotl_dry_out(self) -> None:
        self.summon("axolotl", "dry", extra="Air:0s,NoGravity:1b")
        self.wait(1.5)
        nbt = self.nbt("dry")
        self.require(self.number(nbt, "Health") < 14, "dry axolotl took no damage: " + nbt)

    def frog_tongue(self) -> None:
        self.command("fill 18 199 18 30 199 30 grass_block")
        self.summon("frog", "hunter", 24.5, 200, 24.5)
        self.summon("slime", "prey", 26, 200, 24.5, "Size:0,NoAI:1b")
        self.wait(7.0)
        result = self.nbt("prey")
        self.require("Health" not in result or self.number(result, "Health") <= 0, "frog did not consume small slime: " + result)

    def run(self) -> int:
        if self.run_dir.exists():
            raise QualificationError("--run-dir must be new")
        self.run_dir.mkdir(parents=True)
        if self.args.backend == "pumpkin":
            write_pumpkin_config(self.run_dir, self.port)
            config = self.run_dir / "config" / "pumpkin.toml"
            # The existing harness writes basic.toml at the root config directory.
            candidates = list(self.run_dir.rglob("*.toml"))
            for config in candidates:
                text = config.read_text().replace('default_difficulty = "Peaceful"', 'default_difficulty = "Normal"')
                config.write_text(text)
        else:
            write_vanilla_config(self.run_dir, self.port, self.args.eula_source)
        try:
            self.server.start("Server is now running. Connect using port:" if self.args.backend == "pumpkin" else "Done (")
            self.bot = JavaBot(self.port, load_packet_ids(self.repo), self.deadline, "AIBot")
            self.bot.connect()
            self.bot.wait_ready()
            self.server.wait_for_text("AIBot joined the game", "AI bot join")
            self.command("difficulty normal")
            self.command("gamerule minecraft:spawn_mobs false")
            self.command("gamemode creative AIBot")
            self.command("forceload add 16 16 47 47")
            self.command("tp AIBot 40.5 202 40.5")
            self.wait(1.5)
            self.command("fill 16 199 16 47 199 47 stone")
            self.command("fill 16 200 16 47 213 47 air")
            if self.args.suite in ("all", "no-ai"):
                registry = json.loads((self.repo / "assets" / "entities.json").read_text())
                for name, value in registry.items():
                    if value.get("mob"):
                        self.case("no_ai_" + name, lambda name=name: self.no_ai(name))
            if self.args.suite in ("all", "behavior"):
                for name in ("warden", "guardian", "vex", "phantom", "bee"):
                    self.case("creative_target_veto_" + name, lambda name=name: self.veto_creative(name))
                self.case("pufferfish_inflate_and_deflate", self.pufferfish)
                self.case("vex_limited_lifetime", self.vex_lifetime)
                self.case("tadpole_metamorphosis", self.tadpole_growth)
                self.case("tadpole_age_lock", self.tadpole_locked)
                self.case("turtle_lays_eggs", self.turtle_eggs)
                self.case("bee_nectar_delivery", self.hive_release)
                self.case("axolotl_dry_out", self.axolotl_dry_out)
                self.case("frog_consumes_small_slime", self.frog_tongue)
        except (OSError, ValueError, QualificationError) as error:
            self.observations.append(Observation("harness_prerequisite", False, str(error)))
        finally:
            if self.bot is not None:
                self.bot.close()
            clean = self.server.stop()
            self.observations.append(Observation("clean_server_stop", clean, f"exit code {self.server.returncode}"))
            self.server.kill_owned()
        report = {
            "backend": self.args.backend,
            "protocol": 777,
            "target_version": "26.3",
            "expected_data_version": 5023,
            "executable_sha256": hashlib.file_digest(self.args.executable.open("rb"), "sha256").hexdigest() if self.args.executable else None,
            "observations": [asdict(o) for o in self.observations],
            "commands": self.commands,
        }
        (self.run_dir / "qualification.json").write_text(json.dumps(report, indent=2) + "\n")
        failed = sum(not o.passed for o in self.observations)
        print(f"entity qualification: {len(self.observations)-failed}/{len(self.observations)} passed", flush=True)
        return int(failed > 0)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=("pumpkin", "vanilla"), default="pumpkin")
    parser.add_argument("--executable", type=Path)
    parser.add_argument("--vanilla-jar", type=Path)
    parser.add_argument("--eula-source", type=Path)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--suite", choices=("all", "no-ai", "behavior"), default="all")
    parser.add_argument("--timeout", type=float, default=900)
    args = parser.parse_args()
    if args.backend == "pumpkin" and args.executable is None:
        parser.error("--executable is required for Pumpkin")
    if args.backend == "vanilla" and (args.vanilla_jar is None or args.eula_source is None):
        parser.error("--vanilla-jar and --eula-source are required for vanilla")
    return EntityQualification(args).run()


if __name__ == "__main__":
    raise SystemExit(main())
