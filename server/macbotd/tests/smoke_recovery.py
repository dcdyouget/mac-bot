#!/usr/bin/env python3
"""Durable safe/unsafe recovery acceptance for macbotd.

The daemon, scheduler, execution engine, and provider registry are real.  A
local OpenAI-compatible provider is used so the test never needs a product
credential.  The safe request is killed while the provider call is in flight;
after restart the daemon must resume the same durable run automatically.  The
unsafe request is left suspended at its approval checkpoint and must not
replay the shell side effect during restart.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, rpc, wait_until


TOKEN = "macbot-safe-recovery-fake-token"


class RecoveryProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.safe_calls = 0
        self.unsafe_calls = 0

    def record(self, body: dict[str, Any]) -> tuple[int, int]:
        prompt = json.dumps(body.get("messages", []), ensure_ascii=False)
        with self.lock:
            self.requests.append(body)
            if "UNSAFE_RECOVERY" in prompt:
                self.unsafe_calls += 1
            elif "SAFE_RECOVERY" in prompt:
                self.safe_calls += 1
            return self.safe_calls, self.unsafe_calls

    def snapshot(self) -> tuple[list[dict[str, Any]], int, int]:
        with self.lock:
            return list(self.requests), self.safe_calls, self.unsafe_calls


class RecoveryProviderHandler(BaseHTTPRequestHandler):
    state: RecoveryProviderState
    unsafe_sentinel = "/tmp/macbot-unsafe-recovery-sentinel"

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        body = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _stream(self, payloads: list[dict[str, Any]]) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            self.wfile.write(f"data: {json.dumps(payload, ensure_ascii=False)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _text(self, text: str) -> None:
        self._stream([
            {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 8, "completion_tokens": 4}},
        ])

    def _tool(self, name: str, args: dict[str, Any]) -> None:
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": f"recovery-call-{uuid.uuid4().hex[:10]}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 8, "completion_tokens": 4}},
        ])

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self.headers.get("Authorization") == f"Bearer {TOKEN}":
            self._json(200, {"data": [{"id": "safe-recovery", "object": "model"}]})
            return
        self._json(401, {"error": {"message": "unauthorized"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "unauthorized"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        messages = body.get("messages", [])
        prompt = json.dumps(messages, ensure_ascii=False)
        latest_user = next(
            (message.get("content", "") for message in reversed(messages) if message.get("role") == "user"),
            "",
        )
        safe_calls, _unsafe_calls = self.state.record(body)
        if "UNSAFE_RECOVERY" in latest_user:
            self._tool("bash", {"command": f"touch {type(self).unsafe_sentinel}", "background": False})
            return
        if "SAFE_RECOVERY" in latest_user:
            if safe_calls == 1:
                # The client is killed during this request.  A second request
                # after restart gets a deterministic response immediately.
                time.sleep(8)
            self._text("safe recovery completed")
            return
        self._text("recovery smoke completed")


def start_provider() -> tuple[ThreadingHTTPServer, RecoveryProviderState, str]:
    state = RecoveryProviderState()
    RecoveryProviderHandler.state = state
    server = ThreadingHTTPServer(("127.0.0.1", 0), RecoveryProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, state, f"http://127.0.0.1:{server.server_port}/v1"


class RestartDaemon(Daemon):
    def restart(self) -> None:
        if self.process is None:
            raise AssertionError("safe recovery requires --daemon-command")
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=10)
        time.sleep(0.5)
        self.start()


def latest_job(home: Path, run_id: str) -> dict[str, Any] | None:
    candidates: list[dict[str, Any]] = []
    for path in (home / "data" / "jobs").glob("*.json"):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if value.get("checkpoint", {}).get("run_id") == run_id:
            candidates.append(value)
    return max(candidates, key=lambda item: item.get("commit_seq", 0), default=None)


def assignment_for_dm(base: str, password: str, chat_id: str, marker: str) -> dict[str, Any]:
    items = rpc(base, password, "assignment.list", {"limit": 200})["items"]
    matches = [item for item in items if item.get("origin_chat_id") == chat_id and marker in item.get("instruction", "")]
    if not matches:
        raise LookupError(marker)
    return matches[0]


def trace(base: str, password: str, assignment_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500})["items"]


def configure(base: str, password: str, provider_url: str, suffix: str) -> dict[str, Any]:
    provider = rpc(base, password, "provider.create", {"name": f"safe-recovery-{suffix}", "api_kind": "openai-completions", "base_url": provider_url, "api_key": TOKEN, "client_request_id": f"recovery-provider:{suffix}"})["provider"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider["id"]})
    assert any(item["model_id"] == "safe-recovery" for item in refreshed["models"]), refreshed
    model = rpc(base, password, "model.upsert", {"provider_id": provider["id"], "model_id": "safe-recovery", "display_name": "safe recovery fake", "caps": {"vision": False, "tools": True, "reasoning": False}, "client_request_id": f"recovery-model:{suffix}"})["model"]["ref"]
    worker = rpc(base, password, "bot.create", {"name": f"safe-recovery-worker-{suffix}", "model": model, "max_parallel": 1, "tools": {"files": False, "bash": True, "browser": False, "subagent": False, "web": False, "mcp": False}})["bot"]
    return {"provider": provider, "model": model, "worker": worker}


def safe_recovery(base: str, password: str, home: Path, daemon: RestartDaemon, provider: RecoveryProviderState, worker: dict[str, Any], suffix: str) -> dict[str, Any]:
    dm_chat = worker["dm_chat_id"]
    rpc(base, password, "chat.send", {"chat_id": dm_chat, "text": "SAFE_RECOVERY", "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "SAFE_RECOVERY"}], "client_request_id": f"safe-recovery-chat:{suffix}"})
    assignment: dict[str, Any] = {}

    def find_safe_assignment() -> bool:
        try:
            assignment.update(assignment_for_dm(base, password, dm_chat, "SAFE_RECOVERY"))
        except LookupError:
            return False
        return True

    wait_until(find_safe_assignment, "safe DM assignment", 30)
    assignment_id = assignment["id"]
    run_id: str | None = None
    checkpoint: dict[str, Any] | None = None

    def checkpoint_ready() -> bool:
        nonlocal run_id, checkpoint
        items = trace(base, password, assignment_id)
        starts = [item for item in items if item.get("type") == "run.start"]
        if not starts:
            return False
        run_id = starts[0].get("run_id")
        if not run_id:
            return False
        checkpoint = latest_job(home, run_id)
        return bool(checkpoint and checkpoint.get("unsafe_replay") is False and checkpoint.get("status") in {"queued", "running"})

    wait_until(lambda: provider.snapshot()[1] >= 1, "safe provider request in flight", 30)
    wait_until(checkpoint_ready, "safe durable checkpoint", 30)
    assert run_id and checkpoint
    calls_before_restart = len(provider.snapshot()[0])
    starts_before = [item for item in trace(base, password, assignment_id) if item.get("type") == "run.start"]
    daemon.restart()
    wait_until(lambda: provider.snapshot()[1] >= 2, "safe provider call after restart", 30)
    wait_until(lambda: assignment_for_dm(base, password, dm_chat, "SAFE_RECOVERY").get("status") == "done", "safe assignment completion", 40)
    items = trace(base, password, assignment_id)
    assert sum(item.get("type") == "run.start" for item in items) == len(starts_before) == 1, items
    assert sum(item.get("type") == "run.end" and item.get("data", {}).get("status") == "done" for item in items) == 1, items
    assert any(item.get("type") == "run.resume" and item.get("run_id") == run_id for item in items), items
    history = rpc(base, password, "chat.history", {"chat_id": dm_chat, "limit": 100})["messages"]
    final = [item for item in history if item.get("sender", {}).get("kind") == "bot" and "safe recovery completed" in item.get("fallback_text", "")]
    assert len(final) == 1, history
    assert len(provider.snapshot()[0]) > calls_before_restart
    return {"assignment_id": assignment_id, "run_id": run_id, "provider_calls": len(provider.snapshot()[0]), "final_messages": len(final), "checkpoint_unsafe_replay": checkpoint["unsafe_replay"]}


def unsafe_guard(base: str, password: str, home: Path, daemon: RestartDaemon, provider: RecoveryProviderState, worker: dict[str, Any], suffix: str) -> dict[str, Any]:
    dm_chat = worker["dm_chat_id"]
    sentinel = home / "unsafe-side-effect-sentinel"
    rpc(base, password, "chat.send", {"chat_id": dm_chat, "text": "UNSAFE_RECOVERY", "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "UNSAFE_RECOVERY"}], "client_request_id": f"unsafe-recovery-chat:{suffix}"})
    assignment: dict[str, Any] = {}

    def find_unsafe_assignment() -> bool:
        try:
            assignment.update(assignment_for_dm(base, password, dm_chat, "UNSAFE_RECOVERY"))
        except LookupError:
            return False
        return True

    wait_until(find_unsafe_assignment, "unsafe DM assignment", 30)
    assignment_id = assignment["id"]
    wait_until(lambda: bool(rpc(base, password, "approval.list", {"state": ["pending"]}).get("approvals")), "unsafe approval checkpoint", 30)
    before_calls = len(provider.snapshot()[0])
    items = trace(base, password, assignment_id)
    run_start = next(item for item in items if item.get("type") == "run.start")
    run_id = run_start["run_id"]
    job = latest_job(home, run_id)
    assert job and job.get("unsafe_replay") is True, job
    daemon.restart()
    time.sleep(2)
    after_calls = len(provider.snapshot()[0])
    assert after_calls == before_calls, (before_calls, after_calls)
    assert not sentinel.exists(), "unsafe approval side effect replayed after restart"
    after_trace = trace(base, password, assignment_id)
    assert not any(item.get("type") == "run.end" and item.get("data", {}).get("status") == "done" for item in after_trace), after_trace
    pending = rpc(base, password, "approval.list", {"state": ["pending"]})["approvals"]
    assert pending, "unsafe checkpoint lost its approval card after restart"
    return {"assignment_id": assignment_id, "run_id": run_id, "provider_calls": after_calls, "pending_approvals": len(pending), "checkpoint_unsafe_replay": True}


def acceptance(args: argparse.Namespace) -> None:
    provider_server, provider, provider_url = start_provider()
    RecoveryProviderHandler.unsafe_sentinel = str(args.home / "unsafe-side-effect-sentinel")
    daemon = RestartDaemon(args)
    try:
        daemon.start()
        suffix = uuid.uuid4().hex[:8]
        configured = configure(args.url.rstrip("/"), args.password, provider_url, suffix)
        safe = safe_recovery(args.url.rstrip("/"), args.password, args.home, daemon, provider, configured["worker"], suffix)
        unsafe = unsafe_guard(args.url.rstrip("/"), args.password, args.home, daemon, provider, configured["worker"], suffix)
        print(json.dumps({"ok": True, "safe": safe, "unsafe": unsafe, "home": str(args.home), "port": args.url.rsplit(":", 1)[-1]}, ensure_ascii=False))
    finally:
        daemon.close()
        provider_server.shutdown()
        provider_server.server_close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7791")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--browser-bin")
    args = parser.parse_args()
    if args.home.exists():
        shutil.rmtree(args.home)
    acceptance(args)


if __name__ == "__main__":
    main()
