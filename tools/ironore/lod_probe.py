"""Drive Pumpkin's ironore: background generation with a protocol-only client.

Starts the server in a run directory, joins with a minimal Java 26.3 client,
sends `ironore:lod_request`, acknowledges every `ironore:lod_chunk` the way
Ironore's client does, and waits until the server reports the disc complete.
It checks that chunks arrive inside the disc, once each and nearest first,
and measures the server's generation rate, CPU time per chunk, peak memory
and tick times. Nothing renders, so the numbers are the server's alone.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import struct
import threading
import time
from dataclasses import dataclass, field
from itertools import pairwise
from pathlib import Path
from typing import cast

from tools.ironore.qualify import (
    Deadline,
    JavaBot,
    PacketIds,
    QualificationError,
    ServerProcess,
    decode_varint_bytes,
    encode_string,
    encode_varint,
    free_loopback_port,
    load_packet_ids,
)

REPO = Path(__file__).resolve().parents[2]
REQUEST = "ironore:lod_request"
CHUNK = "ironore:lod_chunk"
ACK = "ironore:lod_ack"
PROGRESS = "ironore:lod_progress"
MAX_UNACKNOWLEDGED = 256
ACK_INTERVAL = 0.1
CLOCK_TICKS = os.sysconf("SC_CLK_TCK")


@dataclass
class ProgressReport:
    total: int
    generated: int
    sent: int
    centre_x: int
    centre_z: int
    complete_through: int


@dataclass
class Received:
    sequences: dict[tuple[int, int], int] = field(default_factory=dict)
    order: list[tuple[int, int]] = field(default_factory=list)
    duplicates: int = 0
    progress: list[ProgressReport] = field(default_factory=list)
    max_unacknowledged: int = 0


class LodBot(JavaBot):
    def __init__(
        self,
        port: int,
        packet_ids: PacketIds,
        deadline: Deadline,
        custom_payload_ids: tuple[int, int],
    ) -> None:
        super().__init__(port, packet_ids, deadline)
        self._custom_clientbound, self._custom_serverbound = custom_payload_ids
        self.received = Received()
        self._acknowledged = 0

    def request(
        self, radius: int, known: int, known_centre: tuple[int, int], known_radius: int
    ) -> None:
        self._send_custom(
            REQUEST,
            encode_varint(radius)
            + struct.pack(">qii", known, *known_centre)
            + encode_varint(known_radius),
        )

    def acknowledge(self) -> None:
        with self._condition:
            applied = len(self.received.order)
            if applied == self._acknowledged:
                return
            self._acknowledged = applied
        self._send_custom(ACK, encode_varint(applied))

    @property
    def max_sequence(self) -> int:
        with self._condition:
            return max(self.received.sequences.values(), default=-1)

    @property
    def acknowledged(self) -> int:
        with self._condition:
            return self._acknowledged

    def snapshot(self) -> tuple[int, ProgressReport | None]:
        with self._condition:
            last = self.received.progress[-1] if self.received.progress else None
            return len(self.received.order), last

    def _send_custom(self, channel: str, data: bytes) -> None:
        self._send(self._custom_serverbound, encode_string(channel) + data)

    def _handle(self, packet_id: int, payload: bytes) -> None:
        if self._state != "play" or packet_id != self._custom_clientbound:
            super()._handle(packet_id, payload)
            return
        length, offset = decode_varint_bytes(payload)
        channel = payload[offset : offset + length].decode("utf-8")
        data = payload[offset + length :]
        if channel == CHUNK:
            sequence, x, z = struct.unpack_from(">qii", data)
            with self._condition:
                received = self.received
                if (x, z) in received.sequences:
                    received.duplicates += 1
                received.sequences[(x, z)] = sequence
                received.order.append((x, z))
                received.max_unacknowledged = max(
                    received.max_unacknowledged,
                    len(received.order) - self._acknowledged,
                )
        elif channel == PROGRESS:
            values = []
            rest = data
            for _ in range(3):
                value, used = decode_varint_bytes(rest)
                values.append(value)
                rest = rest[used:]
            centre_x, centre_z, complete_through = struct.unpack(">iiq", rest)
            with self._condition:
                self.received.progress.append(
                    ProgressReport(
                        values[0],
                        values[1],
                        values[2],
                        centre_x,
                        centre_z,
                        complete_through,
                    )
                )
                self._condition.notify_all()


def custom_payload_ids() -> tuple[int, int]:
    raw = cast(
        dict[str, dict[str, dict[str, int]]],
        json.loads((REPO / "assets" / "packets.json").read_text(encoding="utf-8")),
    )
    return (
        raw["clientbound"]["play"]["custom_payload"],
        raw["serverbound"]["play"]["custom_payload"],
    )


def write_config(run_dir: Path, port: int, view_distance: int) -> None:
    config = f"""seed = "ironore-lod-probe"
default_difficulty = "Peaceful"
default_gamemode = "Creative"
default_level_name = "world"
allow_nether = false
allow_end = false
white_list = false
online_mode = false

[networking.java]
enabled = true
address = "127.0.0.1:{port}"
online_mode = false
encryption = false
max_players = 1
view_distance = {view_distance}
simulation_distance = {view_distance}

[networking.java.compression]
enabled = false

[networking.java.authentication]
enabled = false

[networking.bedrock]
enabled = false

[networking.query]
enabled = false

[networking.lan_broadcast]
enabled = false

[telemetry]
enabled = false
public = false
endpoint = "http://127.0.0.1:9/disabled"

[plugins]
enabled = false

