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
import struct
import time
import uuid
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Callable

from tools.ironore.qualify import (
    Deadline, JavaBot, NbtReader, QualificationError, ServerProcess, decode_varint_bytes, free_loopback_port,
    load_packet_ids, parse_nbt_bool, parse_position, write_pumpkin_config,
    write_vanilla_config,
)


@dataclass
class Observation:
    name: str
    passed: bool
    detail: str


def decode_metadata(payload: bytes) -> tuple[int, dict[int, object]]:
    """Decode the metadata used by the behavior probes; reject unknown layouts."""
    entity_id, offset = decode_varint_bytes(payload)
    values: dict[int, object] = {}

    def take(size: int) -> bytes:
        nonlocal offset
        end = offset + size
        if size < 0 or end > len(payload):
            raise QualificationError("truncated entity metadata")
        value = payload[offset:end]
        offset = end
        return value

    def varint() -> int:
        nonlocal offset
        value, offset = decode_varint_bytes(payload, offset)
        return value

    def string() -> str:
        return take(varint()).decode("utf-8")

    def nbt() -> object:
        nonlocal offset
        reader = NbtReader(payload[offset:])
        tag = reader._u8()
        value = reader._payload(tag) if tag else None
        offset += reader._offset
        return value

    while True:
        index = take(1)[0]
        if index == 255:
            if offset != len(payload):
                raise QualificationError("trailing entity metadata")
            return entity_id, values
        kind = varint()
        if kind == 0:
            value = struct.unpack(">b", take(1))[0]
        elif kind in (1, 12, 14, 15, 19, 20, 35, 36, 38, 42, 43):
            value = varint()
        elif kind == 2:
            value = 0
            for shift in range(0, 70, 7):
                byte = take(1)[0]
                value |= (byte & 127) << shift
                if not byte & 128:
                    break
            else:
                raise QualificationError("VarLong exceeds ten bytes")
            if value & (1 << 63):
                value -= 1 << 64
        elif kind == 3:
            value = struct.unpack(">f", take(4))[0]
        elif kind == 4:
            value = string()
        elif kind == 5:
            value = nbt()
        elif kind == 6:
            value = nbt() if take(1)[0] else None
        elif kind == 8:
            value = bool(take(1)[0])
        elif kind in (9, 39):
            value = struct.unpack(">fff", take(12))
        elif kind == 10:
            value = struct.unpack(">q", take(8))[0]
        elif kind == 11:
            value = struct.unpack(">q", take(8))[0] if take(1)[0] else None
        elif kind == 13:
            value = str(uuid.UUID(bytes=take(16))) if take(1)[0] else None
        elif kind == 17 and varint() == 0:
            value = []
        elif kind == 18:
            value = (varint(), varint(), varint())
        elif kind in (21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 34):
            # These serializers use holderRegistry: zero is a registry ID,
            # not the inline-holder sentinel used by the separate holder codec.
            value = varint()
        elif kind == 33:
            value = (string(), struct.unpack(">q", take(8))[0]) if take(1)[0] else None
        elif kind == 40:
            value = struct.unpack(">ffff", take(16))
        else:
            raise QualificationError(f"unsupported metadata serializer {kind}")
        values[index] = value


class EntityBot(JavaBot):
    def __init__(self, *args: object, packet_ids: dict[str, int], **kwargs: object) -> None:
        super().__init__(*args, **kwargs)
        self.entity_packet_ids = packet_ids
        self.spawned: dict[str, int] = {}
        self.metadata_samples: dict[int, list[dict[int, object]]] = {}
        self.metadata_errors: dict[int, str] = {}

    def _handle(self, packet_id: int, payload: bytes) -> None:
        if self._state == "play":
            if packet_id == self.entity_packet_ids["add_entity"]:
                entity_id, offset = decode_varint_bytes(payload)
                entity_uuid = str(uuid.UUID(bytes=payload[offset:offset + 16]))
                with self._condition:
                    self.spawned[entity_uuid] = entity_id
            elif packet_id == self.entity_packet_ids["set_entity_data"]:
                entity_id, _ = decode_varint_bytes(payload)
                try:
                    _, values = decode_metadata(payload)
                    with self._condition:
                        self.metadata_samples.setdefault(entity_id, []).append(values)
                except (ValueError, struct.error, QualificationError) as error:
                    with self._condition:
                        self.metadata_errors[entity_id] = str(error)
        super()._handle(packet_id, payload)

    def samples(self, nbt: str) -> list[dict[int, object]]:
        match = re.search(r"UUID:\s*\[I;\s*([^]]+)\]", nbt)
        if match is None:
            raise QualificationError("entity UUID is absent from NBT")
        words = [int(word.strip()) & 0xFFFFFFFF for word in match[1].split(",")]
        entity_uuid = str(uuid.UUID(int=sum(word << (96 - i * 32) for i, word in enumerate(words))))
        with self._condition:
            entity_id = self.spawned.get(entity_uuid)
            if entity_id is None:
                raise QualificationError("entity spawn packet was not observed")
            if entity_id in self.metadata_errors:
                raise QualificationError(self.metadata_errors[entity_id])
            return list(self.metadata_samples.get(entity_id, []))


