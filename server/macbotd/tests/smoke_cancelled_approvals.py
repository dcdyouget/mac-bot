#!/usr/bin/env python3
"""Cancellation safety for approval-gated runs.

Only the OpenAI-compatible provider is fake.  Every daemon and data directory
is isolated; the script rejects ports 7788/7789 and never prints provider
bodies, prompts, or credentials.
"""
from __future__ import annotations

import argparse
import shlex
import socket
from urllib.parse import urlparse
import json
from pathlib import Path
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import TOKEN, rpc, wait_until
from smoke_decision_migration import RestartDaemon


class FakeState:
    def __init__(self, target_dir: Path) -> None:
        self.target_dir = target_dir
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []

    def record(self, body: dict[str, Any]) -> None:
        with self.lock:
            self.requests.append(body)

    def count(self) -> int:
        with self.lock:
            return len(self.requests)


class FakeProvider(BaseHTTPRequestHandler):
    state: FakeState

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        body = json.dumps(value).encode()
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
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _tool(self, call_id: str, name: str, args: dict[str, Any]) -> None:
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": call_id, "type": "function",
                "function": {"name": name, "arguments": json.dumps(args)},
            }]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
             "usage": {"prompt_tokens": 9, "completion_tokens": 4}},
        ])

    def _text(self, text: str) -> None:
        self._stream([
            {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}],
             "usage": {"prompt_tokens": 6, "completion_tokens": 2}},
        ])

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/models" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "unauthorized"}})
            return
        self._json(200, {"data": [{"id": "cancel-approval-fake", "object": "model"}]})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "unauthorized"}})
            return
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        except (ValueError, json.JSONDecodeError) as error:
            self._json(400, {"error": {"message": str(error)}})
            return
        self.state.record(body)
        messages = body.get("messages", [])
        latest = next((str(item.get("content", "")) for item in reversed(messages) if item.get("role") == "user"), "")
        marker = next((name for name in ("CANCEL_PRIMARY", "CANCEL_OTHER", "CANCEL_DM", "LEGACY_CANCEL", "DENY_NORMAL") if name in latest), None)
        if marker is None:
            self._text("cancel smoke idle")
            return
        results = {str(item.get("tool_call_id")): str(item.get("content", "")) for item in messages if item.get("role") == "tool"}
        call_id = f"cancel-write-{marker.lower()}"
        if call_id in results:
            self._text(f"{marker} completed")
        else:
            self._tool(call_id, "write", {
                "path": str(self.state.target_dir / f"{marker.lower()}.txt"),
                "content": f"must-not-write-{marker}",
            })


def start_provider(state: FakeState) -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProvider)
    FakeProvider.state = state
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def setup(base: str, password: str, provider_url: str, suffix: str) -> tuple[dict[str, Any], dict[str, Any]]:
    provider = rpc(base, password, "provider.create", {
        "name": f"cancel-approval-{suffix}", "api_kind": "openai-completions",
        "base_url": provider_url, "api_key": TOKEN,
        "client_request_id": f"cancel-provider-{suffix}",
    })["provider"]
    models = rpc(base, password, "model.refresh", {"provider_id": provider["id"]})["models"]
    assert any(item["model_id"] == "cancel-approval-fake" for item in models)
    model = rpc(base, password, "model.upsert", {
        "provider_id": provider["id"], "model_id": "cancel-approval-fake",
        "display_name": "Cancellation approval fake",
        "caps": {"vision": False, "tools": True, "reasoning": False},
    })["model"]["ref"]
    worker = rpc(base, password, "bot.create", {
        "name": f"Cancel worker {suffix}", "model": model, "max_parallel": 3,
        "tools": {"files": True, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
    })["bot"]
    project = rpc(base, password, "project.create", {
        "name": f"Cancel approvals {suffix}", "goal": "approval cancellation", "member_bot_ids": [worker["id"]],
    })
    return worker, project


def assignments(base: str, password: str, marker: str) -> list[dict[str, Any]]:
    return [item for item in rpc(base, password, "assignment.list", {})["items"] if marker in item.get("instruction", "")]


def one_assignment(base: str, password: str, marker: str) -> dict[str, Any]:
    rows = assignments(base, password, marker)
    assert len(rows) == 1, rows
    return rows[0]


def trace(base: str, password: str, assignment_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"assignment_id": assignment_id, "limit": 500})["items"]


