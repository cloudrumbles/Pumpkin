#!/usr/bin/env python3
"""Disposable Java 26.3 qualification for Pumpkin and the official server."""

from __future__ import annotations

import argparse
import gzip
import json
import os
import shutil
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
import uuid
import zlib
from collections.abc import Callable, Sequence
from dataclasses import asdict, dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal, TextIO, TypeAlias, cast

PROTOCOL_VERSION = 777
EXPECTED_VERSION_NAME = "26.3"
BOT_NAME = "IronoreBot"
BOT_UUID = uuid.UUID("8e7157f0-5878-4fb1-a728-a330dc58b64d")
COMMAND_WAIT_SECONDS = 5.0
DEFAULT_VANILLA_JAR = Path(
    "/home/shah/.cache/ironore-java-reference/minecraft-server-26.3.jar"
)
CHECK_NAMES = (
    "server_start",
    "protocol_login",
    "teleport_queue_ack_order",
    "teleport_latest_server_position",
    "datapack_namespaced_function",
    "datapack_default_function",
    "setblock_properties",
    "fill_properties",
    "bat_flags_and_position",
    "clock_configured",
    "block_configured",
    "chest_removed_before_save",
    "clean_first_stop",
    "clock_nbt_persisted",
    "chest_nbt_removed",
    "restart_server",
    "restart_protocol_login",
    "clock_restart_query",
    "block_restart_query",
    "player_position_persisted",
    "bat_persistence",
    "clean_final_stop",
)

Status: TypeAlias = Literal["pass", "fail", "unsupported", "unrun"]
NbtValue: TypeAlias = (
    int | float | str | bytes | list["NbtValue"] | dict[str, "NbtValue"]
)


class QualificationError(RuntimeError):
    """A bounded qualification operation failed."""


class UnsupportedNbt(QualificationError):
    """An NBT storage encoding is valid but unsupported by this harness."""


@dataclass(frozen=True)
class Check:
    name: str
    status: Status
    detail: str
    evidence: dict[str, object] = field(default_factory=dict)


@dataclass(frozen=True)
class CommandResult:
    command: str
    lines: tuple[str, ...]

    @property
    def text(self) -> str:
        return "\n".join(self.lines)

    @property
    def looks_unsupported(self) -> bool:
        lowered = self.text.lower()
        needles = (
            "unknown command",
            "incorrect argument",
            "expected ",
            "can't find element",
            "could not parse",
        )
        return any(needle in lowered for needle in needles)


@dataclass(frozen=True)
class Teleport:
    teleport_id: int
    x: float
    y: float
    z: float
    yaw: float
    pitch: float


@dataclass(frozen=True)
class StoredBlockState:
    name: str
    properties: dict[str, str]


@dataclass(frozen=True)
class FunctionBlockFixture:
    check_name: str
    function_id: str
    position: tuple[int, int, int]
    block_name: str


FUNCTION_BLOCK_FIXTURES = (
    FunctionBlockFixture(
        "datapack_namespaced_function",
        "ironore:namespaced",
        (26, 100, 26),
        "minecraft:diamond_block",
    ),
    FunctionBlockFixture(
        "datapack_default_function",
        "minecraft:default",
        (27, 100, 26),
        "minecraft:emerald_block",
    ),
)


@dataclass(frozen=True)
class PacketIds:
    login_finished: int
    login_compression: int
    login_disconnect: int
    login_acknowledged: int
    config_known_packs_clientbound: int
    config_known_packs_serverbound: int
    config_finish_clientbound: int
    config_finish_serverbound: int
    config_keep_alive_clientbound: int
    config_keep_alive_serverbound: int
    config_ping_clientbound: int
    config_pong_serverbound: int
    config_disconnect: int
    play_login: int
    play_player_position: int
    play_keep_alive_clientbound: int
    play_keep_alive_serverbound: int
    play_chunk_batch_finished: int
    play_chunk_batch_received: int
    play_player_loaded: int
    play_accept_teleportation: int
    play_disconnect: int
    play_ping: int
    play_pong: int


class Deadline:
    def __init__(self, timeout: float) -> None:
        if timeout <= 0:
            raise ValueError("timeout must be positive")
        self._end = time.monotonic() + timeout

    def remaining(self, operation: str) -> float:
        remaining = self._end - time.monotonic()
        if remaining <= 0:
            raise QualificationError(f"overall timeout expired during {operation}")
        return remaining

    def slice(self, operation: str, maximum: float = 20.0) -> float:
        return min(self.remaining(operation), maximum)


class Results:
    def __init__(self) -> None:
        self._checks: dict[str, Check] = {}

    def set(
        self,
        name: str,
        status: Status,
        detail: str,
        **evidence: object,
    ) -> None:
        if name not in CHECK_NAMES:
            raise ValueError(f"unknown check name: {name}")
        self._checks[name] = Check(name, status, detail, evidence)

    def has(self, name: str) -> bool:
        return name in self._checks

    def finish_unrun(self, reason: str) -> None:
        for name in CHECK_NAMES:
            if name not in self._checks:
                self.set(name, "unrun", reason)

    def as_list(self) -> list[dict[str, object]]:
        return [asdict(self._checks[name]) for name in CHECK_NAMES]

    def statuses(self) -> list[Status]:
        return [self._checks[name].status for name in CHECK_NAMES]


def encode_varint(value: int) -> bytes:
    if not -(1 << 31) <= value < (1 << 31):
        raise ValueError("VarInt outside signed 32-bit range")
    unsigned = value & 0xFFFFFFFF
    out = bytearray()
    while True:
        byte = unsigned & 0x7F
        unsigned >>= 7
        if unsigned:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def decode_varint_bytes(data: bytes, offset: int = 0) -> tuple[int, int]:
    result = 0
    for index in range(5):
        position = offset + index
        if position >= len(data):
            raise QualificationError("truncated VarInt")
        byte = data[position]
        result |= (byte & 0x7F) << (7 * index)
        if byte & 0x80 == 0:
            if result & (1 << 31):
                result -= 1 << 32
            return result, position + 1
    raise QualificationError("VarInt exceeds five bytes")


def encode_string(value: str) -> bytes:
    raw = value.encode("utf-8")
    return encode_varint(len(raw)) + raw


def frame(packet_id: int, payload: bytes = b"") -> bytes:
    body = encode_varint(packet_id) + payload
    return encode_varint(len(body)) + body


def _recv_exact(sock: socket.socket, length: int) -> bytes:
    result = bytearray()
    while len(result) < length:
        chunk = sock.recv(length - len(result))
        if not chunk:
            raise EOFError("server closed the protocol connection")
        result.extend(chunk)
    return bytes(result)


def _recv_varint(sock: socket.socket) -> int:
    raw = bytearray()
    for _ in range(5):
        raw.extend(_recv_exact(sock, 1))
        if raw[-1] & 0x80 == 0:
            return decode_varint_bytes(bytes(raw))[0]
    raise QualificationError("network VarInt exceeds five bytes")