class EntityQualification:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.repo = Path(__file__).resolve().parents[2]
        self.run_dir = args.run_dir.resolve()
        self.deadline = Deadline(args.timeout)
        self.port = free_loopback_port()
        self.bot: EntityBot | None = None
        self.observations: list[Observation] = []
        self.commands: list[dict[str, str]] = []
        command = [str(args.executable.resolve())] if args.backend == "pumpkin" else [
            "java", "-Xms256M", "-Xmx768M", "-jar", str(args.vanilla_jar.resolve()), "nogui"
        ]
        self.server = ServerProcess(command, self.run_dir, self.run_dir / "server.log", self.deadline)

    def command(self, text: str) -> str:
        end = time.monotonic() + 30.0
        while True:
            result = self.server.run_command(text, maximum=45.0 if text.startswith(("fill ", "setblock ")) else 5.0).text
            self.commands.append({"command": text, "result": result})
            if "not loaded" in result.lower() and text.startswith(("fill ", "setblock ")):
                if time.monotonic() < end:
                    self.wait(0.5)
                    continue
                raise QualificationError("fixture chunks did not load: " + result)
            if any(error in result.lower() for error in ("incorrect argument", "unknown command", "invalid argument")):
                raise QualificationError("command rejected: " + text + ": " + result)
            return result

    def game_time(self) -> int:
        result = self.command("time query gametime")
        match = re.search(r"game\s*time is\s+(\d+)", result, re.IGNORECASE)
        if match is None:
            raise QualificationError("game-time query was not understood: " + result)
        return int(match[1])

    def wait(self, seconds: float) -> None:
        """Observe simulated ticks even when host load slows the server."""
        assert self.bot is not None
        end = self.game_time() + math.ceil(seconds * 20)
        while self.game_time() < end:
            self.bot.assert_connected_for(0.1)

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
        if self.args.case and name not in self.args.case:
            return
        try:
            self.cleanup()
            probe()
            self.observations.append(Observation(name, True, "observed expected behavior"))
        except (OSError, ValueError, QualificationError) as error:
            self.observations.append(Observation(name, False, str(error)))
            if "command completion" in str(error) or "overall timeout" in str(error):
                # A stalled console invalidates later observations. Do not report
                # unrelated entity behaviors as failures from the same backlog.
                print(json.dumps(asdict(self.observations[-1])), flush=True)
                raise
        print(json.dumps(asdict(self.observations[-1])), flush=True)

    def no_ai(self, kind: str) -> None:
        self.summon(kind, "frozen", extra="NoAI:1b,Invulnerable:1b")
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
        assert self.bot is not None
        samples = self.bot.samples(nbt)
        if kind == "guardian":
            attacks = [sample[17] for sample in samples if 17 in sample]
            self.require(all(target == 0 for target in attacks),
                         f"guardian beam target metadata: {attacks}")
        elif kind == "vex":
            flags = [int(sample[16]) for sample in samples if 16 in sample]
            self.require(all(flag & 1 == 0 for flag in flags),
                         f"vex charged creative player: {flags}")
        elif kind == "bee":
            anger = [int(sample[19]) for sample in samples if 19 in sample]
            self.require(all(end <= 0 for end in anger),
                         f"bee anger metadata: {anger}")
        if kind == "warden":
            self.require(re.search(r'anger:\s*\{\s*\}', nbt) is not None
                         or re.search(r'suspects:\s*\[\s*\]', nbt) is not None,
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

    def guardian_attack(self) -> None:
        self.command("fill 17 199 17 31 205 31 glass hollow")
        self.command("fill 18 200 18 30 204 30 water")
        self.command("tp AIBot 44.5 202 44.5")
        self.summon("guardian", "beaming", 22.5, 202, 24.5)
        self.summon("squid", "beam_prey", 28.5, 202, 24.5, "NoAI:1b")
        self.wait(8.0)
        guardian = self.nbt("beaming")
        assert self.bot is not None
        targets = [sample[17] for sample in self.bot.samples(guardian) if 17 in sample]
        self.require(any(target > 0 for target in targets), f"guardian never announced a beam: {targets}")
        prey = self.nbt("beam_prey")
        self.require("Health" not in prey or self.number(prey, "Health") < 10,
                     "guardian beam did not damage squid: " + prey)

    def neutral_anger_expiry(self, kind: str) -> None:
        self.summon(kind, "angry", 38.5, 200, 24.5,
                    "AngerTime:10,angry_at:[I;1248,5821,3370,4920],NoGravity:1b,Invulnerable:1b")
        self.wait(1.2)
        nbt = self.nbt("angry")
        self.require(self.number(nbt, "anger_end_time") == -1 and "angry_at" not in nbt,
                     "expired persistent anger retained its target: " + nbt)

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

    def tadpole_locked(self) -> None:
        self.summon("tadpole", "locked", extra="Age:23990,AgeLocked:1b,NoAI:1b,NoGravity:1b")
        self.wait(1.0)
        self.require(self.number(self.nbt("locked"), "Age") == 23990, "locked tadpole aged")

    def turtle_eggs(self) -> None:
        self.command("fill 33 198 18 46 198 30 stone")
        self.command("fill 33 199 18 46 199 30 sand")
        self.summon("turtle", "expecting", 38.5, 200, 24.5, 'has_egg:1b,home_pos:[I;38,200,24]')
        end = self.game_time() + 460
        last = ""
        while self.game_time() < end:
            self.wait(1.0)
            last = self.nbt("expecting")
            if parse_nbt_bool(last, "has_egg") is False:
                break
        self.require(parse_nbt_bool(last, "has_egg") is False, "turtle did not lay its eggs: " + last)

    def hive_release(self) -> None:
        self.command("time set 6000")
        self.command("weather clear")
        self.command("setblock 36 200 36 beehive[facing=south,honey_level=0]")
        self.command('data merge block 36 200 36 {bees:[{entity_data:{id:"minecraft:bee",HasNectar:1b,NoAI:1b,Age:-100},ticks_in_hive:2400,min_ticks_in_hive:2400}]}')
        self.wait(1.5)
        bee = self.command("data get entity @e[type=minecraft:bee,limit=1]")
        self.require("Health" in bee and parse_nbt_bool(bee, "HasNectar") is False, "hive did not release delivered bee: " + bee)
        results = [self.command(f"execute in minecraft:overworld if block 36 200 36 beehive[honey_level={level}] run data get block 36 200 36") for level in (1, 2)]
        self.require(any("bees:" in result for result in results),
                     "honey did not increase: " + repr(results))

    def axolotl_dry_out(self) -> None:
        self.summon("axolotl", "dry", 40.5, 202, 40.5, "Air:0s,NoGravity:1b")
        self.wait(1.5)
        nbt = self.nbt("dry")
        self.require(self.number(nbt, "Health") < 14, "dry axolotl took no damage: " + nbt)

    def frog_tongue(self) -> None:
        self.command("fill 33 199 18 46 199 30 grass_block")
        self.command("fill 33 200 18 46 200 30 air")
        self.summon("frog", "hunter", 38.5, 200, 24.5)
        self.summon("slime", "prey", 40, 200, 24.5, "Size:0,NoAI:1b")
        end = self.game_time() + 400
        result = self.nbt("prey")
        while self.game_time() < end and "Health" in result and self.number(result, "Health") > 0:
            self.wait(1.0)
            result = self.nbt("prey")
        self.require("Health" not in result or self.number(result, "Health") <= 0, "frog did not consume small slime: " + result)

    def ground_navigation(self) -> None:
        self.command("gamemode creative AIBot")
        self.command("tp AIBot 44.5 202 44.5")
        self.command("time set 18000")
        self.command("fill 33 199 18 46 199 30 stone")
        self.command("fill 33 200 18 46 205 30 air")
        # Three blocks high: pursuit requires finding a route around an end.
        self.command("fill 39 200 21 39 202 27 stone")
        self.summon("villager", "walk_target", 43.5, 200, 24.5, "NoAI:1b")
        self.summon("zombie", "walker", 35.5, 200, 24.5, "IsBaby:0b")
        # Retaliation establishes the target even before there is line of sight.
        self.command("damage @e[tag=walker,limit=1] 1 minecraft:mob_attack by @e[tag=walk_target,limit=1]")
        end = self.game_time() + 600
        furthest = 0.0
        prey = self.nbt("walk_target")
        while self.game_time() < end and "Health" in prey and self.number(prey, "Health") >= 20:
            self.wait(0.5)
            position = parse_position(self.nbt("walker"))
            self.require(position is not None, "pursuing zombie disappeared")
            assert position is not None
            furthest = max(furthest, math.dist((35.5, 200, 24.5), position))
            prey = self.nbt("walk_target")
        self.require(furthest > 5 and ("Health" not in prey or self.number(prey, "Health") < 20),
                     f"zombie did not route around the wall and hit the villager; displacement={furthest}: {prey}")

    def ambient_movement(self, kind: str, water: bool) -> None:
        self.command("gamemode creative AIBot")
        self.command("tp AIBot 44.5 202 44.5")
        self.command("time set 6000")
        self.command("fill 17 199 17 31 209 31 glass hollow")
        self.command("fill 18 200 18 30 208 30 " + ("water" if water else "air"))
        self.summon(kind, "moving", 24.5, 203, 24.5)
        self.wait(1.0)
        origin = parse_position(self.nbt("moving"))
        self.require(origin is not None, "moving entity was not created")
        assert origin is not None
        furthest = 0.0
        vertical = 0.0
        end = self.game_time() + 400
        while self.game_time() < end and (furthest < 1.5 or vertical < 0.25):
            self.wait(0.5)
            observed = self.nbt("moving")
            position = parse_position(observed)
            self.require(position is not None and self.number(observed, "Health") > 0,
                         "moving entity died or disappeared: " + observed)
            assert position is not None
            furthest = max(furthest, math.dist(origin, position))
            vertical = max(vertical, abs(origin[1] - position[1]))
        self.require(furthest >= 1.5 and vertical >= 0.25,
                     f"{kind} failed to move through its habitat: displacement={furthest}, vertical={vertical}")

    def ageable_growth(self, kind: str, locked: bool) -> None:
        self.command("gamemode creative AIBot")
        self.command("tp AIBot 40.5 202 40.5")
        self.summon(kind, "ageable", 38.5, 202, 40.5,
                    f"Age:-12,AgeLocked:{int(locked)}b,NoAI:1b,NoGravity:1b,Invulnerable:1b")
        self.wait(1.2)
        result = self.nbt("ageable")
        expected = -12 if locked else 0
        self.require(self.number(result, "Age") == expected,
                     f"{kind} age should be {expected}: " + result)
        self.require(parse_nbt_bool(result, "AgeLocked") is locked,
                     f"{kind} lost the age-lock state: " + result)
        assert self.bot is not None
        samples = self.bot.samples(result)
        baby_states = [sample[16] for sample in samples if 16 in sample]
        self.require(bool(baby_states) and baby_states[-1] is locked,
                     f"{kind} baby metadata did not follow age: {baby_states}")
        if locked:
            locks = [sample[17] for sample in samples if 17 in sample]
            self.require(bool(locks) and locks[-1] is True,
                         f"{kind} age-lock metadata was not sent: {locks}")

    def cow_breeding(self) -> None:
        self.command("gamemode creative AIBot")
        self.command("tp AIBot 44.5 202 44.5")
        self.command("fill 33 199 18 46 199 30 grass_block")
        self.command("fill 33 200 18 46 205 30 air")
        self.summon("cow", "breeder_one", 38.5, 200, 24.5, "Age:0,InLove:600")
        self.summon("cow", "breeder_two", 40.5, 200, 24.5, "Age:0,InLove:600")
        self.command("tag @e[type=minecraft:cow] add parent")
        end = self.game_time() + 200
        child = ""
        while self.game_time() < end:
            self.wait(1.0)
            child = self.command("data get entity @e[type=minecraft:cow,tag=!parent,limit=1]")
            if "Health" in child:
                break
        self.require("Health" in child and -24000 <= self.number(child, "Age") < -23000,
                     "breeding did not create a baby cow: " + child)
        for parent in ("breeder_one", "breeder_two"):
            result = self.nbt(parent)
            self.require(5700 < self.number(result, "Age") <= 6000 and self.number(result, "InLove") == 0,
                         "parent did not enter its age-based breeding cooldown: " + result)

    def run(self) -> int:
        if self.run_dir.exists():
            raise QualificationError("--run-dir must be new")
        self.run_dir.mkdir(parents=True)
        if self.args.backend == "pumpkin":
            write_pumpkin_config(self.run_dir, self.port)
            candidates = list(self.run_dir.rglob("*.toml"))
            for config in candidates:
                text = config.read_text().replace('default_difficulty = "Peaceful"', 'default_difficulty = "Normal"')
                config.write_text(text)
        else:
            write_vanilla_config(self.run_dir, self.port, self.args.eula_source)
        try:
            self.server.start("Server is now running. Connect using port:" if self.args.backend == "pumpkin" else "Done (")
            packet_ids = json.loads((self.repo / "assets" / "packets.json").read_text())["clientbound"]["play"]
            self.bot = EntityBot(self.port, load_packet_ids(self.repo), self.deadline, "AIBot", packet_ids=packet_ids)
            self.bot.connect()
            self.bot.wait_ready()
            self.server.wait_for_text("AIBot joined the game", "AI bot join")
            self.command("difficulty normal")
            self.command("gamerule spawn_mobs false")
            self.command("weather clear")
            self.command("gamerule advance_weather false")
            self.command("gamemode creative AIBot")
            self.command("forceload add 16 16 47 47")
            self.command("tp AIBot 40.5 202 40.5")
            self.wait(1.5)
            self.command("fill 20 199 20 29 199 29 stone")
            self.command("fill 20 200 20 29 210 29 air")
            self.command("fill 33 199 33 46 199 46 stone")
            self.command("fill 33 200 33 46 205 46 air")
            if self.args.suite in ("all", "no-ai"):
                registry = json.loads((self.repo / "assets" / "entities.json").read_text())
                for name, value in registry.items():
                    if value.get("mob"):
                        self.case("no_ai_" + name, lambda name=name: self.no_ai(name))
            if self.args.suite in ("all", "behavior"):
                for name in ("warden", "guardian", "vex", "phantom", "bee"):
                    self.case("creative_target_veto_" + name, lambda name=name: self.veto_creative(name))
                self.case("guardian_beam_damages_squid", self.guardian_attack)
                self.case("pufferfish_inflate_and_deflate", self.pufferfish)
                self.case("vex_limited_lifetime", self.vex_lifetime)
                self.case("tadpole_metamorphosis", self.tadpole_growth)
                self.case("tadpole_age_lock", self.tadpole_locked)
                self.case("turtle_lays_eggs", self.turtle_eggs)
                self.case("bee_nectar_delivery", self.hive_release)
                self.case("axolotl_dry_out", self.axolotl_dry_out)
                self.case("frog_consumes_small_slime", self.frog_tongue)
                for name in ("bee", "enderman", "iron_golem", "polar_bear", "wolf", "zombified_piglin"):
                    self.case("neutral_anger_expires_" + name, lambda name=name: self.neutral_anger_expiry(name))
                self.case("ground_zombie_routes_around_wall", self.ground_navigation)
                self.case("water_cod_random_swim", lambda: self.ambient_movement("cod", True))
                self.case("flying_allay_wander", lambda: self.ambient_movement("allay", False))
                inventory = json.loads((self.repo / "tools" / "ironore" / "entity-parity-26.3.json").read_text())
                cannot_be_babies = {"camel_husk", "frog", "magma_cube", "parrot", "slime", "sulfur_cube", "wandering_trader", "zombie_nautilus"}
                for entry in inventory["entities"]:
                    name = entry["entity"]
                    if name not in cannot_be_babies and any(path.endswith("/AgeableMob.java") for path in entry["java_classes"]):
                        # Undead horses retain their age while NoAI; the age lock is
                        # still serialized and round-tripped in the Java reference.
                        if name not in {"skeleton_horse", "zombie_horse"}:
                            self.case("ageable_growth_" + name, lambda name=name: self.ageable_growth(name, False))
                        self.case("ageable_locked_" + name, lambda name=name: self.ageable_growth(name, True))
                self.case("cow_breeding_and_parent_cooldowns", self.cow_breeding)
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
            "vanilla_jar_sha1": hashlib.file_digest(self.args.vanilla_jar.open("rb"), "sha1").hexdigest() if self.args.vanilla_jar else None,
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
    parser.add_argument("--case", action="append", help="Run only the named case (repeatable)")
    parser.add_argument("--timeout", type=float, default=900)
    args = parser.parse_args()
    if args.backend == "pumpkin" and args.executable is None:
        parser.error("--executable is required for Pumpkin")
    if args.backend == "vanilla" and (args.vanilla_jar is None or args.eula_source is None):
        parser.error("--vanilla-jar and --eula-source are required for vanilla")
    return EntityQualification(args).run()


if __name__ == "__main__":
    raise SystemExit(main())
