#!/usr/bin/env python3
"""Isolated regression smoke for user steers and memory-tool recovery.

The daemon is the production binary; only the OpenAI-compatible provider is
fake and bound to localhost.  The smoke deliberately uses the authoritative
``chat.send`` shape (chat, text, and a working Bot mention) for the user steer;
it must not pass an ``assignment_id`` or call ``assignment.steer`` (either
would hide a routing defect behind an internal shortcut).

Covered contracts:

* a user message sent to a working Coder assignment is represented by one
  durable message delivery and one assignment steer, then appears in that same
  run's trace as delivered/read;
* ``memory(scope=project, kind=project_status)`` is rejected as a tool error
  before any approval, while the model can issue the corrected project-scoped
  call in the same run;
* when an old persisted invalid-memory approval is supplied by the caller,
  startup recovery must expire it and append a tool error without authorising
  the old call or changing its arguments.

Example:
  python server/macbotd/tests/smoke_user_steer_memory.py \
    --daemon-command 'cargo run --manifest-path server/macbotd/Cargo.toml -- --port 7798 --password dev' \
    --home /tmp/macbot-user-steer-memory

For the legacy 094 upgrade phase, pass ``--old-command`` and
``--new-command``.  The script creates the invalid pending call with the old
binary, kills it at the durable waiting checkpoint, and resumes the same run
with the new binary.  Both commands must point at isolated binaries.
"""

from __future__ import annotations

import argparse
import json
import shlex
import socket
from urllib.parse import urlparse
from pathlib import Path
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, TOKEN, rpc, wait_until
from smoke_decision_migration import RestartDaemon


class FakeState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.first_steer_started = threading.Event()
        self.release_first_steer = threading.Event()
        self.invalid_memory_seen = threading.Event()
        self.waiting_decision_sent = threading.Event()
        self.project_id = ""
        self.steer_requests = 0

    def record(self, body: dict[str, Any]) -> None:
        with self.lock:
            self.requests.append(body)

    def snapshot(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.requests)


