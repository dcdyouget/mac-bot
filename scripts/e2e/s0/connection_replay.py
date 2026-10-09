#!/usr/bin/env python3
"""S0 main-connection replay check using only Python's standard library.

The scenario creates one uniquely named user skill, disconnects the main
WebSocket, updates that skill through HTTP while the connection is absent, and
reconnects with the same device id and event cursor.  PASS requires a replay
mode response and a persisted ``skill.updated`` event with a sequence greater
than the saved cursor.  It never treats bootstrap/reset as replay evidence.

This is an API and transport check.  It does not replace desktop/Android UI
connection, reconnect, or event rendering evidence.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import struct
import sys
import time
import uuid
from typing import Any
from urllib.parse import urlsplit

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    client_from_args,
    ready_health,
    require_dict,
    run_main,
    unique_marker,
)
from rpc import RpcClient  # noqa: E402


class WsError(ValueError):
    """A protocol or transport failure safe to report through run_main."""


class MiniWebSocket:
    """Small RFC 6455 client for the text-only ``/ws`` protocol channel."""

    MAX_FRAME = 16 * 1024 * 1024
    MAX_HANDSHAKE = 64 * 1024

    def __init__(self, url: str, password: str | None, *, timeout: float) -> None:
        parsed = self._parse_url(url)
        if password is None or not password:
            raise WsError("WebSocket password is required")
        if "\r" in password or "\n" in password:
            raise WsError("WebSocket password contains invalid header characters")
        self._timeout = timeout
        self._sock: socket.socket | ssl.SSLSocket | None = None
        self._receive_buffer = bytearray()
        self._connect(parsed, password)

    @staticmethod
    def _parse_url(value: str) -> tuple[str, str, int, str]:
        raw = value.strip()
        if "://" not in raw:
            raw = "http://" + raw
        parsed = urlsplit(raw)
        if parsed.scheme not in {"http", "https", "ws", "wss"} or not parsed.hostname:
            raise WsError("unsupported WebSocket URL")
        if parsed.username is not None or parsed.password is not None:
            raise WsError("WebSocket URL must not contain userinfo")
        if parsed.query or parsed.fragment:
            raise WsError("WebSocket URL must not contain a query or fragment")
        secure = parsed.scheme in {"https", "wss"}
        scheme = "wss" if secure else "ws"
        try:
            port = parsed.port or (443 if secure else 80)
        except ValueError as exc:
            raise WsError("WebSocket URL has an invalid port") from exc
        path = parsed.path.rstrip("/")
        if path in {"", "/ws"}:
            path = "/ws"
        else:
            raise WsError("main WebSocket URL must use /ws")
        return scheme, parsed.hostname, port, path

    def _connect(self, parsed: tuple[str, str, int, str], password: str) -> None:
        scheme, host, port, path = parsed
        try:
            sock = socket.create_connection((host, port), timeout=self._timeout)
            if scheme == "wss":
                context = ssl.create_default_context()
                sock = context.wrap_socket(sock, server_hostname=host)
            sock.settimeout(self._timeout)
            self._sock = sock
            self._handshake(host, port, path, password)
        except WsError:
            self.close()
            raise
        except (OSError, ssl.SSLError, socket.timeout) as exc:
            self.close()
            raise WsError(f"WebSocket connection failed: {type(exc).__name__}") from exc

    def _handshake(self, host: str, port: int, path: str, password: str) -> None:
        if self._sock is None:
            raise WsError("WebSocket socket is not open")
        key = base64.b64encode(secrets.token_bytes(16)).decode("ascii")
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")).digest()).decode("ascii")
        host_header = host
        if ":" in host and not host.startswith("["):
            host_header = f"[{host}]"
        if port not in {80, 443}:
            host_header = f"{host_header}:{port}"
        request = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {host_header}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            f"Authorization: Bearer {password}\r\n\r\n"
        ).encode("ascii")
        self._sock.sendall(request)
        raw = self._read_until(b"\r\n\r\n", self.MAX_HANDSHAKE)
        header_bytes, frame_bytes = raw.split(b"\r\n\r\n", 1)
        self._receive_buffer.extend(frame_bytes)
        head = header_bytes.split(b"\r\n")
        if not head or not head[0].startswith(b"HTTP/1.1 101"):
            status = head[0].decode("latin-1", "replace") if head else "invalid response"
            raise WsError(f"WebSocket handshake rejected: {status}")
        headers: dict[str, str] = {}
        for line in head[1:]:
            if b":" not in line:
                continue
            name, value = line.split(b":", 1)
            headers[name.decode("latin-1").strip().lower()] = value.decode("latin-1").strip()
        if headers.get("upgrade", "").lower() != "websocket":
            raise WsError("WebSocket handshake omitted Upgrade: websocket")
        if headers.get("sec-websocket-accept") != expected:
            raise WsError("WebSocket handshake has an invalid Sec-WebSocket-Accept")

    def _read_until(self, marker: bytes, maximum: int) -> bytes:
        if self._sock is None:
            raise WsError("WebSocket socket is closed")
        data = bytearray()
        while marker not in data:
            if len(data) >= maximum:
                raise WsError("WebSocket handshake headers are too large")
            try:
                chunk = self._sock.recv(min(4096, maximum - len(data)))
            except socket.timeout as exc:
                raise WsError("WebSocket handshake timed out") from exc
            if not chunk:
                raise WsError("WebSocket closed during handshake")
            data.extend(chunk)
        return bytes(data)

    def _read_exact(self, size: int) -> bytes:
        if self._sock is None:
            raise WsError("WebSocket socket is closed")
        data = bytearray(self._receive_buffer[:size])
        del self._receive_buffer[:size]
        while len(data) < size:
            try:
                chunk = self._sock.recv(size - len(data))
            except socket.timeout as exc:
                raise WsError("WebSocket receive timed out") from exc
            if not chunk:
                raise WsError("WebSocket closed while receiving a frame")
            data.extend(chunk)
        return bytes(data)

    def _recv_frame(self) -> tuple[bool, int, bytes]:
        first, second = self._read_exact(2)
        fin = bool(first & 0x80)
        rsv = first & 0x70
        opcode = first & 0x0F
        if rsv:
            raise WsError("WebSocket frame uses an unnegotiated extension")
        masked = bool(second & 0x80)
        length = second & 0x7F
        if length == 126:
            length = struct.unpack("!H", self._read_exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", self._read_exact(8))[0]
        if length > self.MAX_FRAME:
            raise WsError("WebSocket frame is too large")
        mask = self._read_exact(4) if masked else b""
        payload = bytearray(self._read_exact(length))
        if masked:
            for index in range(length):
                payload[index] ^= mask[index % 4]
        return fin, opcode, bytes(payload)

    def _send_frame(self, opcode: int, payload: bytes, *, fin: bool = True) -> None:
        if self._sock is None:
            raise WsError("WebSocket socket is closed")
        if len(payload) > self.MAX_FRAME:
            raise WsError("WebSocket frame is too large")
        first = (0x80 if fin else 0) | opcode
        length = len(payload)
        if length < 126:
            header = bytes((first, 0x80 | length))
        elif length <= 0xFFFF:
            header = bytes((first, 0x80 | 126)) + struct.pack("!H", length)
        else:
            header = bytes((first, 0x80 | 127)) + struct.pack("!Q", length)
        mask = secrets.token_bytes(4)
        masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
        try:
            self._sock.sendall(header + mask + masked)
        except (OSError, socket.timeout) as exc:
            raise WsError("WebSocket send failed") from exc

    def send_json(self, value: dict[str, Any]) -> None:
        try:
            payload = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        except (TypeError, ValueError) as exc:
            raise WsError("cannot encode WebSocket JSON frame") from exc
        self._send_frame(0x1, payload)

    def recv_json(self) -> dict[str, Any]:
        fragments: list[bytes] = []
        first_opcode: int | None = None
        while True:
            fin, opcode, payload = self._recv_frame()
            if opcode == 0x9:  # ping
                self._send_frame(0xA, payload)
                continue
            if opcode == 0xA:  # pong
                continue
            if opcode == 0x8:  # close
                if self._sock is not None:
                    try:
                        self._send_frame(0x8, payload[:125])
                    except WsError:
                        pass
                raise WsError("WebSocket peer closed the connection")
            if opcode in {0x1, 0x2}:
                if first_opcode is not None:
                    raise WsError("WebSocket started a new message before finishing a fragment")
                first_opcode = opcode
            elif opcode == 0x0:
                if first_opcode is None:
                    raise WsError("WebSocket continuation has no initial frame")
            else:
                raise WsError("WebSocket used an unsupported opcode")
            fragments.append(payload)
            if fin:
                break
        if first_opcode != 0x1:
            raise WsError("main WebSocket returned a binary message")
        try:
            value = json.loads(b"".join(fragments).decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise WsError("main WebSocket returned invalid JSON") from exc
        if not isinstance(value, dict):
            raise WsError("main WebSocket JSON frame is not an object")
        return value

    def request(self, method: str, params: dict[str, Any], *, timeout: float) -> tuple[dict[str, Any], list[dict[str, Any]]]:
        request_id = f"e2e-{uuid.uuid4()}"
        self.send_json({"v": 1, "kind": "req", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        frames: list[dict[str, Any]] = []
        while True:
            if self._sock is None:
                raise WsError("WebSocket socket is closed")
            self._sock.settimeout(max(0.01, deadline - time.monotonic()))
            frame = self.recv_json()
            frames.append(frame)
            if frame.get("kind") != "res" or frame.get("id") != request_id:
                continue
            if frame.get("ok") is not True:
                error = frame.get("error")
                code = error.get("code") if isinstance(error, dict) else "unknown"
                raise WsError(f"WebSocket RPC {method} failed: {code}")
            result = frame.get("result")
            if not isinstance(result, dict):
                raise WsError(f"WebSocket RPC {method} result is not an object")
            return result, frames

    def until_sync_done(self, initial_frames: list[dict[str, Any]], *, timeout: float) -> tuple[list[dict[str, Any]], int]:
        frames = list(initial_frames)
        deadline = time.monotonic() + timeout
        while True:
            sync = next(
                (
                    frame
                    for frame in frames
                    if frame.get("kind") == "evt" and frame.get("event") == "sync.done"
                ),
                None,
            )
            if sync is not None:
                data = sync.get("data")
                if not isinstance(data, dict) or not isinstance(data.get("seq"), int):
                    raise WsError("sync.done has no numeric seq")
                return frames, data["seq"]
            if self._sock is None:
                raise WsError("WebSocket socket is closed")
            self._sock.settimeout(max(0.01, deadline - time.monotonic()))
            frames.append(self.recv_json())
            if time.monotonic() >= deadline:
                raise WsError("timed out waiting for sync.done")

    def close(self) -> None:
        sock, self._sock = self._sock, None
        if sock is None:
            return
        try:
            mask = secrets.token_bytes(4)
            payload = bytes(value ^ mask[index % 4] for index, value in enumerate(b""))
            sock.sendall(bytes((0x88, 0x80)) + mask + payload)
        except (OSError, socket.timeout):
            pass
        try:
            sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        sock.close()


def event_seq(frame: dict[str, Any]) -> int | None:
    value = frame.get("seq")
    return value if frame.get("kind") == "evt" and isinstance(value, int) and not isinstance(value, bool) else None


def skill_event(frame: dict[str, Any], name: str) -> dict[str, Any] | None:
    if frame.get("event") != "skill.updated":
        return None
    data = frame.get("data")
    if not isinstance(data, dict) or not isinstance(data.get("skill"), dict):
        return None
    skill = data["skill"]
    return skill if skill.get("name") == name else None


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--device-id", help="Stable test device id; default is a unique local value")
    return parser


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    http = client_from_args(args)
    health = ready_health(http, args)
    if health.get("mock") is True or urlsplit(http.base_url).port == 7789:
        raise ValueError("connection replay requires production service, never mock")
    password = http.password
    if not password:
        raise ValueError("connection replay requires a Host password")
    ws_url = http.base_url.replace("http://", "ws://", 1).replace("https://", "wss://", 1)
    device_id = args.device_id or f"macbot-e2e-replay-{uuid.uuid4().hex[:12]}"
    marker = unique_marker("macbot-e2e-replay")
    skill_name = marker.lower()
    content_v1 = f"---\nname: {skill_name}\ndescription: connection replay test\n---\n\n# {marker}\n\nversion: one\n"
    content_v2 = content_v1.replace("version: one", "version: two")
    created = False
    first: MiniWebSocket | None = None
    second: MiniWebSocket | None = None
    try:
        first = MiniWebSocket(ws_url, password, timeout=args.timeout)
        hello_frame = first.recv_json()
        if hello_frame.get("kind") != "evt" or hello_frame.get("event") != "hello":
            raise WsError("first WebSocket frame is not hello")
        hello = require_dict(hello_frame.get("data"), "hello data")
        if hello.get("protocol") != 1 or not isinstance(hello.get("node_id"), str):
            raise WsError("hello has no protocol 1 node_id")
        node_id = hello["node_id"]
        initial_last_seq = hello.get("last_seq")
        if not isinstance(initial_last_seq, int) or isinstance(initial_last_seq, bool) or initial_last_seq < 0:
            raise WsError("hello.last_seq must be a non-negative integer")
        resume_params = {
            "last_seq": initial_last_seq,
            "client": {
                "platform": "macos",
                "app_version": "e2e-replay",
                "device_name": "macbot-e2e-replay",
                "device_id": device_id,
            },
        }
        resumed, resume_frames = first.request("session.resume", resume_params, timeout=args.timeout)
        initial_mode = resumed.get("mode")
        if initial_mode not in {"replay", "reset"}:
            raise WsError("initial session.resume returned an unknown mode")
        _, baseline_seq = first.until_sync_done(resume_frames, timeout=args.timeout)
        if initial_mode == "reset":
            # Reset is valid only for the initial cursor.  Rebuild the cursor
            # through the protocol's bootstrap method, but never use this
            # state read as evidence for the later replay assertion.
            initial_bootstrap, _ = first.request("bootstrap", {}, timeout=args.timeout)
            boot_seq = initial_bootstrap.get("seq")
            if not isinstance(boot_seq, int) or isinstance(boot_seq, bool) or boot_seq < baseline_seq:
                raise WsError("initial bootstrap did not return a valid event cursor")
            baseline_seq = boot_seq
        if baseline_seq < initial_last_seq:
            raise WsError("initial sync.done moved the event cursor backwards")
        create_result, create_frames = first.request(
            "skill.create",
            {"name": skill_name, "content": content_v1, "client_request_id": str(uuid.uuid4())},
            timeout=args.timeout,
        )
        created = True
        created_skill = require_dict(create_result.get("skill"), "skill.create.skill")
        if created_skill.get("name") != skill_name or created_skill.get("source") != "user":
            raise WsError("skill.create did not return the unique user skill")
        create_event_frames = list(create_frames)
        deadline = time.monotonic() + args.timeout
        create_event: dict[str, Any] | None = None
        while create_event is None:
            for frame in create_event_frames:
                if skill_event(frame, skill_name) is not None and event_seq(frame) is not None:
                    create_event = frame
                    break
            if create_event is not None:
                break
            if first._sock is None:
                raise WsError("WebSocket closed before skill.create event")
            first._sock.settimeout(max(0.01, deadline - time.monotonic()))
            create_event_frames.append(first.recv_json())
            if time.monotonic() >= deadline:
                raise WsError("timed out waiting for skill.create skill.updated")
        create_seq = event_seq(create_event)
        assert create_seq is not None
        if create_seq < baseline_seq:
            raise WsError("skill.create event sequence moved backwards")
        first.close()
        first = None

        update_result = http.call(
            "skill.update",
            {"name": skill_name, "content": content_v2, "client_request_id": str(uuid.uuid4())},
        )
        updated_skill = require_dict(update_result.get("skill"), "skill.update.skill")
        if updated_skill.get("name") != skill_name:
            raise WsError("skill.update returned a different skill")

        second = MiniWebSocket(ws_url, password, timeout=args.timeout)
        hello2_frame = second.recv_json()
        if hello2_frame.get("kind") != "evt" or hello2_frame.get("event") != "hello":
            raise WsError("reconnected WebSocket frame is not hello")
        hello2 = require_dict(hello2_frame.get("data"), "reconnected hello data")
        if hello2.get("protocol") != 1 or hello2.get("node_id") != node_id:
            raise WsError("reconnected WebSocket is not the same Host node")
        if not isinstance(hello2.get("last_seq"), int) or hello2["last_seq"] < create_seq:
            raise WsError("reconnected Host cursor is older than the create cursor")
        replayed, replay_frames = second.request("session.resume", resume_params | {"last_seq": create_seq}, timeout=args.timeout)
        if replayed.get("mode") != "replay":
            raise WsError("reconnect session.resume returned reset; no event replay proof")
        replay_frames, sync_seq = second.until_sync_done(replay_frames, timeout=args.timeout)
        replay_events = [frame for frame in replay_frames if event_seq(frame) is not None]
        matching = [frame for frame in replay_events if skill_event(frame, skill_name) is not None]
        if len(matching) != 1:
            raise WsError(f"expected one replayed skill.updated for the test skill, found {len(matching)}")
        replay_event = matching[0]
        replay_seq = event_seq(replay_event)
        assert replay_seq is not None
        if replay_seq <= create_seq or replay_seq > sync_seq:
            raise WsError("replayed skill.updated has an invalid sequence")
        get_result, _ = second.request("skill.get", {"name": skill_name}, timeout=args.timeout)
        detail = require_dict(get_result.get("skill"), "skill.get.skill")
        if detail.get("content") != content_v2:
            raise WsError("skill.get after replay does not contain the updated content")
        return {
            "scenario": "S0 main connection replay",
            "status": "PASS",
            "url": http.base_url,
            "health_version": health.get("version"),
            "node_id": node_id,
            "device_id": device_id,
            "skill_name": skill_name,
            "cursor_before_update": create_seq,
            "replayed_skill_updated_seq": replay_seq,
            "sync_seq": sync_seq,
            "mode": replayed.get("mode"),
            "note": "API/WebSocket transport evidence only; client UI reconnect rendering remains manual.",
        }
    finally:
        if first is not None:
            first.close()
        if second is not None:
            second.close()
        if created:
            try:
                http.call("skill.delete", {"name": skill_name, "client_request_id": str(uuid.uuid4())})
            except Exception as exc:
                # Preserve the original failure, but make a cleanup failure on
                # an otherwise successful run fail visibly.
                if sys.exc_info()[0] is None:
                    raise WsError(f"test skill cleanup failed: {type(exc).__name__}") from exc


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