def pending(base: str, password: str) -> list[dict[str, Any]]:
    return [item for item in rpc(base, password, "approval.list", {}).get("approvals", []) if item.get("state") == "pending"]


def waiting(base: str, password: str) -> list[dict[str, Any]]:
    result = rpc(base, password, "workbench.get", {})
    workbench = result.get("workbench", result)
    return workbench.get("waiting", [])


def job_for(home: Path, run_id: str) -> dict[str, Any]:
    for path in (home / "data/jobs").glob("*.json"):
        job = json.loads(path.read_text(encoding="utf-8"))
        if job.get("checkpoint", {}).get("run_id") == run_id:
            return job
    raise AssertionError(f"missing durable job for {run_id}")


def reject_decisions(base: str, password: str, approval_id: str) -> None:
    for decision in ("allow_once", "always_allow", "deny"):
        try:
            rpc(base, password, "approval.decide", {"approval_id": approval_id, "decision": decision})
        except AssertionError:
            continue
        raise AssertionError(f"cancelled approval accepted late decision {decision}")


def send_approval(base: str, password: str, chat_id: str, worker_id: str, marker: str, request_id: str) -> dict[str, Any]:
    rpc(base, password, "chat.send", {
        "chat_id": chat_id, "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker_id, "instruction": marker}],
        "client_request_id": request_id,
    })
    result: dict[str, Any] = {}
    def ready() -> bool:
        row = assignments(base, password, marker)
        if not row:
            return False
        result.update(row[0])
        return bool([item for item in pending(base, password) if item.get("assignment_id") == row[0]["id"]])
    wait_until(ready, f"approval for {marker}", 45)
    return result


def run_current(base: str, password: str, home: Path, worker: dict[str, Any], project: dict[str, Any], fake: FakeState) -> dict[str, Any]:
    primary = send_approval(base, password, project["chat"]["id"], worker["id"], "CANCEL_PRIMARY", "cancel-primary")
    other_project = rpc(base, password, "project.create", {"name": "Cancel unrelated", "goal": "unrelated pending", "member_bot_ids": [worker["id"]]})
    other = send_approval(base, password, other_project["chat"]["id"], worker["id"], "CANCEL_OTHER", "cancel-other")
    dm = rpc(base, password, "chat.send", {"chat_id": worker["dm_chat_id"], "text": "CANCEL_DM", "mentions": [], "client_request_id": "cancel-dm"})
    dm_approval: dict[str, Any] = {}
    def dm_ready() -> bool:
        rows = [item for item in pending(base, password)
                if item.get("assignment_id") is None and item.get("chat_id") == worker["dm_chat_id"]]
        if rows:
            dm_approval.update(rows[0])
        return bool(rows)
    wait_until(dm_ready, "standalone DM approval", 45)
    primary_id = primary["id"]
    approvals_before = pending(base, password)
    primary_approval = next(item for item in approvals_before if item.get("assignment_id") == primary_id)
    provider_calls = fake.count()
    rpc(base, password, "assignment.stop", {"assignment_id": primary_id, "client_request_id": "cancel-primary-stop"})
    wait_until(lambda: one_assignment(base, password, "CANCEL_PRIMARY").get("status") == "cancelled", "primary cancellation", 30)
    wait_until(lambda: not any(item.get("assignment_id") == primary_id for item in pending(base, password)), "cancelled pending approval removed", 30)
    assert not any(item.get("assignment_id") == primary_id for item in waiting(base, password))
    cancelled_approval = next(item for item in rpc(base, password, "approval.list", {}).get("approvals", []) if item.get("id") == primary_approval["id"])
    assert cancelled_approval.get("state") == "expired", cancelled_approval
    reject_decisions(base, password, primary_approval["id"])
    assert fake.count() == provider_calls
    primary_trace = trace(base, password, primary_id)
    assert not any(item.get("type") == "tool.end" and not item.get("data", {}).get("is_error") for item in primary_trace)
    run_id = next(item["run_id"] for item in primary_trace if item.get("type") == "run.start")
    assert job_for(home, run_id).get("status") == "cancelled"
    for path in (fake.target_dir / "cancel_primary.txt",):
        assert not path.exists(), path
    live = pending(base, password)
    assert any(item.get("id") == dm_approval["id"] for item in live), live
    assert {item.get("assignment_id") for item in live} >= {other["id"], None}, live
    boot_live = rpc(base, password, "bootstrap").get("pending", {}).get("approvals", [])
    assert {item.get("assignment_id") for item in boot_live} >= {other["id"], None}, boot_live
    return {"cancelled_assignment": primary_id, "expired_approval": primary_approval["id"], "provider_calls_unchanged": True, "unrelated_pending": [other["id"], None], "job_cancelled": True}