def load_packet_ids(repo: Path) -> PacketIds:
    packet_path = repo / "assets" / "packets.json"
    raw = cast(dict[str, object], json.loads(packet_path.read_text(encoding="utf-8")))
    version = raw.get("version")
    if version != PROTOCOL_VERSION:
        raise QualificationError(
            f"{packet_path} protocol is {version!r}, expected {PROTOCOL_VERSION}"
        )
    serverbound = cast(dict[str, dict[str, int]], raw["serverbound"])
    clientbound = cast(dict[str, dict[str, int]], raw["clientbound"])
    return PacketIds(
        login_finished=clientbound["login"]["login_finished"],
        login_compression=clientbound["login"]["login_compression"],
        login_disconnect=clientbound["login"]["login_disconnect"],
        login_acknowledged=serverbound["login"]["login_acknowledged"],
        config_known_packs_clientbound=clientbound["config"]["select_known_packs"],
        config_known_packs_serverbound=serverbound["config"]["select_known_packs"],
        config_finish_clientbound=clientbound["config"]["finish_configuration"],
        config_finish_serverbound=serverbound["config"]["finish_configuration"],
        config_keep_alive_clientbound=clientbound["config"]["keep_alive"],
        config_keep_alive_serverbound=serverbound["config"]["keep_alive"],
        config_ping_clientbound=clientbound["config"]["ping"],
        config_pong_serverbound=serverbound["config"]["pong"],
        config_disconnect=clientbound["config"]["disconnect"],
        play_login=clientbound["play"]["login"],
        play_player_position=clientbound["play"]["player_position"],
        play_keep_alive_clientbound=clientbound["play"]["keep_alive"],
        play_keep_alive_serverbound=serverbound["play"]["keep_alive"],
        play_chunk_batch_finished=clientbound["play"]["chunk_batch_finished"],
        play_chunk_batch_received=serverbound["play"]["chunk_batch_received"],
        play_player_loaded=serverbound["play"]["player_loaded"],
        play_accept_teleportation=serverbound["play"]["accept_teleportation"],
        play_disconnect=clientbound["play"]["disconnect"],
        play_ping=clientbound["play"]["ping"],
        play_pong=serverbound["play"]["pong"],
    )


class JavaBot:
    def __init__(
        self,
        port: int,
        packet_ids: PacketIds,
        deadline: Deadline,
        name: str = BOT_NAME,
    ) -> None:
        self._port = port
        self._ids = packet_ids
        self._deadline = deadline
        self._name = name
        self._socket: socket.socket | None = None
        self._thread: threading.Thread | None = None
        self._condition = threading.Condition()
        self._state = "new"
        self._error: str | None = None
        self._disconnect: str | None = None
        self._play_login_seen = False
        self._player_loaded_sent = False
        self._teleports: list[Teleport] = []
        self._held: list[Teleport] = []
        self._hold_count = 0

    @property
    def error(self) -> str | None:
        with self._condition:
            return self._error

    @property
    def disconnect(self) -> str | None:
        with self._condition:
            return self._disconnect

    @property
    def teleports(self) -> tuple[Teleport, ...]:
        with self._condition:
            return tuple(self._teleports)

    def connect(self) -> None:
        sock = socket.create_connection(
            ("127.0.0.1", self._port), self._deadline.slice("bot TCP connect")
        )
        sock.settimeout(min(1.0, self._deadline.remaining("bot socket setup")))
        self._socket = sock
        handshake = (
            encode_varint(PROTOCOL_VERSION)
            + encode_string("127.0.0.1")
            + struct.pack(">H", self._port)
            + encode_varint(2)
        )
        self._send(0, handshake)
        self._send(0, encode_string(self._name) + BOT_UUID.bytes)
        self._state = "login"
        self._thread = threading.Thread(
            target=self._run,
            name=f"ironore-java-bot-{self._port}",
            daemon=True,
        )
        self._thread.start()

    def close(self) -> None:
        sock = self._socket
        self._socket = None
        if sock is not None:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            sock.close()
        thread = self._thread
        if thread is not None:
            thread.join(timeout=2.0)

    def wait_ready(self) -> Teleport:
        def ready() -> bool:
            return self._play_login_seen and bool(self._teleports)

        self._wait_for(ready, "protocol login and initial teleport")
        with self._condition:
            if self._error is not None:
                raise QualificationError(self._error)
            return self._teleports[0]

    def hold_next_teleports(self, count: int) -> None:
        if count <= 0:
            raise ValueError("hold count must be positive")
        with self._condition:
            if self._hold_count or self._held:
                raise QualificationError("a teleport hold is already active")
            self._hold_count = count

    def wait_held(self) -> tuple[Teleport, ...]:
        self._wait_for(
            lambda: self._hold_count > 0 and len(self._held) >= self._hold_count,
            "two queued teleport packets",
        )
        with self._condition:
            return tuple(self._held)

    def acknowledge_held_in_order(self) -> tuple[Teleport, ...]:
        with self._condition:
            held = tuple(self._held)
            expected = self._hold_count
            if expected == 0 or len(held) < expected:
                raise QualificationError("held teleports are not ready")
        for teleport in held:
            self._ack_teleport(teleport)
        with self._condition:
            self._held.clear()
            self._hold_count = 0
            self._condition.notify_all()
        return held

    def release_held(self) -> tuple[Teleport, ...]:
        """Acknowledge any collected teleports after a failed hold assertion."""
        with self._condition:
            held = tuple(self._held)
        for teleport in held:
            self._ack_teleport(teleport)
        with self._condition:
            self._held.clear()
            self._hold_count = 0
            self._condition.notify_all()
        return held

    def assert_connected(self) -> None:
        with self._condition:
            if self._error is not None:
                raise QualificationError(self._error)
            if self._disconnect is not None:
                raise QualificationError(f"server disconnected bot: {self._disconnect}")
            if self._socket is None:
                raise QualificationError("bot socket is closed")

    def assert_connected_for(self, seconds: float) -> None:
        end = time.monotonic() + self._deadline.slice(
            "post-teleport connection observation", seconds
        )
        with self._condition:
            while True:
                if self._error is not None:
                    raise QualificationError(self._error)
                if self._disconnect is not None:
                    raise QualificationError(
                        f"server disconnected bot: {self._disconnect}"
                    )
                remaining = end - time.monotonic()
                if remaining <= 0:
                    return
                self._condition.wait(timeout=remaining)

    def _wait_for(self, predicate: Callable[[], bool], operation: str) -> None:
        check = predicate
        end = time.monotonic() + self._deadline.slice(operation)
        with self._condition:
            while not check():
                if self._error is not None:
                    raise QualificationError(self._error)
                if self._disconnect is not None:
                    raise QualificationError(
                        f"server disconnected bot: {self._disconnect}"
                    )
                remaining = end - time.monotonic()
                if remaining <= 0:
                    raise QualificationError(f"timed out waiting for {operation}")
                self._condition.wait(timeout=remaining)

    def _send(self, packet_id: int, payload: bytes = b"") -> None:
        sock = self._socket
        if sock is None:
            raise QualificationError("cannot send on closed bot socket")
        sock.sendall(frame(packet_id, payload))

    def _run(self) -> None:
        try:
            while self._socket is not None:
                try:
                    length = _recv_varint(self._socket)
                    if length < 0 or length > 64 * 1024 * 1024:
                        raise QualificationError(f"invalid packet length {length}")
                    body = _recv_exact(self._socket, length)
                except TimeoutError:
                    self._deadline.remaining("protocol session")
                    continue
                packet_id, offset = decode_varint_bytes(body)
                self._handle(packet_id, body[offset:])
        except EOFError as error:
            with self._condition:
                if self._socket is not None:
                    self._error = str(error)
                self._condition.notify_all()
        except (OSError, QualificationError, struct.error, ValueError) as error:
            with self._condition:
                if self._socket is not None:
                    self._error = f"protocol client failed: {error}"
                self._condition.notify_all()

    def _handle(self, packet_id: int, payload: bytes) -> None:
        if self._state == "login":
            if packet_id == self._ids.login_disconnect:
                self._record_disconnect(payload)
            elif packet_id == self._ids.login_compression:
                raise QualificationError(
                    "server enabled packet compression despite qualification config"
                )
            elif packet_id == self._ids.login_finished:
                self._send(self._ids.login_acknowledged)
                self._state = "config"
            return

        if self._state == "config":
            if packet_id == self._ids.config_disconnect:
                self._record_disconnect(payload)
            elif packet_id == self._ids.config_known_packs_clientbound:
                self._send(self._ids.config_known_packs_serverbound, payload)
            elif packet_id == self._ids.config_keep_alive_clientbound:
                if len(payload) != 8:
                    raise QualificationError("malformed config keepalive")
                self._send(self._ids.config_keep_alive_serverbound, payload)
            elif packet_id == self._ids.config_ping_clientbound:
                if len(payload) != 4:
                    raise QualificationError("malformed config ping")
                self._send(self._ids.config_pong_serverbound, payload)
            elif packet_id == self._ids.config_finish_clientbound:
                self._send(self._ids.config_finish_serverbound)
                self._state = "play"
            return

        if packet_id == self._ids.play_disconnect:
            self._record_disconnect(payload)
        elif packet_id == self._ids.play_login:
            self._play_login_seen = True
            if not self._player_loaded_sent:
                self._send(self._ids.play_player_loaded)
                self._player_loaded_sent = True
            with self._condition:
                self._condition.notify_all()
        elif packet_id == self._ids.play_keep_alive_clientbound:
            if len(payload) != 8:
                raise QualificationError("malformed play keepalive")
            self._send(self._ids.play_keep_alive_serverbound, payload)
        elif packet_id == self._ids.play_chunk_batch_finished:
            self._send(self._ids.play_chunk_batch_received, struct.pack(">f", 64.0))
        elif packet_id == self._ids.play_ping:
            if len(payload) != 4:
                raise QualificationError("malformed play ping")
            self._send(self._ids.play_pong, payload)
        elif packet_id == self._ids.play_player_position:
            teleport = self._parse_teleport(payload)
            with self._condition:
                self._teleports.append(teleport)
                should_hold = self._hold_count > 0
                if should_hold:
                    self._held.append(teleport)
                self._condition.notify_all()
            if not should_hold:
                self._ack_teleport(teleport)

    def _parse_teleport(self, payload: bytes) -> Teleport:
        teleport_id, offset = decode_varint_bytes(payload)
        required = 3 * 8 + 3 * 8 + 2 * 4 + 4
        if len(payload) - offset < required:
            raise QualificationError("truncated 26.3 player-position packet")
        values = struct.unpack_from(">ddddddffi", payload, offset)
        return Teleport(
            teleport_id=teleport_id,
            x=values[0],
            y=values[1],
            z=values[2],
            yaw=values[6],
            pitch=values[7],
        )

    def _ack_teleport(self, teleport: Teleport) -> None:
        payload = encode_varint(teleport.teleport_id) + struct.pack(
            ">dddff",
            teleport.x,
            teleport.y,
            teleport.z,
            teleport.yaw,
            teleport.pitch,
        )
        self._send(self._ids.play_accept_teleportation, payload)

    def _record_disconnect(self, payload: bytes) -> None:
        reason = payload[:512].hex()
        with self._condition:
            self._disconnect = reason
            self._condition.notify_all()


