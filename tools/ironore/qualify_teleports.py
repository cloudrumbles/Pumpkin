"""Exercise Java 26.3 teleport acknowledgements against a disposable Pumpkin server."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import time
from collections.abc import Callable, Sequence
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from typing import cast

from tools.ironore.qualify import (
    Deadline,
    JavaBot,
    QualificationError,
    ServerProcess,
    Teleport,
    close_position,
    free_loopback_port,
    load_packet_ids,
    parse_position,
    write_pumpkin_config,
)


@dataclass(frozen=True)
class Observation:
    name: str
    passed: bool
    detail: str


class ProbeBot(JavaBot):
    def acknowledge(self, accepted: Teleport) -> None:
        self._ack_teleport(accepted)

    def wait_invalid_movement_disconnect(self) -> None:
        end = time.monotonic() + self._deadline.slice(
            "invalid movement disconnect", 3.0
        )
        with self._condition:
            while self._disconnect is None:
                if self._error is not None:
                    raise QualificationError(self._error)
                remaining = end - time.monotonic()
                if remaining <= 0:
                    raise QualificationError("server accepted an invalid teleport pose")
                self._condition.wait(timeout=remaining)
        reason = self.disconnect
        if reason is None or b"invalid_player_movement" not in bytes.fromhex(reason):
            raise QualificationError(f"wrong disconnect reason: {reason}")


def read_rotation(text: str) -> tuple[float, float] | None:
    match = re.search(r'(?<![\w])"?Rotation"?\s*:\s*\[([^\]]+)\]', text)
    if match is None:
        return None
    values = [value.strip().rstrip("fFdD") for value in match.group(1).split(",")]
    if len(values) != 2:
        return None
    try:
        return float(values[0]), float(values[1])
    except ValueError:
        return None


class TeleportQualification:
    def __init__(self, executable: Path, run_dir: Path, timeout: float) -> None:
        self.executable = executable
        self.run_dir = run_dir
        self.deadline = Deadline(timeout)
        self.port = free_loopback_port()
        self.ids = load_packet_ids(Path(__file__).resolve().parents[2])
        self.server = ServerProcess(
            (str(executable),),
            run_dir,
            run_dir / "harness-server.log",
            self.deadline,
        )
        self.observations: list[Observation] = []
        self.case_number = 0

    def run(self) -> int:
        if not self.executable.is_file():
            raise QualificationError(f"missing executable: {self.executable}")
        if self.run_dir.exists():
            raise QualificationError(f"--run-dir must be new: {self.run_dir}")
        self.run_dir.mkdir(parents=True)
        write_pumpkin_config(self.run_dir, self.port)
        try:
            self.server.start("Server is now running. Connect using port:")
            self._case("accepted_pose_applied", self._accepted_pose)
            self._case(
                "stale_ack_after_completion_ignored", self._stale_after_completion
            )
            self._case("invalid_stale_pose_ignored", self._invalid_stale_pose)
            for field in ("x", "y", "z"):
                self._case(
                    f"nan_{field}_rejected",
                    self._invalid_probe(field, math.nan),
                )
            for field in ("yaw", "pitch"):
                for value, label in ((math.nan, "nan"), (math.inf, "infinite")):
                    self._case(
                        f"{label}_{field}_rejected",
                        self._invalid_probe(field, value),
                    )
            self._case("duplicate_latest_ack_rejected", self._duplicate_latest)
            clean = self.server.stop()
            self.observations.append(
                Observation("clean_stop", clean, f"exit={self.server.returncode}")
            )
        except (OSError, QualificationError, ValueError) as error:
            self.observations.append(Observation("server_lifecycle", False, str(error)))
        finally:
            self.server.kill_owned()
        report = {
            "protocol": 777,
            "executable": str(self.executable),
            "executable_sha256": hashlib.sha256(
                self.executable.read_bytes()
            ).hexdigest(),
            "checks": [asdict(check) for check in self.observations],
        }
        destination = self.run_dir / "teleport-qualification.json"
        destination.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, indent=2))
        return 0 if all(check.passed for check in self.observations) else 1

    def _case(self, name: str, probe: Callable[[ProbeBot, str], None]) -> None:
        self.case_number += 1
        username = f"PortBot{self.case_number}"
        bot = ProbeBot(self.port, self.ids, self.deadline, username)
        try:
            bot.connect()
            bot.wait_ready()
            self.server.wait_for_text(f"{username} joined the game", "player join")
            probe(bot, username)
            self.observations.append(
                Observation(name, True, "observed expected protocol behavior")
            )
        except (OSError, QualificationError, ValueError) as error:
            self.observations.append(Observation(name, False, str(error)))
        finally:
            bot.close()
        # Wait for removal from the server's player map before another login.
        self.server.wait_for_text(f"{username} left the game", "player disconnect")

    def _hold(
        self, bot: ProbeBot, username: str, count: int = 1
    ) -> tuple[Teleport, ...]:
        bot.hold_next_teleports(count)
        self.server.flush_commands(
            tuple(
                f"tp {username} {20.5 + index} 100 20.5 0 0" for index in range(count)
            )
        )
        held = bot.wait_held()
        if len(held) != count:
            raise QualificationError(
                f"expected {count} teleports, received {len(held)}"
            )
        return held

    def _position(self, username: str) -> tuple[float, float, float]:
        result = self.server.run_command(f"data get entity {username}")
        position = parse_position(result.text)
        if position is None:
            raise QualificationError(
                "missing position in entity data: " + result.text[-1500:]
            )
        return position

    def _accepted_pose(self, bot: ProbeBot, username: str) -> None:
        requested = self._hold(bot, username)[0]
        accepted = replace(requested, x=requested.x + 0.125, yaw=90.0, pitch=25.0)
        bot.acknowledge(accepted)
        bot.assert_connected_for(0.5)
        result = self.server.run_command(f"data get entity {username}")
        position = parse_position(result.text)
        rotation = read_rotation(result.text)
        if position is None or not close_position(
            position, (accepted.x, accepted.y, accepted.z)
        ):
            raise QualificationError(f"acknowledged position was discarded: {position}")
        if rotation is None or any(
            abs(a - b) > 0.001 for a, b in zip(rotation, (90.0, 25.0))
        ):
            raise QualificationError(f"acknowledged rotation was discarded: {rotation}")

    def _stale_after_completion(self, bot: ProbeBot, username: str) -> None:
        held = self._hold(bot, username, 2)
        bot.acknowledge_held_in_order()
        bot.assert_connected_for(0.25)
        bot.acknowledge(held[0])
        bot.assert_connected_for(0.5)
        actual = self._position(username)
        expected = (held[1].x, held[1].y, held[1].z)
        if not close_position(actual, expected):
            raise QualificationError(
                f"stale acknowledgement changed position: {actual}"
            )

    def _invalid_stale_pose(self, bot: ProbeBot, username: str) -> None:
        held = self._hold(bot, username, 2)
        bot.acknowledge(replace(held[0], x=math.nan, yaw=math.inf))
        bot.assert_connected_for(0.25)
        bot.acknowledge(held[1])
        bot.assert_connected_for(0.5)
        actual = self._position(username)
        if not close_position(actual, (held[1].x, held[1].y, held[1].z)):
            raise QualificationError(f"latest teleport was not retained: {actual}")

    def _invalid_probe(
        self, field: str, value: float
    ) -> Callable[[ProbeBot, str], None]:
        def probe(bot: ProbeBot, username: str) -> None:
            self._invalid_pose(bot, username, field, value)

        return probe

    def _invalid_pose(
        self, bot: ProbeBot, username: str, field: str, value: float
    ) -> None:
        requested = self._hold(bot, username)[0]
        accepted = Teleport(
            requested.teleport_id,
            value if field == "x" else requested.x,
            value if field == "y" else requested.y,
            value if field == "z" else requested.z,
            value if field == "yaw" else requested.yaw,
            value if field == "pitch" else requested.pitch,
        )
        bot.acknowledge(accepted)
        bot.wait_invalid_movement_disconnect()

    def _duplicate_latest(self, bot: ProbeBot, username: str) -> None:
        requested = self._hold(bot, username)[0]
        bot.acknowledge_held_in_order()
        bot.assert_connected_for(0.25)
        bot.acknowledge(requested)
        bot.wait_invalid_movement_disconnect()


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=240.0)
    args = parser.parse_args(argv)
    runner = TeleportQualification(
        cast(Path, args.executable).expanduser().resolve(),
        cast(Path, args.run_dir).expanduser().resolve(),
        cast(float, args.timeout),
    )
    try:
        return runner.run()
    except (OSError, QualificationError, ValueError) as error:
        print(f"teleport qualification failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