[commands]
use_console = true
use_tty = false
log_console = true

[logging]
enabled = true
level = "info"
color = false
file = ""
"""
    (run_dir / "pumpkin.toml").write_text(config, encoding="utf-8")


@dataclass
class ProcessSample:
    cpu_seconds: float
    peak_rss_mib: float


def sample_process(pid: int) -> ProcessSample:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    fields = stat[stat.rindex(")") + 2 :].split()
    cpu = (int(fields[11]) + int(fields[12])) / CLOCK_TICKS
    status = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    match = re.search(r"^VmHWM:\s+(\d+) kB", status, re.MULTILINE)
    peak = int(match.group(1)) / 1024 if match else 0.0
    return ProcessSample(cpu, peak)


def ring(dx: int, dz: int) -> int:
    return math.isqrt(dx * dx + dz * dz)


def run(args: argparse.Namespace) -> dict[str, object]:
    run_dir: Path = args.run_dir
    if args.reuse:
        if not (run_dir / "world").is_dir():
            raise QualificationError(f"--reuse needs an existing world in {run_dir}")
    else:
        run_dir.mkdir(parents=True, exist_ok=False)
    port = free_loopback_port()
    write_config(run_dir, port, args.view_distance)
    deadline = Deadline(args.timeout)
    log_path = run_dir / "probe-server.log"
    server = ServerProcess((str(args.executable),), run_dir, log_path, deadline)
    server.start("Server is now running. Connect using port:")
    process = server._process
    if process is None:
        raise QualificationError("server process did not start")
    pid = process.pid
    bot = LodBot(port, load_packet_ids(REPO), deadline, custom_payload_ids())
    try:
        bot.connect()
        teleport = bot.wait_ready()
        start_sample = sample_process(pid)
        started = time.monotonic()
        bot.request(
            args.radius, args.known, (args.known_x, args.known_z), args.known_radius
        )
        stop_ack = threading.Event()

        def acknowledge_periodically() -> None:
            while not stop_ack.wait(ACK_INTERVAL):
                bot.acknowledge()

        acker = threading.Thread(target=acknowledge_periodically, daemon=True)
        acker.start()
        last_print = started
        finished_at: float | None = None
        last: ProgressReport | None = None
        while True:
            bot.assert_connected()
            count, last = bot.snapshot()
            now = time.monotonic()
            if now - last_print >= 5.0:
                last_print = now
                sample = sample_process(pid)
                print(
                    f"{now - started:7.1f}s received {count}"
                    + f", server cpu {sample.cpu_seconds - start_sample.cpu_seconds:.1f}s"
                    + f", peak rss {sample.peak_rss_mib:.0f} MiB"
                    + (
                        f", progress generated {last.generated} sent {last.sent} of {last.total}"
                        if last
                        else ""
                    ),
                    flush=True,
                )
            if (
                last is not None
                and last.sent == last.total
                and last.generated == last.total
                and count == bot.acknowledged
                and last.complete_through >= bot.max_sequence
            ):
                finished_at = now
                break
            deadline.remaining("background generation of the disc")
            time.sleep(0.05)
        end_sample = sample_process(pid)
        stop_ack.set()
        acker.join()
        bot.acknowledge()
        received = bot.received
        centre = (last.centre_x, last.centre_z) if last else (0, 0)
        rings = [ring(x - centre[0], z - centre[1]) for x, z in received.order]
        inversions = sum(1 for a, b in pairwise(rings) if b < a)
        outside = sum(1 for r in rings if r >= args.radius)
        elapsed = finished_at - started
        generated_lines = [
            line.strip()
            for line in log_path.read_text(encoding="utf-8").splitlines()
            if "Background generation in" in line
        ]
        cpu = end_sample.cpu_seconds - start_sample.cpu_seconds
        chunks = len(received.order)
        return {
            "radius": args.radius,
            "known": args.known,
            "spawn": [teleport.x, teleport.y, teleport.z],
            "centre": list(centre),
            "disc_total": last.total if last else None,
            "chunks_received": chunks,
            "duplicates": received.duplicates,
            "outside_disc": outside,
            "ring_order_inversions": inversions,
            "max_ring_drop": max((a - b for a, b in pairwise(rings)), default=0),
            "max_unacknowledged": received.max_unacknowledged,
            "max_sequence": max(received.sequences.values(), default=-1),
            "complete_through": last.complete_through if last else None,
            "seconds": round(elapsed, 2),
            "chunks_per_second": round(chunks / elapsed, 2) if elapsed else None,
            "server_cpu_seconds": round(cpu, 2),
            "cpu_ms_per_chunk": round(1000 * cpu / chunks, 1) if chunks else None,
            "server_peak_rss_mib": round(end_sample.peak_rss_mib, 1),
            "server_log_rate_lines": generated_lines,
        }
    finally:
        bot.close()
        server.stop()


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--executable", type=Path, required=True)
    result.add_argument("--run-dir", type=Path, required=True)
    result.add_argument("--radius", type=int, default=36)
    result.add_argument("--known", type=int, default=-1)
    result.add_argument("--known-x", type=int, default=0)
    result.add_argument("--known-z", type=int, default=0)
    result.add_argument("--known-radius", type=int, default=0)
    result.add_argument("--view-distance", type=int, default=2)
    result.add_argument("--timeout", type=float, default=1800.0)
    result.add_argument(
        "--reuse", action="store_true", help="rejoin the world already in --run-dir"
    )
    return result


def main() -> int:
    summary = run(parser().parse_args())
    text = json.dumps(summary, indent=2)
    print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