class FakeProvider(BaseHTTPRequestHandler):
    state: FakeState

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

    def _tool(self, call_id: str, name: str, arguments: dict[str, Any]) -> None:
        self._tools([(call_id, name, arguments)])

    def _tools(self, calls: list[tuple[str, str, dict[str, Any]]]) -> None:
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{
                "index": index,
                "id": call_id,
                "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)},
            } for index, (call_id, name, arguments) in enumerate(calls)]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
             "usage": {"prompt_tokens": 11, "completion_tokens": 5}},
        ])

    def _text(self, text: str) -> None:
        self._stream([
            {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}],
             "usage": {"prompt_tokens": 8, "completion_tokens": 3}},
        ])

    @staticmethod
    def _tool_results(messages: list[dict[str, Any]]) -> dict[str, str]:
        return {
            str(message.get("tool_call_id")): str(message.get("content", ""))
            for message in messages
            if message.get("role") == "tool"
        }

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/models":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "fake token required"}})
            return
        self._json(200, {"data": [{"id": "user-steer-memory-fake", "object": "model"}]})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        except (ValueError, json.JSONDecodeError) as error:
            self._json(400, {"error": {"message": str(error)}})
            return
        self.state.record(body)
        messages = body.get("messages", [])
        prompt = json.dumps(messages, ensure_ascii=False)
        tools = self._tool_results(messages)
        user_messages = [
            str(item.get("content", "")) for item in messages if item.get("role") == "user"
        ]
        markers = (
            "USER_WAIT_STEER", "USER_WAIT_REPLY",
            "USER_STEER_MEMORY_STEER",
            "USER_STEER_NOW",
            "MEMORY_SCOPE_REGRESSION",
            "LEGACY_094_INVALID_PENDING",
        )
        active_marker = next(
            (marker for content in reversed(user_messages) for marker in markers if marker in content),
            None,
        )

        if active_marker in {"USER_STEER_MEMORY_STEER", "USER_STEER_NOW"}:
            with self.state.lock:
                self.state.steer_requests += 1
                turn = self.state.steer_requests
            if turn == 1:
                self.state.first_steer_started.set()
                # Keep the assignment working while chat.send admits the user
                # steer.  A timeout prevents a broken test from hanging.
                self.state.release_first_steer.wait(30)
                self._tool("steer-progress", "send_msg", {
                    "intent": "progress", "text": "已收到任务，等待插话", "mentions": []
                })
            elif "USER_STEER_NOW" in prompt:
                self._tool("steer-done", "send_msg", {
                    "intent": "done", "text": "已按用户插话完成", "mentions": []
                })
            else:
                self._tool("steer-progress-2", "send_msg", {
                    "intent": "progress", "text": "正在处理", "mentions": []
                })
            return

        if active_marker in {"USER_WAIT_STEER", "USER_WAIT_REPLY"}:
            if active_marker == "USER_WAIT_STEER":
                self.state.waiting_decision_sent.set()
                self._tool("wait-decision", "send_msg", {
                    "intent": "decision", "text": "请确认邮箱登录方式", "options": ["邮箱登录", "其他方式"], "mentions": [],
                })
            else:
                self._tool("wait-done", "send_msg", {
                    "intent": "done", "text": "已按等待态插话完成", "mentions": [],
                })
            return

        if active_marker == "MEMORY_SCOPE_REGRESSION":
            invalid = next((call_id for call_id, content in tools.items() if "not valid for scope project" in content), None)
            corrected = next((call_id for call_id in tools if call_id == "memory-corrected"), None)
            if corrected is not None:
                self._tool("memory-done", "send_msg", {
                    "intent": "done", "text": "记忆已更新", "mentions": []
                })
            elif invalid is not None:
                self._tool("memory-corrected", "memory", {
                    "scope": "project", "project_id": self.state.project_id,
                    "action": "add", "kind": "project", "content": "MEMORY_SCOPE_REGRESSION",
                })
            else:
                self.state.invalid_memory_seen.set()
                self._tool("memory-invalid", "memory", {
                    "scope": "project", "project_id": self.state.project_id,
                    "action": "replace", "id": "MEMORY_SCOPE_REGRESSION",
                    "kind": "project_status",
                    "content": "MEMORY_SCOPE_REGRESSION",
                })
            return

        if active_marker == "LEGACY_094_INVALID_PENDING":
            invalid = next((call_id for call_id, content in tools.items() if "not valid for scope project" in content), None)
            corrected = next((call_id for call_id in tools if call_id == "legacy-memory-corrected"), None)
            if corrected is not None:
                self._tool("legacy-done", "send_msg", {"intent": "done", "text": "094 recovery complete", "mentions": []})
            elif invalid is not None:
                self._tool("legacy-memory-corrected", "memory", {
                    "scope": "project", "project_id": self.state.project_id,
                    "action": "add", "kind": "project", "content": "LEGACY_094_INVALID_PENDING",
                })
            else:
                self._tools([
                    ("legacy-memory-invalid", "memory", {
                        "scope": "project", "project_id": self.state.project_id,
                        "action": "replace", "id": "LEGACY_094_INVALID_PENDING",
                        "kind": "project_status", "content": "LEGACY_094_INVALID_PENDING",
                    }),
                    ("legacy-deferred-write", "write", {
                        "path": "legacy-094-must-not-write.txt", "content": "forbidden",
                    }),
                ])
            return
        self._text("smoke complete")


def start_provider(state: FakeState) -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProvider)
    FakeProvider.state = state
    thread = threading.Thread(target=server.serve_forever, name="user-steer-memory-fake", daemon=True)
    thread.start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def traces(base: str, password: str, assignment_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"assignment_id": assignment_id, "limit": 500})["items"]


def assignment_for(base: str, password: str, marker: str) -> dict[str, Any]:
    rows = rpc(base, password, "assignment.list", {})["items"]
    return next(row for row in rows if marker in row.get("instruction", ""))


def provider_setup(base: str, password: str, provider_url: str, suffix: str) -> tuple[str, dict[str, Any], dict[str, Any]]:
    provider = rpc(base, password, "provider.create", {
        "name": f"user-steer-memory-fake-{suffix}",
        "api_kind": "openai-completions",
        "base_url": provider_url,
        "api_key": TOKEN,
        "client_request_id": f"user-steer-provider-{suffix}",
    })["provider"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider["id"]})
    assert any(item["model_id"] == "user-steer-memory-fake" for item in refreshed["models"])
    model = rpc(base, password, "model.upsert", {
        "provider_id": provider["id"],
        "model_id": "user-steer-memory-fake",
        "display_name": "User steer memory fake",
        "caps": {"vision": False, "tools": True, "reasoning": False},
        "client_request_id": f"user-steer-model-{suffix}",
    })["model"]["ref"]
    worker = rpc(base, password, "bot.create", {
        "name": f"Coder-{suffix}", "model": model,
        "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
        "max_parallel": 1,
    })["bot"]
    project = rpc(base, password, "project.create", {
        "name": f"User steer memory {suffix}", "goal": "isolated regression",
        "member_bot_ids": [worker["id"]], "client_request_id": f"user-steer-project-{suffix}",
    })
    return model, worker, project


