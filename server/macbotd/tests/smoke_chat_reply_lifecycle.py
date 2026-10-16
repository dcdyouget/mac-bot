#!/usr/bin/env python3
"""Production chat reply lifecycle acceptance with a local fake provider.

The daemon, durable runtime, chat RPCs, events, and message store are real.  The
only provider is a local OpenAI-compatible HTTP server.  Each invocation uses
a fresh home/config and random loopback ports; no real credential or model is
read or contacted.

This covers native DM streaming, a DM send_msg acknowledgement followed by a
native final answer, two Bot claims in one project chat, empty/null/omitted
message_id behavior, per-Bot delivery and placeholder identity, scoped
message_id rejection, failed-run settlement, and same-run recovery with pending
steers after a daemon crash.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, rpc, wait_until


TOKEN = "macbot-chat-reply-lifecycle-fake-token"
MODEL_A = "chat-reply-lifecycle-a"
MODEL_B = "chat-reply-lifecycle-b"


class ProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.models = {MODEL_A, MODEL_B}
        self.markers: dict[str, str] = {}
        self.requests: list[dict[str, Any]] = []
        self.requested: dict[str, threading.Event] = {}
        self.counts: dict[tuple[str, str], int] = {}
        self.placeholders: dict[tuple[str, str], str] = {}
        self.foreign_active_id: str | None = None
        self.foreign_finished_id: str | None = None
        self.steer_markers_seen: set[str] = set()
        self.steer_source_labels: dict[str, str] = {}
        self.group_after_b = threading.Event()
        self.multi_steer_after_ack = threading.Event()
        self.multi_steer_ready_for_restart = threading.Event()
        self.multi_steer_after_null = threading.Event()
        self.multi_steer_after_omitted = threading.Event()
        self.releases = {
            "group_a": threading.Event(),
            "group_a_after_b": threading.Event(),
            "group_b": threading.Event(),
            "multi_steer_initial": threading.Event(),
            "multi_steer_after_ack": threading.Event(),
            "multi_steer_ready_for_restart": threading.Event(),
            "multi_steer_after_null": threading.Event(),
            "multi_steer_after_omitted": threading.Event(),
            "native_dm": threading.Event(),
            "dm_final": threading.Event(),
            "recovery_old": threading.Event(),
            "failure": threading.Event(),
            "cancel": threading.Event(),
        }

    def register_markers(self, values: dict[str, str]) -> None:
        with self.lock:
            self.markers = dict(values)
            self.requested = {name: threading.Event() for name in values}
            self.counts.clear()
            self.placeholders.clear()
            self.steer_markers_seen.clear()
            self.steer_source_labels.clear()
            self.group_after_b.clear()
            self.multi_steer_after_ack.clear()
            self.multi_steer_ready_for_restart.clear()
            self.multi_steer_after_null.clear()
            self.multi_steer_after_omitted.clear()
            self.foreign_active_id = None
            self.foreign_finished_id = None
            for event in self.releases.values():
                event.clear()

    def record(self, model: str, marker: str, body: dict[str, Any]) -> tuple[int, str | None]:
        placeholder = placeholder_from_schema(body)
        with self.lock:
            key = (marker, model)
            self.counts[key] = self.counts.get(key, 0) + 1
            count = self.counts[key]
            if placeholder:
                self.placeholders.setdefault(key, placeholder)
            self.requests.append({"marker": marker, "model": model, "body": body})
            marker_name = next(name for name, value in self.markers.items() if value == marker)
            self.requested[marker_name].set()
            request_messages = json.dumps(body.get("messages", []), ensure_ascii=False)
            self.steer_markers_seen.update(
                value for name, value in self.markers.items()
                if name.startswith("steer_") and value in request_messages
            )
            return count, placeholder

    def snapshot(self) -> dict[str, Any]:
        with self.lock:
            return {
                "requests": list(self.requests),
                "counts": dict(self.counts),
                "placeholders": dict(self.placeholders),
                "foreign_active_id": self.foreign_active_id,
                "foreign_finished_id": self.foreign_finished_id,
                "steer_markers_seen": set(self.steer_markers_seen),
            }


STATE = ProviderState()


def placeholder_from_schema(body: dict[str, Any]) -> str | None:
    """Read the run-owned placeholder ID advertised by the production schema."""
    reservations = reservations_from_schema(body)
    return reservations[0].get("message_id") if reservations else None


def reservations_from_schema(body: dict[str, Any]) -> list[dict[str, Any]]:
    """Read all source-to-placeholder pairs advertised by send_msg."""
    for tool in body.get("tools", []):
        function = tool.get("function", {}) if isinstance(tool, dict) else {}
        if function.get("name") != "send_msg":
            continue
        prop = function.get("parameters", {}).get("properties", {}).get("message_id", {})
        description = prop.get("description", "") if isinstance(prop, dict) else ""
        if isinstance(description, str):
            marker = "pending_reservations="
            marker_index = description.find(marker)
            if marker_index < 0:
                continue
            array_start = description.find("[", marker_index + len(marker))
            array_end = description.find("]", array_start + 1)
            if array_start < 0 or array_end < 0:
                continue
            try:
                reservations = json.loads(description[array_start:array_end + 1])
            except json.JSONDecodeError:
                continue
            if isinstance(reservations, list):
                return [reservation for reservation in reservations if isinstance(reservation, dict)]
    return []


class LifecycleProviderHandler(BaseHTTPRequestHandler):
    state = STATE

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
        try:
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
        except (BrokenPipeError, ConnectionResetError, OSError):
            # A daemon restart/cancel may close its provider socket while a
            # scripted response is being released.
            return

    def _text_chunks(self, chunks: list[str]) -> None:
        payloads = [
            {"choices": [{"delta": {"content": chunk}, "finish_reason": None}]}
            for chunk in chunks
        ]
        payloads.append({
            "choices": [{"delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 4},
        })
        self._stream(payloads)

    def _tool(self, arguments: dict[str, Any], content: str | None = None) -> None:
        call_id = f"chat-reply-lifecycle-{uuid.uuid4().hex}"
        delta: dict[str, Any] = {
            "tool_calls": [{
                "index": 0,
                "id": call_id,
                "type": "function",
                "function": {"name": "send_msg", "arguments": json.dumps(arguments, ensure_ascii=False)},
            }],
        }
        if content is not None:
            # In a group, this text is model-internal and must never become a
            # public chat message alongside the send_msg call.
            delta["content"] = content
        self._stream([
            {"choices": [{"delta": delta, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 9, "completion_tokens": 5}},
        ])

    @staticmethod
    def _send_msg_calls(messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
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
            self._json(200, {"data": [{"id": model, "object": "model"} for model in sorted(self.state.models)]})
            return
        self._json(401 if not self._authorized() else 404, {"error": {"message": "not found"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or not self._authorized():
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        messages = body.get("messages", [])
        with self.state.lock:
            marker = None
            # Prefer the latest user turn. A group steer is an extra user
            # message on the same run, so skip its marker and keep the
            # original task marker for the fake provider's scenario routing.
            for message in reversed(messages):
                if message.get("role") != "user":
                    continue
                content = str(message.get("content", ""))
                marker = next((
                    value for name, value in self.state.markers.items()
                    if not name.startswith("steer_") and value in content
                ), None)
                if marker is not None:
                    break
        if marker is None:
            self._text_chunks(["lifecycle idle"])
            return
        model = body.get("model", "")
        count, placeholder = self.state.record(model, marker, body)
        previous_calls = self._send_msg_calls(messages)

        marker_name = next(name for name, value in self.state.markers.items() if value == marker)
        if marker_name == "group":
            if model == MODEL_A:
                if len(previous_calls) == 0:
                    self.state.releases["group_a"].wait(40)
                    if not placeholder:
                        self._json(500, {"error": {"message": "send_msg schema omitted the placeholder id"}})
                        return
                    self._tool({"intent": "ack", "text": "GROUP_ACK_A", "message_id": placeholder, "mentions": []}, content="GROUP_INTERNAL_A")
                    return
                if len(previous_calls) == 1:
                    self._tool({"intent": "ack", "text": "GROUP_ACK_B_EMPTY_ID", "message_id": "", "mentions": []})
                    return
                if len(previous_calls) == 2:
                    self.state.group_after_b.set()
                    self.state.releases["group_a_after_b"].wait(40)
                    self._tool({"intent": "ack", "text": "GROUP_ACK_C_NULL_ID", "message_id": None, "mentions": []})
                    return
                if len(previous_calls) == 3:
                    self._tool({"intent": "done", "text": "GROUP_DONE_D_OMITTED_ID", "mentions": []})
                    return
            elif model == MODEL_B:
                if count == 1:
                    self.state.releases["group_b"].wait(40)
                    if not self.state.foreign_active_id:
                        self._json(500, {"error": {"message": "test did not provide an active foreign placeholder"}})
                        return
                    self._tool({"intent": "progress", "text": "MUST_NOT_REPLACE_A", "message_id": self.state.foreign_active_id, "mentions": []}, content="GROUP_INTERNAL_B")
                    return
                if len(previous_calls) == 1:
                    if not placeholder:
                        self._json(500, {"error": {"message": "send_msg schema omitted Bot B placeholder id"}})
                        return
                    self._tool({"intent": "done", "text": "GROUP_DONE_B", "message_id": placeholder, "mentions": []})
                    return

        elif marker_name == "dm_ack_native":
            if count == 1:
                if not placeholder:
                    self._json(500, {"error": {"message": "send_msg schema omitted DM placeholder id"}})
                    return
                self._tool({"intent": "ack", "text": "DM_ACK_STAYS", "message_id": placeholder, "mentions": []})
                return
            if len(previous_calls) == 1:
                if count == 2:
                    self.state.releases["dm_final"].wait(40)
                self._text_chunks(["DM_NATIVE_", "FINAL_AFTER_ACK"])
                return

        elif marker_name == "native_dm":
            if count == 1:
                self.state.releases["native_dm"].wait(40)
                self._text_chunks(["DM_NATIVE_", "STREAM_ONLY"])
                return

        elif marker_name == "cross_scope":
            if count == 1:
                foreign_id = self.state.foreign_finished_id
                if not foreign_id:
                    self._json(500, {"error": {"message": "test did not provide a finished foreign message id"}})
                    return
                self._tool({"intent": "progress", "text": "MUST_NOT_BE_SENT", "message_id": foreign_id, "mentions": []})
                return
            if len(previous_calls) == 1:
                if not placeholder:
                    self._json(500, {"error": {"message": "cross-scope run placeholder id missing"}})
                    return
                self._tool({"intent": "done", "text": "CROSS_SCOPE_RETRIED", "message_id": placeholder, "mentions": []})
                return

        elif marker_name == "finished_id":
            if count == 1:
                foreign_id = self.state.foreign_finished_id
                if not foreign_id:
                    self._json(500, {"error": {"message": "test did not provide a finished same-chat message id"}})
                    return
                self._tool({"intent": "progress", "text": "MUST_NOT_EDIT_FINISHED", "message_id": foreign_id, "mentions": []})
                return
            if len(previous_calls) == 1:
                if not placeholder:
                    self._json(500, {"error": {"message": "finished-id run placeholder id missing"}})
                    return
                self._tool({"intent": "done", "text": "FINISHED_ID_RETRIED", "message_id": placeholder, "mentions": []})
                return

        elif marker_name == "recovery":
            if count == 1:
                self.state.releases["recovery_old"].wait(40)
                # The first daemon is killed while waiting here; writing to its
                # closed socket is expected and is swallowed below.
                self._tool({"intent": "done", "text": "STALE_RECOVERY_RESPONSE", "message_id": placeholder or "", "mentions": []})
                return
            if count == 2:
                if not placeholder:
                    self._json(500, {"error": {"message": "recovery placeholder id missing"}})
                    return
                self._tool({"intent": "done", "text": "RECOVERED_SAME_PLACEHOLDER", "message_id": placeholder, "mentions": []})
                return

        elif marker_name == "failure":
            self.state.releases["failure"].wait(40)
            self._json(503, {"error": {"message": "scripted fake provider failure"}})
            return

        elif marker_name == "multi_steer":
            if count == 1:
                self.state.releases["multi_steer_initial"].wait(40)
                if not placeholder:
                    self._json(500, {"error": {"message": "multi-steer source placeholder missing"}})
                    return
                self._tool({"intent": "ack", "text": "MULTI_STEER_ACK_A", "message_id": placeholder, "mentions": []})
                return
            if count == 2:
                self.state.multi_steer_after_ack.set()
                self.state.releases["multi_steer_after_ack"].wait(40)
                self._tool({
                    "intent": "ack",
                    "text": "MULTI_STALE_BEFORE_STEER_1",
                    "message_id": None,
                    "mentions": [],
                })
                return
            if count == 3:
                self._tool({
                    "intent": "ack",
                    "text": "MULTI_EMPTY_BEFORE_RESTART",
                    "message_id": "",
                    "mentions": [],
                })
                return
            if count == 4:
                self.state.multi_steer_ready_for_restart.set()
                self.state.releases["multi_steer_ready_for_restart"].wait(40)
                # The first daemon is killed while this provider request is
                # blocked. Writing its response to the closed socket is safe.
                self._tool({"intent": "ack", "text": "MULTI_STALE_PRE_RESTART", "mentions": []})
                return
            if count == 5:
                self._tool({
                    "intent": "ack",
                    "text": "MULTI_NULL_DURING_PENDING",
                    "message_id": None,
                    "mentions": [],
                })
                return
            if count == 6:
                self.state.multi_steer_after_null.set()
                self.state.releases["multi_steer_after_null"].wait(40)
                self._tool({
                    "intent": "ack",
                    "text": "MULTI_EMPTY_DURING_PENDING",
                    "message_id": "",
                    "mentions": [],
                })
                return
            if count == 7:
                self._tool({
                    "intent": "ack",
                    "text": "MULTI_OMITTED_DURING_PENDING",
                    "mentions": [],
                })
                return
            if count == 8:
                self.state.multi_steer_after_omitted.set()
                self.state.releases["multi_steer_after_omitted"].wait(40)
            reservations = reservations_from_schema(body)
            steer_reservations = [
                item for item in reservations
                if item.get("source_message_id") in self.state.steer_source_labels
            ]
            if steer_reservations:
                reservation = min(
                    steer_reservations,
                    key=lambda item: self.state.steer_source_labels[item["source_message_id"]],
                )
                source_id = reservation.get("source_message_id")
                label = self.state.steer_source_labels[source_id]
                self._tool({
                    "intent": "ack",
                    "text": f"MULTI_STEER_ACK_{label}",
                    "message_id": reservation.get("message_id"),
                    "mentions": [],
                })
                return
            if reservations:
                reservation = reservations[0]
                self._tool({"intent": "ack", "text": "MULTI_STEER_ACK_OTHER", "message_id": reservation.get("message_id"), "mentions": []})
                return
            if len(previous_calls) < 3:
                self._json(500, {"error": {"message": "pending steer reservations were lost during recovery"}})
                return
            self._tool({"intent": "done", "text": "MULTI_STEER_DONE_D", "mentions": []})
            return

        elif marker_name == "cancel":
            self.state.releases["cancel"].wait(40)
            self._tool({"intent": "done", "text": "STALE_CANCEL_RESPONSE", "message_id": placeholder or "", "mentions": []})
            return

        self._json(500, {"error": {"message": f"unexpected fake-provider turn {marker_name} #{count}"}})


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), LifecycleProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def history(base: str, password: str, chat_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 100})["messages"]


def traces(base: str, password: str, chat_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"chat_id": chat_id, "tail": True, "limit": 500})["items"]


def all_events(home: Path) -> list[dict[str, Any]]:
    path = home / "data" / "events" / "events.jsonl"
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def bot_messages(messages: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [message for message in messages if message.get("sender", {}).get("kind") == "bot"]


def sender_bot_id(message: dict[str, Any]) -> str | None:
    sender = message.get("sender", {})
    return sender.get("bot_id") if isinstance(sender, dict) else None


def wait_for_run_end(base: str, password: str, chat_id: str, previous_ends: int, timeout: float = 40) -> list[dict[str, Any]]:
    found: list[dict[str, Any]] = []

    def ready() -> bool:
        found[:] = traces(base, password, chat_id)
        return sum(item.get("type") == "run.end" for item in found) > previous_ends

    wait_until(ready, f"run.end in {chat_id}", timeout)
    return found


def run_end_count(items: list[dict[str, Any]]) -> int:
    return sum(item.get("type") == "run.end" for item in items)


def wait_bot_message(base: str, password: str, chat_id: str, text: str, timeout: float = 40) -> list[dict[str, Any]]:
    found: list[dict[str, Any]] = []

    def ready() -> bool:
        found[:] = history(base, password, chat_id)
        return any(text in message.get("fallback_text", "") for message in bot_messages(found))

    wait_until(ready, f"bot message containing {text}", timeout)
    return found


class RestartableDaemon(Daemon):
    def kill9(self) -> None:
        if self.process is None or self.process.poll() is not None:
            raise AssertionError("kill9 requires a live managed daemon")
        os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait(timeout=10)


def acceptance(args: argparse.Namespace) -> dict[str, Any]:
    base = args.url
    password = args.password
    provider, provider_url = start_provider()
    daemon = RestartableDaemon(args)
    suffix = uuid.uuid4().hex[:8]
    marker = {
        "group": f"LIFECYCLE_GROUP_{suffix}",
        "dm_ack_native": f"LIFECYCLE_DM_ACK_NATIVE_{suffix}",
        "native_dm": f"LIFECYCLE_NATIVE_DM_{suffix}",
        "cross_scope": f"LIFECYCLE_CROSS_SCOPE_{suffix}",
        "finished_id": f"LIFECYCLE_FINISHED_ID_{suffix}",
        "recovery": f"LIFECYCLE_RECOVERY_{suffix}",
        "failure": f"LIFECYCLE_FAILURE_{suffix}",
        "cancel": f"LIFECYCLE_CANCEL_{suffix}",
        "multi_steer": f"LIFECYCLE_MULTI_STEER_{suffix}",
        "steer_1": f"LIFECYCLE_STEER_1_{suffix}",
        "steer_2": f"LIFECYCLE_STEER_2_{suffix}",
    }
    STATE.register_markers(marker)
    try:
        daemon.start()
        provider_result = rpc(base, password, "provider.create", {
            "name": f"reply-lifecycle-{suffix}",
            "api_kind": "openai-completions",
            "base_url": provider_url,
            "api_key": TOKEN,
            "client_request_id": f"reply-lifecycle-provider:{suffix}",
        })["provider"]
        assert TOKEN not in json.dumps(provider_result), provider_result
        rpc(base, password, "model.refresh", {"provider_id": provider_result["id"]})

        def add_model(name: str) -> str:
            return rpc(base, password, "model.upsert", {
                "provider_id": provider_result["id"],
                "model_id": name,
                "display_name": name,
                "caps": {"vision": False, "tools": True, "reasoning": False},
                "client_request_id": f"reply-lifecycle-model:{name}:{suffix}",
            })["model"]["ref"]

        model_a = add_model(MODEL_A)
        model_b = add_model(MODEL_B)
        no_tools = {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False}
        bot_a = rpc(base, password, "bot.create", {"name": f"Lifecycle A {suffix}", "model": model_a, "max_parallel": 2, "tools": no_tools})["bot"]
        bot_b = rpc(base, password, "bot.create", {"name": f"Lifecycle B {suffix}", "model": model_b, "max_parallel": 2, "tools": no_tools})["bot"]
        project_result = rpc(base, password, "project.create", {
            "name": f"Reply lifecycle {suffix}",
            "goal": "Verify source message reply identity",
            "member_bot_ids": [bot_a["id"], bot_b["id"]],
            "client_request_id": f"reply-lifecycle-project:{suffix}",
        })
        project = project_result["project"]
        group_chat = project_result["chat"]["id"]
        dm_chat = bot_a["dm_chat_id"]
        bot_b_dm = bot_b["dm_chat_id"]

        # Two Bot claims for one group user message.  Hold both model requests
        # after claim so the test can observe two distinct empty placeholders.
        group_sent = rpc(base, password, "chat.send", {
            "chat_id": group_chat,
            "text": marker["group"],
            "mentions": [
                {"kind": "bot", "bot_id": bot_a["id"], "instruction": marker["group"]},
                {"kind": "bot", "bot_id": bot_b["id"], "instruction": marker["group"]},
            ],
            "client_request_id": f"reply-lifecycle-group:{suffix}",
        })["message"]
        duplicate_group = rpc(base, password, "chat.send", {
            "chat_id": group_chat,
            "text": marker["group"],
            "mentions": [
                {"kind": "bot", "bot_id": bot_a["id"], "instruction": marker["group"]},
                {"kind": "bot", "bot_id": bot_b["id"], "instruction": marker["group"]},
            ],
            "client_request_id": f"reply-lifecycle-group:{suffix}",
        })["message"]
        assert duplicate_group["id"] == group_sent["id"], (group_sent, duplicate_group)
        assert STATE.requested["group"].wait(20), "no Bot started the group run"
        wait_until(lambda: STATE.snapshot()["counts"].get((marker["group"], MODEL_A), 0) >= 1 and STATE.snapshot()["counts"].get((marker["group"], MODEL_B), 0) >= 1, "both group Bots reach fake provider", 25)
        placeholder_a = STATE.snapshot()["placeholders"].get((marker["group"], MODEL_A))
        placeholder_b = STATE.snapshot()["placeholders"].get((marker["group"], MODEL_B))
        assert placeholder_a and placeholder_b and placeholder_a != placeholder_b, STATE.snapshot()
        group_before = history(base, password, group_chat)
        empty_placeholders = [message for message in bot_messages(group_before) if message.get("source_message_id") == group_sent["id"]]
        assert {message["id"] for message in empty_placeholders} == {placeholder_a, placeholder_b}, (placeholder_a, placeholder_b, empty_placeholders)
        for placeholder in empty_placeholders:
            assert placeholder.get("fallback_text", "") == "" and placeholder.get("streaming") is True, placeholder
        with STATE.lock:
            STATE.foreign_active_id = placeholder_a
        STATE.releases["group_b"].set()
        # Bot B first tries Bot A's active placeholder. That must fail without
        # changing A; its retry then fills its own run-owned placeholder.
        wait_bot_message(base, password, group_chat, "GROUP_DONE_B")
        wait_until(
            lambda: next(
                (item.get("status") == "done" for item in rpc(base, password, "assignment.list", {"project_id": project["id"], "limit": 100})["items"]
                 if item.get("bot_id") == bot_b["id"] and item.get("trigger_message_id") == group_sent["id"]),
                False,
            ),
            "Bot B assignment terminates while Bot A remains active",
            20,
        )
        after_b = history(base, password, group_chat)
        a_while_active = next(message for message in bot_messages(after_b) if message["id"] == placeholder_a)
        assert a_while_active.get("fallback_text") == "" and a_while_active.get("streaming") is True, a_while_active

        assignments = rpc(base, password, "assignment.list", {"project_id": project["id"], "limit": 100})["items"]
        group_assignments = [item for item in assignments if item.get("trigger_message_id") == group_sent["id"]]
        assert len(group_assignments) == 2, group_assignments
        assert {item.get("bot_id") for item in group_assignments} == {bot_a["id"], bot_b["id"]}, group_assignments
        assignment_a = next(item for item in group_assignments if item.get("bot_id") == bot_a["id"])
        STATE.releases["group_a"].set()
        wait_until(lambda: STATE.group_after_b.is_set(), "Bot A persists its second independent ack", 40)
        checkpoint_history = history(base, password, group_chat)
        a_ack_checkpoint = next(message for message in bot_messages(checkpoint_history) if message["id"] == placeholder_a)
        b_ack_checkpoint = next(message for message in bot_messages(checkpoint_history) if message.get("fallback_text") == "GROUP_ACK_B_EMPTY_ID")
        assert a_ack_checkpoint.get("fallback_text") == "GROUP_ACK_A", a_ack_checkpoint
        assert b_ack_checkpoint["id"] != placeholder_a, b_ack_checkpoint
        daemon.kill9()
        STATE.releases["group_a_after_b"].set()
        time.sleep(0.4)
        daemon.start()
        wait_until(lambda: len([m for m in bot_messages(history(base, password, group_chat)) if m.get("fallback_text") == "GROUP_DONE_D_OMITTED_ID"]) == 1, "Bot A resumes after its second ack and completes", 60)
        group_history = history(base, password, group_chat)
        a_ack = next(message for message in bot_messages(group_history) if message["id"] == placeholder_a)
        a_ack_b = next(message for message in bot_messages(group_history) if message.get("fallback_text") == "GROUP_ACK_B_EMPTY_ID")
        a_ack_c = next(message for message in bot_messages(group_history) if message.get("fallback_text") == "GROUP_ACK_C_NULL_ID")
        a_done = next(message for message in bot_messages(group_history) if message.get("fallback_text") == "GROUP_DONE_D_OMITTED_ID")
        b_done = next(message for message in bot_messages(group_history) if message.get("fallback_text") == "GROUP_DONE_B")
        group_trace = traces(base, password, group_chat)
        bridge_failures = [
            item for item in group_trace
            if item.get("type") == "tool.end"
            and item.get("data", {}).get("is_error") is True
            and item.get("data", {}).get("preview") == "send_msg bridge failed"
        ]
        assert not bridge_failures, bridge_failures
        assert a_ack.get("fallback_text") == "GROUP_ACK_A" and a_ack.get("streaming") is False, a_ack
        assert len({placeholder_a, a_ack_b["id"], a_ack_c["id"], a_done["id"]}) == 4, (a_ack, a_ack_b, a_ack_c, a_done)
        assert a_ack_b.get("fallback_text") == "GROUP_ACK_B_EMPTY_ID" and a_ack_b.get("streaming") is False, a_ack_b
        assert a_ack_c.get("fallback_text") == "GROUP_ACK_C_NULL_ID" and a_ack_c.get("streaming") is False, a_ack_c
        assert a_done.get("fallback_text") == "GROUP_DONE_D_OMITTED_ID" and a_done.get("streaming") is False, a_done
        assert all(message.get("source_message_id") == group_sent["id"] for message in (a_ack_b, a_ack_c, a_done)), (a_ack_b, a_ack_c, a_done)
        assert a_ack.get("fallback_text") == "GROUP_ACK_A", a_ack
        assert b_done["id"] == placeholder_b and b_done.get("source_message_id") == group_sent["id"], b_done
        assert a_ack.get("source_message_id") == group_sent["id"], a_ack
        assert "GROUP_INTERNAL_A" not in json.dumps(group_history, ensure_ascii=False)
        assert "GROUP_INTERNAL_B" not in json.dumps(group_history, ensure_ascii=False)
        assert "MUST_NOT_REPLACE_A" not in json.dumps(group_history, ensure_ascii=False)
        wait_until(lambda: all(item.get("status") == "done" for item in rpc(base, password, "assignment.list", {"project_id": project["id"], "limit": 100})["items"] if item.get("trigger_message_id") == group_sent["id"]), "both group assignments finish", 20)
        wait_until(
            lambda: (lambda message: message is not None
                and {delivery.get("bot_id") for delivery in message.get("delivery", [])} == {bot_a["id"], bot_b["id"]}
                and all(delivery.get("state") == "read" for delivery in message.get("delivery", []))
            )(next((message for message in history(base, password, group_chat) if message.get("id") == group_sent["id"]), None)),
            "both Bots consume the group message",
            20,
        )
        group_history = history(base, password, group_chat)
        latest_group_user = next(message for message in group_history if message.get("id") == group_sent["id"])
        deliveries = latest_group_user.get("delivery", [])
        assert {item.get("bot_id") for item in deliveries} == {bot_a["id"], bot_b["id"]}, deliveries
        assert all(item.get("state") == "read" for item in deliveries), deliveries
        group_created_events = [
            event for event in all_events(args.home)
            if event.get("event") == "message.created"
            and event.get("data", {}).get("message", {}).get("id") == group_sent["id"]
        ]
        assert len(group_created_events) == 1, group_created_events
        lifecycle_events = all_events(args.home)
        for placeholder_id in (placeholder_a, placeholder_b):
            placeholder_events = [
                event for event in lifecycle_events
                if event.get("data", {}).get("message", {}).get("id") == placeholder_id
                and event.get("event") in {"message.created", "message.updated"}
            ]
            assert sum(event.get("event") == "message.created" for event in placeholder_events) == 1, placeholder_events
            assert any(event.get("data", {}).get("message", {}).get("streaming") is False for event in placeholder_events), placeholder_events
        for followup in (a_ack_b, a_ack_c, a_done):
            followup_creates = [
                event for event in lifecycle_events
                if event.get("event") == "message.created"
                and event.get("data", {}).get("message", {}).get("id") == followup["id"]
            ]
            assert len(followup_creates) == 1, followup_creates
        # Keep the finished Bot A ID as a cross-chat/cross-Bot misuse target.
        with STATE.lock:
            STATE.foreign_finished_id = placeholder_a

        # A separate assignment exercises multiple queued steers through a
        # daemon crash. The first Bot ack fills the source placeholder. Its
        # next model request is held after the first steer was checkpointed;
        # the second steer remains queued until recovery.
        multi_project = rpc(base, password, "project.create", {
            "name": f"Reply lifecycle multi-steer {suffix}",
            "goal": "Verify persisted reply reservations across multiple steers",
            "member_bot_ids": [bot_a["id"]],
            "client_request_id": f"reply-lifecycle-multi-steer-project:{suffix}",
        })
        multi_chat = multi_project["chat"]["id"]
        multi_sent = rpc(base, password, "chat.send", {
            "chat_id": multi_chat,
            "text": marker["multi_steer"],
            "mentions": [{"kind": "bot", "bot_id": bot_a["id"], "instruction": marker["multi_steer"]}],
            "client_request_id": f"reply-lifecycle-multi-steer-source:{suffix}",
        })["message"]
        assert STATE.requested["multi_steer"].wait(20), "multi-steer assignment did not reach the fake provider"
        multi_placeholder = STATE.snapshot()["placeholders"].get((marker["multi_steer"], MODEL_A))
        assert multi_placeholder, STATE.snapshot()
        wait_until(
            lambda: any(message.get("id") == multi_placeholder and message.get("streaming") is True for message in bot_messages(history(base, password, multi_chat))),
            "multi-steer source placeholder persisted",
            15,
        )
        multi_assignments = rpc(base, password, "assignment.list", {"project_id": multi_project["project"]["id"], "limit": 100})["items"]
        multi_assignment = next(item for item in multi_assignments if item.get("trigger_message_id") == multi_sent["id"])

        def send_steer(label: str) -> dict[str, Any]:
            message = rpc(base, password, "chat.send", {
                "chat_id": multi_chat,
                "text": marker[f"steer_{label}"],
                "mentions": [],
                "client_request_id": f"reply-lifecycle-steer-{label}:{suffix}",
            })["message"]
            delivery = next((item for item in message.get("delivery", []) if item.get("bot_id") == bot_a["id"]), None)
            assert delivery and delivery.get("assignment_id") == multi_assignment["id"] and delivery.get("state") == "queued", message
            steered = rpc(base, password, "message.steer", {
                "message_id": message["id"],
                "bot_id": bot_a["id"],
                "assignment_id": multi_assignment["id"],
                "client_request_id": f"reply-lifecycle-steer-promote-{label}:{suffix}",
            })["message"]
            assert steered["id"] == message["id"], steered
            return message

        STATE.releases["multi_steer_initial"].set()
        assert STATE.multi_steer_after_ack.wait(40), "multi-steer run did not reach its post-ack checkpoint"
        multi_checkpoint_history = history(base, password, multi_chat)
        multi_ack = next(message for message in bot_messages(multi_checkpoint_history) if message.get("id") == multi_placeholder)
        assert multi_ack.get("fallback_text") == "MULTI_STEER_ACK_A" and multi_ack.get("streaming") is False, multi_ack
        steer_1 = send_steer("1")
        steer_2 = send_steer("2")
        with STATE.lock:
            STATE.steer_source_labels = {steer_1["id"]: "1", steer_2["id"]: "2"}
        STATE.releases["multi_steer_after_ack"].set()
        assert STATE.multi_steer_ready_for_restart.wait(40), "both steers were not checkpointed before restart"
        pre_restart_multi_requests = [
            item for item in STATE.snapshot()["requests"]
            if item.get("marker") == marker["multi_steer"] and item.get("model") == MODEL_A
        ]
        assert len(pre_restart_multi_requests) == 4, pre_restart_multi_requests
        pre_restart_context = json.dumps(pre_restart_multi_requests[-1]["body"].get("messages", []), ensure_ascii=False)
        assert marker["steer_1"] in pre_restart_context and marker["steer_2"] in pre_restart_context, pre_restart_multi_requests[-1]
        pre_restart_reservations = {
            reservation.get("source_message_id"): reservation.get("message_id")
            for reservation in reservations_from_schema(pre_restart_multi_requests[-1]["body"])
        }
        steer_1_placeholder = pre_restart_reservations.get(steer_1["id"])
        steer_2_placeholder = pre_restart_reservations.get(steer_2["id"])
        assert steer_1_placeholder and steer_2_placeholder, pre_restart_multi_requests[-1]
        for source_id, placeholder_id in ((steer_1["id"], steer_1_placeholder), (steer_2["id"], steer_2_placeholder)):
            open_placeholder = next(message for message in bot_messages(history(base, password, multi_chat)) if message.get("id") == placeholder_id)
            assert open_placeholder.get("source_message_id") == source_id, open_placeholder
            assert open_placeholder.get("fallback_text", "") == "" and open_placeholder.get("streaming") is True, open_placeholder
        request_count_before_multi_recovery = len(STATE.snapshot()["requests"])
        daemon.kill9()
        STATE.releases["multi_steer_ready_for_restart"].set()
        time.sleep(0.4)
        daemon.start()
        assert STATE.multi_steer_after_null.wait(60), "recovered run did not preserve both pending reservations after a null-id message"
        multi_after_null_history = history(base, password, multi_chat)
        multi_null = next(message for message in bot_messages(multi_after_null_history) if message.get("fallback_text") == "MULTI_NULL_DURING_PENDING")
        steer_1_open_placeholder = next(message for message in bot_messages(multi_after_null_history) if message.get("id") == steer_1_placeholder)
        steer_2_open_placeholder = next(message for message in bot_messages(multi_after_null_history) if message.get("id") == steer_2_placeholder)
        assert multi_null["id"] != steer_1_placeholder and multi_null.get("streaming") is False, multi_null
        assert multi_null["id"] != steer_2_placeholder, multi_null
        assert steer_1_open_placeholder.get("source_message_id") == steer_1["id"], steer_1_open_placeholder
        assert steer_1_open_placeholder.get("fallback_text", "") == "" and steer_1_open_placeholder.get("streaming") is True, steer_1_open_placeholder
        assert steer_2_open_placeholder.get("source_message_id") == steer_2["id"], steer_2_open_placeholder
        assert steer_2_open_placeholder.get("fallback_text", "") == "" and steer_2_open_placeholder.get("streaming") is True, steer_2_open_placeholder
        STATE.releases["multi_steer_after_null"].set()
        assert STATE.multi_steer_after_omitted.wait(60), "recovered run did not continue after empty-id and omitted-id followups"
        multi_after_absent_history = history(base, password, multi_chat)
        multi_empty = next(message for message in bot_messages(multi_after_absent_history) if message.get("fallback_text") == "MULTI_EMPTY_DURING_PENDING")
        multi_omitted = next(message for message in bot_messages(multi_after_absent_history) if message.get("fallback_text") == "MULTI_OMITTED_DURING_PENDING")
        steer_1_still_open = next(message for message in bot_messages(multi_after_absent_history) if message.get("id") == steer_1_placeholder)
        steer_2_still_open = next(message for message in bot_messages(multi_after_absent_history) if message.get("id") == steer_2_placeholder)
        assert len({multi_null["id"], multi_empty["id"], multi_omitted["id"], steer_1_placeholder, steer_2_placeholder}) == 5, (multi_null, multi_empty, multi_omitted, steer_1_placeholder, steer_2_placeholder)
        assert all(message.get("streaming") is False for message in (multi_empty, multi_omitted)), (multi_empty, multi_omitted)
        assert steer_1_still_open.get("fallback_text", "") == "" and steer_1_still_open.get("streaming") is True, steer_1_still_open
        assert steer_2_still_open.get("fallback_text", "") == "" and steer_2_still_open.get("streaming") is True, steer_2_still_open
        STATE.releases["multi_steer_after_omitted"].set()
        wait_bot_message(base, password, multi_chat, "MULTI_STEER_DONE_D", 60)
        resumed_multi_requests = [
            item for item in STATE.snapshot()["requests"][request_count_before_multi_recovery:]
            if item.get("marker") == marker["multi_steer"] and item.get("model") == MODEL_A
        ]
        assert resumed_multi_requests, STATE.snapshot()
        resumed_multi_context = json.dumps(resumed_multi_requests, ensure_ascii=False)
        assert marker["steer_1"] in resumed_multi_context and marker["steer_2"] in resumed_multi_context, resumed_multi_requests
        resumed_reservation_sources = {
            reservation.get("source_message_id")
            for item in resumed_multi_requests
            for reservation in reservations_from_schema(item["body"])
        }
        assert steer_1["id"] in resumed_reservation_sources and steer_2["id"] in resumed_reservation_sources, resumed_multi_requests
        multi_history = history(base, password, multi_chat)
        multi_ack_after_resume = next(message for message in bot_messages(multi_history) if message.get("id") == multi_placeholder)
        assert multi_ack_after_resume.get("fallback_text") == "MULTI_STEER_ACK_A" and multi_ack_after_resume.get("streaming") is False, multi_ack_after_resume
        multi_null_after_resume = next(message for message in bot_messages(multi_history) if message.get("fallback_text") == "MULTI_NULL_DURING_PENDING")
        assert multi_null_after_resume["id"] == multi_null["id"] and multi_null_after_resume["id"] != steer_1_placeholder, multi_null_after_resume
        multi_empty_after_resume = next(message for message in bot_messages(multi_history) if message.get("fallback_text") == "MULTI_EMPTY_DURING_PENDING")
        multi_omitted_after_resume = next(message for message in bot_messages(multi_history) if message.get("fallback_text") == "MULTI_OMITTED_DURING_PENDING")
        multi_empty_before_restart = next(message for message in bot_messages(multi_history) if message.get("fallback_text") == "MULTI_EMPTY_BEFORE_RESTART")
        steer_1_reply = next(message for message in bot_messages(multi_history) if message.get("id") == steer_1_placeholder)
        assert steer_1_reply.get("fallback_text") == "MULTI_STEER_ACK_1" and steer_1_reply.get("streaming") is False, steer_1_reply
        steer_2_reply = next(message for message in bot_messages(multi_history) if message.get("id") == steer_2_placeholder)
        assert steer_2_reply.get("fallback_text") == "MULTI_STEER_ACK_2" and steer_2_reply.get("streaming") is False, steer_2_reply
        multi_done = next(message for message in bot_messages(multi_history) if message.get("fallback_text") == "MULTI_STEER_DONE_D")
        assert all(message.get("source_message_id") == steer_2["id"] for message in (multi_null_after_resume, multi_empty_after_resume, multi_omitted_after_resume, multi_done)), (multi_null_after_resume, multi_empty_after_resume, multi_omitted_after_resume, multi_done)
        assert multi_empty_before_restart.get("source_message_id") == steer_2["id"] and multi_empty_before_restart.get("streaming") is False, multi_empty_before_restart
        assert len({multi_empty_before_restart["id"], multi_null_after_resume["id"], multi_empty_after_resume["id"], multi_omitted_after_resume["id"], steer_1_placeholder, steer_2_placeholder}) == 6, (multi_empty_before_restart, multi_null_after_resume, multi_empty_after_resume, multi_omitted_after_resume, steer_1_placeholder, steer_2_placeholder)
        assert not any(message.get("fallback_text", "").startswith("MULTI_STALE_") for message in bot_messages(multi_history)), multi_history
        for steer_message, label in ((steer_1, "1"), (steer_2, "2")):
            replies = [message for message in bot_messages(multi_history) if message.get("source_message_id") == steer_message["id"]]
            if label == "1":
                assert replies == [steer_1_reply], replies
            else:
                assert len(replies) == 6, replies
                assert {message["id"] for message in replies} == {
                    multi_empty_before_restart["id"],
                    multi_null_after_resume["id"],
                    multi_empty_after_resume["id"],
                    multi_omitted_after_resume["id"],
                    steer_2_reply["id"],
                    multi_done["id"],
                }, replies
        multi_assignment_final = rpc(base, password, "assignment.get", {"assignment_id": multi_assignment["id"]})["assignment"]
        for steer_message in (steer_1, steer_2):
            assert any(item.get("message_id") == steer_message["id"] and item.get("applied_at") for item in multi_assignment_final.get("steers", [])), multi_assignment_final

        # A private Bot response first uses send_msg to fill its placeholder.
        # Its later native answer must be a separate message and must not
        # overwrite that already-public acknowledgement.
        dm_ack_sent = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker["dm_ack_native"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-dm-ack:{suffix}",
        })["message"]
        assert STATE.requested["dm_ack_native"].wait(20), "DM ack/native run did not reach the fake provider"
        ack_placeholder = STATE.snapshot()["placeholders"].get((marker["dm_ack_native"], MODEL_A))
        assert ack_placeholder, STATE.snapshot()
        wait_until(lambda: STATE.snapshot()["counts"].get((marker["dm_ack_native"], MODEL_A), 0) >= 2, "DM acknowledgement is committed before native final", 30)
        ack_before_restart = next(message for message in bot_messages(history(base, password, dm_chat)) if message.get("id") == ack_placeholder)
        assert ack_before_restart.get("fallback_text") == "DM_ACK_STAYS" and ack_before_restart.get("streaming") is False, ack_before_restart
        daemon.kill9()
        STATE.releases["dm_final"].set()
        time.sleep(0.4)
        daemon.start()
        wait_bot_message(base, password, dm_chat, "FINAL_AFTER_ACK")
        dm_ack_history = history(base, password, dm_chat)
        dm_ack_messages = [
            message for message in bot_messages(dm_ack_history)
            if message.get("id") == ack_placeholder or "FINAL_AFTER_ACK" in message.get("fallback_text", "")
        ]
        dm_ack = next(message for message in dm_ack_messages if message.get("id") == ack_placeholder)
        dm_native_final = next(message for message in dm_ack_messages if "FINAL_AFTER_ACK" in message.get("fallback_text", ""))
        assert dm_ack.get("source_message_id") == dm_ack_sent["id"], dm_ack
        assert dm_ack.get("fallback_text") == "DM_ACK_STAYS" and dm_ack.get("streaming") is False, dm_ack
        assert dm_native_final["id"] != ack_placeholder and "DM_NATIVE_FINAL_AFTER_ACK" in dm_native_final.get("fallback_text", ""), dm_native_final
        assert dm_ack.get("fallback_text") == "DM_ACK_STAYS", dm_ack
        assert sum(message.get("id") == ack_placeholder for message in bot_messages(dm_ack_history)) == 1, dm_ack_history
        dm_ack_user = next(message for message in dm_ack_history if message.get("id") == dm_ack_sent["id"])
        dm_ack_delivery = next((item for item in dm_ack_user.get("delivery", []) if item.get("bot_id") == bot_a["id"]), None)
        assert dm_ack_delivery and dm_ack_delivery.get("assignment_id") is None and dm_ack_delivery.get("state") == "read", dm_ack_user

        # A native-only private response streams into the source placeholder.
        native_sent = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker["native_dm"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-native-dm:{suffix}",
        })["message"]
        assert STATE.requested["native_dm"].wait(20), "native-only DM did not reach the gated provider"
        wait_until(lambda: any(message.get("source_message_id") == native_sent["id"] for message in bot_messages(history(base, password, dm_chat))), "native DM placeholder is persisted", 15)
        native_placeholder = next(message for message in bot_messages(history(base, password, dm_chat)) if message.get("source_message_id") == native_sent["id"])
        assert native_placeholder.get("fallback_text", "") == "" and native_placeholder.get("streaming") is True, native_placeholder
        assert native_placeholder["id"] == STATE.snapshot()["placeholders"].get((marker["native_dm"], MODEL_A)), STATE.snapshot()
        STATE.releases["native_dm"].set()
        wait_bot_message(base, password, dm_chat, "DM_NATIVE_STREAM_ONLY")
        native_history = history(base, password, dm_chat)
        native_replies = [message for message in bot_messages(native_history) if message.get("source_message_id") == native_sent["id"]]
        assert len(native_replies) == 1, native_replies
        assert native_replies[0]["id"] == native_placeholder["id"]
        assert native_replies[0].get("fallback_text") == "DM_NATIVE_STREAM_ONLY" and native_replies[0].get("streaming") is False, native_replies

        # A foreign finished message ID from another Bot and another chat is
        # rejected; the current run can then use its own placeholder ID.
        cross_sent = rpc(base, password, "chat.send", {
            "chat_id": bot_b_dm,
            "text": marker["cross_scope"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-cross-scope:{suffix}",
        })["message"]
        assert STATE.requested["cross_scope"].wait(20), "cross-scope run did not reach fake provider"
        cross_placeholder = STATE.snapshot()["placeholders"].get((marker["cross_scope"], MODEL_B))
        assert cross_placeholder, STATE.snapshot()
        wait_bot_message(base, password, bot_b_dm, "CROSS_SCOPE_RETRIED")
        cross_history = history(base, password, bot_b_dm)
        cross_reply = next(message for message in bot_messages(cross_history) if message.get("id") == cross_placeholder)
        assert cross_reply.get("source_message_id") == cross_sent["id"]
        assert cross_reply.get("fallback_text") == "CROSS_SCOPE_RETRIED", cross_reply
        assert all("MUST_NOT_BE_SENT" not in json.dumps(message, ensure_ascii=False) for message in cross_history), cross_history
        group_history_after_cross = history(base, password, group_chat)
        assert next(message for message in bot_messages(group_history_after_cross) if message["id"] == placeholder_a).get("fallback_text") == "GROUP_ACK_A"

        # Reusing a completed message in the same Bot/chat is rejected too.
        with STATE.lock:
            STATE.foreign_finished_id = ack_placeholder
        finish_sent = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker["finished_id"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-finished-id:{suffix}",
        })["message"]
        assert STATE.requested["finished_id"].wait(20), "finished-ID run did not reach fake provider"
        finish_placeholder = STATE.snapshot()["placeholders"].get((marker["finished_id"], MODEL_A))
        assert finish_placeholder and finish_placeholder != ack_placeholder, STATE.snapshot()
        wait_bot_message(base, password, dm_chat, "FINISHED_ID_RETRIED")
        dm_after_finished = history(base, password, dm_chat)
        assert next(message for message in bot_messages(dm_after_finished) if message["id"] == ack_placeholder).get("fallback_text") == "DM_ACK_STAYS"
        assert next(message for message in bot_messages(dm_after_finished) if message["id"] == finish_placeholder).get("fallback_text") == "FINISHED_ID_RETRIED"
        assert finish_sent["id"] != dm_ack_sent["id"]

        # Crash while a safe DM run is waiting on the local provider. Restart
        # must resume the same run/placeholder and publish only one bubble.
        before_recovery_trace = traces(base, password, dm_chat)
        recovery_sent = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker["recovery"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-recovery:{suffix}",
        })["message"]
        assert STATE.requested["recovery"].wait(20), "recovery run did not reach its first provider call"
        recovery_placeholder = STATE.snapshot()["placeholders"].get((marker["recovery"], MODEL_A))
        assert recovery_placeholder, STATE.snapshot()
        wait_until(lambda: any(message.get("id") == recovery_placeholder for message in bot_messages(history(base, password, dm_chat))), "recovery placeholder persisted", 15)
        daemon.kill9()
        STATE.releases["recovery_old"].set()
        time.sleep(0.4)
        daemon.start()
        wait_bot_message(base, password, dm_chat, "RECOVERED_SAME_PLACEHOLDER", 60)
        recovery_history = history(base, password, dm_chat)
        recovery_replies = [message for message in bot_messages(recovery_history) if message.get("source_message_id") == recovery_sent["id"]]
        assert len(recovery_replies) == 1, recovery_replies
        assert recovery_replies[0]["id"] == recovery_placeholder and recovery_replies[0].get("streaming") is False, recovery_replies
        created_recovery = [
            event for event in all_events(args.home)
            if event.get("event") == "message.created"
            and event.get("data", {}).get("message", {}).get("id") == recovery_placeholder
        ]
        assert len(created_recovery) == 1, created_recovery
        after_recovery_trace = traces(base, password, dm_chat)
        assert sum(item.get("type") == "run.start" for item in after_recovery_trace) == sum(item.get("type") == "run.start" for item in before_recovery_trace) + 1, after_recovery_trace
        assert STATE.snapshot()["counts"].get((marker["recovery"], MODEL_A)) == 2, STATE.snapshot()

        # Stopping an in-flight group task must settle its claimed-message
        # placeholder even if the fake provider later tries to return a
        # terminal send_msg result for the cancelled run.
        cancel_project = rpc(base, password, "project.create", {
            "name": f"Reply lifecycle cancel {suffix}",
            "goal": "Verify cancelled placeholder settlement",
            "member_bot_ids": [bot_a["id"]],
            "client_request_id": f"reply-lifecycle-cancel-project:{suffix}",
        })
        cancel_chat = cancel_project["chat"]["id"]
        cancel_sent = rpc(base, password, "chat.send", {
            "chat_id": cancel_chat,
            "text": marker["cancel"],
            "mentions": [{"kind": "bot", "bot_id": bot_a["id"], "instruction": marker["cancel"]}],
            "client_request_id": f"reply-lifecycle-cancel:{suffix}",
        })["message"]
        assert STATE.requested["cancel"].wait(20), "cancel run did not reach fake provider"
        cancel_placeholder = STATE.snapshot()["placeholders"].get((marker["cancel"], MODEL_A))
        assert cancel_placeholder, STATE.snapshot()
        wait_until(lambda: any(message.get("id") == cancel_placeholder and message.get("streaming") is True for message in bot_messages(history(base, password, cancel_chat))), "cancel placeholder observed before stop", 15)
        cancel_assignments = rpc(base, password, "assignment.list", {"project_id": cancel_project["project"]["id"], "limit": 100})["items"]
        cancel_assignment = next(item for item in cancel_assignments if item.get("trigger_message_id") == cancel_sent["id"])
        wait_until(lambda: rpc(base, password, "assignment.get", {"assignment_id": cancel_assignment["id"]})["assignment"].get("status") == "working", "cancel assignment starts", 20)
        rpc(base, password, "assignment.stop", {
            "assignment_id": cancel_assignment["id"],
            "client_request_id": f"reply-lifecycle-cancel-stop:{suffix}",
        })
        wait_until(
            lambda: rpc(base, password, "assignment.get", {"assignment_id": cancel_assignment["id"]})["assignment"].get("status") in {"stopped", "cancelled"},
            "cancel assignment stops",
            20,
        )
        STATE.releases["cancel"].set()
        wait_until(
            lambda: any(message.get("id") == cancel_placeholder and message.get("streaming") is False for message in bot_messages(history(base, password, cancel_chat))),
            "cancelled placeholder closes",
            20,
        )
        cancel_history = history(base, password, cancel_chat)
        cancel_replies = [message for message in bot_messages(cancel_history) if message.get("source_message_id") == cancel_sent["id"]]
        assert len(cancel_replies) == 1 and cancel_replies[0]["id"] == cancel_placeholder, cancel_replies
        assert cancel_replies[0].get("source_message_id") == cancel_sent["id"]
        assert "STALE_CANCEL_RESPONSE" not in json.dumps(cancel_history, ensure_ascii=False), cancel_history

        # A terminal provider failure must close, rather than strand, its
        # already-persisted empty reply placeholder.
        failure_sent = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker["failure"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-failure:{suffix}",
        })["message"]
        assert STATE.requested["failure"].wait(20), "failure run did not reach fake provider"
        failure_placeholder = STATE.snapshot()["placeholders"].get((marker["failure"], MODEL_A))
        assert failure_placeholder, STATE.snapshot()
        wait_until(lambda: any(message.get("id") == failure_placeholder and message.get("streaming") is True for message in bot_messages(history(base, password, dm_chat))), "failure placeholder observed before provider error", 15)
        previous_failure_ends = run_end_count(traces(base, password, dm_chat))
        STATE.releases["failure"].set()
        failure_trace = wait_for_run_end(base, password, dm_chat, previous_failure_ends, 60)
        failure_history = history(base, password, dm_chat)
        failure_reply = next(message for message in bot_messages(failure_history) if message.get("id") == failure_placeholder)
        assert failure_reply.get("streaming") is False, failure_reply
        assert failure_reply.get("source_message_id") == failure_sent["id"], failure_reply
        assert any(item.get("type") == "run.end" and item.get("data", {}).get("status") in {"failed", "error", "cancelled", "stopped"} for item in failure_trace), failure_trace

        print(json.dumps({
            "ok": True,
            "home": str(args.home),
            "group_chat": group_chat,
            "group_source_message": group_sent["id"],
            "group_placeholder_ids": [placeholder_a, placeholder_b],
            "group_followup_ids": [a_ack_b["id"], a_ack_c["id"], a_done["id"]],
            "dm_ack_id": ack_placeholder,
            "dm_native_final_id": dm_native_final["id"],
            "native_stream_id": native_placeholder["id"],
            "recovery_placeholder_id": recovery_placeholder,
            "cancel_placeholder_id": cancel_placeholder,
            "failure_placeholder_closed": failure_reply.get("streaming") is False,
        }, ensure_ascii=False))
        return {"ok": True}
    finally:
        for event in STATE.releases.values():
            event.set()
        daemon.close()
        provider.shutdown()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True, help="fresh isolated home directory")
    parser.add_argument("--binary", type=Path, required=True, help="built server/target/debug/macbotd")
    parser.add_argument("--password", default="reply-lifecycle-local")
    args = parser.parse_args()
    args.home = args.home.resolve()
    if args.home.exists():
        raise SystemExit(f"refusing to overwrite existing home: {args.home}")
    if not args.home.is_relative_to(Path("/tmp").resolve()):
        raise SystemExit("--home must be under /tmp")
    args.home.mkdir(parents=True, exist_ok=False)
    config = args.home / "config.toml"
    config.write_text('host_name = "Chat reply lifecycle smoke"\n', encoding="utf-8")
    port = free_port()
    args.url = f"http://127.0.0.1:{port}"
    args.browser_bin = None
    args.daemon_command = shlex.join([
        str(args.binary.resolve()),
        "--config", str(config),
        "--home", str(args.home),
        "--port", str(port),
        "--password", args.password,
    ])
    acceptance(args)


if __name__ == "__main__":
    main()
