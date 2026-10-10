#!/usr/bin/env python3
"""Production screen acceptance with a local fake OpenAI provider.

This test drives the real macbotd production backend.  The fake provider emits
``browser_open`` calls, so BrowserManager creates real agent-browser tabs and
``/ws/screen`` then exercises the native sidecar stream.  No network model or
API key is used.

Use a port different from the takeover smoke (the default is 7815):

  MACBOT_BROWSER_BIN=/tmp/macbot-agent-browser-0.39.0 \
    server/target/debug/macbotd --port 7815 --password dev \
      --home /tmp/macbot-screen-smoke
  python3 server/macbotd/tests/smoke_screen.py --url http://127.0.0.1:7815 \
      --home /tmp/macbot-screen-smoke

Pass ``--daemon-command`` to let the script start and stop an isolated daemon.
Dependencies: Python 3.9+ and ``websockets``.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable
import urllib.error
import urllib.request
import uuid


REPO = Path(__file__).resolve().parents[3]
DUMMY_TOKEN = "macbot-screen-fake-token-do-not-use"
DEFAULT_PORT = 7815


class FakeProviderState:
    def __init__(self, local_page_url: str) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.browser_calls = 0
        self.local_page_url = local_page_url

    def record(self, body: dict[str, Any]) -> None:
        with self.lock:
            self.requests.append(body)
            messages = json.dumps(body.get("messages", []), ensure_ascii=False)
            if "SCREEN_OPEN_" in messages:
                self.browser_calls += 1


class FakeProviderHandler(BaseHTTPRequestHandler):
    state: FakeProviderState

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def _stream(self, payloads: list[dict[str, Any]]) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _text(self, text: str = "screen smoke complete") -> None:
        self._stream(
            [
                {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
                {
                    "choices": [{"delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 4},
                },
            ]
        )

    def _browser_open(self) -> None:
        call_id = f"call_screen_{uuid.uuid4().hex[:10]}"
        self._stream(
            [
                {
                    "choices": [
                        {
                            "delta": {
                                "tool_calls": [
                                    {
                                        "index": 0,
                                        "id": call_id,
                                        "type": "function",
                                        "function": {
                                            "name": "browser_open",
                                            "arguments": json.dumps(
                                                {"url": self.state.local_page_url}
                                            ),
                                        },
                                    }
                                ]
                            },
                            "finish_reason": None,
                        }
                    ]
                },
                {
                    "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
                    "usage": {"prompt_tokens": 24, "completion_tokens": 8},
                },
            ]
        )

    def _request_takeover(self) -> None:
        call_id = f"call_takeover_{uuid.uuid4().hex[:10]}"
        self._stream(
            [
                {
                    "choices": [
                        {
                            "delta": {
                                "tool_calls": [
                                    {
                                        "index": 0,
                                        "id": call_id,
                                        "type": "function",
                                        "function": {
                                            "name": "request_takeover",
                                            "arguments": json.dumps({"reason": "screen smoke takeover"}),
                                        },
                                    }
                                ]
                            },
                            "finish_reason": None,
                        }
                    ]
                },
                {
                    "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
                    "usage": {"prompt_tokens": 24, "completion_tokens": 8},
                },
            ]
        )

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/models":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self.headers.get("Authorization") != f"Bearer {DUMMY_TOKEN}":
            self._json(401, {"error": {"message": "fake token required"}})
            return
        self._json(200, {"data": [{"id": "fake-screen", "object": "model"}]})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self.headers.get("Authorization") != f"Bearer {DUMMY_TOKEN}":
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        if body.get("model") != "fake-screen":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        self.state.record(body)
        messages = body.get("messages", [])
        prompt = json.dumps(messages, ensure_ascii=False)
        # provider.test and model refresh must remain ordinary text calls.  A
        # tool result marks the second execution turn, which completes it.
        last_user_index = max(
            (index for index, message in enumerate(messages) if isinstance(message, dict) and message.get("role") == "user"),
            default=-1,
        )
        has_tool_result = any(
            index > last_user_index and isinstance(message, dict) and message.get("role") == "tool"
            for index, message in enumerate(messages)
        )
        user_messages = [
            message.get("content", "")
            for message in messages
            if isinstance(message, dict) and message.get("role") == "user"
        ]
        latest_user = str(user_messages[-1]) if user_messages else prompt
        if "TAKEOVER_REQUEST_" in latest_user and not has_tool_result:
            self._request_takeover()
        elif "SCREEN_OPEN_" in latest_user and not has_tool_result:
            self._browser_open()
        else:
            self._text()


class LocalPageHandler(BaseHTTPRequestHandler):
    marker: str

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802
        body = f"""<!doctype html>