def run_user_steer(base: str, password: str, worker: dict[str, Any], project: dict[str, Any], state: FakeState) -> dict[str, Any]:
    marker = "USER_STEER_MEMORY_STEER"
    chat_id = project["chat"]["id"]
    initial = rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": None}],
        "client_request_id": "user-steer-initial",
    })
    wait_until(lambda: state.first_steer_started.is_set(), "working Coder provider turn", 30)
    assignment = assignment_for(base, password, marker)
    assignment_id = assignment["id"]
    wait_until(lambda: assignment_for(base, password, marker).get("status") == "working", "working Coder assignment", 15)

    steer_text = "USER_STEER_NOW: 请优先完成邮箱登录"
    result = rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": steer_text,
        "mentions": [{"kind": "bot", "bot_id": worker["id"]}],
        "client_request_id": "user-steer-message",
    })
    replayed = rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": steer_text,
        "mentions": [{"kind": "bot", "bot_id": worker["id"]}],
        "client_request_id": "user-steer-message",
    })
    assert replayed == result, "steer client_request_id replay created a duplicate"
    message = result["message"]
    deliveries = [item for item in message.get("delivery", []) if item.get("assignment_id") == assignment_id]
    assert deliveries and deliveries[0].get("state") in {"queued", "delivered", "read"}, result
    state.release_first_steer.set()

    def done() -> bool:
        return assignment_for(base, password, marker).get("status") == "done"
    wait_until(done, "steered run completion", 45)
    current = assignment_for(base, password, marker)
    steer_rows = [row for row in current.get("steers", []) if row.get("message_id") == message["id"]]
    assert len(steer_rows) == 1 and steer_rows[0].get("applied_at"), current
    def message_read() -> bool:
        history = rpc(base, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 200})["messages"]
        candidate = next((item for item in history if item.get("id") == message["id"]), None)
        return bool(candidate and any(
            item.get("assignment_id") == assignment_id and item.get("state") == "read"
            for item in candidate.get("delivery", [])
        ))
    wait_until(message_read, "user steer delivery read state", 30)
    history = rpc(base, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 200})["messages"]
    assert sum(item.get("fallback_text") == steer_text for item in history) == 1, history
    items = traces(base, password, assignment_id)
    run_ids = {item.get("run_id") for item in items if item.get("run_id")}
    steer_trace = next((item for item in items if item.get("type") == "steer" and item.get("data", {}).get("message_id") == message["id"]), None)
    matching_assignments = [item for item in rpc(base, password, "assignment.list", {})["items"] if marker in item.get("instruction", "")]
    assert len(matching_assignments) == 1, matching_assignments
    worker_assignments = [
        item for item in rpc(base, password, "assignment.list", {})["items"]
        if item.get("origin_chat_id") == chat_id and item.get("bot_id") == worker["id"]
        and item.get("trigger_message_id") in {initial["message"]["id"], message["id"]}
    ]
    assert len(worker_assignments) == 1, worker_assignments
    assert steer_trace and len(run_ids) == 1, items
    assert sum(item.get("type") == "run.start" for item in items) == 1, items
    assert any("USER_STEER_NOW" in json.dumps(item, ensure_ascii=False) for item in items)
    bodies = state.snapshot()
    steer_body = next(
        body for body in reversed(bodies)
        if any(
            item.get("role") == "user"
            and "USER_STEER_NOW" in str(item.get("content", ""))
            for item in body.get("messages", [])
        )
    )
    assert sum(
        item.get("role") == "user" and item.get("content") == steer_text
        for item in steer_body.get("messages", [])
    ) == 1, steer_body
    return {
        "assignment_id": assignment_id,
        "message_id": message["id"],
        "run_id": next(iter(run_ids)),
        "delivery": deliveries[0],
        "provider_steer_user_count": 1,
    }