def run_legacy(base: str, password: str, home: Path, worker: dict[str, Any], project: dict[str, Any], daemon: RestartDaemon, old_command: str, new_command: str, fake: FakeState) -> dict[str, Any]:
    daemon.kill9_stop(); daemon.args.daemon_command = old_command; daemon.start()
    assignment = send_approval(base, password, project["chat"]["id"], worker["id"], "LEGACY_CANCEL", "legacy-cancel")
    approval = next(item for item in pending(base, password) if item.get("assignment_id") == assignment["id"])
    before_detail = approval.get("detail")
    before_trace = trace(base, password, assignment["id"])
    run_id = next(item["run_id"] for item in before_trace if item.get("type") == "run.start")
    calls = fake.count()
    rpc(base, password, "assignment.stop", {"assignment_id": assignment["id"], "client_request_id": "legacy-cancel-stop"})
    old_job = job_for(home, run_id)
    assert old_job["status"] == "cancelled", old_job["status"]
    old_pending = next(item for item in pending(base, password) if item["id"] == approval["id"])
    assert old_pending["detail"] == before_detail
    old_assignment = one_assignment(base, password, "LEGACY_CANCEL")
    daemon.kill9_stop(); daemon.args.daemon_command = new_command; daemon.start()
    def expired() -> bool:
        state = json.loads((home / "data/orchestrator/state.json").read_text(encoding="utf-8"))
        return state.get("approvals", {}).get(approval["id"], {}).get("state") == "expired"
    wait_until(expired, "legacy cancelled approval expiry", 30)
    reject_decisions(base, password, approval["id"])
    wait_until(lambda: one_assignment(base, password, "LEGACY_CANCEL").get("status") == "cancelled", "legacy assignment remains cancelled", 30)
    final_trace = trace(base, password, assignment["id"])
    assert {item.get("run_id") for item in final_trace if item.get("run_id")} == {run_id}
    assert not any(item.get("type") == "tool.end" and not item.get("data", {}).get("is_error") for item in final_trace)
    assert fake.count() == calls
    state = json.loads((home / "data/orchestrator/state.json").read_text(encoding="utf-8"))
    expired = state["approvals"][approval["id"]]
    assert expired.get("state") == "expired"
    assert expired.get("detail") == before_detail
    decided_at = expired.get("decided_at")
    daemon.kill9_restart()
    repeat_state = json.loads((home / "data/orchestrator/state.json").read_text(encoding="utf-8"))
    assert repeat_state["approvals"][approval["id"]].get("decided_at") == decided_at
    rpc(base, password, "assignment.stop", {"assignment_id": assignment["id"], "client_request_id": "legacy-cancel-stop-repeat"})
    assert fake.count() == calls
    current_job = job_for(home, run_id)
    assert current_job["id"] == old_job["id"] and current_job["status"] == "cancelled"
    assert current_job["checkpoint"] == old_job["checkpoint"]
    current_assignment = one_assignment(base, password, "LEGACY_CANCEL")
    assert current_assignment["finished_at"] == old_assignment["finished_at"]
    events = [json.loads(line) for line in (home / "data/events/events.jsonl").read_text().splitlines() if line]
    resolved = [event for event in events if event.get("event") == "approval.resolved" and event.get("data", {}).get("approval", {}).get("id") == approval["id"]]
    assert len(resolved) == 1, resolved
    assert not (fake.target_dir / "legacy_cancel.txt").exists()
    return {"assignment_id": assignment["id"], "run_id": run_id, "job_id":current_job["id"], "approval_id": approval["id"], "detail_unchanged": True, "expired": True, "checkpoint_unchanged":True, "resolution_events":1, "provider_calls_unchanged": True}


