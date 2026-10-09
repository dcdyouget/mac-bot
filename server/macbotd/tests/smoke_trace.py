#!/usr/bin/env python3
"""Production streaming trace acceptance with a local fake provider.

The test starts a real macbotd on port 7840, creates a provider/model and a
private Bot chat, then verifies trace subscription replay while the model is
still streaming.  It uses only a local fake HTTP provider; no API key leaves
the process.

Python 3.9+; dependency: ``websockets``.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Dict, List, Optional, Tuple
import urllib.error
import urllib.request
import uuid


REPO = Path(__file__).resolve().parents[3]
DUMMY_TOKEN = "macbot-trace-fake-token-do-not-use"


class FakeProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.calls = 0

    def increment(self) -> int:
        with self.lock:
            self.calls += 1
            return self.calls


class FakeProviderHandler(BaseHTTPRequestHandler):
    state: FakeProviderState

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: Dict[str, Any]) -> None:
        encoded = json.dumps(value).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def _authorized(self) -> bool:
        return self.headers.get("Authorization") == "Bearer " + DUMMY_TOKEN

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self._authorized():
            self._json(200, {"data": [{"id": "trace-fake", "object": "model"}]})
            return
        self._json(401 if not self._authorized() else 404, {"error": {"message": "not found"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions":
            self._json(404, {"error": {"message": "not found"}})
            return
        if not self._authorized():
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        if body.get("model") != "trace-fake":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        self.state.increment()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        # Keep the request active long enough for a second client to subscribe
        # and observe in_flight text.  Each payload is a valid OpenAI SSE chunk.
        for index, text in enumerate(("trace-", "stream-", "alpha", "beta", "omega")):
            time.sleep(0.55 if index else 1.1)
            payload = {"choices": [{"delta": {"content": text}, "finish_reason": None}]}
            self.wfile.write(("data: " + json.dumps(payload) + "\n\n").encode("utf-8"))
            self.wfile.flush()
        final = {
            "choices": [{"delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 5},
        }
        self.wfile.write(("data: " + json.dumps(final) + "\n\ndata: [DONE]\n\n").encode("utf-8"))
        self.wfile.flush()


def start_fake_provider() -> Tuple[ThreadingHTTPServer, FakeProviderState, str]:
    state = FakeProviderState()
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    FakeProviderHandler.state = state
    threading.Thread(target=server.serve_forever, name="trace-fake-provider", daemon=True).start()
    return server, state, "http://127.0.0.1:{}/v1".format(server.server_port)


def http_json(url: str, value: Optional[Dict[str, Any]] = None, token: Optional[str] = None) -> Any:
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(
        url,
        data=None if value is None else json.dumps(value).encode("utf-8"),
        headers=headers,
        method="GET" if value is None else "POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        raise AssertionError("HTTP {} {}: {}".format(error.code, url, body)) from error


def rpc(base: str, password: str, method: str, params: Optional[Dict[str, Any]] = None) -> Any:
    result = http_json(
        base.rstrip("/") + "/api/v1/rpc",
        {"method": method, "params": params or {}},
        password,
    )
    if not result.get("ok"):
        raise AssertionError("{} failed: {}".format(method, result.get("error")))
    return result["result"]


def wait_http(base: str, timeout: float = 30.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if http_json(base.rstrip("/") + "/api/v1/health").get("ok") is True:
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.2)
    raise AssertionError("timed out waiting for macbotd health")


def history(base: str, password: str, chat_id: str) -> List[Dict[str, Any]]:
    return rpc(base, password, "trace.history", {"chat_id": chat_id, "tail": True, "limit": 500})["items"]


async def wait_history(base: str, password: str, chat_id: str, predicate: Any, timeout: float = 30.0) -> List[Dict[str, Any]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        items = await asyncio.to_thread(history, base, password, chat_id)
        if predicate(items):
            return items
        await asyncio.sleep(0.15)
    raise AssertionError("timed out waiting for trace history")


async def connect_ws(ws_url: str, password: str) -> Any:
    import websockets

    ws = await websockets.connect(
        ws_url.rstrip("/") + "/ws",
        additional_headers={"Authorization": "Bearer " + password},
        proxy=None,
    )
    hello = json.loads(await ws.recv())
    if hello.get("event") != "hello":
        await ws.close()
        raise AssertionError("missing websocket hello: {}".format(hello))
    return ws


async def receive_json(ws: Any, timeout: float = 5.0) -> Dict[str, Any]:
    raw = await asyncio.wait_for(ws.recv(), timeout)
    if not isinstance(raw, str):
        return await receive_json(ws, timeout)
    return json.loads(raw)


async def receive_until(ws: Any, predicate: Any, timeout: float = 15.0) -> Dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = await receive_json(ws, max(0.1, deadline - time.monotonic()))
        if predicate(value):
            return value
    raise AssertionError("timed out waiting for websocket frame")


async def ws_request(ws: Any, method: str, params: Dict[str, Any]) -> Dict[str, Any]:
    request_id = str(uuid.uuid4())
    await ws.send(json.dumps({"v": 1, "kind": "req", "id": request_id, "method": method, "params": params}))
    response = await receive_until(ws, lambda value: value.get("kind") == "res" and value.get("id") == request_id)
    if not response.get("ok"):
        raise AssertionError("{} failed: {}".format(method, response.get("error")))
    return response["result"]


class Daemon:
    def __init__(self, command: str, home: Path, url: str, env: Dict[str, str]) -> None:
        self.command = command
        self.home = home
        self.url = url
        self.env = env
        self.process: Optional[subprocess.Popen[bytes]] = None

    def start(self) -> None:
        log = self.home / "trace-smoke-daemon.log"
        handle = log.open("wb")
        self.process = subprocess.Popen(
            shlex.split(self.command),
            cwd=str(REPO),
            env=self.env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        handle.close()
        wait_http(self.url)

    def close(self) -> None:
        if self.process is None or self.process.poll() is not None:
            return
        try:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


async def acceptance(args: argparse.Namespace, fake_url: str, fake_state: FakeProviderState) -> Dict[str, Any]:
    base = args.url.rstrip("/")
    password = args.password
    rpc(base, password, "bootstrap")
    provider = rpc(
        base,
        password,
        "provider.create",
        {
            "name": "trace-local-fake",
            "api_kind": "openai-completions",
            "base_url": fake_url,
            "api_key": DUMMY_TOKEN,
            "client_request_id": "trace-provider-create",
        },
    )["provider"]
    provider_id = provider["id"]
    try:
        refreshed = rpc(base, password, "model.refresh", {"provider_id": provider_id})
        assert any(item["model_id"] == "trace-fake" for item in refreshed["models"]), refreshed
        model = rpc(
            base,
            password,
            "model.upsert",
            {
                "provider_id": provider_id,
                "model_id": "trace-fake",
                "display_name": "Trace fake",
                "caps": {"vision": False, "tools": True, "reasoning": False},
                "price": {
                    "input_per_mtok": 1.0,
                    "output_per_mtok": 1.0,
                    "cache_read_per_mtok": 0.0,
                    "cache_write_per_mtok": 0.0,
                },
            },
        )["model"]
        worker = rpc(
            base,
            password,
            "bot.create",
            {
                "name": "trace-fake-worker",
                "model": model["ref"],
                "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
            },
        )["bot"]
        chats = rpc(base, password, "chat.list")["chats"]
        chat = next(item for item in chats if item.get("kind") == "direct" and item.get("bot_id") == worker["id"])
        chat_id = chat["id"]
        ws_url = base.replace("http://", "ws://").replace("https://", "wss://")
        async with await connect_ws(ws_url, password) as subscribed, await connect_ws(ws_url, password) as plain:
            sent = rpc(
                base,
                password,
                "chat.send",
                {"chat_id": chat_id, "text": "TRACE_STREAM_SMOKE", "mentions": [], "client_request_id": "trace-chat-send"},
            )
            assert sent["message"]["chat_id"] == chat_id
            await wait_history(base, password, chat_id, lambda items: any(item.get("type") == "llm.request" for item in items))
            subscription = await ws_request(subscribed, "trace.subscribe", {"chat_id": chat_id, "since_aseq": 0})
            stream_id = subscription["stream"]
            deltas = []
            while not deltas:
                frame = await receive_json(subscribed, 8.0)
                if frame.get("event") == "trace.delta":
                    deltas.append(frame)
                    assert frame.get("data", {}).get("stream") == stream_id
            async with await connect_ws(ws_url, password) as inflight_ws:
                in_flight = await ws_request(inflight_ws, "trace.subscribe", {"chat_id": chat_id, "since_aseq": 0})
                assert in_flight["in_flight"], in_flight
                assert any(item.get("text") for item in in_flight["in_flight"]), in_flight
            run_items = await wait_history(
                base,
                password,
                chat_id,
                lambda items: any(item.get("type") == "run.end" for item in items),
                30.0,
            )
            while True:
                try:
                    frame = await asyncio.wait_for(receive_json(subscribed, 0.2), 0.2)
                except asyncio.TimeoutError:
                    break
                if frame.get("event") == "trace.delta":
                    deltas.append(frame)
            assert len(deltas) >= 3, len(deltas)

            # The connection that never subscribed may receive protocol-global
            # message events, but must never receive trace-only events.
            leaked = []
            while True:
                try:
                    frame = await asyncio.wait_for(receive_json(plain, 0.2), 0.2)
                except asyncio.TimeoutError:
                    break
                if frame.get("event") in {"trace.delta", "trace.tool_output"}:
                    leaked.append(frame)
            assert not leaked, leaked

        expected = history(base, password, chat_id)
        expected_aseq = sorted({item["aseq"] for item in expected})
        async with await connect_ws(ws_url, password) as replay_ws:
            replay_result = await ws_request(replay_ws, "trace.subscribe", {"chat_id": chat_id, "since_aseq": 0})
            assert replay_result["in_flight"] == [], replay_result
            replay = []
            deadline = time.monotonic() + 15
            while len(replay) < len(expected_aseq) and time.monotonic() < deadline:
                frame = await receive_json(replay_ws, max(0.1, deadline - time.monotonic()))
                if frame.get("event") == "trace.item":
                    replay.append(frame["data"]["item"])
            replay_aseq = [item["aseq"] for item in replay]
            assert replay_aseq == expected_aseq, {"expected": expected_aseq, "actual": replay_aseq}
            assert len(replay_aseq) == len(set(replay_aseq)), replay_aseq

        return {
            "chat_id": chat_id,
            "provider_calls": fake_state.calls,
            "trace_items": len(expected_aseq),
            "replay_items": len(replay_aseq),
            "delta_frames": len(deltas),
            "inflight_text": in_flight["in_flight"][0]["text"],
            "checks": ["in_flight", "cursor_replay_sorted_unique", "unsubscribed_trace_filter", "post_run_replay"],
        }
    finally:
        # Provider/model data is intentionally left in the isolated home for
        # post-run inspection; the secret directory is removed by main().
        pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--url", default="http://127.0.0.1:7840")
    parser.add_argument("--password", default="dev")
    args = parser.parse_args()
    if args.home.resolve().is_relative_to(REPO.resolve()):
        parser.error("--home must be outside the repository")
    args.home.mkdir(mode=0o700, parents=True, exist_ok=True)
    secret_dir = Path("/tmp") / "macbot-trace-secrets-{}".format(os.getpid())
    shutil.rmtree(secret_dir, ignore_errors=True)
    secret_dir.mkdir(mode=0o700, parents=True)
    env = os.environ.copy()
    env["MACBOT_HOME"] = str(args.home)
    env["MACBOT_SECRET_BACKEND"] = "file"
    env["MACBOT_SECRET_DIR"] = str(secret_dir)
    daemon = Daemon(args.daemon_command, args.home, args.url, env)
    fake_server, fake_state, fake_url = start_fake_provider()
    try:
        daemon.start()
        result = asyncio.run(acceptance(args, fake_url, fake_state))
        print(json.dumps({"ok": True, "home": str(args.home), "url": args.url, **result}, ensure_ascii=False))
        return 0
    finally:
        daemon.close()
        fake_server.shutdown()
        fake_server.server_close()
        shutil.rmtree(secret_dir, ignore_errors=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, urllib.error.URLError) as error:
        print("smoke_trace: FAIL: {}".format(error), file=sys.stderr)
        raise SystemExit(1)