def run_memory(base: str, password: str, worker: dict[str, Any], project: dict[str, Any]) -> dict[str, Any]:
    marker = "MEMORY_SCOPE_REGRESSION"
    chat_id = project["chat"]["id"]
    # The fake needs the canonical project id for the corrected call.
    FakeProvider.state.project_id = project["project"]["id"]
    rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": None}],
        "client_request_id": "memory-scope-message",
    })
    assignment = assignment_for(base, password, marker)
    assignment_id = assignment["id"]
    wait_until(lambda: FakeProvider.state.invalid_memory_seen.is_set(), "invalid memory call", 30)
    wait_until(lambda: any(item.get("type") == "tool.end" and item.get("data", {}).get("call_id") == "memory-invalid" for item in traces(base, password, assignment_id)), "invalid memory tool error", 30)
    before = rpc(base, password, "approval.list", {}).get("approvals", [])
    assert not any(item.get("assignment_id") == assignment_id
        and json.loads(item.get("detail", "{}" )).get("kind") == "project_status" for item in before), before
    wait_until(lambda: any(item.get("type") == "tool.start" and item.get("data", {}).get("call_id") == "memory-corrected" for item in traces(base, password, assignment_id)), "corrected project memory call", 30)
    # tool.start precedes approval persistence.  Wait for the exact corrected
    # call's approval or successful result instead of racing a single list read.
    def corrected_call_ready() -> bool:
        pending = rpc(base, password, "approval.list", {}).get("approvals", [])
        return any(item.get("assignment_id") == assignment_id and item.get("state") == "pending" for item in pending) or any(
            item.get("type") == "tool.end"
            and item.get("data", {}).get("call_id") == "memory-corrected"
            for item in traces(base, password, assignment_id)
        )

    wait_until(corrected_call_ready, "corrected memory approval or result", 30)
    # A future policy may gate the corrected write; if so, approve only this
    # exact corrected call.  The invalid project_status call is never approved.
    approvals = [item for item in rpc(base, password, "approval.list", {}).get("approvals", []) if item.get("assignment_id") == assignment_id and item.get("state") == "pending"]
    if approvals:
        assert all("project_status" not in json.dumps(item, ensure_ascii=False) for item in approvals)
        assert all(
            json.loads(item.get("detail", "{}")).get("kind") == "project"
            and json.loads(item.get("detail", "{}")).get("project_id") == project["project"]["id"]
            for item in approvals
        ), approvals
        rpc(base, password, "approval.decide", {"approval_id": approvals[0]["id"], "decision": "allow_once"})
    wait_until(lambda: assignment_for(base, password, marker).get("status") == "done", "corrected memory run completion", 45)
    items = traces(base, password, assignment_id)
    invalid = next(item for item in items if item.get("type") == "tool.end" and item.get("data", {}).get("call_id") == "memory-invalid")
    assert invalid.get("data", {}).get("is_error") is True
    assert any(item.get("type") == "tool.end" and item.get("data", {}).get("call_id") == "memory-corrected" and not item.get("data", {}).get("is_error") for item in items), items
    same_run = len({item.get("run_id") for item in items if item.get("run_id")}) == 1
    assert same_run, items
    return {"assignment_id": assignment_id, "invalid_call": "memory-invalid", "corrected_call": "memory-corrected", "same_run": same_run}


def run_waiting_steer(base: str, password: str, worker: dict[str, Any], project: dict[str, Any], state: FakeState) -> dict[str, Any]:
    marker = "USER_WAIT_STEER"
    reply_marker = "USER_WAIT_REPLY"
    chat_id = project["chat"]["id"]
    rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker["id"]}],
        "client_request_id": "waiting-steer-initial",
    })
    wait_until(lambda: state.waiting_decision_sent.is_set(), "waiting decision provider turn", 30)
    assignment = assignment_for(base, password, marker)
    assignment_id = assignment["id"]
    wait_until(
        lambda: assignment_for(base, password, marker).get("status") in {"waiting_user", "waiting_bot"},
        "decision assignment waiting",
        20,
    )
    before = traces(base, password, assignment_id)
    assert any(item.get("type") == "run.wait" for item in before), before

    waiting_params = {
        "chat_id": chat_id, "text": reply_marker,
        "mentions": [{"kind": "bot", "bot_id": worker["id"]}],
        "client_request_id": "waiting-steer-reply",
    }
    assert "assignment_id" not in waiting_params and "reply_to" not in waiting_params
    result = rpc(base, password, "chat.send", waiting_params)
    message = result["message"]
    deliveries = [item for item in message.get("delivery", []) if item.get("assignment_id") == assignment_id]
    assert deliveries and deliveries[0].get("state") in {"queued", "delivered", "read"}, result
    wait_until(lambda: assignment_for(base, password, marker).get("status") == "done", "waiting steer completion", 45)
    current = assignment_for(base, password, marker)
    steers = [item for item in current.get("steers", []) if item.get("message_id") == message["id"]]
    assert len(steers) == 1 and steers[0].get("applied_at"), current
    items = traces(base, password, assignment_id)
    run_ids = {item.get("run_id") for item in items if item.get("run_id")}
    assert len(run_ids) == 1 and sum(item.get("type") == "run.start" for item in items) == 1, items
    assert any(
        item.get("type") == "steer"
        and item.get("data", {}).get("message_id") == message["id"]
        for item in items
    ), items
    assert any(
        item.get("type") == "run.end" and item.get("data", {}).get("status") == "done"
        for item in items
    ), items
    bodies = state.snapshot()
    reply_body = next(
        body for body in reversed(bodies)
        if any(
            item.get("role") == "user" and reply_marker in str(item.get("content", ""))
            for item in body.get("messages", [])
        )
    )
    assert sum(
        item.get("role") == "user" and item.get("content") == reply_marker
        for item in reply_body.get("messages", [])
    ) == 1, reply_body
    return {"assignment_id": assignment_id, "message_id": message["id"], "run_id": next(iter(run_ids)), "same_run": True}


