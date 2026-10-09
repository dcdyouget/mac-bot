#!/usr/bin/env python3
"""Production send_msg target routing acceptance.

The daemon is started by this script and is always isolated to a fresh /tmp
home.  A local OpenAI-compatible provider is the only model implementation;
no real credentials or model endpoint are contacted.

The scenario exercises the failure boundary that matters for cross-chat
delivery: a project UUID is not a chat id.  The fake model first emits that
wrong target, observes the tool error in the same durable run, and then emits
the real project chat id.  It also exercises ``to: {bot: ...}`` Bot DM routing
and a successful explicit cross-project chat target.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, http_json, rpc, wait_until


TOKEN = "macbot-chat-targets-fake-token"
DEFAULT_PORT = 7864
DEFAULT_HOME = Path("/tmp/macbot-chat-targets-20261010")


class ProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.tool_results: list[dict[str, Any]] = []
        self.calls: list[dict[str, Any]] = []
        self.scenario: dict[str, Any] = {}
        self.error_seen = False
        self.correction_seen = False
        self.dm_seen = False
        self.explicit_dm_seen = False
        self.cross_seen = False
        self.finished: set[str] = set()

    def record(self, body: dict[str, Any]) -> None:
        with self.lock:
            self.requests.append(body)

    def mark_tool_result(self, marker: str, message: dict[str, Any]) -> bool:
        content = message.get("content", "")
        if not isinstance(content, str):
            content = json.dumps(content, ensure_ascii=False)
        error = bool(message.get("is_error")) or any(
            word in content.lower()
            for word in ("unknown chat", "chat target", "chat not found", "not found", "invalid chat")
        )
        admitted = "message admitted" in content.lower()
        with self.lock:
            self.tool_results.append({"marker": marker, "content": content, "error": error, "admitted": admitted})
            if error and marker == self.scenario.get("wrong_marker"):
                self.error_seen = True
            return error if not admitted else False

    def mark_call(self, marker: str, args: dict[str, Any]) -> None:
        with self.lock:
            self.calls.append({"marker": marker, "args": args})
            if marker == self.scenario.get("wrong_marker") and args.get("chat_id") == self.scenario.get("target_chat_id"):
                self.correction_seen = True
            elif marker == self.scenario.get("dm_marker") and args.get("to"):
                self.dm_seen = True
            elif marker == self.scenario.get("explicit_dm_marker") and args.get("chat_id") == self.scenario.get("explicit_dm_chat_id"):
                self.explicit_dm_seen = True
            elif marker == self.scenario.get("cross_marker") and args.get("chat_id") == self.scenario.get("cross_chat_id"):
                self.cross_seen = True

    def snapshot(self) -> dict[str, Any]:
        with self.lock:
            return {
                "requests": list(self.requests),
                "tool_results": list(self.tool_results),
                "calls": list(self.calls),
                "error_seen": self.error_seen,
                "correction_seen": self.correction_seen,
                "dm_seen": self.dm_seen,
                "explicit_dm_seen": self.explicit_dm_seen,
                "cross_seen": self.cross_seen,
                "finished": set(self.finished),
            }


STATE = ProviderState()


class FakeProviderHandler(BaseHTTPRequestHandler):
    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        payload = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _stream(self, payloads: list[dict[str, Any]]) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
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
            {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 11, "completion_tokens": 5}},
        ])

    def _tool(self, name: str, arguments: dict[str, Any], call_number: int) -> None:
        self._stream([
            {"choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": f"chat-target-call-{call_number}",
                "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)},
            }]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 13, "completion_tokens": 7}},
        ])

    @staticmethod
    def _marker(messages: list[dict[str, Any]]) -> str | None:
        markers = {name: value for name, value in STATE.scenario.items() if name.endswith("_marker") and isinstance(value, str)}
        candidates = [
            (index, marker)
            for index, message in enumerate(messages)
            if message.get("role") == "user"
            for marker in markers.values()
            if marker in str(message.get("content", ""))
        ]
        return max(candidates, default=(-1, None))[1]

    @staticmethod
    def _calls(messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
        result: list[dict[str, Any]] = []
        for message in messages:
            for call in message.get("tool_calls", []):
                function = call.get("function", {})
                if function.get("name") != "send_msg":
                    continue
                try:
                    args = json.loads(function.get("arguments", "{}"))
                except json.JSONDecodeError:
                    args = {}
                if isinstance(args, dict):
                    result.append(args)
        return result

    def _authorized(self) -> bool:
        return self.headers.get("Authorization") == f"Bearer {TOKEN}"

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self._authorized():
            self._json(200, {"data": [{"id": "chat-targets-fake", "object": "model"}]})
            return
        self._json(401 if not self._authorized() else 404, {"error": {"message": "not found"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or not self._authorized():
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        if body.get("model") != "chat-targets-fake":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        STATE.record(body)
        messages = body.get("messages", [])
        marker = self._marker(messages)
        if marker is None:
            self._text("chat target smoke idle")
            return
        tool_messages = [message for message in messages if message.get("role") == "tool"]
        for message in tool_messages:
            STATE.mark_tool_result(marker, message)
        calls = self._calls(messages)
        call_number = len(STATE.snapshot()["requests"])
        scenario = STATE.scenario
        if marker == scenario.get("wrong_marker"):
            saw_error = any(item["error"] for item in STATE.snapshot()["tool_results"] if item["marker"] == marker)
            if saw_error:
                if not any(item["marker"] == marker and item["args"].get("chat_id") == scenario.get("target_chat_id") for item in STATE.snapshot()["calls"]):
                    args = {
                        "intent": "progress",
                        "text": scenario["correct_text"],
                        "chat_id": scenario["target_chat_id"],
                        "mentions": [{"kind": "bot", "bot_id": scenario["target_bot_id"], "instruction": "继续校验正确群目标"}],
                    }
                    STATE.mark_call(marker, args)
                    self._tool("send_msg", args, call_number)
                    return
                self._text("wrong project UUID was rejected and the corrected chat was delivered")
                with STATE.lock:
                    STATE.finished.add(marker)
                return
            if not calls:
                args = {
                    "intent": "progress",
                    "text": scenario["wrong_text"],
                    "chat_id": scenario["target_project_id"],
                    "mentions": [{"kind": "bot", "bot_id": scenario["target_bot_id"], "instruction": "错误目标不得派发"}],
                }
                STATE.mark_call(marker, args)
                self._tool("send_msg", args, call_number)
                return
        elif marker == scenario.get("dm_marker"):
            if not calls:
                args = {"intent": "progress", "text": scenario["dm_text"], "to": {"bot": scenario["target_bot_id"]}, "mentions": []}
                STATE.mark_call(marker, args)
                self._tool("send_msg", args, call_number)
                return
            self._text("valid Bot DM delivered")
            with STATE.lock:
                STATE.finished.add(marker)
            return
        elif marker == scenario.get("explicit_dm_marker"):
            if not calls:
                args = {"intent": "progress", "text": scenario["explicit_dm_text"], "chat_id": scenario["explicit_dm_chat_id"], "mentions": []}
                STATE.mark_call(marker, args)
                self._tool("send_msg", args, call_number)
                return
            self._text("valid explicit Bot DM chat delivered")
            with STATE.lock:
                STATE.finished.add(marker)
            return
        elif marker == scenario.get("cross_marker"):
            if not calls:
                args = {
                    "intent": "progress",
                    "text": scenario["cross_text"],
                    "chat_id": scenario["cross_chat_id"],
                    "mentions": [{"kind": "bot", "bot_id": scenario["target_bot_id"], "instruction": "跨群显式目标"}],
                }
                STATE.mark_call(marker, args)
                self._tool("send_msg", args, call_number)
                return
            self._text("valid cross-project chat delivered")
            with STATE.lock:
                STATE.finished.add(marker)
            return
        self._text("chat target smoke complete")


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def history(base: str, password: str, chat_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"]


def assignments(base: str, password: str, project_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "assignment.list", {"project_id": project_id, "limit": 100})["items"]


def chats(base: str, password: str) -> list[dict[str, Any]]:
    return rpc(base, password, "chat.list")["chats"]


def jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    values: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            values.append(json.loads(line))
    return values


def run_evidence(home: Path, instruction: str) -> tuple[str, dict[str, Any], list[dict[str, Any]]]:
    requests = []
    for path in (home / "data" / "run_requests").glob("run_*.json"):
        try:
            request = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if request.get("instruction") == instruction:
            requests.append(request)
    assert len(requests) == 1, requests
    request = requests[0]
    run_id = request.get("run_id")
    assert isinstance(run_id, str) and run_id
    entries = jsonl(home / "data" / "runs" / run_id / "entries.jsonl")
    assert entries, run_id
    return run_id, request, entries


def wait_run_evidence(home: Path, instruction: str, timeout: float = 20) -> tuple[str, dict[str, Any], list[dict[str, Any]]]:
    result: list[tuple[str, dict[str, Any], list[dict[str, Any]]]] = []

    def ready() -> bool:
        try:
            result[:] = [run_evidence(home, instruction)]
            return True
        except (AssertionError, OSError, json.JSONDecodeError):
            return False

    wait_until(ready, "durable run evidence", timeout)
    return result[0]


def entry_type(entry: dict[str, Any]) -> str | None:
    value = entry.get("type") or entry.get("event")
    if isinstance(value, str):
        return value
    item = entry.get("item")
    return item.get("type") if isinstance(item, dict) and isinstance(item.get("type"), str) else None


def assert_no_wrong_target(home: Path, target_project_id: str, target_chat_id: str, before_assignment_ids: set[str], after_messages: list[dict[str, Any]], after_assignments: list[dict[str, Any]], wrong_text: str, correct_text: str) -> dict[str, Any]:
    assert not (home / "data" / "chats" / target_project_id).exists(), "project UUID was materialized as a chat directory"
    assert all(wrong_text not in json.dumps(message, ensure_ascii=False) for message in after_messages), after_messages
    correct_messages = [message for message in after_messages if correct_text in json.dumps(message, ensure_ascii=False)]
    assert len(correct_messages) == 1, correct_messages
    matching_assignments = [
        item for item in after_assignments
        if item.get("trigger_message_id") == correct_messages[0].get("id")
    ]
    assert len(matching_assignments) == 1, (correct_messages[0], after_assignments)
    assert matching_assignments[0].get("origin_chat_id") == target_chat_id, matching_assignments[0]
    assert matching_assignments[0].get("id") not in before_assignment_ids, matching_assignments[0]
    return matching_assignments[0]


def acceptance(args: argparse.Namespace, provider_url: str) -> None:
    base = args.url.rstrip("/")
    password = args.password
    if args.home is None or not str(args.home).startswith("/tmp/"):
        raise AssertionError("--home must be a fresh path under /tmp")
    assert http_json(f"{base}/api/v1/health").get("protocol") == 1
    boot = rpc(base, password, "bootstrap")
    assert any(bot.get("is_main") for bot in boot["bots"]), boot
    suffix = uuid.uuid4().hex[:8]

    provider = rpc(base, password, "provider.create", {
        "name": f"chat-targets-fake-{suffix}",
        "api_kind": "openai-completions",
        "base_url": provider_url,
        "api_key": TOKEN,
        "client_request_id": f"chat-targets-provider-{suffix}",
    })["provider"]
    assert TOKEN not in json.dumps(provider)
    provider_id = provider["id"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider_id})
    assert any(model["model_id"] == "chat-targets-fake" for model in refreshed["models"])
    model = rpc(base, password, "model.upsert", {
        "provider_id": provider_id,
        "model_id": "chat-targets-fake",
        "display_name": "Chat targets fake",
        "caps": {"vision": False, "tools": True, "reasoning": False},
        "client_request_id": f"chat-targets-model-{suffix}",
    })["model"]["ref"]
    rpc(base, password, "bot.update", {"bot_id": "main", "patch": {"model": model}, "client_request_id": f"chat-targets-main-{suffix}"})

    worker = rpc(base, password, "bot.create", {"name": f"目标 Bot-{suffix}", "model": model, "max_parallel": 2})["bot"]
    source = rpc(base, password, "project.create", {"name": f"来源群-{suffix}", "goal": "send_msg target source", "member_bot_ids": [worker["id"]], "flow": ["目标"], "client_request_id": f"chat-targets-source-{suffix}"})
    target = rpc(base, password, "project.create", {"name": f"正确目标群-{suffix}", "goal": "correct project chat target", "member_bot_ids": [worker["id"]], "flow": ["目标"], "client_request_id": f"chat-targets-target-{suffix}"})
    cross = rpc(base, password, "project.create", {"name": f"跨群目标-{suffix}", "goal": "explicit cross chat target", "member_bot_ids": [worker["id"]], "flow": ["目标"], "client_request_id": f"chat-targets-cross-{suffix}"})
    source_chat = source["chat"]["id"]
    target_id, target_chat = target["project"]["id"], target["chat"]["id"]
    cross_chat = cross["chat"]["id"]
    STATE.scenario = {
        "wrong_marker": f"CHAT_TARGET_WRONG_{suffix}",
        "dm_marker": f"CHAT_TARGET_DM_{suffix}",
        "explicit_dm_marker": f"CHAT_TARGET_EXPLICIT_DM_{suffix}",
        "cross_marker": f"CHAT_TARGET_CROSS_{suffix}",
        "target_project_id": target_id,
        "target_chat_id": target_chat,
        "cross_chat_id": cross_chat,
        "target_bot_id": worker["id"],
        "explicit_dm_chat_id": worker["dm_chat_id"],
        "wrong_text": f"WRONG_TARGET_ATTEMPT_{suffix}",
        "correct_text": f"CORRECT_TARGET_DELIVERED_{suffix}",
        "dm_text": f"VALID_BOT_DM_{suffix}",
        "explicit_dm_text": f"VALID_EXPLICIT_DM_{suffix}",
        "cross_text": f"VALID_CROSS_CHAT_{suffix}",
    }

    before_assignment_ids = {item["id"] for item in assignments(base, password, target_id)}
    # A group follow-up has no explicit mention and therefore exercises the
    # Main Bot's project-scoped run with no source assignment.
    rpc(base, password, "chat.send", {"chat_id": source_chat, "text": STATE.scenario["wrong_marker"], "mentions": [], "client_request_id": f"chat-targets-wrong-{suffix}"})
    wait_until(lambda: STATE.snapshot()["correction_seen"] and STATE.scenario["wrong_marker"] in STATE.snapshot()["finished"], "same-run target correction", 45)
    wait_until(lambda: any(STATE.scenario["correct_text"] in json.dumps(item, ensure_ascii=False) for item in history(base, password, target_chat)), "correct target message", 20)
    after_messages = history(base, password, target_chat)
    after_assignments = assignments(base, password, target_id)
    new_assignment = assert_no_wrong_target(args.home, target_id, target_chat, before_assignment_ids, after_messages, after_assignments, STATE.scenario["wrong_text"], STATE.scenario["correct_text"])
    assert all(STATE.scenario["wrong_text"] not in json.dumps(item, ensure_ascii=False) for item in history(base, password, source_chat)), "failed target leaked a message into the source chat"
    for event_path in (args.home / "data").rglob("*events*.jsonl"):
        for event in jsonl(event_path):
            event_name = event.get("event") or event.get("type")
            if event_name in {"message.created", "message.updated", "assignment.created", "assignment.updated"}:
                assert STATE.scenario["wrong_text"] not in json.dumps(event, ensure_ascii=False), (event_path, event)
    provider_snapshot = STATE.snapshot()
    assert provider_snapshot["error_seen"], provider_snapshot["tool_results"]
    assert any(item["marker"] == STATE.scenario["wrong_marker"] and item["error"] for item in provider_snapshot["tool_results"]), provider_snapshot
    correction = next(item for item in provider_snapshot["calls"] if item["marker"] == STATE.scenario["wrong_marker"] and item["args"].get("chat_id") == target_chat)
    assert correction["args"]["chat_id"] == target_chat
    assert new_assignment["project_id"] == target_id and new_assignment["origin_chat_id"] == target_chat, new_assignment
    assert not assignments(base, password, source["project"]["id"]), "a no-mention group follow-up created a source assignment"
    run_id, run_request, entries = wait_run_evidence(args.home, STATE.scenario["wrong_marker"])
    assert run_request.get("project_id") == source["project"]["id"], run_request
    assert run_request.get("assignment_id") in (None, ""), run_request
    types = [entry_type(item) for item in entries]
    assert {"run.start", "tool.start", "tool.end", "send_msg", "run.end"}.issubset(types), (run_id, types)
    assert any(item.get("type") == "tool.end" and item.get("data", {}).get("is_error") for item in entries), entries
    assert any(item.get("type") == "send_msg" and item.get("data", {}).get("chat_id") == target_chat for item in entries), entries
    assert not any(item.get("type") == "send_msg" and item.get("data", {}).get("chat_id") == target_id for item in entries), entries
    submissions = jsonl(args.home / "data" / "submissions.jsonl")
    invalid_submissions = [item for item in submissions if item.get("receipt", {}).get("payload", {}).get("chat_id") == target_id]
    assert not invalid_submissions, {"target_project_id": target_id, "target_chat_id": target_chat, "invalid_submissions": invalid_submissions, "all_submissions": submissions}
    assert sum(item.get("receipt", {}).get("payload", {}).get("chat_id") == target_chat for item in submissions) == 1, submissions

    dm_routes_before = {chat["id"] for chat in chats(base, password) if chat.get("kind") == "bot_dm"}
    source_before_dm = history(base, password, source_chat)
    rpc(base, password, "chat.send", {"chat_id": source_chat, "text": STATE.scenario["dm_marker"], "mentions": [], "client_request_id": f"chat-targets-dm-{suffix}"})
    wait_until(lambda: STATE.snapshot()["dm_seen"] and STATE.scenario["dm_marker"] in STATE.snapshot()["finished"], "Bot DM target", 45)
    dm_routes = [chat for chat in chats(base, password) if chat.get("kind") == "bot_dm" and chat["id"] not in dm_routes_before and worker["id"] in chat.get("member_bot_ids", [])]
    if not dm_routes:
        dm_routes = [chat for chat in chats(base, password) if chat.get("kind") == "bot_dm" and worker["id"] in chat.get("member_bot_ids", [])]
    assert dm_routes, chats(base, password)
    dm_route = dm_routes[-1]["id"]
    dm_after = history(base, password, dm_route)
    assert sum(STATE.scenario["dm_text"] in json.dumps(item, ensure_ascii=False) for item in dm_after) == 1, dm_after
    dm_refs = [item for item in history(base, password, source_chat) if any(block.get("type") == "bot_dm_ref" and block.get("chat_id") == dm_route for block in item.get("blocks", []))]
    assert dm_refs and len(history(base, password, source_chat)) > len(source_before_dm), dm_refs

    explicit_before = len(history(base, password, worker["dm_chat_id"]))
    rpc(base, password, "chat.send", {"chat_id": source_chat, "text": STATE.scenario["explicit_dm_marker"], "mentions": [], "client_request_id": f"chat-targets-explicit-dm-{suffix}"})
    wait_until(lambda: STATE.snapshot()["explicit_dm_seen"] and STATE.scenario["explicit_dm_marker"] in STATE.snapshot()["finished"], "explicit worker.dm_chat_id target", 45)
    explicit_after = history(base, password, worker["dm_chat_id"])
    assert len(explicit_after) == explicit_before + 1 and STATE.scenario["explicit_dm_text"] in json.dumps(explicit_after[-1], ensure_ascii=False), explicit_after

    rpc(base, password, "chat.send", {"chat_id": source_chat, "text": STATE.scenario["cross_marker"], "mentions": [], "client_request_id": f"chat-targets-cross-{suffix}"})
    wait_until(lambda: STATE.snapshot()["cross_seen"] and STATE.scenario["cross_marker"] in STATE.snapshot()["finished"], "explicit cross-project target", 45)
    cross_after = history(base, password, cross_chat)
    cross_asg_after = assignments(base, password, cross["project"]["id"])
    cross_messages = [item for item in cross_after if STATE.scenario["cross_text"] in json.dumps(item, ensure_ascii=False)]
    assert len(cross_messages) == 1, cross_after
    cross_assignments = [item for item in cross_asg_after if item.get("trigger_message_id") == cross_messages[0].get("id")]
    assert len(cross_assignments) == 1, (cross_messages[0], cross_asg_after)
    assert cross_assignments[0]["project_id"] == cross["project"]["id"] and cross_assignments[0]["origin_chat_id"] == cross_chat, cross_assignments[0]
    print(json.dumps({"ok": True, "port": args.port, "home": str(args.home), "target_project": target_id, "target_chat": target_chat, "provider_tool_results": len(provider_snapshot["tool_results"])}, ensure_ascii=False))


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--daemon-command", required=True, help="exact command used to start the isolated macbotd")
    result.add_argument("--home", type=Path, default=DEFAULT_HOME, help="fresh temporary MACBOT_HOME under /tmp")
    result.add_argument("--port", type=int, default=DEFAULT_PORT)
    result.add_argument("--url", help="override daemon URL; defaults to http://127.0.0.1:<port>")
    result.add_argument("--password", default="dev")
    result.add_argument("--browser-bin", default=None)
    return result


def main() -> int:
    args = parser().parse_args()
    if args.url is None:
        args.url = f"http://127.0.0.1:{args.port}"
    if args.home is None or not str(args.home).startswith("/tmp/"):
        raise SystemExit("--home must be a fresh path under /tmp")
    if args.home.exists():
        raise SystemExit(f"refusing to overwrite existing acceptance home: {args.home}")
    probe = socket.socket()
    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        probe.bind(("127.0.0.1", args.port))
    except OSError as error:
        raise SystemExit(f"port {args.port} is not free; refusing to touch an existing daemon") from error
    finally:
        probe.close()
    provider, provider_url = start_provider()
    daemon = Daemon(args)
    try:
        daemon.start()
        acceptance(args, provider_url)
        return 0
    finally:
        daemon.close()
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    raise SystemExit(main())
