#!/usr/bin/env python3
"""Regression test for WebSocket handshake bytes read ahead of the first frame.

The local TCP peer deliberately sends the HTTP 101 response and WebSocket
frames in the same stream.  The test runs the old drop-the-read-ahead behavior
as a negative control, then verifies that ``MiniWebSocket`` preserves and
consumes two ordered text envelopes.  It never connects to a MacBot host and
does not print the test Authorization value.
"""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path
import socket
import threading
import time
import sys
from typing import Callable

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent / "s0"))

from connection_replay import MiniWebSocket, WsError  # noqa: E402


TEST_PASSWORD = "local-buffer-regression-only"


def text_frame(value: dict[str, object]) -> bytes:
    payload = json.dumps(value, separators=(",", ":")).encode("utf-8")
    if len(payload) >= 126:
        raise AssertionError("fixture frame must use the short WebSocket length")
    return bytes((0x81, len(payload))) + payload


def response_for(request: bytes) -> bytes:
    key = next(
        (
            line.split(b":", 1)[1].strip().decode("ascii")
            for line in request.split(b"\r\n")
            if line.lower().startswith(b"sec-websocket-key:")
        ),
        None,
    )
    if key is None:
        raise AssertionError("local client did not send Sec-WebSocket-Key")
    accept = base64.b64encode(
        hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")).digest()
    ).decode("ascii")
    return (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept}\r\n\r\n"
    ).encode("ascii")


def local_peer(split: bool) -> tuple[socket.socket, threading.Thread, list[BaseException]]:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    errors: list[BaseException] = []
    first = text_frame({"kind": "evt", "event": "buffer.first", "seq": 1})
    second = text_frame({"kind": "evt", "event": "buffer.second", "seq": 2})

    def serve() -> None:
        conn: socket.socket | None = None
        try:
            conn, _ = listener.accept()
            conn.settimeout(2.0)
            request = bytearray()
            while b"\r\n\r\n" not in request:
                chunk = conn.recv(4096)
                if not chunk:
                    raise AssertionError("client closed before the local handshake")
                request.extend(chunk)
            reply = response_for(bytes(request))
            if split:
                chunks = [reply + first[:2], first[2:5], first[5:] + second[:4], second[4:]]
                for chunk in chunks:
                    conn.sendall(chunk)
                    time.sleep(0.03)
            else:
                # One sendall intentionally coalesces 101 + both complete frames.
                conn.sendall(reply + first + second)
        except BaseException as exc:  # report to the test thread, never stdout
            errors.append(exc)
        finally:
            if conn is not None:
                try:
                    conn.close()
                except OSError:
                    pass
            listener.close()

    thread = threading.Thread(target=serve, name="ws-buffer-peer", daemon=True)
    thread.start()
    return listener, thread, errors


def client_for(listener: socket.socket) -> MiniWebSocket:
    host, port = listener.getsockname()
    client = MiniWebSocket.__new__(MiniWebSocket)
    client._timeout = 2.0
    client._sock = socket.create_connection((host, port), timeout=2.0)
    client._sock.settimeout(2.0)
    client._receive_buffer = bytearray()
    client._handshake(host, port, "/ws", TEST_PASSWORD)
    return client


def run_case(name: str, split: bool, *, preserve_buffer: bool) -> None:
    listener, thread, errors = local_peer(split)
    client: MiniWebSocket | None = None
    try:
        client = client_for(listener)
        if not preserve_buffer:
            # This is the pre-fix behavior: bytes read beyond HTTP headers are
            # discarded before the first WebSocket frame is decoded.
            client._receive_buffer.clear()
            try:
                client.recv_json()
            except WsError:
                return
            raise AssertionError(f"legacy behavior unexpectedly decoded {name}")
        first = client.recv_json()
        second = client.recv_json()
        if [first.get("event"), second.get("event")] != ["buffer.first", "buffer.second"]:
            raise AssertionError(f"{name} envelopes arrived out of order")
        if [first.get("seq"), second.get("seq")] != [1, 2]:
            raise AssertionError(f"{name} sequence evidence is not [1, 2]")
    finally:
        if client is not None:
            client.close()
        thread.join(timeout=2.0)
        if thread.is_alive():
            raise AssertionError(f"{name} local peer did not finish")
        if errors and not (not preserve_buffer and isinstance(errors[0], BrokenPipeError)):
            raise AssertionError(f"{name} local peer failed: {type(errors[0]).__name__}") from errors[0]


def main() -> int:
    for name, split in (("coalesced", False), ("split", True)):
        run_case(name, split, preserve_buffer=False)
        run_case(name, split, preserve_buffer=True)
    print(json.dumps({
        "status": "PASS",
        "test": "scripts/e2e/test_ws_buffer.py",
        "cases": ["coalesced", "split"],
        "negative_control": "old_handshake_drop_buffer_rejected",
        "positive_control": "two_ordered_text_frames_received",
        "transport": "local_tcp_only",
    }, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