def run_normal_deny(base: str, password: str, home: Path, worker: dict[str, Any], fake: FakeState) -> dict[str, Any]:
    marker = "DENY_NORMAL"
    target = fake.target_dir / "deny_normal.txt"
    assert not target.exists()
    project = rpc(base, password, "project.create", {
        "name": "Normal deny", "goal": "deny pending approval", "member_bot_ids": [worker["id"]],
    })
    assignment = send_approval(base, password, project["chat"]["id"], worker["id"], marker, "deny-normal")
    approval = next(item for item in pending(base, password) if item.get("assignment_id") == assignment["id"])
    run_id = next(item["run_id"] for item in trace(base, password, assignment["id"]) if item.get("type") == "run.start")
    calls = fake.count()
    result = rpc(base, password, "approval.decide", {
        "approval_id": approval["id"], "decision": "deny", "client_request_id": "deny-normal-decision",
    })
    denied = result.get("approval", result)
    assert denied.get("state") == "denied", result
    wait_until(lambda: one_assignment(base, password, marker).get("status") == "failed", "normal deny failed assignment", 30)
    wait_until(lambda: job_for(home, run_id).get("status") == "cancelled", "normal deny cancelled job", 30)
    assert fake.count() == calls
    items = trace(base, password, assignment["id"])
    assert not any(item.get("type") == "tool.end" and not item.get("data", {}).get("is_error") for item in items)
    assert not target.exists(), target
    history = rpc(base, password, "chat.history", {"chat_id": project["chat"]["id"], "after_seq": 0, "limit": 200})["messages"]
    stopped = [
        item for item in history
        if item.get("assignment_id") == assignment["id"]
        and any(block.get("type") == "system" and block.get("code") == "task_stopped" for block in item.get("blocks", []))
    ]
    assert len(stopped) == 1, stopped
    event_path = home / "data/events/events.jsonl"
    events = [json.loads(line) for line in event_path.read_text(encoding="utf-8").splitlines() if line]
    updated = [
        event for event in events
        if event.get("event") == "assignment.updated"
        and event.get("data", {}).get("assignment", {}).get("id") == assignment["id"]
        and event.get("data", {}).get("assignment", {}).get("status") == "failed"
    ]
    assert len(updated) == 1, updated
    return {"assignment_id": assignment["id"], "run_id": run_id, "approval_id": approval["id"], "state": "denied", "assignment_failed": True, "job_cancelled": True, "system_stop_messages": 1, "assignment_updated_events": 1, "provider_calls_unchanged": True}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7796")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command")
    parser.add_argument("--old-command")
    parser.add_argument("--new-command")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--browser-bin")
    args = parser.parse_args()
    if args.url.rsplit(":", 1)[-1].rstrip("/") in {"7788", "7789"}:
        parser.error("isolated cancellation smoke cannot use production/mock ports")
    if bool(args.old_command) != bool(args.new_command):
        parser.error("--old-command and --new-command must be provided together")
    if args.new_command:
        args.daemon_command = args.new_command
    if not args.daemon_command:
        parser.error("--daemon-command is required")
    parsed = urlparse(args.url)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "localhost"} or not parsed.port:
        parser.error("use an explicit loopback HTTP port")
    for command in (args.daemon_command, args.old_command):
        if not command:
            continue
        parts = shlex.split(command)
        try:
            port = parts[parts.index("--port") + 1]
        except (ValueError, IndexError):
            parser.error("each command must include --port matching --url")
        if port != str(parsed.port) or "--home" in parts:
            parser.error("isolated commands must use the requested port and MACBOT_HOME")
    with socket.socket() as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        probe.bind(("127.0.0.1", parsed.port))
    if args.home.exists():
        raise SystemExit(f"use a fresh isolated home: {args.home}")
    args.home.parent.mkdir(parents=True, exist_ok=True)
    fake = FakeState(args.home / "test-artifacts")
    provider, provider_url = start_provider(fake)
    daemon = RestartDaemon(args)
    try:
        daemon.start()
        base = args.url.rstrip("/")
        worker, project = setup(base, args.password, provider_url, uuid.uuid4().hex[:8])
        legacy = None
        if args.old_command:
            legacy = run_legacy(base, args.password, args.home, worker, project, daemon, args.old_command, args.new_command, fake)
        current = run_current(base, args.password, args.home, worker, project, fake)
        denied = run_normal_deny(base, args.password, args.home, worker, fake)
        result: dict[str, Any] = {"ok": True, "current": current, "normal_deny": denied, "provider_calls": fake.count()}
        if legacy:
            result["legacy"] = legacy
        print(json.dumps(result, ensure_ascii=False, indent=2))
    finally:
        daemon.close(); provider.shutdown(); provider.server_close()


if __name__ == "__main__":
    main()
