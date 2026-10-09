#!/usr/bin/env python3
"""Restart recovery for legacy decision waits.

The daemon and execution engine are production code.  The only fake component
is a local OpenAI-compatible provider.  Each case first creates a real
``send_msg(intent=decision, options=...)`` wait, then simulates the old crash
boundary by removing only Question/qid and the orchestrator wait from the
isolated latest snapshot.  Restart must reconstruct the Question from the
canonical message plus the durable checkpoint, without another provider call.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, rpc, wait_until


TOKEN = "macbot-decision-migration-fake-token"
OPTIONS = ["继续", "停止"]


class ProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.calls = 0
        self.bodies: list[dict[str, Any]] = []

    def record(self, body: dict[str, Any]) -> None:
        with self.lock:
            self.calls += 1
            self.bodies.append(body)

    def count(self) -> int:
        with self.lock:
            return self.calls


class DecisionProvider(BaseHTTPRequestHandler):
    state: ProviderState
    call_counter = 0

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
            {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 8, "completion_tokens": 3}},
        ])

    def _decision(self, marker: str) -> None:
        type(self).call_counter += 1
        call_id = f"migration-decision-{type(self).call_counter}"
        arguments = {
            "intent": "decision",
            "text": f"{marker}：请选择继续或停止",
            "mentions": [],
            "options": OPTIONS,
        }
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": call_id,
                "type": "function",
                "function": {"name": "send_msg", "arguments": json.dumps(arguments, ensure_ascii=False)},
            }]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}},
        ])

    def _done_message(self, marker: str) -> None:
        type(self).call_counter += 1
        call_id = f"migration-done-{type(self).call_counter}"
        arguments = {"intent": "done", "text": f"{marker}-done", "mentions": []}
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": call_id,
                "type": "function",
                "function": {"name": "send_msg", "arguments": json.dumps(arguments, ensure_ascii=False)},
            }]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 8, "completion_tokens": 4}},
        ])

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self.headers.get("Authorization") == f"Bearer {TOKEN}":
            self._json(200, {"data": [{"id": "decision-migration-fake", "object": "model"}]})
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
        if body.get("model") != "decision-migration-fake":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        self.state.record(body)
        messages = body.get("messages", [])
        latest_user = next(
            (item.get("content", "") for item in reversed(messages) if item.get("role") == "user"),
            "",
        )
        marker = "MIGRATION_PRIVATE" if "MIGRATION_PRIVATE" in latest_user else "MIGRATION_ASSIGNMENT"
        if latest_user.strip() == OPTIONS[0]:
            for item in reversed(messages):
                calls = item.get("tool_calls", [])
                decisions = [call for call in calls if call.get("function", {}).get("name") == "send_msg"]
                if decisions:
                    arguments = json.loads(decisions[-1]["function"]["arguments"])
                    marker = "MIGRATION_PRIVATE" if "MIGRATION_PRIVATE" in arguments.get("text", "") else "MIGRATION_ASSIGNMENT"
                    break
        has_admitted_send = any(
            item.get("role") == "tool" and "message admitted" in str(item.get("content", ""))
            for item in messages
        )
        if "MIGRATION_MAIN_NO_OPTIONS" in latest_user and not has_admitted_send:
            self._done_message("MIGRATION_MAIN_NO_OPTIONS")
            return
        if has_admitted_send:
            self._done_message(f"{marker}-answer-complete")
        else:
            self._decision(marker)


def start_provider() -> tuple[ThreadingHTTPServer, ProviderState, str]:
    state = ProviderState()
    DecisionProvider.state = state
    server = ThreadingHTTPServer(("127.0.0.1", 0), DecisionProvider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, state, f"http://127.0.0.1:{server.server_port}/v1"


class RestartDaemon(Daemon):
    def kill9_stop(self) -> None:
        if self.process is None:
            raise AssertionError("restart requires --daemon-command")
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=10)
        self.process = None
        time.sleep(0.5)

    def kill9_restart(self) -> None:
        self.kill9_stop()
        self.start()


def traces(base: str, password: str, assignment_id: str | None, chat_id: str) -> list[dict[str, Any]]:
    params: dict[str, Any] = {"limit": 500}
    if assignment_id:
        params["assignment_id"] = assignment_id
    else:
        params["chat_id"] = chat_id
    return rpc(base, password, "trace.history", params)["items"]


def decision_wait(base: str, password: str, assignment_id: str | None, chat_id: str, marker: str) -> dict[str, Any]:
    result: dict[str, Any] = {}

    def ready() -> bool:
        items = traces(base, password, assignment_id, chat_id)
        wait = next(
            (
                item for item in items
                if item.get("type") == "run.wait"
                and item.get("data", {}).get("reason") == "decision"
            ),
            None,
        )
        questions = rpc(base, password, "bootstrap").get("pending", {}).get("questions", [])
        question = next((item for item in questions if marker in item.get("text", "") and item.get("options") == OPTIONS), None)
        if wait is None or question is None:
            return False
        result.update({"wait": wait, "question": question})
        result["run_id"] = wait.get("run_id") or wait.get("data", {}).get("run_id")
        result["message_id"] = wait.get("data", {}).get("message_id")
        return bool(result["run_id"] and result["message_id"])

    wait_until(ready, f"decision wait {marker}", 45)
    history = rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 500})["messages"]
    message = next((item for item in history if item.get("id") == result["message_id"]), None)
    assert message is not None, (result, history)
    result["message"] = message
    return result


def assignment_for_marker(base: str, password: str, marker: str) -> dict[str, Any] | None:
    items = rpc(base, password, "assignment.list", {"limit": 500})["items"]
    matches = [item for item in items if marker in item.get("instruction", "")]
    return matches[-1] if matches else None


def mutate_old_snapshot(home: Path, assignment_id: str | None, message_id: str) -> None:
    operations_path = home / "data/orchestrator/operations.jsonl"
    lines = operations_path.read_text(encoding="utf-8").splitlines()
    target_index = -1
    snapshot: dict[str, Any] | None = None
    for index in range(len(lines) - 1, -1, -1):
        operation = json.loads(lines[index])
        candidate = operation.get("snapshot")
        if isinstance(candidate, dict) and message_id in candidate.get("messages", {}):
            target_index = index
            snapshot = candidate
            break
    assert target_index >= 0 and snapshot is not None, "no operation snapshot contains canonical decision message"
    message = snapshot["messages"][message_id]
    assert message.get("options") == OPTIONS, message
    lost_question_id = message.pop("question_id", None)
    assert lost_question_id is not None
    for field in ("questions", "question_created_at", "question_scopes"):
        snapshot.setdefault(field, {}).pop(lost_question_id, None)
    wire_path = home / "data/chats" / message["chat_id"] / "messages.jsonl"
    wire_rows = [json.loads(line) for line in wire_path.read_text().splitlines() if line]
    wire = next(row for row in reversed(wire_rows) if row["id"] == message_id)
    wire["blocks"] = [{"type": "text", "markdown": message["text"]}]
    with wire_path.open("a", encoding="utf-8") as output:
        output.write(json.dumps(wire, ensure_ascii=False) + "\n")
    if assignment_id is not None:
        assignment = snapshot["assignments"][assignment_id]
        assignment["status"] = "working"
        assignment["wait"] = None
    operation = json.loads(lines[target_index])
    operation["snapshot"] = snapshot
    lines[target_index] = json.dumps(operation, ensure_ascii=False, separators=(",", ":"))
    operations_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    state_path = home / "data/orchestrator/state.json"
    state_path.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def restored_question(base: str, password: str, chat_id: str, message_id: str, marker: str) -> dict[str, Any]:
    result: dict[str, Any] = {}

    def ready() -> bool:
        pending = rpc(base, password, "bootstrap").get("pending", {}).get("questions", [])
        matches = [item for item in pending if marker in item.get("text", "") and item.get("options") == OPTIONS]
        if len(matches) != 1:
            return False
        workbench = rpc(base, password, "workbench.get", {})
        workbench = workbench.get("workbench", workbench)
        waiting = [
            item for item in workbench.get("waiting", [])
            if item.get("kind") == "question" and item.get("question", {}).get("id") == matches[0].get("id")
        ]
        history = rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 500})["messages"]
        message = next((item for item in history if item.get("id") == message_id), None)
        blocks = message.get("blocks", []) if message else []
        linked = any(block.get("type") == "question" and block.get("question_id") == matches[0].get("id") for block in blocks)
        if len(waiting) != 1 or not linked:
            return False
        result.update(question=matches[0], waiting=waiting, message=message)
        return True

    wait_until(ready, f"restored question {marker}", 30)
    return result


def run_case(args: argparse.Namespace, base: str, daemon: RestartDaemon, marker: str, assignment_id: str | None, chat_id: str, provider: ProviderState) -> dict[str, Any]:
    sent = rpc(
        base,
        args.password,
        "chat.send",
        {
            "chat_id": chat_id,
            "text": marker,
            "mentions": [] if assignment_id is None else [{"kind": "bot", "bot_id": assignment_id, "instruction": marker}],
            "client_request_id": f"migration-{marker.lower()}-{uuid.uuid4().hex[:8]}",
        },
    )
    # The assignment id is supplied by the caller only for routing; the actual
    # assignment is discovered from the created instruction below.
    actual_assignment: dict[str, Any] | None = None
    if assignment_id is not None:
        def assignment_ready() -> bool:
            nonlocal actual_assignment
            actual_assignment = assignment_for_marker(base, args.password, marker)
            return actual_assignment is not None
        wait_until(assignment_ready, f"assignment {marker}", 30)
    else:
        actual_assignment = assignment_for_marker(base, args.password, marker)
    trace_assignment = actual_assignment["id"] if actual_assignment else None
    wait = decision_wait(base, args.password, trace_assignment, chat_id, marker)
    calls_at_wait = provider.count()
    run_id = wait["run_id"]
    message_id = wait["message_id"]
    daemon.kill9_stop()
    mutate_old_snapshot(args.home, trace_assignment, message_id)
    daemon.start()
    restored = restored_question(base, args.password, chat_id, message_id, marker)
    assert provider.count() == calls_at_wait, (marker, calls_at_wait, provider.count())
    for field in ("id", "seq", "created_at"):
        assert restored["message"][field] == wait["message"][field]
    question_id = restored["question"]["id"]
    rpc(
        base,
        args.password,
        "question.answer",
        {"question_id": question_id, "option_index": 0, "client_request_id": f"migration-answer-{marker.lower()}"},
    )

    def done() -> bool:
        items = traces(base, args.password, trace_assignment, chat_id)
        return any(
            (item.get("run_id") or item.get("data", {}).get("run_id")) == run_id
            and item.get("type") == "run.end"
            and item.get("data", {}).get("status") == "done"
            for item in items
        )

    wait_until(done, f"same run completion {marker}", 45)
    history = rpc(base, args.password, "chat.history", {"chat_id": chat_id, "limit": 500})["messages"]
    assert any(f"{marker}-answer-complete" in item.get("fallback_text", "") for item in history), history
    starts = [item for item in traces(base, args.password, trace_assignment, chat_id) if item.get("type") == "run.start" and (item.get("run_id") or item.get("data", {}).get("run_id")) == run_id]
    assert len(starts) == 1, starts
    before_restart_calls = provider.count()
    daemon.kill9_restart()
    assert provider.count() == before_restart_calls
    assert not [item for item in rpc(base, args.password, "bootstrap").get("pending", {}).get("questions", []) if marker in item.get("text", "")]
    return {"marker": marker, "run_id": run_id, "message_id": message_id, "question_id": question_id, "assignment_id": trace_assignment, "provider_calls": provider.count(), "sent_id": sent["message"]["id"]}


def acceptance(args: argparse.Namespace, provider_url: str, provider: ProviderState, daemon: RestartDaemon) -> dict[str, Any]:
    base = args.url.rstrip("/")
    boot = rpc(base, args.password, "bootstrap")
    main = next(item for item in boot["bots"] if item["is_main"])
    suffix = uuid.uuid4().hex[:8]
    created = rpc(base, args.password, "provider.create", {"name": f"decision-migration-{suffix}", "api_kind": "openai-completions", "base_url": provider_url, "api_key": TOKEN})["provider"]
    refreshed = rpc(base, args.password, "model.refresh", {"provider_id": created["id"]})
    assert any(item["model_id"] == "decision-migration-fake" for item in refreshed["models"])
    model = rpc(base, args.password, "model.upsert", {"provider_id": created["id"], "model_id": "decision-migration-fake", "display_name": "decision migration fake", "caps": {"vision": False, "tools": True, "reasoning": False}})["model"]["ref"]
    worker = rpc(base, args.password, "bot.create", {"name": f"decision-migration-worker-{suffix}", "model": model})["bot"]

    assignment_case = run_case(args, base, daemon, "MIGRATION_ASSIGNMENT", worker["id"], "chat_main", provider)
    private_case = run_case(args, base, daemon, "MIGRATION_PRIVATE", None, worker["dm_chat_id"], provider)
    rpc(base, args.password, "bot.update", {"bot_id": main["id"], "patch": {"model": model}})
    no_options = rpc(base, args.password, "chat.send", {"chat_id": "chat_main", "text": "MIGRATION_MAIN_NO_OPTIONS", "mentions": [{"kind": "main"}], "client_request_id": f"migration-main-{suffix}"})
    main_assignment = assignment_for_marker(base, args.password, "MIGRATION_MAIN_NO_OPTIONS")
    main_trace_id = main_assignment["id"] if main_assignment else None
    wait_until(
        lambda: any(
            item.get("type") == "run.end" and item.get("data", {}).get("status") == "done"
            for item in traces(base, args.password, main_trace_id, "chat_main")
        ),
        "main no-options history",
        45,
    )
    main_history = rpc(base, args.password, "chat.history", {"chat_id": "chat_main", "limit": 500})["messages"]
    main_message = next(item for item in main_history if item.get("fallback_text") == "MIGRATION_MAIN_NO_OPTIONS-done")
    assert not any(block.get("type") == "question" for block in main_message.get("blocks", [])), main_message
    assert not any("MIGRATION_MAIN_NO_OPTIONS" in item.get("text", "") for item in rpc(base, args.password, "bootstrap").get("pending", {}).get("questions", []))
    return {"checks": {"assignment_wait_recovered": True, "private_wait_recovered": True, "same_run_resumed": True, "restart_idempotent": True, "no_provider_call_during_restore": True, "no_question_for_no_options_history": True}, "cases": [assignment_case, private_case], "provider_calls": provider.count(), "main_bot": main["id"], "worker": worker["id"], "no_options_message_id": no_options["message"]["id"]}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7854")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--browser-bin", default=os.environ.get("MACBOT_BROWSER_BIN"))
    args = parser.parse_args()
    provider_server, provider, provider_url = start_provider()
    daemon = RestartDaemon(args)
    try:
        daemon.start()
        result = acceptance(args, provider_url, provider, daemon)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    finally:
        daemon.close()
        provider_server.shutdown()
        provider_server.server_close()


if __name__ == "__main__":
    main()