def command_completion_probe() -> tuple[str, str]:
    # `say` does not write to Pumpkin's console. `list` emits one feedback line
    # even with no connected players, so it also frames commands after a kick.
    return "list", "players online"


class ServerProcess:
    def __init__(
        self,
        command: Sequence[str],
        run_dir: Path,
        log_path: Path,
        deadline: Deadline,
    ) -> None:
        self._command = tuple(command)
        self._run_dir = run_dir
        self._log_path = log_path
        self._deadline = deadline
        self._process: subprocess.Popen[str] | None = None
        self._condition = threading.Condition()
        self._lines: list[str] = []
        self._reader: threading.Thread | None = None
        self._log_file: TextIO | None = None

    @property
    def returncode(self) -> int | None:
        return None if self._process is None else self._process.poll()

    def start(self, readiness: str) -> None:
        log_file = self._log_path.open("a", encoding="utf-8")
        self._log_file = log_file
        log_file.write(f"\n=== START {' '.join(self._command)} ===\n")
        log_file.flush()
        process = subprocess.Popen(
            self._command,
            cwd=self._run_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            start_new_session=True,
        )
        self._process = process
        self._reader = threading.Thread(
            target=self._read_output,
            name=f"ironore-server-output-{process.pid}",
            daemon=True,
        )
        self._reader.start()
        self.wait_for_text(readiness, "server readiness")

    def run_command(self, command: str, *, maximum: float = COMMAND_WAIT_SECONDS) -> CommandResult:
        completion_command, completion_marker = command_completion_probe()
        with self._condition:
            start = len(self._lines)
        self._write_stdin(f"{command}\n{completion_command}\n")
        self.wait_for_text(
            completion_marker,
            f"command completion: {command}",
            start,
            maximum=maximum,
        )
        with self._condition:
            relevant = tuple(
                line for line in self._lines[start:] if completion_marker not in line
            )
        return CommandResult(command, relevant)

    def flush_commands(self, commands: Sequence[str]) -> None:
        self._write_stdin("".join(f"{command}\n" for command in commands))

    def stop(self) -> bool:
        process = self._process
        if process is None:
            return True
        if process.poll() is None:
            try:
                self._write_stdin("stop\n")
                process.wait(timeout=self._deadline.slice("clean server stop", 30.0))
            except (QualificationError, subprocess.TimeoutExpired):
                self.kill_owned()
                return False
        clean = process.returncode == 0
        self._close_streams()
        return clean

    def kill_owned(self) -> None:
        process = self._process
        if process is not None and process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=3.0)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                if process.poll() is None:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    try:
                        process.wait(timeout=3.0)
                    except subprocess.TimeoutExpired:
                        pass
        self._close_streams()

    def wait_for_text(
        self,
        needle: str,
        operation: str,
        start: int = 0,
        maximum: float = 60.0,
    ) -> None:
        end = time.monotonic() + self._deadline.slice(operation, maximum)
        with self._condition:
            while not any(needle in line for line in self._lines[start:]):
                process = self._process
                if process is not None and process.poll() is not None:
                    excerpt = "\n".join(self._lines[-30:])
                    raise QualificationError(
                        f"server exited {process.returncode} during {operation}\n{excerpt}"
                    )
                remaining = end - time.monotonic()
                if remaining <= 0:
                    excerpt = "\n".join(self._lines[-30:])
                    raise QualificationError(
                        f"timed out during {operation}; output tail:\n{excerpt}"
                    )
                self._condition.wait(timeout=remaining)

    def _write_stdin(self, text: str) -> None:
        process = self._process
        if process is None or process.poll() is not None or process.stdin is None:
            raise QualificationError("server stdin is unavailable")
        process.stdin.write(text)
        process.stdin.flush()

    def _read_output(self) -> None:
        process = self._process
        log_file = self._log_file
        if process is None or process.stdout is None or log_file is None:
            return
        writer = log_file
        for raw_line in process.stdout:
            line = raw_line.rstrip("\r\n")
            writer.write(line + "\n")
            writer.flush()
            with self._condition:
                self._lines.append(line)
                self._condition.notify_all()
        with self._condition:
            self._condition.notify_all()

    def _close_streams(self) -> None:
        process = self._process
        if process is not None:
            if process.stdin is not None:
                try:
                    process.stdin.close()
                except OSError:
                    pass
            if process.stdout is not None:
                process.stdout.close()
        reader = self._reader
        if reader is not None:
            reader.join(timeout=2.0)
        log_file = self._log_file
        if log_file is not None:
            log_file.close()
        self._log_file = None


