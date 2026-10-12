#!/usr/bin/env python3
"""Reproduce sustained memory state I/O failure through a real macbotd.

This harness uses an isolated temporary MACBOT_HOME and a local deterministic
OpenAI-compatible provider. It holds ``data/memory/state.tmp`` as a directory
until the run has ended, then saves redacted runtime evidence in this folder.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace
from typing import Any
import uuid


ROOT = Path(__file__).resolve().parents[3]
TESTS = ROOT / "server" / "macbotd" / "tests"
sys.path.insert(0, str(TESTS))
from smoke_collaboration import Daemon, rpc, wait_until  # noqa: E402


TOKEN = "macbot-local-memory-io-failure-dummy-token"


class ProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.project_id = ""

    def record(self, body: dict[str, Any]) -> int:
        with self.lock:
            self.requests.append(body)
            return len(self.requests)

    def snapshot(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.requests)


STATE = ProviderState()


class Provider(BaseHTTPRequestHandler):
    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        payload = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

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

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/models":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "dummy token required"}})
            return
        self._json(200, {"data": [{"id": "memory-io-fake", "object": "model"}]})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions":
            self._json(404, {"error": {"message": "not found"}})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "dummy token required"}})
            return
        try:
            size = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(size))
        except (TypeError, ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        if body.get("model") != "memory-io-fake":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        call_number = STATE.record(body)
        if call_number == 1:
            tool = {
                "index": 0,
                "id": "memory-io-failure-call",
                "type": "function",
                "function": {
                    "name": "memory",
                    "arguments": json.dumps({
                        "scope": "project",
                        "project_id": STATE.project_id,
                        "action": "add",
                        "kind": "project",
                        "content": "Persistent state.tmp I/O failure regression sentinel.",
                    }),
                },
            }
            self._stream([
                {"choices": [{"delta": {"tool_calls": [tool]}, "finish_reason": None}]},
                {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
                 "usage": {"prompt_tokens": 9, "completion_tokens": 5}},
            ])
            return
        self._stream([
            {"choices": [{"delta": {"content": "I received the memory write error and continued."},
                          "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}],
             "usage": {"prompt_tokens": 12, "completion_tokens": 8}},
        ])


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def run(args: argparse.Namespace) -> dict[str, Any]:
    suffix = uuid.uuid4().hex[:10]
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    provider_server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    provider_thread = threading.Thread(target=provider_server.serve_forever, daemon=True)
    home = Path(tempfile.mkdtemp(prefix="macbot-memory-io-fix-"))
    daemon = Daemon(SimpleNamespace(
        url=base,
        password=args.password,
        home=home,
        browser_bin=None,
        daemon_command=f"{ROOT / 'server' / 'target' / 'debug' / 'macbotd'} --port {port} --password {args.password}",
    ))
    injection: Path | None = None
    provider_thread.start()
    try:
        daemon.start()
        created = rpc(base, args.password, "provider.create", {
            "name": f"memory-io-fake-{suffix}",
            "api_kind": "openai-completions",
            "base_url": f"http://127.0.0.1:{provider_server.server_port}/v1",
            "api_key": TOKEN,
            "client_request_id": f"memory-io-provider-{suffix}",
        })["provider"]
        rpc(base, args.password, "model.refresh", {"provider_id": created["id"]})
        model = rpc(base, args.password, "model.upsert", {
            "provider_id": created["id"],
            "model_id": "memory-io-fake",
            "display_name": "Memory I/O fake",
            "caps": {"vision": False, "tools": True, "reasoning": False},
            "client_request_id": f"memory-io-model-{suffix}",
        })["model"]["ref"]
        worker = rpc(base, args.password, "bot.create", {
            "name": f"memory-io-worker-{suffix}",
            "model": model,
            "tools": {"files": False, "bash": False, "browser": False,
                      "subagent": False, "web": False, "mcp": False},
        })["bot"]
        project = rpc(base, args.password, "project.create", {
            "name": f"memory-io-project-{suffix}",
            "goal": "Sustained memory state write failure regression",
            "member_bot_ids": [worker["id"]],
        })
        STATE.project_id = project["project"]["id"]
        chat_id = project["chat"]["id"]
        marker = f"MEMORY_IO_FAILURE_{suffix}: trigger one memory write, then report the error."
        rpc(base, args.password, "chat.send", {
            "chat_id": chat_id,
            "text": marker,
            "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": marker}],
            "client_request_id": f"memory-io-send-{suffix}",
        })

        assignment: dict[str, Any] = {}
        def find_assignment() -> bool:
            matches = [item for item in rpc(base, args.password, "assignment.list", {}).get("items", [])
                       if item.get("origin_chat_id") == chat_id and item.get("bot_id") == worker["id"]]
            if matches:
                assignment.update(matches[-1])
                return True
            return False
        wait_until(find_assignment, "memory I/O assignment")
        assignment_id = assignment["id"]
        run_id: str | None = None

        def find_pending_approval() -> bool:
            nonlocal run_id
            trace = rpc(base, args.password, "trace.history", {
                "assignment_id": assignment_id, "tail": True, "limit": 500,
            }).get("items", [])
            starts = [item for item in trace if item.get("type") == "run.start"]
            if starts:
                run_id = starts[-1].get("run_id")
            pending = rpc(base, args.password, "approval.list", {"state": ["pending"]}).get("approvals", [])
            return bool(pending)
        wait_until(find_pending_approval, "memory tool approval", timeout=60)

        injection = home / "data" / "memory" / "state.tmp"
        injection.parent.mkdir(parents=True, exist_ok=True)
        injection.mkdir()
        approval_rows = rpc(base, args.password, "approval.list", {"state": ["pending"]}).get("approvals", [])
        assert approval_rows, "expected the provider's memory call to await approval"
        approval_id = approval_rows[0]["id"]
        rpc(base, args.password, "approval.decide", {
            "approval_id": approval_id,
            "decision": "allow_once",
            "client_request_id": f"memory-io-approval-{suffix}",
        })

        deadline = time.monotonic() + 90
        trace: list[dict[str, Any]] = []
        while time.monotonic() < deadline:
            trace = rpc(base, args.password, "trace.history", {
                "assignment_id": assignment_id, "tail": True, "limit": 500,
            }).get("items", [])
            matching = [item for item in trace if item.get("run_id") == run_id]
            end = next((item for item in reversed(matching) if item.get("type") == "run.end"), None)
            if end:
                break
            time.sleep(0.2)
        else:
            raise AssertionError(f"timed out waiting for run.end, run_id={run_id}")

        # The injected directory stays in place through run.end. This keeps the
        # second save attempt on the same persistent failure path.
        requests = STATE.snapshot()
        matching = [item for item in trace if item.get("run_id") == run_id]
        original_call_id = "memory-io-failure-call"
        failed_tools = []
        context_tool_results = []
        assistant_tool_call_ids_unique = True
        for request_index, body in enumerate(requests, start=1):
            assistant_calls: dict[str, dict[str, Any]] = {}
            for message in body.get("messages", []):
                for call in message.get("tool_calls", []):
                    assistant_call_id = call.get("id")
                    if not isinstance(assistant_call_id, str) or assistant_call_id in assistant_calls:
                        assistant_tool_call_ids_unique = False
                    else:
                        assistant_calls[assistant_call_id] = call
            for message in body.get("messages", []):
                if message.get("role") != "tool" or not message.get("is_error"):
                    continue
                try:
                    content = json.loads(message.get("content", ""))
                except (TypeError, json.JSONDecodeError):
                    continue
                if content.get("ok") is False and "I/O error" in content.get("message", ""):
                    tool_call_id = message.get("tool_call_id")
                    call = assistant_calls.get(tool_call_id)
                    error_result = {"request_index": request_index, "message": message,
                                    "parsed_content": content,
                                    "matching_assistant_tool_call": call is not None}
                    failed_tools.append(error_result)
                    if call is not None:
                        try:
                            call_args = json.loads(call.get("function", {}).get("arguments", "{}"))
                        except (TypeError, json.JSONDecodeError):
                            call_args = {}
                        if call_args.get("_gateway_context_error") == "snapshot_cache_put":
                            context_tool_results.append(error_result)

        event_file = home / "data" / "events" / "events.jsonl"
        event_rows = [json.loads(line) for line in event_file.read_text().splitlines() if line.strip()]
        seqs = [row.get("seq") for row in event_rows]
        unique_seq = len(seqs) == len(set(seqs))
        memory_failed = [row for row in event_rows
                         if row.get("event") == "memory.failed"
                         and row.get("data", {}).get("run_id") == run_id]
        original_failure_events = [row for row in memory_failed
                                   if row.get("data", {}).get("call_id") == original_call_id]
        context_failure_events = [row for row in memory_failed
                                  if row.get("data", {}).get("stage") == "snapshot_cache_put"]
        context_call_ids = [row.get("data", {}).get("call_id") for row in context_failure_events]
        all_failure_call_ids = [row.get("data", {}).get("call_id") for row in memory_failed]
        context_result_ids = [item["message"].get("tool_call_id") for item in context_tool_results]
        held_through_end = injection.is_dir()
        run_end = next(item for item in reversed(matching) if item.get("type") == "run.end")
        run_status = run_end.get("data", {}).get("status")
        checks = {
            "provider_received_subsequent_request": len(requests) >= 2,
            "model_received_error_tool_result": bool(failed_tools),
            "original_memory_write_error_received": any(
                item["message"].get("tool_call_id") == original_call_id and item["matching_assistant_tool_call"]
                for item in failed_tools
            ),
            "snapshot_error_received_with_matching_tool_call": bool(context_tool_results),
            "assistant_tool_call_ids_unique_within_requests": assistant_tool_call_ids_unique,
            "run_completed_done": run_status == "done",
            "original_memory_failed_persisted": len(original_failure_events) == 1,
            "snapshot_memory_failed_persisted": bool(context_failure_events),
            "context_error_wal_ids_match_provider_tool_results": (
                bool(context_result_ids) and set(context_result_ids) == set(context_call_ids)
            ),
            "memory_failed_call_ids_unique": len(all_failure_call_ids) == len(set(all_failure_call_ids)),
            "wal_seq_values_unique": unique_seq,
            "injection_held_through_run_end": held_through_end,
        }
        evidence = {
            "result": "PASS" if all(checks.values()) else "FAIL",
            "checks": checks,
            "scenario": "persistent state.tmp directory held until run.end",
            "gateway": f"127.0.0.1:{port}",
            "provider": f"127.0.0.1:{provider_server.server_port}/v1",
            "home": str(home),
            "injection_path": str(injection),
            "injection_held_through_run_end": held_through_end,
            "run_id": run_id,
            "assignment_id": assignment_id,
            "bot_id": worker["id"],
            "project_id": STATE.project_id,
            "provider_request_count": len(requests),
            "model_error_tool_messages": failed_tools,
            "snapshot_error_tool_messages": context_tool_results,
            "memory_failed_events": memory_failed,
            "wal_event_count": len(event_rows),
            "wal_seq_values_unique": unique_seq,
            "run_end": run_end,
            "trace": matching,
            "provider_requests": requests,
        }
        output = args.evidence
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n")
        summary = {key: value for key, value in evidence.items() if key not in {"trace", "provider_requests"}}
        if not all(checks.values()):
            raise AssertionError(f"sustained memory I/O checks failed: {checks}")
        return summary
    finally:
        if injection is not None and injection.is_dir():
            injection.rmdir()
        daemon.close()
        provider_server.shutdown()
        provider_server.server_close()
        import shutil
        shutil.rmtree(home, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--password", default="dev")
    parser.add_argument("--evidence", type=Path,
                        default=Path(tempfile.gettempdir()) / "macbot-memory-io-fix-evidence.json")
    args = parser.parse_args()
    result = run(args)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
