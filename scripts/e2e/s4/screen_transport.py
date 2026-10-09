#!/usr/bin/env python3
"""S4 ``/ws/screen`` transport-only acceptance.

The check opens one Bot's screen WebSocket at low quality, validates the
initial state and two binary JPEG frames, and acknowledges every frame.  With
``--takeover`` it uses only the documented takeover.start/release RPCs to
check the driver transition; it never sends mouse, keyboard, wheel, or touch
input.  This does not prove browser login, frame painting, or mobile UI.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import socket
import struct
import sys
import time
from typing import Any
from urllib.parse import urlencode, urlsplit

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    client_from_args,
    ready_health,
    require_dict,
    require_list,
    run_main,
)
from s0.connection_replay import MiniWebSocket, WsError  # noqa: E402


class ScreenWebSocket(MiniWebSocket):
    """MiniWebSocket variant that permits the documented screen query path."""

    @staticmethod
    def _parse_url(value: str) -> tuple[str, str, int, str]:
        raw = value.strip()
        if "://" not in raw:
            raw = "http://" + raw
        parsed = urlsplit(raw)
        if parsed.scheme not in {"http", "https", "ws", "wss"} or not parsed.hostname:
            raise WsError("unsupported screen WebSocket URL")
        if parsed.username is not None or parsed.password is not None or parsed.fragment:
            raise WsError("screen WebSocket URL must not contain userinfo or fragment")
        secure = parsed.scheme in {"https", "wss"}
        scheme = "wss" if secure else "ws"
        try:
            port = parsed.port or (443 if secure else 80)
        except ValueError as exc:
            raise WsError("screen WebSocket URL has an invalid port") from exc
        path = parsed.path.rstrip("/")
        if path != "/ws/screen":
            raise WsError("screen WebSocket URL must use /ws/screen")
        if not parsed.query:
            raise WsError("screen WebSocket URL requires query parameters")
        return scheme, parsed.hostname, port, path + "?" + parsed.query

    def recv_message(self) -> tuple[str, bytes | dict[str, Any]]:
        """Receive one complete text or binary WebSocket message."""

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
                try:
                    self._send_frame(0x8, payload[:125])
                except WsError:
                    pass
                raise WsError("screen WebSocket peer closed the connection")
            if opcode in {0x1, 0x2}:
                if first_opcode is not None:
                    raise WsError("screen WebSocket started a new message before finishing a fragment")
                first_opcode = opcode
            elif opcode == 0x0:
                if first_opcode is None:
                    raise WsError("screen WebSocket continuation has no initial frame")
            else:
                raise WsError("screen WebSocket used an unsupported opcode")
            fragments.append(payload)
            if fin:
                break
        raw = b"".join(fragments)
        if first_opcode == 0x2:
            return "binary", raw
        if first_opcode != 0x1:
            raise WsError("screen WebSocket returned an invalid message")
        try:
            value = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise WsError("screen WebSocket returned invalid JSON") from exc
        if not isinstance(value, dict):
            raise WsError("screen WebSocket text message is not an object")
        return "text", value


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--bot-id", required=True, help="Bot with an existing browser session")
    parser.add_argument(
        "--output",
        help="Directory for validated JPEG evidence (default: docs/progress/S4/screen-transport-<timestamp>)",
    )
    parser.add_argument(
        "--takeover",
        action="store_true",
        help="Call takeover.start/release and verify driver user -> bot/idle; sends no input",
    )
    return parser


def validate_state(value: Any, bot_id: str) -> dict[str, Any]:
    state = require_dict(value, "screen state")
    if state.get("bot_id") != bot_id:
        raise WsError("screen state belongs to a different Bot")
    if state.get("driver") not in {"bot", "user", "idle"}:
        raise WsError("screen state has an invalid driver")
    if not isinstance(state.get("width"), int) or isinstance(state.get("width"), bool) or state["width"] <= 0:
        raise WsError("screen state width is invalid")
    if not isinstance(state.get("height"), int) or isinstance(state.get("height"), bool) or state["height"] <= 0:
        raise WsError("screen state height is invalid")
    tabs = require_list(state.get("tabs"), "screen state.tabs")
    if not tabs:
        raise WsError("screen state has no browser tab")
    for tab in tabs:
        tab = require_dict(tab, "screen state tab")
        if not isinstance(tab.get("tab_id"), str) or not tab["tab_id"]:
            raise WsError("screen state tab_id is invalid")
        if not isinstance(tab.get("title"), str) or not isinstance(tab.get("url"), str):
            raise WsError("screen state tab title/url is invalid")
        if tab.get("assignment_id") is not None and not isinstance(tab.get("assignment_id"), str):
            raise WsError("screen state tab assignment_id is invalid")
        if not isinstance(tab.get("active"), bool):
            raise WsError("screen state tab active is invalid")
    return state


def validate_screen_frame(raw: bytes) -> tuple[dict[str, Any], bytes]:
    if len(raw) < 4:
        raise WsError("screen binary frame has no header length")
    header_length = struct.unpack("!I", raw[:4])[0]
    if header_length <= 0 or header_length > 64 * 1024 or len(raw) <= 4 + header_length:
        raise WsError("screen binary frame header length is invalid")
    header_bytes = raw[4 : 4 + header_length]
    jpeg = raw[4 + header_length :]
    try:
        header = json.loads(header_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise WsError("screen binary frame header is invalid JSON") from exc
    if not isinstance(header, dict):
        raise WsError("screen binary frame header is not an object")
    if not isinstance(header.get("seq"), int) or isinstance(header.get("seq"), bool):
        raise WsError("screen frame seq is not an integer")
    if not isinstance(header.get("tab_id"), str) or not header["tab_id"]:
        raise WsError("screen frame tab_id is invalid")
    for key in ("w", "h"):
        if not isinstance(header.get(key), int) or isinstance(header.get(key), bool) or header[key] <= 0:
            raise WsError(f"screen frame {key} is invalid")
    if not isinstance(header.get("ts"), (int, float)) or isinstance(header.get("ts"), bool):
        raise WsError("screen frame ts is invalid")
    if not isinstance(header.get("url"), str):
        raise WsError("screen frame url is invalid")
    if not jpeg.startswith(b"\xff\xd8") or not jpeg.endswith(b"\xff\xd9"):
        raise WsError("screen frame is not a complete JPEG")
    return header, jpeg


def wait_for_state(ws: ScreenWebSocket, *, timeout: float, allowed: set[str], bot_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise WsError("timed out waiting for screen state")
        if ws._sock is None:
            raise WsError("screen WebSocket is closed")
        ws._sock.settimeout(min(ws._timeout, max(0.01, remaining)))
        kind, value = ws.recv_message()
        if kind == "binary" and isinstance(value, bytes):
            header, _jpeg = validate_screen_frame(value)
            ws.send_json({"type": "ack", "seq": header["seq"]})
            continue
        if kind != "text" or not isinstance(value, dict) or value.get("type") != "state":
            continue
        state = validate_state(value.get("state"), bot_id)
        if state["driver"] in allowed:
            return state


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    http = client_from_args(args)
    health = ready_health(http, args)
    if health.get("mock") is True or urlsplit(http.base_url).port == 7789:
        raise ValueError("screen transport requires production service, never mock")
    if not http.password:
        raise ValueError("screen transport requires a Host password")
    state = bootstrap(http)
    bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
    bot = next((item for item in bots if item.get("id") == args.bot_id), None)
    if not isinstance(bot, dict):
        raise ValueError("selected Bot is absent from bootstrap")
    output = Path(args.output) if args.output else Path("docs/progress/S4") / f"screen-transport-{int(time.time())}"
    output.mkdir(parents=True, exist_ok=True)
    ws: ScreenWebSocket | None = None
    takeover_started = False
    released = False
    try:
        screen_base = http.base_url.replace("http://", "ws://", 1).replace("https://", "wss://", 1).rstrip("/")
        query = urlencode({"bot_id": args.bot_id, "quality": "low"})
        ws = ScreenWebSocket(f"{screen_base}/ws/screen?{query}", http.password, timeout=args.timeout)
        initial = wait_for_state(ws, timeout=args.timeout, allowed={"bot", "user", "idle"}, bot_id=args.bot_id)
        if args.takeover:
            require_dict(http.call("takeover.start", {"bot_id": args.bot_id}), "takeover.start result")
            takeover_started = True
            user_state = wait_for_state(ws, timeout=args.timeout, allowed={"user"}, bot_id=args.bot_id)
        else:
            user_state = None
        frames: list[dict[str, Any]] = []
        for index in range(2):
            deadline = time.monotonic() + args.timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise WsError("timed out waiting for screen JPEG frame")
                if ws._sock is None:
                    raise WsError("screen WebSocket is closed")
                ws._sock.settimeout(min(ws._timeout, max(0.01, remaining)))
                kind, value = ws.recv_message()
                if kind != "binary" or not isinstance(value, bytes):
                    if kind == "text" and isinstance(value, dict) and value.get("type") == "state":
                        validate_state(value.get("state"), args.bot_id)
                    continue
                header, jpeg = validate_screen_frame(value)
                if frames and header["seq"] <= frames[-1]["seq"]:
                    raise WsError("screen frame seq is not increasing")
                path = output / f"frame-{index + 1:02d}-seq-{header['seq']}.jpg"
                path.write_bytes(jpeg)
                frames.append({"header": header, "path": str(path), "bytes": len(jpeg), "seq": header["seq"]})
                ws.send_json({"type": "ack", "seq": header["seq"]})
                break
        release_state = None
        if takeover_started:
            require_dict(
                http.call("takeover.release", {"bot_id": args.bot_id, "note": "screen transport check complete"}),
                "takeover.release result",
            )
            released = True
            release_state = wait_for_state(ws, timeout=args.timeout, allowed={"bot", "idle"}, bot_id=args.bot_id)
        return {
            "scenario": "S4 screen transport API checks",
            "status": "PASS",
            "url": http.base_url,
            "health_version": health.get("version"),
            "bot_id": args.bot_id,
            "quality": "low",
            "initial_state": initial,
            "takeover": {"enabled": args.takeover, "user_state": user_state, "release_state": release_state},
            "frames": frames,
            "output": str(output),
            "note": "Transport only; no input, browser login, frame painting, notification, or mobile UI evidence.",
        }
    finally:
        active_exception = sys.exc_info()[1]
        cleanup_error: Exception | None = None
        if takeover_started and not released:
            try:
                http.call("takeover.release", {"bot_id": args.bot_id, "note": "screen transport cleanup"})
            except Exception as exc:
                cleanup_error = exc
        if ws is not None:
            ws.close()
        if cleanup_error is not None:
            message = f"takeover cleanup failed: {cleanup_error}"
            if active_exception is None:
                raise WsError(message)
            if hasattr(active_exception, "add_note"):
                active_exception.add_note(message)
            else:
                print(f"Cleanup also failed: {message}", file=sys.stderr)


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