class NbtReader:
    def __init__(self, data: bytes) -> None:
        self._data = data
        self._offset = 0

    def read_root(self) -> dict[str, NbtValue]:
        tag = self._u8()
        if tag != 10:
            raise QualificationError(f"NBT root tag is {tag}, expected compound")
        self._string()
        value = self._payload(tag)
        if not isinstance(value, dict):
            raise QualificationError("NBT root did not decode as a compound")
        return value

    def _take(self, length: int) -> bytes:
        end = self._offset + length
        if length < 0 or end > len(self._data):
            raise QualificationError("truncated NBT")
        value = self._data[self._offset : end]
        self._offset = end
        return value

    def _unpack(self, fmt: str) -> int | float:
        size = struct.calcsize(fmt)
        return cast(int | float, struct.unpack(fmt, self._take(size))[0])

    def _u8(self) -> int:
        return cast(int, self._unpack(">B"))

    def _i32(self) -> int:
        return cast(int, self._unpack(">i"))

    def _string(self) -> str:
        length = cast(int, self._unpack(">H"))
        return self._take(length).decode("utf-8", errors="replace")

    def _payload(self, tag: int) -> NbtValue:
        if tag == 1:
            return cast(int, self._unpack(">b"))
        if tag == 2:
            return cast(int, self._unpack(">h"))
        if tag == 3:
            return self._i32()
        if tag == 4:
            return cast(int, self._unpack(">q"))
        if tag == 5:
            return cast(float, self._unpack(">f"))
        if tag == 6:
            return cast(float, self._unpack(">d"))
        if tag == 7:
            return self._take(self._i32())
        if tag == 8:
            return self._string()
        if tag == 9:
            element_tag = self._u8()
            length = self._i32()
            if length < 0:
                raise QualificationError("negative NBT list length")
            return [self._payload(element_tag) for _ in range(length)]
        if tag == 10:
            compound: dict[str, NbtValue] = {}
            while True:
                child_tag = self._u8()
                if child_tag == 0:
                    return compound
                name = self._string()
                compound[name] = self._payload(child_tag)
        if tag == 11:
            length = self._i32()
            if length < 0:
                raise QualificationError("negative NBT int-array length")
            return [self._i32() for _ in range(length)]
        if tag == 12:
            length = self._i32()
            if length < 0:
                raise QualificationError("negative NBT long-array length")
            return [cast(int, self._unpack(">q")) for _ in range(length)]
        raise QualificationError(f"unknown NBT tag {tag}")


def read_gzip_nbt(path: Path) -> dict[str, NbtValue]:
    return NbtReader(gzip.decompress(path.read_bytes())).read_root()


def read_region_chunk(path: Path, chunk_x: int, chunk_z: int) -> dict[str, NbtValue]:
    local_x = chunk_x % 32
    local_z = chunk_z % 32
    index = local_x + local_z * 32
    with path.open("rb") as stream:
        stream.seek(index * 4)
        location = stream.read(4)
        if len(location) != 4:
            raise QualificationError(f"truncated region header: {path}")
        sector = int.from_bytes(location[:3], "big")
        sectors = location[3]
        if sector == 0 or sectors == 0:
            raise QualificationError(
                f"chunk {chunk_x},{chunk_z} has no region entry in {path}"
            )
        stream.seek(sector * 4096)
        length_raw = stream.read(4)
        if len(length_raw) != 4:
            raise QualificationError("truncated region chunk length")
        length = int.from_bytes(length_raw, "big")
        compression_raw = stream.read(1)
        if len(compression_raw) != 1:
            raise QualificationError("missing region compression type")
        compression = compression_raw[0]
        payload = stream.read(length - 1)
    if compression & 0x80:
        raise UnsupportedNbt("external Anvil chunk storage is unsupported")
    if compression == 1:
        decoded = gzip.decompress(payload)
    elif compression == 2:
        decoded = zlib.decompress(payload)
    elif compression == 3:
        decoded = payload
    elif compression == 4:
        raise UnsupportedNbt("LZ4-compressed Anvil chunks are unsupported")
    else:
        raise UnsupportedNbt(f"Anvil compression type {compression} is unsupported")
    return NbtReader(decoded).read_root()


def find_region(run_dir: Path, chunk_x: int, chunk_z: int) -> Path:
    region_name = f"r.{chunk_x // 32}.{chunk_z // 32}.mca"
    candidates = [
        path
        for path in (run_dir / "world").rglob(region_name)
        if path.parent.name == "region"
    ]
    if not candidates:
        raise QualificationError(f"could not find overworld region {region_name}")
    candidates.sort(key=lambda path: ("overworld" not in path.parts, len(path.parts)))
    return candidates[0]


def nested_mapping(
    root: dict[str, NbtValue], keys: Sequence[str]
) -> dict[str, NbtValue]:
    current = root
    for key in keys:
        child = current.get(key)
        if not isinstance(child, dict):
            raise QualificationError(f"NBT path {'/'.join(keys)} missing at {key}")
        current = child
    return current


def persisted_clock(run_dir: Path) -> int:
    path = run_dir / "world" / "data" / "minecraft" / "world_clocks.dat"
    root = read_gzip_nbt(path)
    overworld = nested_mapping(root, ("data", "minecraft:overworld"))
    value = overworld.get("total_ticks")
    if not isinstance(value, int):
        raise QualificationError("world_clocks.dat lacks overworld total_ticks")
    return value