<meta charset="utf-8">
<title>{self.marker}</title>
<style>html,body{{margin:0;width:100%;height:100%;background:#18324a;color:white;font:32px sans-serif}}
#marker{{position:fixed;inset:35% 0 0;text-align:center}}</style>
<body tabindex="0"><div id="marker">{self.marker}</div>
<script>
const marker = {json.dumps(self.marker)};
document.body.focus();
document.addEventListener('click', () => {{ location.hash = 'clicked-' + marker; }});
document.addEventListener('keydown', (event) => {{
  if (event.key.toLowerCase() === 'k') location.hash = 'key-k-' + marker;
}});
</script></body>""".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def start_local_page(marker: str) -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), LocalPageHandler)
    LocalPageHandler.marker = marker
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/?marker={marker}"


def start_fake_provider(local_page_url: str) -> tuple[ThreadingHTTPServer, FakeProviderState, str]:
    state = FakeProviderState(local_page_url)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    FakeProviderHandler.state = state
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, state, f"http://127.0.0.1:{server.server_port}/v1"


def http_json(url: str, value: dict[str, Any] | None = None, token: str | None = None) -> Any:
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        url,
        data=None if value is None else json.dumps(value).encode(),
        headers=headers,
        method="GET" if value is None else "POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise AssertionError(
            f"HTTP {error.code} {url}: {error.read().decode(errors='replace')}"
        ) from error


def rpc(base_url: str, password: str, method: str, params: dict[str, Any] | None = None) -> Any:
    value = http_json(
        f"{base_url.rstrip('/')}/api/v1/rpc",
        {"method": method, "params": params or {}},
        password,
    )
    if not value.get("ok"):
        raise AssertionError(f"{method} failed: {value.get('error')}")
    return value["result"]


def wait_until(predicate: Callable[[], bool], timeout: float, description: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, urllib.error.URLError, KeyError, StopIteration):
            pass
        time.sleep(0.2)
    raise AssertionError(f"timed out waiting for {description}")


class Daemon:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.process: subprocess.Popen[bytes] | None = None

    def start(self) -> None:
        if not self.args.daemon_command:
            wait_until(
                lambda: http_json(f"{self.args.url.rstrip('/')}/api/v1/health").get("ok")
                is True,
                20,
                "existing macbotd",
            )
            return
        env = os.environ.copy()
        env["MACBOT_HOME"] = str(self.args.home)
        env["MACBOT_SECRET_BACKEND"] = "file"
        env["MACBOT_SECRET_DIR"] = str(self.args.home / "secrets")
        if self.args.browser_bin:
            env["MACBOT_BROWSER_BIN"] = self.args.browser_bin
        self.process = subprocess.Popen(
            shlex.split(self.args.daemon_command),
            cwd=REPO,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        wait_until(
            lambda: http_json(f"{self.args.url.rstrip('/')}/api/v1/health").get("ok")
            is True,
            40,
            "macbotd startup",
        )

    def close(self) -> None:
        if self.process is None or self.process.poll() is not None:
            return
        try:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            self.process.kill()


def assignment_for(base: str, password: str, chat_id: str) -> dict[str, Any]:
    return next(
        item
        for item in rpc(base, password, "assignment.list").get("items", [])
        if item.get("origin_chat_id") == chat_id
    )


def trace_has_browser_open(base: str, password: str, assignment_id: str) -> bool:
    items = rpc(base, password, "trace.history", {"assignment_id": assignment_id, "limit": 500}).get("items", [])
    return any(
        item.get("type") == "tool.start"
        and item.get("data", {}).get("name") == "browser_open"
        for item in items
    )


async def recv_screen_message(ws: Any, timeout: float = 20) -> tuple[str, Any]:
    raw = await asyncio.wait_for(ws.recv(), timeout)
    if isinstance(raw, str):
        return "text", json.loads(raw)
    if len(raw) < 4:
        raise AssertionError("screen binary frame is shorter than its header")
    header_size = int.from_bytes(raw[:4], "big")
    if len(raw) < 4 + header_size:
        raise AssertionError("screen frame header is truncated")
    header = json.loads(raw[4 : 4 + header_size])
    jpeg = raw[4 + header_size :]
    if not jpeg.startswith(b"\xff\xd8"):
        raise AssertionError("screen frame payload is not JPEG")
    return "frame", (header, jpeg)


def jpeg_dimensions(jpeg: bytes) -> tuple[int, int] | None:
    if not jpeg.startswith(b"\xff\xd8"):
        return None
    offset = 2
    sof_markers = set(range(0xC0, 0xC4)) | set(range(0xC5, 0xC8)) | set(range(0xC9, 0xCC)) | set(range(0xCD, 0xD0))
    while offset + 9 < len(jpeg):
        if jpeg[offset] != 0xFF:
            offset += 1
            continue
        while offset < len(jpeg) and jpeg[offset] == 0xFF:
            offset += 1
        if offset >= len(jpeg):
            return None
        marker = jpeg[offset]
        offset += 1
        if marker in (0xD8, 0xD9):
            continue
        if offset + 2 > len(jpeg):
            return None
        length = int.from_bytes(jpeg[offset : offset + 2], "big")
        if length < 2 or offset + length > len(jpeg):
            return None
        if marker in sof_markers and length >= 7:
            height = int.from_bytes(jpeg[offset + 3 : offset + 5], "big")
            width = int.from_bytes(jpeg[offset + 5 : offset + 7], "big")
            return width, height
        offset += length
    return None


async def recv_frame(ws: Any, timeout: float = 20) -> tuple[dict[str, Any], bytes]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        kind, value = await recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        if kind == "text":
            if value.get("type") == "error":
                raise AssertionError(f"screen error: {value}")
            continue
        return value
    raise AssertionError("timed out waiting for screen frame")


async def recv_state_with_ack(
    ws: Any,
    expected_driver: str,
    timeout: float = 15,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        kind, value = await recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        if kind == "frame":
            await ws.send(json.dumps({"type": "ack", "seq": value[0]["seq"]}))
            continue
        if value.get("type") == "error":
            raise AssertionError(f"screen error: {value}")
        if value.get("type") == "state" and value["state"].get("driver") == expected_driver:
            return value["state"]
    raise AssertionError(f"timed out waiting for screen driver={expected_driver}")


async def recv_url_fragment(ws: Any, fragment: str, timeout: float = 15) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        kind, value = await recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        if kind == "frame":
            await ws.send(json.dumps({"type": "ack", "seq": value[0]["seq"]}))
            continue
        if value.get("type") == "error":
            raise AssertionError(f"screen error: {value}")
        if value.get("type") != "state":
            continue
        for tab in value["state"].get("tabs", []):
            url = tab.get("url", "")
            if fragment in url:
                return url
    raise AssertionError(f"timed out waiting for URL fragment {fragment!r}")


async def recv_state(ws: Any, timeout: float = 10) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        kind, value = await recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        if kind == "text":
            if value.get("type") == "error":
                raise AssertionError(f"screen error: {value}")
            if value.get("type") == "state":
                return value["state"]
    raise AssertionError("timed out waiting for screen state")


async def recv_active_state(ws: Any, tab_id: str, timeout: float = 15) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state = await recv_state(ws, max(0.1, deadline - time.monotonic()))
        if any(tab.get("tab_id") == tab_id and tab.get("active") for tab in state.get("tabs", [])):
            return state
    raise AssertionError(f"tab {tab_id} did not become active")


def browser_stream_status(binary: str, bot_id: str) -> dict[str, Any]:
    result = subprocess.run(
        [
            binary,
            "--session",
            f"macbot-{bot_id}",
            "--json",
            "stream",
            "status",
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=15,
    )
    value = json.loads(result.stdout)
    assert value.get("success") is True, value
    return value["data"]


def browser_tabs(binary: str, bot_id: str) -> tuple[int, str, str]:
    result = subprocess.run(
        [
            binary,
            "--session",
            f"macbot-{bot_id}",
            "--json",
            "tab",
            "list",
        ],
        capture_output=True,
        text=True,
        timeout=15,
    )
    return result.returncode, result.stdout, result.stderr


async def acceptance(args: argparse.Namespace, fake_url: str, fake: FakeProviderState, daemon: Daemon) -> None:
    import websockets

    base = args.url.rstrip("/")
    password = args.password
    bootstrap = rpc(base, password, "bootstrap")
    assert bootstrap["hello"]["node_id"]

    provider_id = rpc(
        base,
        password,
        "provider.create",
        {
            "name": "screen-fake-provider",
            "api_kind": "openai-completions",
            "base_url": fake_url,
            "api_key": DUMMY_TOKEN,
            "client_request_id": "screen-provider-create",
        },
    )["provider"]["id"]
    rpc(base, password, "model.refresh", {"provider_id": provider_id})
    model = rpc(
        base,
        password,
        "model.upsert",
        {
            "provider_id": provider_id,
            "model_id": "fake-screen",
            "display_name": "Screen fake",
            "caps": {"vision": False, "tools": True, "reasoning": False},
            "price": {
                "input_per_mtok": 1.0,
                "output_per_mtok": 1.0,
                "cache_read_per_mtok": 0.1,
                "cache_write_per_mtok": 0.1,
            },
            "client_request_id": "screen-model-upsert",
        },
    )["model"]["ref"]
    worker = rpc(
        base,
        password,
        "bot.create",
        {
            "name": "screen-fake-worker",
            "model": model,
            "tools": {
                "files": False,
                "bash": False,
                "browser": True,
                "subagent": False,
                "web": False,
                "mcp": False,
            },
            "client_request_id": "screen-worker-create",
        },
    )["bot"]

    def settle_pending() -> bool:
        pending = rpc(base, password, "bootstrap").get("pending", {})
        for approval in pending.get("approvals", []):
            if approval.get("state") == "pending":
                rpc(
                    base,
                    password,
                    "approval.decide",
                    {"approval_id": approval["id"], "decision": "allow_once"},
                )
        for question in pending.get("questions", []):
            if question.get("state") == "pending":
                rpc(
                    base,
                    password,
                    "question.answer",
                    {"question_id": question["id"], "option_index": 0},
                )
        return False

    async def create_assignment(index: int) -> tuple[str, dict[str, Any]]:
        project = rpc(
            base,
            password,
            "project.create",
            {
                "name": f"screen-project-{index}",
                "goal": "Open a real browser tab for screen acceptance",
                "member_bot_ids": [worker["id"]],
                "client_request_id": f"screen-project-create-{index}",
            },
        )
        chat_id = project["chat"]["id"]
        marker = f"SCREEN_OPEN_{index}_{uuid.uuid4().hex[:8]}"
        rpc(
            base,
            password,
            "chat.send",
            {
                "chat_id": chat_id,
                "text": marker,
                "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "open a browser tab"}],
                "client_request_id": f"screen-chat-send-{index}",
            },
        )
        wait_until(
            lambda: any(item.get("origin_chat_id") == chat_id for item in rpc(base, password, "assignment.list")["items"]),
            20,
            f"assignment {index} admission",
        )
        assignment = assignment_for(base, password, chat_id)
        wait_until(
            lambda: settle_pending() or trace_has_browser_open(base, password, assignment["id"]),
            45,
            f"browser_open tool for assignment {index}",
        )
        wait_until(
            lambda: (settle_pending() or True)
            and next(
                item for item in rpc(base, password, "assignment.list")["items"] if item["id"] == assignment["id"]
            ).get("status") == "done",
            45,
            f"assignment {index} completion",
        )
        return chat_id, assignment

    _, first = await create_assignment(1)
    assert fake.browser_calls >= 1, f"fake provider browser calls: {fake.browser_calls}"

    ws_url = base.replace("http://", "ws://").replace("https://", "wss://")
    screen_url = f"{ws_url}/ws/screen?bot_id={worker['id']}&quality=low"
    headers = {"Authorization": f"Bearer {password}"}
    marker = fake.local_page_url.split("marker=", 1)[-1]
    try:
        async with websockets.connect(screen_url, additional_headers=headers, proxy=None) as ws:
            initial = await recv_state(ws)
            assert initial["driver"] == "bot", initial
            tabs = initial["tabs"]
            assert tabs, initial
            tab = tabs[0]
            tab_id = tab["tab_id"]
            assert marker in tab["url"], tab

            header, jpeg = await recv_frame(ws)
            assert header["tab_id"] == tab_id, header
            assert header["w"] <= 640, header
            assert jpeg_dimensions(jpeg) == (header["w"], header["h"]), header
            assert marker in header["url"], header
            await ws.send(json.dumps({"type": "ack", "seq": header["seq"]}))

            active = rpc(base, password, "takeover.start", {"bot_id": worker["id"]})
            assert active == {}, active
            user_state = await recv_state_with_ack(ws, "user")
            assert user_state["bot_id"] == worker["id"], user_state

            # Coordinates are frame pixels. Use the low-quality frame center;
            # the gateway must scale 640x360 back into the browser viewport.
            await ws.send(
                json.dumps(
                    {
                        "type": "input",
                        "event": {
                            "type": "mouse",
                            "action": "click",
                            "x": header["w"] / 2,
                            "y": header["h"] / 2,
                            "button": "left",
                            "click_count": 1,
                        },
                    }
                )
            )
            clicked = await recv_url_fragment(ws, "#clicked-", timeout=15)
            assert marker in clicked, clicked

            await ws.send(
                json.dumps(
                    {
                        "type": "input",
                        "event": {
                            "type": "key",
                            "action": "press",
                            "key": "k",
                            "code": "KeyK",
                            "text": "k",
                            "modifiers": [],
                        },
                    }
                )
            )
            keyed = await recv_url_fragment(ws, "#key-k-", timeout=15)
            assert marker in keyed, keyed

            released = rpc(
                base,
                password,
                "takeover.release",
                {"bot_id": worker["id"], "note": "screen smoke released"},
            )
            assert released == {}, released
            released_state = await recv_state_with_ack(ws, "bot")
            assert released_state["bot_id"] == worker["id"], released_state

            await ws.send(
                json.dumps(
                    {
                        "type": "input",
                        "event": {
                            "type": "mouse",
                            "action": "click",
                            "x": header["w"] / 2,
                            "y": header["h"] / 2,
                            "button": "left",
                            "click_count": 1,
                        },
                    }
                )
            )
            denied = False
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                kind, value = await recv_screen_message(ws, deadline - time.monotonic())
                if kind == "frame":
                    await ws.send(json.dumps({"type": "ack", "seq": value[0]["seq"]}))
                elif value.get("type") == "error":
                    assert value["error"]["code"] == "permission_denied", value
                    denied = True
                    break
            assert denied, "input after takeover.release was not rejected"
        disabled = browser_stream_status(args.browser_bin, worker["id"])
        assert disabled.get("enabled") is False and not disabled.get("screencasting"), disabled
    finally:
        await asyncio.sleep(0.5)

    restart_checks = []
    if args.daemon_command:
        with fake.lock:
            provider_requests_before = len(fake.requests)
        requests_before = {path.name for path in (args.home / "data/run_requests").glob("*.json")}

        async def check_restored_screen() -> None:
            async with websockets.connect(screen_url, additional_headers=headers, proxy=None) as ws:
                state = await recv_state(ws)
                assert state["driver"] == "bot", state
                assert state["tabs"] and all(marker in tab["url"] for tab in state["tabs"]), state
                frame, image = await recv_frame(ws)
                assert marker in frame["url"], frame
                assert frame["w"] <= 640, frame
                assert jpeg_dimensions(image) == (frame["w"], frame["h"]), frame
                assert frame["tab_id"] in {tab["tab_id"] for tab in state["tabs"]}, (state, frame)
                await ws.send(json.dumps({"type": "ack", "seq": frame["seq"]}))
            with fake.lock:
                assert len(fake.requests) == provider_requests_before, "screen recovery invoked the model"
            assert {path.name for path in (args.home / "data/run_requests").glob("*.json")} == requests_before

        daemon.close()
        daemon.start()
        await check_restored_screen()
        restart_checks.append("restart_without_provider_run")

        # Close only this test's disposable headless sidecar. This also
        # exercises recovery when durable tab metadata outlives real CDP tabs.
        daemon.close()
        subprocess.run(
            [args.browser_bin, "--session", f"macbot-{worker['id']}", "--json", "close"],
            check=True, capture_output=True, timeout=30,
        )
        daemon.start()
        await check_restored_screen()
        restart_checks.append("restart_with_closed_sidecar_without_provider_run")

    print(
        json.dumps(
            {
                "ok": True,
                "bot_id": worker["id"],
                "assignment": first["id"],
                "marker": marker,
                "checks": [
                    "local_marker_url",
                    "low_jpeg_header_dimensions",
                    "ack_latest",
                    "same_ws_bot_user_bot",
                    "scaled_click",
                    "paired_keypress",
                    "release_denied",
                ] + restart_checks,
                "fake_provider_browser_calls": fake.browser_calls,
            },
            ensure_ascii=False,
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default=f"http://127.0.0.1:{DEFAULT_PORT}")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, default=None)
    parser.add_argument("--browser-bin", default="/tmp/macbot-agent-browser-0.39.0")
    parser.add_argument("--daemon-command")
    parser.add_argument("--keep-home", action="store_true")
    args = parser.parse_args()
    socket_dir: Path | None = None
    temporary_home = args.home is None
    if args.home is None:
        args.home = Path(tempfile.mkdtemp(prefix="macbot-screen-smoke-"))
    if args.daemon_command:
        # BrowserManager invokes the configured executable with only its
        # session arguments. Wrap the native binary to force a disposable
        # namespace/config and headless mode for this acceptance run.
        browser_root = args.home / "browser-test"
        browser_root.mkdir(parents=True, exist_ok=True)
        config = browser_root / "config.json"
        config.write_text("{}", encoding="utf-8")
        wrapper = browser_root / "agent-browser-wrapper"
        namespace = f"macbot-screen-{uuid.uuid4().hex[:10]}"
        # Keep this path deliberately short: agent-browser's Unix socket also
        # contains namespace and the UUID Bot session name (macOS limit 103).
        socket_dir = Path("/tmp/m")
        socket_dir.mkdir(parents=True, exist_ok=True)
        args.browser_socket_dir = socket_dir
        wrapper.write_text(
            "#!/bin/sh\n"
            f"export AGENT_BROWSER_SOCKET_DIR={shlex.quote(str(socket_dir))}\n"
            f"log={shlex.quote(str(browser_root / 'agent-browser.log'))}\n"
            "printf '%s\\n' \"argv:$*\" >>\"$log\"\n"
            f"output=$( {shlex.quote(str(Path(args.browser_bin).resolve()))} "
            f"--namespace {shlex.quote(namespace)} "
            f"--config {shlex.quote(str(config))} --headed false \"$@\" 2>>\"$log\")\n"
            "rc=$?\nprintf '%s rc=%s\\n' \"$output\" \"$rc\" >>\"$log\"\nprintf '%s' \"$output\"\nexit $rc\n",
            encoding="utf-8",
        )
        wrapper.chmod(0o700)
        args.browser_bin = str(wrapper)
    marker = f"SCREEN_LOCAL_{uuid.uuid4().hex[:10]}"
    local_page_server, local_page_url = start_local_page(marker)
    fake_server, fake_state, fake_url = start_fake_provider(local_page_url)
    daemon = Daemon(args)
    try:
        daemon.start()
        asyncio.run(acceptance(args, fake_url, fake_state, daemon))
    finally:
        daemon.close()
        fake_server.shutdown()
        local_page_server.shutdown()
        if temporary_home and not args.keep_home:
            import shutil

            shutil.rmtree(args.home, ignore_errors=True)


if __name__ == "__main__":
    main()
