#!/usr/bin/env python3
"""Focused automatic-dispatch acceptance for macbotd.

The provider chooses a script from the current Bot/instruction marker and
never uses a process-global turn cursor.  This keeps scheduler assertions
independent from request ordering.

Example:
  python server/macbotd/tests/smoke_dispatch.py \
    --daemon-command 'server/target/debug/macbotd --port 7792 --password dev' \
    --home /tmp/macbot-dispatch-smoke
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import shlex
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, TOKEN, http_json, rpc, wait_until


class ScriptState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.active: set[str] = set()
        self.overlap: set[str] = set()
        self.calls = 0

    def record(self, marker: str, body: dict[str, Any]) -> None:
        with self.lock:
            self.calls += 1
            self.requests.append({"marker": marker, "body": body})
            if marker in {"QUEUE_A", "QUEUE_B", "SERIAL_A", "SERIAL_B", "PAR_A", "PAR_B"}:
                if self.active:
                    self.overlap.update(self.active)
                    self.overlap.add(marker)
                self.active.add(marker)

    def finish(self, marker: str) -> None:
        with self.lock:
            self.active.discard(marker)


class DispatchProvider(BaseHTTPRequestHandler):
    state = ScriptState()
    tester_id = ""
    coder_id = ""
    model_name = "dispatch-fake"
    stop_path = ""

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

    def _text(self, text: str = "done") -> None:
        self._stream([
            {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 4, "completion_tokens": 2}},
        ])

    def _tool(self, name: str, args: dict[str, Any]) -> None:
        with self.state.lock:
            call_id = f"dispatch-call-{self.state.calls}"
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 5, "completion_tokens": 3}},
        ])

    @staticmethod
    def _prompt(messages: list[dict[str, Any]]) -> str:
        return json.dumps(messages, ensure_ascii=False)

    @staticmethod
    def _called(messages: list[dict[str, Any]]) -> set[str]:
        result: set[str] = set()
        for message in messages:
            for call in message.get("tool_calls", []):
                name = call.get("function", {}).get("name")
                if isinstance(name, str):
                    result.add(name)
        return result

    @staticmethod
    def _project_id(messages: list[dict[str, Any]]) -> str | None:
        for message in reversed(messages):
            if message.get("role") != "tool":
                continue
            try:
                value = json.loads(message.get("content", ""))
            except (TypeError, json.JSONDecodeError):
                continue
            for key in ("project", "result"):
                candidate = value.get(key) if isinstance(value, dict) else None
                if isinstance(candidate, dict) and isinstance(candidate.get("id"), str):
                    return candidate["id"]
        return None

    def _script(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        prompt = self._prompt(messages)
        called = self._called(messages)
        latest = next(
            (message.get("content", "") for message in reversed(messages) if message.get("role") == "user"),
            "",
        )
        is_main = any(message.get("role") == "system" and "\n总管\n" in message.get("content", "") for message in messages)
        project_id = self._project_id(messages)

        if "DISPATCH_MAIN" in latest and is_main:
            if "create_project" not in called:
                return "create_project", {"name": "dispatch-project", "goal": "automatic dispatch", "member_bot_ids": [self.coder_id, self.tester_id], "flow": ["编码", "测试"]}
            if "assign" not in called:
                return "assign", {"bot_id": self.coder_id, "project_id": project_id, "title": "编码", "instruction": "CODER_DONE"}
            return None
        if "CODER_DONE" in latest and "send_msg" not in called:
            return "send_msg", {"intent": "done", "text": "编码完成", "mentions": [{"bot_id": self.tester_id, "instruction": "TESTER_DONE"}]}
        if "TESTER_DONE" in latest and "send_msg" not in called:
            return "send_msg", {"intent": "done", "text": "测试完成", "mentions": []}
        if "WAIT_DECISION" in latest or ("WAIT_DECISION" in prompt and "用户继续" in prompt):
            if "用户继续" in prompt and "send_msg" in called:
                return "send_msg", {"intent": "done", "text": "等待任务已完成", "mentions": []}
            if "send_msg" not in called:
                return "send_msg", {"intent": "decision", "text": "需要用户确认", "options": ["继续", "停止"]}
            return None
        if "STOP_BASH" in latest and "bash" not in called:
            return "bash", {"command": f"sleep 20; touch {self.stop_path}", "background": False}
        if any(marker in latest for marker in ("QUEUE_A", "QUEUE_B", "SERIAL_A", "SERIAL_B", "PAR_A", "PAR_B")):
            marker = next(marker for marker in ("QUEUE_A", "QUEUE_B", "SERIAL_A", "SERIAL_B", "PAR_A", "PAR_B") if marker in latest)
            self.state.record(marker, {"messages": messages})
            time.sleep(4.0)
            self.state.finish(marker)
            return None
        return None

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self.headers.get("Authorization") == f"Bearer {TOKEN}":
            self._json(200, {"data": [{"id": self.model_name, "object": "model"}]})
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
        prompt = self._prompt(messages)
        scripted = self._script(messages)
        if scripted is not None:
            self._tool(*scripted)
        else:
            self._text("dispatch complete")


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), DispatchProvider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def assignments(base: str, password: str) -> list[dict[str, Any]]:
    return rpc(base, password, "assignment.list", {"limit": 200})["items"]


def assignment_for(base: str, password: str, instruction: str) -> dict[str, Any]:
    return next(item for item in assignments(base, password) if item.get("instruction") == instruction)


def traces(base: str, password: str, assignment_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"assignment_id": assignment_id, "limit": 500})["items"]


def settle_approvals(base: str, password: str) -> None:
    pending = rpc(base, password, "bootstrap")["pending"]
    for approval in pending["approvals"]:
        if approval.get("state") == "pending":
            rpc(base, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once"})


def wait_status(base: str, password: str, instruction: str, statuses: set[str], timeout: float = 30) -> dict[str, Any]:
    found: list[dict[str, Any]] = []
    def ready() -> bool:
        settle_approvals(base, password)
        try:
            item = assignment_for(base, password, instruction)
        except StopIteration:
            return False
        found[:] = [item]
        return item.get("status") in statuses
    wait_until(ready, f"{instruction} status {statuses}", timeout)
    return found[0]


def setup(base: str, password: str, provider_url: str, suffix: str) -> tuple[str, dict[str, Any], dict[str, Any], dict[str, Any]]:
    provider = rpc(base, password, "provider.create", {"name": f"dispatch-{suffix}", "api_kind": "openai-completions", "base_url": provider_url, "api_key": TOKEN, "client_request_id": f"dispatch-provider-{suffix}"})["provider"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider["id"]})
    assert any(model["model_id"] == "dispatch-fake" for model in refreshed["models"])
    model = rpc(base, password, "model.upsert", {"provider_id": provider["id"], "model_id": "dispatch-fake", "display_name": "dispatch fake", "caps": {"vision": False, "tools": True, "reasoning": False}, "client_request_id": f"dispatch-model-{suffix}"})["model"]["ref"]
    rpc(base, password, "bot.update", {"bot_id": "main", "patch": {"model": model}, "client_request_id": f"dispatch-main-{suffix}"})
    coder = rpc(base, password, "bot.create", {"name": f"dispatch-coder-{suffix}", "model": model, "max_parallel": 1, "tools": {"files": True, "bash": True, "browser": False, "subagent": False, "web": False, "mcp": False}})["bot"]
    tester = rpc(base, password, "bot.create", {"name": f"dispatch-tester-{suffix}", "model": model, "max_parallel": 1, "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False}})["bot"]
    return model, coder, tester, provider


def acceptance(args: argparse.Namespace) -> None:
    base = args.url.rstrip("/")
    provider, provider_url = start_provider()
    daemon = Daemon(args)
    try:
        daemon.start()
        assert http_json(f"{base}/api/v1/health").get("protocol") == 1
        suffix = uuid.uuid4().hex[:8]
        model, coder, tester, _ = setup(base, args.password, provider_url, suffix)
        DispatchProvider.coder_id = coder["id"]
        DispatchProvider.tester_id = tester["id"]
        DispatchProvider.stop_path = str(args.home / "stop-sentinel")

        # Main assign -> coder done/send_msg mention -> tester done.
        project = rpc(base, args.password, "project.create", {"name": f"dispatch-main-{suffix}", "goal": "dispatch", "member_bot_ids": [coder["id"], tester["id"]]})
        rpc(base, args.password, "chat.send", {"chat_id": "chat_main", "text": "DISPATCH_MAIN", "mentions": [{"kind": "main"}], "client_request_id": f"dispatch-main-chat-{suffix}"})
        wait_status(base, args.password, "CODER_DONE", {"working", "done"}, 30)
        wait_status(base, args.password, "TESTER_DONE", {"working", "done"}, 30)
        wait_until(lambda: all(item.get("status") == "done" for item in assignments(base, args.password) if item.get("instruction") in {"CODER_DONE", "TESTER_DONE"}), "handoff completion", 30)
        assert len([x for x in assignments(base, args.password) if x.get("instruction") == "TESTER_DONE"]) == 1

        # max_parallel=1 gives a real one-slot queue; two projects must not overlap.
        q_projects = [rpc(base, args.password, "project.create", {"name": f"queue-{suffix}-{i}", "goal": "queue", "member_bot_ids": [coder["id"]]}) for i in (1, 2)]
        for i, created in enumerate(q_projects, 1):
            rpc(base, args.password, "chat.send", {"chat_id": created["chat"]["id"], "text": f"QUEUE_{'A' if i == 1 else 'B'}", "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": f"QUEUE_{'A' if i == 1 else 'B'}"}], "client_request_id": f"queue-{suffix}-{i}"})
        wait_status(base, args.password, "QUEUE_A", {"working"}, 20)
        queued = wait_status(base, args.password, "QUEUE_B", {"queued"}, 10)
        assert queued.get("queue_reason") in {"global_limit", "bot_limit", "bot_parallel_limit", "serial_in_project", "serial_in_bot"}, queued
        wait_until(lambda: all(item.get("status") == "done" for item in assignments(base, args.password) if item.get("instruction") in {"QUEUE_A", "QUEUE_B"}), "queue drain", 40)
        assert not DispatchProvider.state.overlap.intersection({"QUEUE_A", "QUEUE_B"})

        # Same project/Bot serializes; separate groups run in parallel.
        serial_project = rpc(base, args.password, "project.create", {"name": f"serial-{suffix}", "goal": "serial", "member_bot_ids": [coder["id"]]})
        for marker in ("SERIAL_A", "SERIAL_B"):
            rpc(base, args.password, "chat.send", {"chat_id": serial_project["chat"]["id"], "text": marker, "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": marker}], "client_request_id": f"{marker}-{suffix}"})
        wait_status(base, args.password, "SERIAL_A", {"working"}, 20)
        wait_status(base, args.password, "SERIAL_B", {"queued"}, 10)
        wait_until(lambda: all(item.get("status") == "done" for item in assignments(base, args.password) if item.get("instruction") in {"SERIAL_A", "SERIAL_B"}), "serial drain", 40)
        parallel_projects = [
            rpc(base, args.password, "project.create", {"name": f"parallel-{suffix}-1", "goal": "parallel", "member_bot_ids": [coder["id"]]}),
            rpc(base, args.password, "project.create", {"name": f"parallel-{suffix}-2", "goal": "parallel", "member_bot_ids": [tester["id"]]}),
        ]
        for i, created in enumerate(parallel_projects, 1):
            marker = f"PAR_{'A' if i == 1 else 'B'}"
            bot_id = coder["id"] if i == 1 else tester["id"]
            rpc(base, args.password, "chat.send", {"chat_id": created["chat"]["id"], "text": marker, "mentions": [{"kind": "bot", "bot_id": bot_id, "instruction": marker}], "client_request_id": f"{marker}-{suffix}"})
        wait_until(lambda: {"PAR_A", "PAR_B"}.issubset({x["marker"] for x in DispatchProvider.state.requests}), "parallel provider requests", 20)
        assert {"PAR_A", "PAR_B"}.issubset(DispatchProvider.state.overlap)
        wait_until(lambda: all(item.get("status") == "done" for item in assignments(base, args.password) if item.get("instruction") in {"PAR_A", "PAR_B"}), "parallel drain", 40)

        # Waiting releases the slot; resuming continues the original run id.
        waiting_project = rpc(base, args.password, "project.create", {"name": f"waiting-{suffix}", "goal": "waiting", "member_bot_ids": [coder["id"]]})
        rpc(base, args.password, "chat.send", {"chat_id": waiting_project["chat"]["id"], "text": "WAIT_DECISION", "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": "WAIT_DECISION"}], "client_request_id": f"wait-{suffix}"})
        waiting = wait_status(base, args.password, "WAIT_DECISION", {"waiting_user", "blocked"}, 30)
        first_run = next(item["run_id"] for item in traces(base, args.password, waiting["id"]) if item.get("type") == "run.start")
        rpc(base, args.password, "chat.send", {"chat_id": waiting_project["chat"]["id"], "text": "WAITING_SECOND", "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": "WAIT_SECOND"}], "client_request_id": f"wait-second-{suffix}"})
        wait_status(base, args.password, "WAIT_SECOND", {"working", "done"}, 20)
        rpc(base, args.password, "chat.send", {"chat_id": waiting_project["chat"]["id"], "assignment_id": waiting["id"], "text": "用户继续", "mentions": [], "client_request_id": f"wait-resume-{suffix}"})
        wait_until(lambda: any(item.get("type") == "run.resume" and item.get("run_id") == first_run for item in traces(base, args.password, waiting["id"])), "waiting resume same run", 30)
        wait_until(lambda: assignment_for(base, args.password, "WAIT_DECISION").get("status") == "done", "waiting completion", 30)

        # Bash approval, then assignment.stop must kill the foreground process.
        stop_project = rpc(base, args.password, "project.create", {"name": f"stop-{suffix}", "goal": "cancel", "member_bot_ids": [coder["id"]]})
        rpc(base, args.password, "chat.send", {"chat_id": stop_project["chat"]["id"], "text": "STOP_BASH", "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": "STOP_BASH"}], "client_request_id": f"stop-{suffix}"})
        stop_assignment = wait_status(base, args.password, "STOP_BASH", {"working"}, 20)
        settle_approvals(base, args.password)
        wait_until(lambda: any(item.get("type") == "tool.start" and item.get("data", {}).get("name") == "bash" for item in traces(base, args.password, stop_assignment["id"])), "bash process start", 30)
        rpc(base, args.password, "assignment.stop", {"assignment_id": stop_assignment["id"], "client_request_id": f"stop-now-{suffix}"})
        wait_until(lambda: assignment_for(base, args.password, "STOP_BASH").get("status") in {"cancelled", "stopped", "failed", "done"}, "assignment stop", 20)
        time.sleep(1)
        assert not Path(DispatchProvider.stop_path).exists(), "cancelled bash created the sentinel"
        print(json.dumps({"ok": True, "port": 7792, "handoff": True, "queue_reason": queued.get("queue_reason"), "parallel_overlap": sorted(DispatchProvider.state.overlap)}, ensure_ascii=False))
    finally:
        daemon.close()
        provider.shutdown()
        provider.server_close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7792")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command")
    parser.add_argument("--home", type=Path, default=Path("/tmp/macbot-dispatch-smoke"))
    parser.add_argument("--browser-bin")
    args = parser.parse_args()
    if args.home.exists():
        shutil.rmtree(args.home)
    acceptance(args)


if __name__ == "__main__":
    main()