def chest_absent_from_chunk(run_dir: Path, x: int, y: int, z: int) -> tuple[bool, Path]:
    root = read_region_chunk(find_region(run_dir, x // 16, z // 16), x // 16, z // 16)
    block_entities = root.get("block_entities")
    if block_entities is None and isinstance(root.get("Level"), dict):
        block_entities = cast(dict[str, NbtValue], root["Level"]).get("TileEntities")
    if not isinstance(block_entities, list):
        raise QualificationError("chunk NBT has no block entity list")
    for value in block_entities:
        if not isinstance(value, dict):
            continue
        if value.get("x") == x and value.get("y") == y and value.get("z") == z:
            return False, find_region(run_dir, x // 16, z // 16)
    return True, find_region(run_dir, x // 16, z // 16)


def _decode_palette_entry(value: NbtValue) -> StoredBlockState:
    if isinstance(value, str):
        return StoredBlockState(value, {})
    if not isinstance(value, dict):
        raise QualificationError(
            "block-state palette entry is not a string or compound"
        )

    # Older Pumpkin saves use Name/Properties. Java 26.3 and current Pumpkin use
    # id/properties when any entry has properties, with an empty-name key for
    # property-free entries in that compound palette.
    raw_name = value.get("Name", value.get("id", value.get("")))
    if not isinstance(raw_name, str):
        raise QualificationError("block-state palette entry has no block identifier")
    raw_properties = value.get("Properties", value.get("properties", {}))
    if not isinstance(raw_properties, dict):
        raise QualificationError("block-state palette Properties is not a compound")
    properties: dict[str, str] = {}
    for key, item in raw_properties.items():
        if not isinstance(item, str):
            raise QualificationError(f"block-state property {key!r} is not a string")
        properties[key] = item
    return StoredBlockState(raw_name, properties)


def block_state_from_chunk(
    root: dict[str, NbtValue], x: int, y: int, z: int
) -> StoredBlockState:
    sections = root.get("sections")
    if not isinstance(sections, list):
        raise QualificationError("chunk NBT has no sections list")
    section_y = y // 16
    section: dict[str, NbtValue] | None = None
    for candidate in sections:
        if isinstance(candidate, dict) and candidate.get("Y") == section_y:
            section = candidate
            break
    if section is None:
        raise QualificationError(f"chunk NBT has no section Y={section_y}")
    block_states = section.get("block_states")
    if not isinstance(block_states, dict):
        raise QualificationError(f"section Y={section_y} has no block_states compound")
    palette = block_states.get("palette")
    if not isinstance(palette, list) or not palette:
        raise QualificationError(f"section Y={section_y} has no block-state palette")

    palette_index = 0
    if len(palette) > 1:
        data = block_states.get("data")
        if not isinstance(data, list) or not all(
            isinstance(word, int) for word in data
        ):
            raise QualificationError(
                "heterogeneous block-state palette has no long data"
            )
        bits_per_entry = max(4, (len(palette) - 1).bit_length())
        entries_per_word = 64 // bits_per_entry
        flattened = (y % 16) * 256 + (z % 16) * 16 + (x % 16)
        word_index, entry_index = divmod(flattened, entries_per_word)
        if word_index >= len(data):
            raise QualificationError(
                "block-state packed data is shorter than the section"
            )
        word = cast(int, data[word_index]) & 0xFFFFFFFFFFFFFFFF
        palette_index = (word >> (entry_index * bits_per_entry)) & (
            (1 << bits_per_entry) - 1
        )
        if palette_index >= len(palette):
            raise QualificationError(
                f"block-state palette index {palette_index} is out of range"
            )
    return _decode_palette_entry(palette[palette_index])


def read_block_state(
    run_dir: Path, x: int, y: int, z: int
) -> tuple[StoredBlockState, Path]:
    region = find_region(run_dir, x // 16, z // 16)
    root = read_region_chunk(region, x // 16, z // 16)
    return block_state_from_chunk(root, x, y, z), region


def free_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return cast(int, sock.getsockname()[1])


def write_datapack(run_dir: Path) -> None:
    root = run_dir / "world" / "datapacks" / "ironore_e2e"
    pack = {
        "pack": {
            "description": "Ironore disposable qualification fixtures",
            "pack_format": 121,
            "min_format": 121,
            "max_format": 121,
        }
    }
    root.mkdir(parents=True)
    (root / "pack.mcmeta").write_text(json.dumps(pack), encoding="utf-8")
    for fixture in FUNCTION_BLOCK_FIXTURES:
        namespace, function_name = fixture.function_id.split(":", 1)
        function_dir = root / "data" / namespace / "function"
        function_dir.mkdir(parents=True, exist_ok=True)
        x, y, z = fixture.position
        (function_dir / f"{function_name}.mcfunction").write_text(
            f"setblock {x} {y} {z} {fixture.block_name}\n",
            encoding="utf-8",
        )


def write_pumpkin_config(run_dir: Path, port: int) -> None:
    config = f"""seed = "ironore-qualification"
default_difficulty = "Peaceful"
default_gamemode = "Creative"
default_level_name = "world"
force_gamemode = true
allow_nether = false
allow_end = false
allow_chat_reports = false
white_list = false
enforce_whitelist = false
accepts_transfers = false
spawn_protection = 0
use_favicon = false
scrub_ips = true

[networking.java]
enabled = true
address = "127.0.0.1:{port}"
online_mode = false
encryption = false
max_players = 1
view_distance = 3
simulation_distance = 3
keep_alive_time = 15
motd = "ironore-qualification"

[networking.java.compression]
enabled = false

[networking.java.authentication]
enabled = false

[networking.bedrock]
enabled = false
online_mode = false

[networking.query]
enabled = false

[networking.rcon]
enabled = false

[networking.lan_broadcast]
enabled = false

[telemetry]
enabled = false
public = false
endpoint = "http://127.0.0.1:9/disabled"

[plugins]
enabled = false
hot_reload = false
ask_permission_confirmation = false
allow_unsigned = false
inherit_env = false
loopback_only = true

[commands]
use_console = true
use_tty = false
log_console = true
broadcast_console_to_ops = false
default_op_level = 0

[logging]
enabled = true
level = "info"
color = false
file = ""

[world]
autosave_ticks = 1200

[world.chunk]
type = "anvil"
write_in_place = false

[world.chunk.compression]
algorithm = "ZLib"
level = 6
"""
    (run_dir / "pumpkin.toml").write_text(config, encoding="utf-8")


def write_vanilla_config(run_dir: Path, port: int, eula_source: Path) -> None:
    eula = eula_source.read_text(encoding="utf-8")
    if not any(line.strip().lower() == "eula=true" for line in eula.splitlines()):
        raise QualificationError(f"accepted EULA source is invalid: {eula_source}")
    shutil.copyfile(eula_source, run_dir / "eula.txt")
    flat = json.dumps(
        {
            "layers": [
                {"block": "minecraft:bedrock", "height": 1},
                {"block": "minecraft:dirt", "height": 2},
                {"block": "minecraft:grass_block", "height": 1},
            ],
            "biome": "minecraft:plains",
            "features": False,
            "lakes": False,
        },
        separators=(",", ":"),
    )
    properties = {
        "allow-flight": "true",
        "enable-command-block": "false",
        "enable-query": "false",
        "enable-rcon": "false",
        "enforce-whitelist": "false",
        "enforce-secure-profile": "false",
        "force-gamemode": "true",
        "gamemode": "creative",
        "generate-structures": "false",
        "generator-settings": flat,
        "hardcore": "false",
        "level-name": "world",
        "level-seed": "ironore-qualification",
        "level-type": "minecraft:flat",
        "max-players": "1",
        "motd": "ironore-qualification",
        "network-compression-threshold": "-1",
        "online-mode": "false",
        "pause-when-empty-seconds": "-1",
        "prevent-proxy-connections": "false",
        "server-ip": "127.0.0.1",
        "server-port": str(port),
        "simulation-distance": "3",
        "spawn-protection": "0",
        "sync-chunk-writes": "false",
        "view-distance": "3",
        "white-list": "false",
    }
    text = "".join(f"{key}={value}\n" for key, value in properties.items())
    (run_dir / "server.properties").write_text(text, encoding="utf-8")


def parse_position(text: str) -> tuple[float, float, float] | None:
    import re

    position = re.search(r'(?<![\w])"?Pos"?\s*:\s*', text)
    if position is not None:
        text = text[position.end() :]
    elif "{" in text:
        return None
    match = re.search(
        r"\[\s*(-?\d+(?:\.\d+)?)[dDfF]?,\s*"
        r"(-?\d+(?:\.\d+)?)[dDfF]?,\s*"
        r"(-?\d+(?:\.\d+)?)[dDfF]?\s*\]",
        text,
    )
    if match is None:
        return None
    return (
        float(match.group(1)),
        float(match.group(2)),
        float(match.group(3)),
    )


def parse_nbt_bool(text: str, name: str) -> bool | None:
    import re

    match = re.search(
        rf'(?<![\w])"?{re.escape(name)}"?\s*:\s*(1b|0b|true|false)\b',
        text,
        flags=re.IGNORECASE,
    )
    if match is None:
        return None
    return match.group(1).lower() in ("1b", "true")


def parse_query_bool(result: CommandResult, name: str) -> bool | None:
    import re

    named = parse_nbt_bool(result.text, name)
    if named is not None:
        return named
    scalar = re.search(
        r"entity data:\s*(1b|0b|true|false)\s*$",
        result.text,
        flags=re.IGNORECASE,
    )
    if scalar is None:
        return None
    return scalar.group(1).lower() in ("1b", "true")


def entity_data_command(
    backend: Literal["pumpkin", "vanilla"], target: str, path: str
) -> str:
    return f"data get entity {target} {path}"


def datapack_enable_command(
    backend: Literal["pumpkin", "vanilla"],
) -> str | None:
    if backend == "pumpkin":
        return 'datapack enable "file/ironore_e2e"'
    return None


def block_placement_succeeded(result: CommandResult, x: int, y: int, z: int) -> bool:
    marker = f"Changed the block at {x}, {y}, {z}"
    return not result.looks_unsupported and (
        marker in result.text
        or any(line.strip() == "Block placed" for line in result.lines)
    )


def close_position(
    actual: tuple[float, float, float],
    expected: tuple[float, float, float],
    tolerance: float = 0.05,
) -> bool:
    return all(abs(left - right) <= tolerance for left, right in zip(actual, expected))


def command_check(
    results: Results,
    name: str,
    command_result: CommandResult,
    success_marker: str,
) -> None:
    if command_result.looks_unsupported:
        results.set(
            name,
            "unsupported",
            f"backend rejected {command_result.command!r}",
            output=command_result.text[-2000:],
        )
    elif success_marker in command_result.text:
        results.set(
            name, "pass", f"{command_result.command!r} emitted its fixture marker"
        )
    else:
        results.set(
            name,
            "fail",
            f"{command_result.command!r} completed without its success marker",
            output=command_result.text[-2000:],
        )


class Qualification:
    def __init__(self, args: argparse.Namespace) -> None:
        self.backend = cast(Literal["pumpkin", "vanilla"], args.backend)
        self.executable = cast(Path | None, args.executable)
        self.vanilla_jar = cast(Path, args.vanilla_jar)
        self.eula_source = cast(Path | None, args.eula_source)
        self.run_dir = cast(Path, args.run_dir)
        self.deadline = Deadline(cast(float, args.timeout))
        self.results = Results()
        self.started = datetime.now(UTC)
        self.port = free_loopback_port()
        self.server: ServerProcess | None = None
        self.bot: JavaBot | None = None
        self.repo = Path(__file__).resolve().parents[2]
        self.packet_ids = load_packet_ids(self.repo)
        self.log_path = self.run_dir / "harness-server.log"
        self.created_run_dir = False
        self.function_probes: dict[str, CommandResult] = {}
        self.setblock_probe: tuple[CommandResult, CommandResult] | None = None
        self.fill_probe: tuple[CommandResult, CommandResult] | None = None

    def run(self) -> int:
        if self.run_dir.exists():
            raise QualificationError(f"--run-dir must be new: {self.run_dir}")
        self.run_dir.mkdir(parents=True)
        self.created_run_dir = True
        write_datapack(self.run_dir)
        if self.backend == "pumpkin":
            if self.executable is None:
                raise QualificationError("--executable is required for Pumpkin")
            if not self.executable.is_file():
                raise QualificationError(
                    f"Pumpkin executable not found: {self.executable}"
                )
            write_pumpkin_config(self.run_dir, self.port)
        else:
            if not self.vanilla_jar.is_file():
                raise QualificationError(f"vanilla jar not found: {self.vanilla_jar}")
            if self.eula_source is None:
                raise QualificationError(
                    "--eula-source must name an already-accepted eula.txt"
                )
            write_vanilla_config(self.run_dir, self.port, self.eula_source)

        try:
            self._first_run()
            self._offline_checks()
            self._restart_run()
        except (OSError, QualificationError, ValueError) as error:
            self.results.finish_unrun(f"prerequisite failed: {error}")
        finally:
            self._cleanup_owned()
        return self._write_report()

    def _server_command(self) -> tuple[str, ...]:
        if self.backend == "pumpkin":
            if self.executable is None:
                raise QualificationError("missing Pumpkin executable")
            return (str(self.executable),)
        java = str(self.executable) if self.executable is not None else "java"
        return (
            java,
            "-Xms256M",
            "-Xmx768M",
            "-jar",
            str(self.vanilla_jar),
            "nogui",
        )

    def _start_server(self) -> None:
        readiness = (
            "Server is now running. Connect using port:"
            if self.backend == "pumpkin"
            else "Done ("
        )
        self.server = ServerProcess(
            self._server_command(), self.run_dir, self.log_path, self.deadline
        )
        self.server.start(readiness)

    def _connect_bot(self) -> Teleport:
        self.bot = JavaBot(self.port, self.packet_ids, self.deadline)
        self.bot.connect()
        return self.bot.wait_ready()

    def _first_run(self) -> None:
        try:
            self._start_server()
            self.results.set(
                "server_start",
                "pass",
                "server emitted its readiness marker",
                port=self.port,
            )
        except QualificationError as error:
            self.results.set("server_start", "fail", str(error))
            raise

        assert self.server is not None
        datapack_command = datapack_enable_command(self.backend)
        if datapack_command is not None:
            datapack_result = self.server.run_command(datapack_command)
            if (
                datapack_result.looks_unsupported
                or "ironore_e2e" not in datapack_result.text
            ):
                raise QualificationError(
                    "failed to enable world/datapacks/ironore_e2e: "
                    + datapack_result.text[-2000:]
                )

        try:
            initial = self._connect_bot()
            self.server.wait_for_text(
                f"{BOT_NAME} joined the game",
                "player join completion",
                maximum=20.0,
            )
            self._prepare_fixture_chunk()
            self.results.set(
                "protocol_login",
                "pass",
                "protocol 777 login completed and the fixture chunk became writable",
                initial_position=[initial.x, initial.y, initial.z],
            )
        except QualificationError as error:
            self.results.set("protocol_login", "fail", str(error))
            raise

        assert self.server is not None
        assert self.bot is not None
        target_old = (12.5, 100.0, 12.5)
        target_new = (20.5, 100.0, 20.5)
        self.bot.hold_next_teleports(2)
        self.server.flush_commands(
            (
                f"tp {BOT_NAME} {target_old[0]} {target_old[1]} {target_old[2]}",
                f"tp {BOT_NAME} {target_new[0]} {target_new[1]} {target_new[2]}",
            )
        )
        try:
            held = self.bot.wait_held()
            acknowledged = self.bot.acknowledge_held_in_order()
            if len(held) != 2 or tuple(item.teleport_id for item in held) != tuple(
                item.teleport_id for item in acknowledged
            ):
                raise QualificationError(
                    "teleport acknowledgement order was not preserved"
                )
            self.bot.assert_connected_for(0.5)
            self.results.set(
                "teleport_queue_ack_order",
                "pass",
                "received both queued teleports before acknowledging old then new",
                teleport_ids=[item.teleport_id for item in held],
                positions=[[item.x, item.y, item.z] for item in held],
            )
        except (OSError, QualificationError) as error:
            self.results.set("teleport_queue_ack_order", "fail", str(error))
            try:
                self.bot.release_held()
            except (OSError, QualificationError):
                pass

        position_result = self.server.run_command(
            entity_data_command(self.backend, BOT_NAME, "Pos")
        )
        try:
            self.bot.assert_connected()
        except QualificationError as error:
            self.results.set(
                "teleport_queue_ack_order",
                "fail",
                f"bot disconnected after queued acknowledgements: {error}",
            )
        position = parse_position(position_result.text)
        if position is not None and close_position(position, target_new):
            self.results.set(
                "teleport_latest_server_position",
                "pass",
                "server retained the newest queued teleport position",
                position=list(position),
            )
        elif position_result.looks_unsupported:
            self.results.set(
                "teleport_latest_server_position",
                "unsupported",
                "backend does not support the entity data query",
                output=position_result.text[-2000:],
            )
        else:
            self.results.set(
                "teleport_latest_server_position",
                "fail",
                "server data did not report the newest queued teleport",
                expected=list(target_new),
                output=position_result.text[-2000:],
            )

        for fixture in FUNCTION_BLOCK_FIXTURES:
            command_name = fixture.function_id.removeprefix("minecraft:")
            self.function_probes[fixture.check_name] = self.server.run_command(
                f"function {command_name}"
            )

        setblock_command = self.server.run_command(
            "setblock 21 100 21 minecraft:oak_stairs[facing=east,half=top]"
        )
        setblock_marker = self.server.run_command(
            "execute if block 21 100 21 minecraft:oak_stairs run help setblock"
        )
        self.setblock_probe = (setblock_command, setblock_marker)
        fill_command = self.server.run_command(
            "fill 22 100 21 23 100 22 minecraft:oak_stairs[facing=west,half=bottom]"
        )
        fill_marker = self.server.run_command(
            "execute if block 23 100 22 minecraft:oak_stairs run help fill"
        )
        self.fill_probe = (fill_command, fill_marker)

        self._bat_first_run()

        clock_command = (
            "gamerule advance_time false"
            if self.backend == "pumpkin"
            else "gamerule minecraft:advance_time false"
        )
        clock_rule = self.server.run_command(clock_command)
        clock_set = self.server.run_command("time set 6000")
        clock_query_command = (
            "time query daytime" if self.backend == "pumpkin" else "time query time"
        )
        clock_query = self.server.run_command(clock_query_command)
        if any(
            result.looks_unsupported for result in (clock_rule, clock_set, clock_query)
        ):
            self.results.set(
                "clock_configured",
                "unsupported",
                "backend rejected the frozen-clock fixture",
                output=(
                    clock_rule.text + "\n" + clock_set.text + "\n" + clock_query.text
                )[-2000:],
            )
        elif "false" in clock_rule.text.lower() and "6000" in clock_query.text:
            self.results.set(
                "clock_configured",
                "pass",
                "disabled clock advancement and queried overworld daytime 6000",
            )
        else:
            self.results.set(
                "clock_configured",
                "fail",
                "frozen-clock commands completed without confirming both values",
                output=(
                    clock_rule.text + "\n" + clock_set.text + "\n" + clock_query.text
                )[-2000:],
            )

        self.server.run_command("setblock 20 99 20 minecraft:gold_block")
        block_before = self.server.run_command(
            "execute if block 20 99 20 minecraft:gold_block run help clone"
        )
        command_check(self.results, "block_configured", block_before, "/clone")

        chest_set = self.server.run_command("setblock 24 100 24 minecraft:chest")
        chest_data_query = self.server.run_command("data get block 24 100 24")
        chest_remove = self.server.run_command("setblock 24 100 24 minecraft:air")
        chest_absent = self.server.run_command(
            "execute if block 24 100 24 minecraft:air run help data"
        )
        if any(
            result.looks_unsupported
            for result in (chest_set, chest_data_query, chest_remove)
        ):
            self.results.set(
                "chest_removed_before_save",
                "unsupported",
                "backend rejected a chest/NBT fixture command",
                output=(chest_set.text + chest_data_query.text + chest_remove.text)[
                    -2000:
                ],
            )
        elif "minecraft:chest" not in chest_data_query.text.lower():
            self.results.set(
                "chest_removed_before_save",
                "fail",
                "data get block did not identify a chest block entity before removal",
                output=chest_data_query.text[-2000:],
            )
        else:
            command_check(
                self.results,
                "chest_removed_before_save",
                chest_absent,
                "/data",
            )

        clean = self.server.stop()
        self.results.set(
            "clean_first_stop",
            "pass" if clean else "fail",
            "server exited cleanly after stop"
            if clean
            else "server required termination",
            returncode=self.server.returncode,
        )
        self.bot.close()
        self.bot = None
        self.server = None

    def _prepare_fixture_chunk(self) -> None:
        assert self.server is not None
        forced = self.server.run_command("forceload add 16 16")
        if forced.looks_unsupported:
            raise QualificationError(
                "backend rejected fixture forceload: " + forced.text[-2000:]
            )

        x, y, z = 20, 98, 20
        end = time.monotonic() + self.deadline.slice("fixture chunk readiness", 15.0)
        last = CommandResult("setblock 20 98 20 minecraft:stone", ())
        while True:
            last = self.server.run_command(last.command)
            if block_placement_succeeded(last, x, y, z):
                cleanup = self.server.run_command(f"setblock {x} {y} {z} minecraft:air")
                if not block_placement_succeeded(cleanup, x, y, z):
                    raise QualificationError(
                        "fixture readiness probe cleanup failed: "
                        + cleanup.text[-2000:]
                    )
                return
            if last.looks_unsupported:
                raise QualificationError(
                    "fixture readiness probe was rejected: " + last.text[-2000:]
                )
            remaining = end - time.monotonic()
            if remaining <= 0:
                raise QualificationError(
                    "fixture chunk did not become writable: " + last.text[-2000:]
                )
            time.sleep(min(0.1, remaining))

    def _bat_first_run(self) -> None:
        assert self.server is not None
        summon = self.server.run_command(
            'summon minecraft:bat 25 100 25 {Tags:["ironore_bat"],NoAI:1b,'
            "NoGravity:1b,PersistenceRequired:1b}"
        )
        first_pos_result = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "Pos",
            )
        )
        first_no_ai = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "NoAI",
            )
        )
        first_no_gravity = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "NoGravity",
            )
        )
        if any(
            result.looks_unsupported
            for result in (summon, first_pos_result, first_no_ai, first_no_gravity)
        ):
            self.results.set(
                "bat_flags_and_position",
                "unsupported",
                "backend rejected the bat summon or entity data query",
                output=(
                    summon.text
                    + first_pos_result.text
                    + first_no_ai.text
                    + first_no_gravity.text
                )[-3000:],
            )
            return
        before = parse_position(first_pos_result.text)
        time.sleep(min(1.0, self.deadline.slice("bat tick observation", 1.0)))
        after_result = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "Pos",
            )
        )
        after = parse_position(after_result.text)
        flags = first_no_ai.text + "\n" + first_no_gravity.text
        flags_true = (
            parse_query_bool(first_no_ai, "NoAI") is True
            and parse_query_bool(first_no_gravity, "NoGravity") is True
        )
        if (
            before is not None
            and after is not None
            and close_position(after, before, 0.1)
            and flags_true
        ):
            self.results.set(
                "bat_flags_and_position",
                "pass",
                "tagged bat retained NoAI/NoGravity and remained stationary across ticks",
                before=list(before),
                after=list(after),
            )
        else:
            self.results.set(
                "bat_flags_and_position",
                "fail",
                "bat flags or position did not match the fixture",
                before=None if before is None else list(before),
                after=None if after is None else list(after),
                flag_output=flags[-2000:],
            )

    def _offline_checks(self) -> None:
        self._offline_function_checks()
        self._offline_block_state_checks()
        try:
            clock = persisted_clock(self.run_dir)
            self.results.set(
                "clock_nbt_persisted",
                "pass" if clock == 6000 else "fail",
                f"world_clocks.dat overworld total_ticks is {clock}",
                total_ticks=clock,
            )
        except (OSError, QualificationError) as error:
            self.results.set("clock_nbt_persisted", "fail", str(error))

        try:
            absent, region = chest_absent_from_chunk(self.run_dir, 24, 100, 24)
            self.results.set(
                "chest_nbt_removed",
                "pass" if absent else "fail",
                "removed chest has no saved block entity"
                if absent
                else "removed chest block entity remains in chunk NBT",
                region=str(region),
            )
        except UnsupportedNbt as error:
            self.results.set("chest_nbt_removed", "unsupported", str(error))
        except (OSError, QualificationError) as error:
            self.results.set("chest_nbt_removed", "fail", str(error))

    def _offline_function_checks(self) -> None:
        for fixture in FUNCTION_BLOCK_FIXTURES:
            command_result = self.function_probes.get(fixture.check_name)
            if command_result is None:
                self.results.set(
                    fixture.check_name,
                    "unrun",
                    "function fixture command did not run",
                )
                continue
            try:
                state, region = read_block_state(self.run_dir, *fixture.position)
                evidence: dict[str, object] = {
                    "position": list(fixture.position),
                    "stored_name": state.name,
                    "stored_properties": state.properties,
                    "expected_name": fixture.block_name,
                    "region": str(region),
                    "command_output": command_result.text[-2000:],
                }
                if state.name == fixture.block_name:
                    self.results.set(
                        fixture.check_name,
                        "pass",
                        "stopped-world Anvil state proves the function body ran",
                        **evidence,
                    )
                elif command_result.looks_unsupported:
                    self.results.set(
                        fixture.check_name,
                        "unsupported",
                        "backend rejected the function fixture command",
                        **evidence,
                    )
                else:
                    self.results.set(
                        fixture.check_name,
                        "fail",
                        "function body did not place its expected block",
                        **evidence,
                    )
            except UnsupportedNbt as error:
                self.results.set(fixture.check_name, "unsupported", str(error))
            except (OSError, QualificationError) as error:
                status: Status = (
                    "unsupported" if command_result.looks_unsupported else "fail"
                )
                self.results.set(
                    fixture.check_name,
                    status,
                    str(error),
                    command_output=command_result.text[-2000:],
                )

    def _offline_block_state_checks(self) -> None:
        self._offline_block_state_check(
            "setblock_properties",
            self.setblock_probe,
            ((21, 100, 21),),
            {"facing": "east", "half": "top"},
            "/setblock",
        )
        self._offline_block_state_check(
            "fill_properties",
            self.fill_probe,
            (
                (22, 100, 21),
                (23, 100, 21),
                (22, 100, 22),
                (23, 100, 22),
            ),
            {"facing": "west", "half": "bottom"},
            "/fill",
        )

    def _offline_block_state_check(
        self,
        name: str,
        probe: tuple[CommandResult, CommandResult] | None,
        positions: Sequence[tuple[int, int, int]],
        expected_properties: dict[str, str],
        marker: str,
    ) -> None:
        if probe is None:
            self.results.set(name, "unrun", "block-state command fixture did not run")
            return
        setup_result, marker_result = probe
        marker_seen = marker in marker_result.text
        try:
            stored: list[tuple[tuple[int, int, int], StoredBlockState, Path]] = []
            for position in positions:
                state, region = read_block_state(self.run_dir, *position)
                stored.append((position, state, region))
            matches = all(
                state.name == "minecraft:oak_stairs"
                and all(
                    state.properties.get(key) == value
                    for key, value in expected_properties.items()
                )
                for _, state, _ in stored
            )
            evidence: dict[str, object] = {
                "predicate_marker_seen": marker_seen,
                "stored_states": [
                    {
                        "position": list(position),
                        "name": state.name,
                        "properties": state.properties,
                        "region": str(region),
                    }
                    for position, state, region in stored
                ],
            }
            if matches:
                self.results.set(
                    name,
                    "pass",
                    "stopped-world Anvil palette stores the requested stair properties",
                    **evidence,
                )
            elif setup_result.looks_unsupported:
                self.results.set(
                    name,
                    "unsupported",
                    "backend rejected the property-bearing block command",
                    command_output=setup_result.text[-2000:],
                    **evidence,
                )
            else:
                self.results.set(
                    name,
                    "fail",
                    "stopped-world Anvil palette does not store the requested stair properties",
                    **evidence,
                )
        except UnsupportedNbt as error:
            self.results.set(
                name,
                "unsupported",
                str(error),
                predicate_marker_seen=marker_seen,
            )
        except (OSError, QualificationError) as error:
            status: Status = "unsupported" if setup_result.looks_unsupported else "fail"
            self.results.set(
                name,
                status,
                str(error),
                predicate_marker_seen=marker_seen,
                command_output=setup_result.text[-2000:],
            )

    def _restart_run(self) -> None:
        try:
            self._start_server()
            self.results.set(
                "restart_server", "pass", "server became ready after restart"
            )
        except QualificationError as error:
            self.results.set("restart_server", "fail", str(error))
            raise
        try:
            initial = self._connect_bot()
            self.results.set(
                "restart_protocol_login",
                "pass",
                "protocol client rejoined after restart",
                initial_position=[initial.x, initial.y, initial.z],
            )
        except QualificationError as error:
            self.results.set("restart_protocol_login", "fail", str(error))
            raise

        assert self.server is not None
        assert self.bot is not None
        query = "time query time" if self.backend == "vanilla" else "time query daytime"
        clock_result = self.server.run_command(query)
        if "6000" in clock_result.text:
            self.results.set(
                "clock_restart_query",
                "pass",
                "restarted server reported overworld clock 6000",
            )
        elif clock_result.looks_unsupported:
            self.results.set(
                "clock_restart_query",
                "unsupported",
                f"backend rejected {query!r}; disk NBT result remains authoritative",
                output=clock_result.text[-2000:],
            )
        else:
            self.results.set(
                "clock_restart_query",
                "fail",
                "restart clock query did not report 6000",
                output=clock_result.text[-2000:],
            )

        command_check(
            self.results,
            "block_restart_query",
            self.server.run_command(
                "execute if block 20 99 20 minecraft:gold_block run help fill"
            ),
            "/fill",
        )

        player_result = self.server.run_command(
            entity_data_command(self.backend, BOT_NAME, "Pos")
        )
        player_position = parse_position(player_result.text)
        expected = (20.5, 100.0, 20.5)
        if player_position is not None and close_position(player_position, expected):
            self.results.set(
                "player_position_persisted",
                "pass",
                "player position survived stop and restart",
                position=list(player_position),
            )
        elif player_result.looks_unsupported:
            self.results.set(
                "player_position_persisted",
                "unsupported",
                "backend rejected the player entity data query",
                output=player_result.text[-2000:],
            )
        else:
            self.results.set(
                "player_position_persisted",
                "fail",
                "player position did not survive stop and restart",
                output=player_result.text[-2000:],
            )

        bat_pos = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "Pos",
            )
        )
        bat_no_ai = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "NoAI",
            )
        )
        bat_no_gravity = self.server.run_command(
            entity_data_command(
                self.backend,
                "@e[type=minecraft:bat,tag=ironore_bat,limit=1]",
                "NoGravity",
            )
        )
        combined = bat_pos.text + "\n" + bat_no_ai.text + "\n" + bat_no_gravity.text
        if any(
            result.looks_unsupported for result in (bat_pos, bat_no_ai, bat_no_gravity)
        ):
            self.results.set(
                "bat_persistence",
                "unsupported",
                "backend rejected a persisted bat data query",
                output=combined[-3000:],
            )
        else:
            position = parse_position(bat_pos.text)
            flags_true = (
                parse_query_bool(bat_no_ai, "NoAI") is True
                and parse_query_bool(bat_no_gravity, "NoGravity") is True
            )
            if position is not None and flags_true:
                self.results.set(
                    "bat_persistence",
                    "pass",
                    "tagged bat and both flags survived restart",
                    position=list(position),
                )
            else:
                self.results.set(
                    "bat_persistence",
                    "fail",
                    "tagged bat or NoAI/NoGravity did not survive restart",
                    output=combined[-3000:],
                )

        clean = self.server.stop()
        self.results.set(
            "clean_final_stop",
            "pass" if clean else "fail",
            "server exited cleanly after final stop"
            if clean
            else "server required termination after final stop",
            returncode=self.server.returncode,
        )
        self.bot.close()
        self.bot = None
        self.server = None

    def _cleanup_owned(self) -> None:
        bot = self.bot
        if bot is not None:
            bot.close()
            self.bot = None
        server = self.server
        if server is not None:
            server.kill_owned()
            self.server = None

    def _write_report(self) -> int:
        self.results.finish_unrun("qualification did not reach this check")
        statuses = self.results.statuses()
        if "fail" in statuses:
            overall = "fail"
            exit_code = 1
        elif "unsupported" in statuses or "unrun" in statuses:
            overall = "partial"
            exit_code = 2
        else:
            overall = "pass"
            exit_code = 0
        report: dict[str, object] = {
            "schema": 1,
            "backend": self.backend,
            "minecraft_version": EXPECTED_VERSION_NAME,
            "protocol_version": PROTOCOL_VERSION,
            "run_dir": str(self.run_dir),
            "started_at": self.started.isoformat(),
            "finished_at": datetime.now(UTC).isoformat(),
            "overall": overall,
            "checks": self.results.as_list(),
            "artifacts": {
                "server_log": str(self.log_path),
                "report": str(self.run_dir / "qualification.json"),
            },
        }
        report_path = self.run_dir / "qualification.json"
        report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, indent=2))
        return exit_code


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--backend", choices=("pumpkin", "vanilla"), required=True)
    result.add_argument(
        "--executable",
        type=Path,
        help="Pumpkin binary, or an optional Java executable for vanilla",
    )
    result.add_argument(
        "--vanilla-jar",
        type=Path,
        default=DEFAULT_VANILLA_JAR,
        help=f"official 26.3 server jar (default: {DEFAULT_VANILLA_JAR})",
    )
    result.add_argument(
        "--eula-source",
        type=Path,
        help="already-accepted eula.txt to copy (required for vanilla)",
    )
    result.add_argument(
        "--run-dir", type=Path, required=True, help="new disposable directory"
    )
    result.add_argument(
        "--timeout",
        type=float,
        default=240.0,
        help="overall timeout in seconds, covering both starts (default: 240)",
    )
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    run_dir = cast(Path, args.run_dir).expanduser().resolve()
    args.run_dir = run_dir
    if args.executable is not None:
        args.executable = cast(Path, args.executable).expanduser().resolve()
    args.vanilla_jar = cast(Path, args.vanilla_jar).expanduser().resolve()
    if args.eula_source is not None:
        args.eula_source = cast(Path, args.eula_source).expanduser().resolve()
    qualification: Qualification | None = None
    try:
        qualification = Qualification(args)
        return qualification.run()
    except (OSError, QualificationError, ValueError) as error:
        if qualification is not None and qualification.created_run_dir:
            qualification.results.finish_unrun(f"harness setup failed: {error}")
            qualification._cleanup_owned()
            return qualification._write_report()
        print(f"ironore qualification: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