def safe_body_log(state: FakeState) -> dict[str, int]:
    markers = ("USER_STEER_NOW", "USER_WAIT_REPLY", "MEMORY_SCOPE_REGRESSION", "LEGACY_094_INVALID_PENDING")
    counts = {marker: 0 for marker in markers}
    for body in state.snapshot():
        for message in body.get("messages", []):
            if message.get("role") != "user":
                continue
            content = str(message.get("content", ""))
            for marker in markers:
                if marker in content:
                    counts[marker] += 1
    return counts


def pending_approvals(base: str, password: str, assignment_id: str) -> list[dict[str, Any]]:
    return [
        item for item in rpc(base, password, "bootstrap").get("pending", {}).get("approvals", [])
        if item.get("assignment_id") == assignment_id and item.get("state") == "pending"
    ]


def latest_job(home: Path, run_id: str) -> dict[str, Any] | None:
    jobs = [json.loads(path.read_text(encoding="utf-8")) for path in (home / "data/jobs").glob("*.json")]
    return next((job for job in jobs if job.get("checkpoint", {}).get("run_id") == run_id), None)


def run_legacy_recovery(
    base: str,
    password: str,
    home: Path,
    worker: dict[str, Any],
    project: dict[str, Any],
    state: FakeState,
    daemon: RestartDaemon,
    old_command: str,
    new_command: str,
) -> dict[str, Any]:
    """Create the invalid pending call with 094, then recover it with HEAD."""
    marker = "LEGACY_094_INVALID_PENDING"
    daemon.kill9_stop()
    daemon.args.daemon_command = old_command
    daemon.start()
    chat_id = project["chat"]["id"]
    sent = rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker["id"]}],
        "client_request_id": "legacy-094-message",
    })
    assignment: dict[str, Any] = {}
    def assigned() -> bool:
        row = assignment_for(base, password, marker)
        if row:
            assignment.update(row)
        return bool(row)
    wait_until(assigned, "legacy invalid assignment", 30)
    assignment_id = assignment["id"]
    cards: list[dict[str, Any]] = []
    def approval_ready() -> bool:
        cards[:] = pending_approvals(base, password, assignment_id)
        return bool(cards)
    wait_until(approval_ready, "legacy invalid approval", 45)
    first = cards[0]
    original_detail = json.loads(first["detail"])
    assert original_detail["scope"] == "project"
    assert original_detail["kind"] == "project_status"
    items = traces(base, password, assignment_id)
    run_id = next(item["run_id"] for item in items if item.get("type") == "run.start")
    wait_until(lambda: (job := latest_job(home, run_id)) is not None
        and job.get("status") in {"waiting", "suspended"}
        and job.get("checkpoint", {}).get("pending_tool", {}).get("call_id") == "legacy-memory-invalid",
        "legacy waiting checkpoint", 15)
    job = latest_job(home, run_id)
    assert job and job.get("status") in {"waiting", "suspended"}, job
    pending_tool = job.get("checkpoint", {}).get("pending_tool", {})
    assert pending_tool.get("call_id") == "legacy-memory-invalid"
    assert pending_tool.get("args") == original_detail

    daemon.kill9_stop()
    daemon.args.daemon_command = new_command
    daemon.start()
    def expired() -> bool:
        state_path = home / "data/orchestrator/state.json"
        if not state_path.is_file():
            return False
        snapshot = json.loads(state_path.read_text(encoding="utf-8"))
        return snapshot.get("approvals", {}).get(first["id"], {}).get("state") == "expired"
    wait_until(expired, "legacy invalid approval expiry", 30)
    try:
        rpc(base, password, "approval.decide", {"approval_id": first["id"], "decision": "allow_once"})
    except AssertionError as error:
        assert "expired" in str(error) or "decided" in str(error), str(error)
    else:
        raise AssertionError("expired invalid approval was accepted")
    wait_until(
        lambda: any(
            item.get("type") == "tool.end"
            and item.get("data", {}).get("call_id") == "legacy-memory-invalid"
            and item.get("data", {}).get("is_error") is True
            for item in traces(base, password, assignment_id)
        ),
        "legacy invalid tool error",
        45,
    )
    approvals = pending_approvals(base, password, assignment_id)
    if approvals:
        detail = json.loads(approvals[0]["detail"])
        assert detail.get("scope") == "project" and detail.get("kind") == "project", detail
        rpc(base, password, "approval.decide", {"approval_id": approvals[0]["id"], "decision": "allow_once"})
    wait_until(lambda: assignment_for(base, password, marker).get("status") == "done", "legacy same-run completion", 45)
    final = traces(base, password, assignment_id)
    assert {item.get("run_id") for item in final if item.get("run_id")} == {run_id}
    assert not any(
        item.get("type") == "tool.end"
        and item.get("data", {}).get("call_id") == "legacy-memory-invalid"
        and item.get("data", {}).get("is_error") is not True
        for item in final
    )
    assert not any(
        item.get("type") == "tool.end"
        and item.get("data", {}).get("call_id") == "legacy-deferred-write"
        and item.get("data", {}).get("is_error") is not True
        for item in final
    )
    return {
        "assignment_id": assignment_id,
        "run_id": run_id,
        "sent_id": sent["message"]["id"],
        "old_approval": first["id"],
        "old_args_unchanged": True,
        "old_expired": True,
        "same_run_done": True,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7798")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command")
    parser.add_argument("--old-command", help="094 binary used only for the legacy invalid-pending phase")
    parser.add_argument("--new-command", help="current binary used after the legacy kill9")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--browser-bin")
    args = parser.parse_args()
    assert not args.home.exists(), "use a fresh isolated home"
    if bool(args.old_command) != bool(args.new_command):
        parser.error("--old-command and --new-command must be provided together")
    if args.new_command:
        args.daemon_command = args.new_command
    if not args.daemon_command:
        parser.error("--daemon-command is required unless --new-command is used")
    endpoint = urlparse(args.url)
    if endpoint.hostname not in {"127.0.0.1", "localhost"} or not endpoint.port or endpoint.port in {7788, 7789}:
        parser.error("use an isolated loopback port other than 7788/7789")
    for command in (args.daemon_command, args.old_command, args.new_command):
        if not command: continue
        words = shlex.split(command)
        if "--port" not in words or words[words.index("--port") + 1] != str(endpoint.port) or "--home" in words:
            parser.error("daemon commands must explicitly match --url port and use isolated MACBOT_HOME")
    with socket.socket() as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        probe.bind(("127.0.0.1", endpoint.port))
    state = FakeState()
    provider, provider_url = start_provider(state)
    daemon = RestartDaemon(args) if args.old_command else Daemon(args)
    try:
        daemon.start()
        base = args.url.rstrip("/")
        suffix = uuid.uuid4().hex[:8]
        _, worker, project = provider_setup(base, args.password, provider_url, suffix)
        state.project_id = project["project"]["id"]
        legacy: dict[str, Any] | None = None
        if args.old_command:
            legacy = run_legacy_recovery(
                base, args.password, args.home, worker, project, state, daemon,
                args.old_command, args.new_command,
            )
        steer = run_user_steer(base, args.password, worker, project, state)
        waiting = run_waiting_steer(base, args.password, worker, project, state)
        memory = run_memory(base, args.password, worker, project)
        result: dict[str, Any] = {
            "ok": True,
            "steer": steer,
            "waiting_steer": waiting,
            "memory": memory,
            "provider_body_marker_counts": safe_body_log(state),
        }
        if legacy is not None:
            result["legacy"] = legacy
        print(json.dumps(result, ensure_ascii=False, indent=2))
    finally:
        daemon.close()
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
