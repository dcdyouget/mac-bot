#!/usr/bin/env python3
"""S1 production-runtime acceptance against a local fake OpenAI provider.

The daemon is deliberately external to this script so the same checks work on a
LaunchAgent deployment and in CI.  For the complete restart check, pass the exact
command used to start macbotd, for example:

  python server/macbotd/tests/smoke_runtime.py \
    --daemon-command 'cargo run --manifest-path server/macbotd/Cargo.toml -- --port 7791 --password dev' \
    --home /tmp/macbot-runtime-smoke

The fake provider accepts only the dummy token below.  It records requests,
returns streaming tool calls, and never calls a real model.

Dependencies: Python 3.10+, ``websockets``.  No API key or product data is
written by this test.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import re
import shlex
import signal
import shutil
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable
import urllib.error
import urllib.request
import uuid


REPO = Path(__file__).resolve().parents[3]
DUMMY_TOKEN = "macbot-fake-token-do-not-use"


class FakeProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.auth_headers: list[str] = []
        self.work_calls = 0
        self.session_calls: dict[str, int] = {}

    def record(self, body: dict[str, Any], auth: str) -> int:
        with self.lock:
            self.requests.append(body)
            self.auth_headers.append(auth)
            prompt = json.dumps(body.get("messages", []), ensure_ascii=False)
            session = str(body.get("session_id") or "default")
            self.session_calls[session] = self.session_calls.get(session, 0) + 1
            if "Reply OK." not in prompt:
                self.work_calls += 1
            return self.session_calls[session]

    def snapshot(self) -> tuple[list[dict[str, Any]], list[str]]:
        with self.lock:
            return list(self.requests), list(self.auth_headers)


class FakeProviderHandler(BaseHTTPRequestHandler):
    state: FakeProviderState

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _auth(self) -> str:
        return self.headers.get("Authorization", "")

    def _json(self, status: int, value: dict[str, Any]) -> None:
        encoded = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models":
            if self._auth() != f"Bearer {DUMMY_TOKEN}":
                self._json(401, {"error": {"message": "fake token required"}})
                return
            self._json(200, {"data": [{"id": "fake-runtime", "object": "model"}]})
            return
        self._json(404, {"error": {"message": "not found"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self._auth() != f"Bearer {DUMMY_TOKEN}":
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        if body.get("model") != "fake-runtime":
            self._json(400, {"error": {"message": f"unexpected model {body.get('model')!r}"}})
            return
        call_number = self.state.record(body, self._auth())
        body_text = json.dumps(body, ensure_ascii=False)
        if any(marker in body_text for marker in ("fake provider tool workflow", "parallel task", "UNSAFE_APPROVAL")):
            # Leave a deterministic window for the steer/concurrency checks
            # while the durable run is waiting inside the provider request.
            time.sleep(2.0)
        messages = body.get("messages", [])
        prompt = json.dumps(messages, ensure_ascii=False)
        if "PRIVATE_TOOL_WORKFLOW" in prompt:
            names = set(re.findall(r'"name"\s*:\s*"([a-z_]+)"', prompt))
            if "write" not in names:
                self._stream_tool(
                    "write",
                    {"path": "s1-runtime-tool.txt", "content": "write-sentinel"},
                    7,
                    3,
                )
            elif "read" not in names:
                self._stream_tool("read", {"path": "s1-runtime-tool.txt"}, 7, 3)
            elif "edit" not in names:
                self._stream_tool(
                    "edit",
                    {
                        "path": "s1-runtime-tool.txt",
                        "edits": [{"oldText": "write-sentinel", "newText": "edit-sentinel"}],
                    },
                    7,
                    3,
                )
            elif "bash" not in names:
                self._stream_tool(
                    "bash",
                    {"command": "pwd && printf bash-sentinel", "background": False},
                    7,
                    3,
                )
            elif "bash_job" not in names:
                match = re.search(r"started background job ([A-Za-z0-9_-]+)", prompt)
                self._stream_tool(
                    "bash_job",
                    {"job_id": match.group(1) if match else "missing-job", "action": "status"},
                    7,
                    3,
                )
            else:
                self._stream_text("private tool workflow complete bash-sentinel", 7, 3)
        elif "MAIN_TOOL_SET" in prompt:
            self._stream_text("main tool set complete", 7, 3)
        elif "PRIVATE_RUNTIME" in prompt:
            self._stream_text("private fake reply", 9, 4)
        elif "UNSAFE_APPROVAL" in prompt:
            self._stream_tool(
                "bash",
                {"command": "echo macbot-unsafe-approval", "cwd": "."},
                7,
                3,
            )
        elif "fake provider" in prompt or "parallel task" in prompt:
            if "fake provider completed" in prompt:
                self._stream_text("group run finalized", 6, 3)
            elif "message admitted" not in prompt:
                self._stream_tool(
                    "send_msg",
                    {"intent": "progress", "text": "fake provider progress", "mentions": []},
                    11,
                    4,
                )
            else:
                self._stream_tool(
                    "send_msg",
                    {"intent": "done", "text": "fake provider completed", "mentions": []},
                    13,
                    5,
                )
        elif "Reply OK." in prompt:
            self._stream_text("OK", 3, 2)
        else:
            self._stream_text("OK", 3, 2)

    def _stream(self, payloads: list[dict[str, Any]]) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            data = f"data: {json.dumps(payload, ensure_ascii=False)}\n\n".encode()
            self.wfile.write(data)
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _stream_text(self, text: str, input_tokens: int, output_tokens: int) -> None:
        self._stream(
            [
                {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
                {
                    "choices": [{"delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": input_tokens, "completion_tokens": output_tokens},
                },
            ]
        )

    def _stream_tool(
        self, name: str, args: dict[str, Any], input_tokens: int, output_tokens: int
    ) -> None:
        self._stream(
            [
                {
                    "choices": [
                        {
                            "delta": {
                                "tool_calls": [
                                    {
                                        "index": 0,
                                        "id": f"call_fake_{uuid.uuid4().hex[:10]}",
                                        "function": {
                                            "name": name,
                                            "arguments": json.dumps(args, ensure_ascii=False),
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
                    "usage": {"prompt_tokens": input_tokens, "completion_tokens": output_tokens},
                },
            ]
        )


def start_fake_provider() -> tuple[ThreadingHTTPServer, FakeProviderState, str]:
    state = FakeProviderState()
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    FakeProviderHandler.state = state
    thread = threading.Thread(target=server.serve_forever, name="fake-provider", daemon=True)
    thread.start()
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
        body = error.read().decode(errors="replace")
        raise AssertionError(f"HTTP {error.code} {url}: {body}") from error


def rpc(base_url: str, password: str, method: str, params: dict[str, Any] | None = None) -> Any:
    body = http_json(
        f"{base_url}/api/v1/rpc",
        {"method": method, "params": params or {}},
        password,
    )
    if not body.get("ok"):
        raise AssertionError(f"{method} failed: {body.get('error')}")
    return body["result"]


def health(base_url: str) -> dict[str, Any]:
    return http_json(f"{base_url}/api/v1/health")


def wait_until(predicate: Callable[[], bool], timeout: float, description: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, urllib.error.URLError):
            # A daemon launched by cargo may refuse connections while compiling.
            pass
        time.sleep(0.2)
    raise AssertionError(f"timed out waiting for {description}")


class Daemon:
    def __init__(self, command: str | None, home: Path | None, url: str, password: str) -> None:
        self.command = command
        self.home = home
        self.url = url
        self.password = password
        self.process: subprocess.Popen[bytes] | None = None

    def start(self) -> None:
        if self.command is None:
            wait_until(lambda: health(self.url).get("ok") is True, 15, "existing macbotd")
            return
        env = os.environ.copy()
        if self.home is not None:
            env["MACBOT_HOME"] = str(self.home)
        self.process = subprocess.Popen(
            shlex.split(self.command),
            cwd=REPO,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        wait_until(lambda: health(self.url).get("ok") is True, 30, "macbotd startup")

    def _kill_group(self, sig: signal.Signals) -> None:
        if self.process is None or self.process.poll() is not None:
            return
        try:
            os.killpg(self.process.pid, sig)
        except ProcessLookupError:
            pass
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)

    def restart(self) -> None:
        if self.command is None:
            raise AssertionError("restart acceptance requires --daemon-command")
        self._kill_group(signal.SIGKILL)
        time.sleep(0.5)
        self.start()

    def close(self) -> None:
        self._kill_group(signal.SIGTERM)


async def receive(ws: Any, predicate: Callable[[dict[str, Any]], bool], timeout: float = 15) -> dict[str, Any]:
    async def matching() -> dict[str, Any]:
        while True:
            raw = await ws.recv()
            if not isinstance(raw, str):
                continue
            value = json.loads(raw)
            if predicate(value):
                return value

    return await asyncio.wait_for(matching(), timeout)


async def ws_request(ws: Any, method: str, params: dict[str, Any]) -> Any:
    request_id = str(uuid.uuid4())
    await ws.send(json.dumps({"v": 1, "kind": "req", "id": request_id, "method": method, "params": params}))
    frame = await receive(ws, lambda value: value.get("kind") == "res" and value.get("id") == request_id)
    if not frame.get("ok"):
        raise AssertionError(f"WebSocket {method} failed: {frame.get('error')}")
    return frame["result"]


def value_contains(value: Any, text: str) -> bool:
    return text in json.dumps(value, ensure_ascii=False)


def assert_usage(base_url: str, password: str, home: Path | None) -> None:
    try:
        result = rpc(
            base_url,
            password,
            "usage.summary",
            {"from": "2000-01-01T00:00:00Z", "to": "2999-01-01T00:00:00Z"},
        )
        current = result.get("current", {})
        assert current.get("requests", 0) >= 2, result
        assert current.get("input_tokens", 0) > 0, result
        assert current.get("output_tokens", 0) > 0, result
        assert current.get("cost", 0) > 0, result
        return
    except AssertionError:
        # The current formal RPC surface does not expose usage.summary yet;
        # inspect the same durable ledger that the runtime writes.
        if home is None:
            raise
        rows = []
        for path in (home / "data" / "usage" / "raw").glob("*.jsonl"):
            rows.extend(json.loads(line) for line in path.read_text().splitlines() if line.strip())
        assert sum(row.get("requests", 0) for row in rows) >= 2, rows
        assert sum(row.get("input_tokens", 0) for row in rows) > 0, rows
        assert sum(row.get("output_tokens", 0) for row in rows) > 0, rows
        assert any((row.get("cost") or 0) > 0 for row in rows), rows


async def acceptance(args: argparse.Namespace, fake_url: str, fake: FakeProviderState) -> None:
    import websockets

    base_url = args.url.rstrip("/")
    password = args.password
    assert health(base_url).get("protocol") == 1
    bootstrap = rpc(base_url, password, "bootstrap")
    main = next(bot for bot in bootstrap["bots"] if bot["is_main"])
    main_chats = [chat for chat in bootstrap["chats"] if chat["kind"] == "main"]
    assert len(main_chats) == 1 and main_chats[0]["id"] == main["dm_chat_id"]
    assert main["dm_chat_id"] == "chat_main"

    created = rpc(
        base_url,
        password,
        "provider.create",
        {
            "name": "local-fake-runtime",
            "api_kind": "openai-completions",
            "base_url": fake_url,
            "api_key": DUMMY_TOKEN,
            "client_request_id": "runtime-provider-create",
        },
    )
    provider_id = created["provider"]["id"]
    args.cleanup_provider_id = provider_id
    assert DUMMY_TOKEN not in json.dumps(created)
    listed = rpc(base_url, password, "provider.list")
    assert DUMMY_TOKEN not in json.dumps(listed)
    provider = next(item for item in listed["providers"] if item["id"] == provider_id)
    assert provider["has_key"] is True

    refreshed = rpc(
        base_url,
        password,
        "model.refresh",
        {"provider_id": provider_id, "client_request_id": "runtime-model-refresh"},
    )
    assert any(item["model_id"] == "fake-runtime" for item in refreshed["models"])
    tested = rpc(
        base_url,
        password,
        "provider.test",
        {"provider_id": provider_id, "model_id": "fake-runtime"},
    )
    assert tested["ok"], tested
    model = rpc(
        base_url,
        password,
        "model.upsert",
        {
            "provider_id": provider_id,
            "model_id": "fake-runtime",
            "display_name": "Local fake model",
            "caps": {"vision": False, "tools": True, "reasoning": False},
            "price": {
                "input_per_mtok": 1.0,
                "output_per_mtok": 2.0,
                "cache_read_per_mtok": 0.1,
                "cache_write_per_mtok": 0.2,
            },
            "client_request_id": "runtime-model-upsert",
        },
    )
    model_ref = model["model"]["ref"]
    assert model_ref == f"{provider_id}/fake-runtime"

    worker_result = rpc(
        base_url,
        password,
        "bot.create",
        {
            "name": "runtime-fake-worker",
            "model": model_ref,
            "tools": {"files": True, "bash": True, "browser": False, "subagent": False, "web": False, "mcp": False},
            "client_request_id": "runtime-worker-create",
        },
    )
    worker = worker_result["bot"]
    assert worker_result["dm_chat"]["id"] == worker["dm_chat_id"]
    assert worker_result["dm_chat"]["kind"] == "direct"
    dm_chat = rpc(base_url, password, "chat.get", {"chat_id": worker["dm_chat_id"]})["chat"]
    assert dm_chat["kind"] == "direct" and dm_chat["bot_id"] == worker["id"]
    refreshed_bootstrap = rpc(base_url, password, "bootstrap")
    for bot in refreshed_bootstrap["bots"]:
        matching = [chat for chat in refreshed_bootstrap["chats"] if chat["id"] == bot["dm_chat_id"]]
        assert len(matching) == 1, (bot, matching)
        assert matching[0]["kind"] == ("main" if bot["is_main"] else "direct")
    project = rpc(
        base_url,
        password,
        "project.create",
        {
            "name": "runtime-fake-project",
            "goal": "Exercise production execution",
            "member_bot_ids": [worker["id"]],
            "client_request_id": "runtime-project-create",
        },
    )
    chat_id = project["chat"]["id"]

    ws_url = base_url.replace("http://", "ws://").replace("https://", "wss://")
    async with websockets.connect(
        ws_url + "/ws",
        additional_headers={"Authorization": f"Bearer {password}"},
        proxy=None,
    ) as ws:
        hello = json.loads(await ws.recv())
        assert hello["event"] == "hello"
        await ws_request(ws, "session.resume", {"last_seq": 0, "client": {"platform": "smoke", "app_version": "test", "device_name": "runtime", "device_id": "runtime-smoke"}})
        await receive(ws, lambda value: value.get("event") == "sync.done")
        chat_params = {
            "chat_id": chat_id,
            "text": "run the fake provider tool workflow",
            "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "exercise send_msg"}],
            "client_request_id": "runtime-chat-send",
        }
        sent = rpc(base_url, password, "chat.send", chat_params)
        replayed = rpc(base_url, password, "chat.send", chat_params)
        assert replayed == sent, "chat.send client_request_id was not idempotent"
        assert sent["message"]["sender"]["kind"] == "user"
        # The RPC response is the admission point.  Do not wait for the live
        # event here: this keeps the steer assertion inside the model's first
        # provider request window after a cold daemon start.

    assignments = rpc(base_url, password, "assignment.list", {})["items"]
    assignment = next(item for item in assignments if item.get("origin_chat_id") == chat_id)
    assignment_id = assignment["id"]
    wait_until(
        lambda: next(item for item in rpc(base_url, password, "assignment.list", {})["items"] if item["id"] == assignment_id)["status"] == "working",
        10,
        "assignment starts before steer",
    )
    steer = rpc(
        base_url,
        password,
        "assignment.steer",
        {
            "bot_id": worker["id"],
            "project_id": assignment.get("project_id"),
            "chat_id": chat_id,
            "text": "只做邮箱登录，不要手机号",
            "message_id": "runtime-steer-1",
            "client_request_id": "runtime-steer-1",
        },
    )
    assert steer["state"] in {"queued", "delivered"}, steer

    def work_started() -> bool:
        requests, _ = fake.snapshot()
        return len(requests) >= 3  # provider.test + two execution turns

    try:
        wait_until(work_started, 30, "fake provider execution calls")
    except AssertionError as error:
        current = rpc(base_url, password, "assignment.list", {})["items"]
        raise AssertionError(f"{error}; assignments={current}") from error
    requests, auth_headers = fake.snapshot()
    assert all(token == f"Bearer {DUMMY_TOKEN}" for token in auth_headers)
    assert all(DUMMY_TOKEN not in json.dumps(request) for request in requests)

    wait_until(
        lambda: next(
            item for item in rpc(base_url, password, "assignment.list", {})["items"] if item["id"] == assignment_id
        )["status"] == "done",
        30,
        "group assignment completion",
    )
    def steer_applied() -> bool:
        current = next(item for item in rpc(base_url, password, "assignment.list", {})["items"] if item["id"] == assignment_id)
        return any(item.get("message_id") == "runtime-steer-1" and item.get("applied_at") for item in current.get("steers", []))

    wait_until(steer_applied, 30, "steer read/applied_at persistence")
    history = rpc(base_url, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 100})["messages"]
    bot_messages = [
        message
        for message in history
        if message.get("sender", {}).get("kind") == "bot"
        and message.get("assignment_id") == assignment_id
    ]
    assert [message.get("intent") for message in bot_messages] == ["progress", "done"], history
    assert not any(message.get("streaming") for message in bot_messages)
    assert value_contains(bot_messages, "fake provider progress")
    assert value_contains(bot_messages, "fake provider completed")
    assert len(bot_messages) == 2, "send_msg replay produced duplicate group messages"

    # Two independent projects for the same worker may run concurrently. The
    # worker has max_parallel=3; both assignments must become working/done
    # rather than serializing behind one project.
    concurrent_projects = [
        rpc(
            base_url,
            password,
            "project.create",
            {"name": f"runtime-concurrent-{index}", "goal": "parallel scheduling", "member_bot_ids": [worker["id"]], "client_request_id": f"runtime-concurrent-project-{index}"},
        )
        for index in (1, 2)
    ]
    concurrent_assignments = []
    for index, created_project in enumerate(concurrent_projects, 1):
        result = rpc(
            base_url,
            password,
            "chat.send",
            {
                "chat_id": created_project["chat"]["id"],
                "text": f"parallel task {index}",
                "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": f"parallel task {index}"}],
                "client_request_id": f"runtime-concurrent-chat-{index}",
            },
        )
        concurrent_assignments.append(result["message"]["id"])
    wait_until(
        lambda: sum(
            item.get("status") in {"working", "done"}
            for item in rpc(base_url, password, "assignment.list", {})["items"]
            if item.get("instruction") in {"parallel task 1", "parallel task 2"}
        ) >= 2,
        15,
        "two concurrent assignments working",
    )
    parallel_items = [
        item for item in rpc(base_url, password, "assignment.list", {})["items"]
        if item.get("instruction") in {"parallel task 1", "parallel task 2"}
    ]
    assert len(parallel_items) == 2, parallel_items
    assert parallel_items[0].get("started_at") and parallel_items[1].get("started_at"), parallel_items

    trace = rpc(base_url, password, "trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500})
    items = trace["items"]
    assert items and [item["aseq"] for item in items] == sorted({item["aseq"] for item in items})
    first_aseq = items[0]["aseq"]
    tail = rpc(base_url, password, "trace.history", {"assignment_id": assignment_id, "after_aseq": first_aseq, "limit": 500})
    assert all(item["aseq"] > first_aseq for item in tail["items"])
    assert {item["type"] for item in items} >= {"run.start", "llm.request", "llm.response", "tool.start", "send_msg", "run.end"}
    assert_usage(base_url, password, args.home)

    # A private bot DM must stream deltas into its chat and use the same
    # provider/usage path as a group assignment.  The response marker keeps
    # the fake provider deterministic without a real model.
    bot_chats = rpc(base_url, password, "chat.list", {})["chats"]
    private_chat = next(item for item in bot_chats if item.get("kind") == "direct" and item.get("bot_id") == worker["id"])
    private_sent = rpc(
        base_url,
        password,
        "chat.send",
        {
            "chat_id": private_chat["id"],
            "text": "PRIVATE_RUNTIME",
            "mentions": [],
            "client_request_id": "runtime-private-chat",
        },
    )
    wait_until(
        lambda: any(
            message.get("sender", {}).get("kind") == "bot"
            and "private fake reply" in message.get("fallback_text", "")
            for message in rpc(base_url, password, "chat.history", {"chat_id": private_chat["id"], "after_seq": 0, "limit": 100})["messages"]
        ),
        30,
        "private streaming response",
    )
    private_history = rpc(base_url, password, "chat.history", {"chat_id": private_chat["id"], "after_seq": 0, "limit": 100})["messages"]
    assert any(message.get("streaming") is False for message in private_history if message.get("sender", {}).get("kind") == "bot")

    # The main Bot is coordination-only: its model-facing schemas must not
    # expose file mutation or shell execution even when its persisted Bot
    # configuration contains broad defaults.
    rpc(
        base_url,
        password,
        "bot.update",
        {
            "bot_id": main["id"],
            "patch": {"model": model_ref},
            "client_request_id": "runtime-main-model",
        },
    )
    rpc(
        base_url,
        password,
        "chat.send",
        {"chat_id": "chat_main", "text": "MAIN_TOOL_SET", "mentions": [], "client_request_id": "runtime-main-tools"},
    )
    wait_until(
        lambda: any(item.get("type") == "run.end" for item in rpc(base_url, password, "trace.history", {"chat_id": "chat_main", "limit": 100})["items"]),
        30,
        "main tool-set trace",
    )
    main_trace = rpc(base_url, password, "trace.history", {"chat_id": "chat_main", "limit": 100})["items"]
    main_requests = [item for item in main_trace if item.get("type") == "llm.request"]
    provider_requests, _ = fake.snapshot()
    main_provider_request = next(
        request
        for request in reversed(provider_requests)
        if "MAIN_TOOL_SET" in json.dumps(request.get("messages", []), ensure_ascii=False)
    )
    main_schema_names = {
        tool.get("function", {}).get("name")
        for tool in main_provider_request.get("tools", [])
    }
    assert main_requests and not main_schema_names.intersection({"write", "edit", "bash", "bash_job"}), main_provider_request

    # Exercise the complete private file/command path. Each unsafe operation
    # must be approved, and the resulting trace must include both tool start
    # and tool end records with the real command output.
    rpc(
        base_url,
        password,
        "chat.send",
        {
            "chat_id": private_chat["id"],
            "text": "PRIVATE_TOOL_WORKFLOW",
            "mentions": [],
            "client_request_id": "runtime-private-tools",
        },
    )

    def approve_private_tools() -> bool:
        for approval in rpc(base_url, password, "approval.list", {}).get("approvals", []):
            if approval.get("state") == "pending":
                rpc(
                    base_url,
                    password,
                    "approval.decide",
                    {"approval_id": approval["id"], "decision": "allow_once"},
                )
        history = rpc(base_url, password, "chat.history", {"chat_id": private_chat["id"], "after_seq": 0, "limit": 200})["messages"]
        return any("private tool workflow complete" in message.get("fallback_text", "") for message in history)

    wait_until(approve_private_tools, 60, "private file/bash workflow")
    tool_path = args.home / "bots" / worker["id"] / "s1-runtime-tool.txt"
    assert tool_path.read_text() == "edit-sentinel", tool_path
    private_trace = rpc(base_url, password, "trace.history", {"chat_id": private_chat["id"], "limit": 500})["items"]
    starts = {item.get("data", {}).get("name") for item in private_trace if item.get("type") == "tool.start"}
    assert {"write", "read", "edit", "bash", "bash_job"} <= starts, starts
    assert sum(item.get("type") == "tool.end" for item in private_trace) >= 5
    assert value_contains(private_trace, "bash-sentinel")
    assert value_contains(private_trace, f"/bots/{worker['id']}")

    # A second run asks for an unsafe bash call. It must checkpoint before the
    # side effect, survive a process restart, and continue only after approval.
    unsafe_project = rpc(
        base_url,
        password,
        "project.create",
        {"name": "runtime-approval-project", "goal": "unsafe recovery", "member_bot_ids": [worker["id"]], "client_request_id": "runtime-approval-project"},
    )
    unsafe_chat = unsafe_project["chat"]["id"]
    rpc(
        base_url,
        password,
        "chat.send",
        {
            "chat_id": unsafe_chat,
            "text": "UNSAFE_APPROVAL",
            "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "run unsafe command"}],
            "client_request_id": "runtime-approval-chat",
        },
    )
    wait_until(lambda: bool(rpc(base_url, password, "approval.list", {})["approvals"]), 30, "pending unsafe approval")
    before_restart = rpc(base_url, password, "approval.list", {})["approvals"]
    if args.no_restart:
        raise AssertionError("approval restart check was disabled with --no-restart")
    args.daemon.restart()
    replay_after_restart = rpc(base_url, password, "chat.send", chat_params)
    assert replay_after_restart == sent, "client_request_id replay changed after SIGKILL restart"
    after_restart = rpc(base_url, password, "approval.list", {})["approvals"]
    assert {item["id"] for item in after_restart} >= {item["id"] for item in before_restart}
    approval = after_restart[0]
    resolved = rpc(base_url, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once"})
    assert resolved["approval"]["state"] in {"resolved", "allowed_once"}, resolved
    wait_until(lambda: len(fake.snapshot()[0]) >= len(requests) + 1, 30, "approved run resume")
    print("S1 runtime acceptance passed: fake provider, model registry, tool/send_msg, trace cursor, durable approval restart, group routing, usage")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7791")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command", required=True, help="command used to start macbotd; required for restart acceptance")
    parser.add_argument("--home", type=Path, required=True, help="isolated MACBOT_HOME for --daemon-command")
    parser.add_argument("--no-restart", action="store_true", help="reserved for diagnosis; complete acceptance rejects this mode")
    args = parser.parse_args()
    if args.no_restart:
        parser.error("complete runtime acceptance requires restart; omit --no-restart")
    if args.home.resolve().is_relative_to(REPO.resolve()):
        parser.error("--home must be outside the repository")
    secret_dir = Path("/tmp") / f"macbot-runtime-secrets-{os.getpid()}"
    shutil.rmtree(secret_dir, ignore_errors=True)
    secret_dir.mkdir(mode=0o700, parents=True)
    args.secret_dir = secret_dir
    args.home.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.environ["MACBOT_SECRET_BACKEND"] = "file"
    os.environ["MACBOT_SECRET_DIR"] = str(secret_dir)
    args.daemon = Daemon(args.daemon_command, args.home, args.url.rstrip("/"), args.password)
    fake_server, fake_state, fake_url = start_fake_provider()
    try:
        args.daemon.start()
        asyncio.run(acceptance(args, fake_url, fake_state))
    finally:
        args.daemon.close()
        fake_server.shutdown()
        fake_server.server_close()
        shutil.rmtree(args.secret_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
