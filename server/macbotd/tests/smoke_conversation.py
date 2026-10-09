#!/usr/bin/env python3
"""S1 conversation, approval, streaming-message, and cursor acceptance.

The daemon and execution engine are real.  The provider is a local fake which
returns one assistant response containing write/read/bash tool calls, then
distinct responses for later user turns.  The test intentionally uses an
isolated home and port and never writes a real credential.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import shutil
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, rpc, wait_until


TOKEN = "macbot-conversation-fake-token"


class ConversationProviderState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.bodies: list[dict[str, Any]] = []

    def record(self, body: dict[str, Any]) -> None:
        with self.lock:
            self.bodies.append(body)

    def snapshot(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.bodies)


class ConversationProviderHandler(BaseHTTPRequestHandler):
    state: ConversationProviderState
    suffix = ""

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

    def _tools(self) -> None:
        calls = [
            {"index": 0, "id": f"conversation-write-{uuid.uuid4().hex[:8]}", "type": "function", "function": {"name": "write", "arguments": json.dumps({"path": f"conversation-{type(self).suffix}.txt", "content": "conversation-write-marker"})}},
            {"index": 1, "id": f"conversation-read-{uuid.uuid4().hex[:8]}", "type": "function", "function": {"name": "read", "arguments": json.dumps({"path": f"conversation-{type(self).suffix}.txt"})}},
            {"index": 2, "id": f"conversation-bash-{uuid.uuid4().hex[:8]}", "type": "function", "function": {"name": "bash", "arguments": json.dumps({"command": f"printf bash-marker > conversation-bash-{type(self).suffix}.txt", "background": False})}},
        ]
        self._stream([
            {"choices": [{"delta": {"tool_calls": calls}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 12, "completion_tokens": 8}},
        ])

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self.headers.get("Authorization") == f"Bearer {TOKEN}":
            self._json(200, {"data": [{"id": "conversation-fake", "object": "model"}]})
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
        self.state.record(body)
        messages = body.get("messages", [])
        latest_user = next((item.get("content", "") for item in reversed(messages) if item.get("role") == "user"), "")
        has_tool_turn = any(item.get("role") == "assistant" and item.get("tool_calls") for item in messages)
        if "MULTI_TOOLS" in latest_user and not has_tool_turn:
            self._tools()
        elif "MULTI_TOOLS" in latest_user:
            self._text("multi-tools-finished")
        elif "SECOND_PROMPT" in latest_user:
            self._text("second-new-marker")
        elif "POST_RESTART" in latest_user:
            self._text("post-restart-new-marker")
        else:
            self._text("conversation-fake-finished")


def start_provider(suffix: str) -> tuple[ThreadingHTTPServer, ConversationProviderState, str]:
    state = ConversationProviderState()
    ConversationProviderHandler.state = state
    ConversationProviderHandler.suffix = suffix
    server = ThreadingHTTPServer(("127.0.0.1", 0), ConversationProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, state, f"http://127.0.0.1:{server.server_port}/v1"


class RestartDaemon(Daemon):
    def restart(self) -> None:
        if self.process is None:
            raise AssertionError("restart requires --daemon-command")
        if self.process.poll() is None:
            os.killpg(self.process.pid, 9)
            self.process.wait(timeout=10)
        time.sleep(0.5)
        self.start()


def trace_items(base: str, password: str, assignment_id: str | None, chat_id: str) -> list[dict[str, Any]]:
    params = {"assignment_id": assignment_id} if assignment_id else {"chat_id": chat_id}
    params.update({"tail": True, "limit": 500})
    return rpc(base, password, "trace.history", params)["items"]


def pending(base: str, password: str) -> list[dict[str, Any]]:
    return rpc(base, password, "approval.list", {"state": ["pending"]}).get("approvals", [])


def wait_assignment(base: str, password: str, chat_id: str, marker: str) -> dict[str, Any]:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        items = rpc(base, password, "assignment.list", {"limit": 200})["items"]
        matches = [item for item in items if item.get("origin_chat_id") == chat_id and marker in item.get("instruction", "")]
        if matches:
            return matches[0]
        # Direct private-chat execution can be represented only by a durable
        # run request, without an orchestrator assignment.  Trace history is
        # still keyed by chat_id and is sufficient for the remainder of this
        # conversation check.
        if any(item.get("type") == "run.start" for item in trace_items(base, password, None, chat_id)):
            return {"id": None, "origin_chat_id": chat_id, "instruction": marker}
        time.sleep(0.2)
    raise AssertionError(f"timed out waiting for assignment or direct DM run {marker}")


def execution_done(base: str, password: str, assignment: dict[str, Any], chat_id: str) -> bool:
    if assignment.get("id"):
        return any(
            item.get("id") == assignment["id"] and item.get("status") == "done"
            for item in rpc(base, password, "assignment.list", {"limit": 200})["items"]
        )
    return any(item.get("type") == "run.end" and item.get("data", {}).get("status") == "done" for item in trace_items(base, password, None, chat_id))


def decide(base: str, password: str, approval: dict[str, Any]) -> None:
    rpc(base, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once", "client_request_id": f"conversation-approval:{approval['id']}"})


def workbench_waiting(base: str, password: str) -> list[dict[str, Any]]:
    result = rpc(base, password, "workbench.get", {})
    workbench = result.get("workbench", result)
    return workbench.get("waiting", [])


def bot_waiting_user(base: str, password: str, bot_id: str) -> bool:
    bots = rpc(base, password, "bot.list", {"include_hidden": True})["bots"]
    bot = next(item for item in bots if item["id"] == bot_id)
    status = bot.get("status", {})
    return status.get("summary") == "waiting_user" and status.get("waiting", 0) >= 1


def event_messages(home: Path, chat_id: str) -> list[tuple[str, dict[str, Any]]]:
    events_path = home / "data" / "events" / "events.jsonl"
    rows: list[tuple[str, dict[str, Any]]] = []
    if not events_path.exists():
        return rows
    for line in events_path.read_text(encoding="utf-8").splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = event.get("data", {}).get("message")
        if isinstance(message, dict) and message.get("chat_id") == chat_id and event.get("event") in {"message.created", "message.updated"}:
            rows.append((event["event"], message))
    return rows


def write_redacted_provider_bodies(path: Path, bodies: list[dict[str, Any]]) -> None:
    """Persist only model/messages, with the fake token scrubbed."""
    rows = []
    for body in bodies:
        row = {"model": body.get("model"), "messages": body.get("messages", [])}
        encoded = json.dumps(row, ensure_ascii=False).replace(TOKEN, "<redacted-token>")
        rows.append(encoded)
    path.write_text("\n".join(rows) + "\n", encoding="utf-8")


def assert_stream_ids(home: Path, chat_id: str, response_markers: list[str]) -> None:
    rows = event_messages(home, chat_id)
    stream_ids = {message["id"] for event, message in rows if event == "message.created" and message.get("sender", {}).get("kind") == "bot" and message.get("streaming") is True}
    assert stream_ids, rows
    for message_id in stream_ids:
        updates = [message for event, message in rows if event == "message.updated" and message.get("id") == message_id]
        assert updates and updates[-1].get("streaming") is False, (message_id, rows)
    history = rpc_chat_history(home, chat_id)
    bot_texts = [message.get("fallback_text", "") for message in history if message.get("sender", {}).get("kind") == "bot"]
    for marker in response_markers:
        assert sum(marker in text for text in bot_texts) == 1, (marker, history)


def wait_missing_model_message(base: str, password: str, chat_id: str, after_seq: int) -> dict[str, Any]:
    found: list[dict[str, Any]] = []

    def ready() -> bool:
        messages = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": chat_id, "after_seq": after_seq, "limit": 100},
        )["messages"]
        matches = [
            message
            for message in messages
            if message.get("intent") == "blocked"
            and "未配置默认模型" in message.get("fallback_text", "")
        ]
        if matches:
            found.append(matches[-1])
            return True
        return False

    wait_until(ready, f"missing-model notification {chat_id}", 20)
    return found[0]


async def _capture_live_message_events(
    base: str,
    password: str,
    chat_id: str,
    last_seq: int,
    send_params: dict[str, Any],
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    """Observe one request over the real control WebSocket.

    The RPC response is sent from a worker thread so the websocket remains
    live while message.created/message.updated frames arrive.
    """
    import websockets

    ws_url = base.replace("http://", "ws://").replace("https://", "wss://")
    async with websockets.connect(
        ws_url + "/ws",
        additional_headers={"Authorization": f"Bearer {password}"},
        proxy=None,
    ) as ws:
        hello = json.loads(await ws.recv())
        assert hello.get("event") == "hello", hello
        request_id = f"conversation-resume-{uuid.uuid4().hex}"
        await ws.send(json.dumps({
            "v": 1,
            "kind": "req",
            "id": request_id,
            "method": "session.resume",
            "params": {
                "last_seq": last_seq,
                "client": {
                    "platform": "macos",
                    "app_version": "conversation-smoke",
                    "device_name": "conversation-smoke",
                    "device_id": f"conversation-{uuid.uuid4().hex}",
                },
            },
        }))
        while True:
            frame = json.loads(await asyncio.wait_for(ws.recv(), 10))
            if frame.get("kind") == "res" and frame.get("id") == request_id:
                assert frame.get("ok", True), frame
                break
        # A replay response can be followed by sync.done.  Consume it before
        # the new request so captured frames belong only to this send.
        while True:
            frame = json.loads(await asyncio.wait_for(ws.recv(), 10))
            if frame.get("event") == "sync.done":
                break
            if frame.get("kind") == "res" and frame.get("id") == request_id:
                break

        sent = await asyncio.to_thread(rpc, base, password, "chat.send", send_params)
        message_id = sent["message"]["id"]
        frames: list[dict[str, Any]] = []
        final_bot_id: str | None = None
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            try:
                frame = json.loads(await asyncio.wait_for(ws.recv(), max(0.1, deadline - time.monotonic())))
            except asyncio.TimeoutError:
                break
            if frame.get("event") not in {"message.created", "message.updated"}:
                continue
            message = frame.get("data", {}).get("message", {})
            if message.get("chat_id") != chat_id:
                continue
            frames.append(frame)
            if message.get("id") == message_id:
                continue
            if message.get("sender", {}).get("kind") == "bot":
                final_bot_id = message.get("id")
                if message.get("streaming") is False:
                    break
        assert final_bot_id, frames
        return sent, frames


def capture_live_message_events(
    base: str,
    password: str,
    chat_id: str,
    last_seq: int,
    send_params: dict[str, Any],
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    return asyncio.run(_capture_live_message_events(base, password, chat_id, last_seq, send_params))


def assert_live_message_event_sequences(
    base: str,
    password: str,
    chat_id: str,
    frames: list[dict[str, Any]],
) -> None:
    history = rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"]
    by_id = {message["id"]: message for message in history}
    event_seqs = [frame["seq"] for frame in frames]
    assert len(event_seqs) == len(set(event_seqs)), frames
    for frame in frames:
        message = frame["data"]["message"]
        stored = by_id.get(message["id"])
        assert stored, (frame, history)
        assert message.get("seq") == stored.get("seq"), (frame, stored)
    grouped: dict[str, list[dict[str, Any]]] = {}
    for frame in frames:
        grouped.setdefault(frame["data"]["message"]["id"], []).append(frame)
    assert any(len(events) >= 2 for events in grouped.values()), frames


def assert_persisted_message_event_sequences(
    home: Path,
    base: str,
    password: str,
    chat_id: str,
) -> None:
    """Check the durable event cursor for every conversation turn.

    WebSocket frames are checked separately for the post-restart turn.  This
    verifies the two earlier turns too, including the stable chat message
    sequence shared by a streaming placeholder and its final update.
    """
    events_path = home / "data" / "events" / "events.jsonl"
    history = rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"]
    by_id = {message["id"]: message for message in history}
    event_seqs: list[int] = []
    grouped: dict[str, list[dict[str, Any]]] = {}
    for line in events_path.read_text(encoding="utf-8").splitlines():
        event = json.loads(line)
        if event.get("event") not in {"message.created", "message.updated"}:
            continue
        message = event.get("data", {}).get("message", {})
        if message.get("chat_id") != chat_id:
            continue
        event_seqs.append(event["seq"])
        stored = by_id.get(message.get("id"))
        assert stored, (event, history)
        assert message.get("seq") == stored.get("seq"), (event, stored)
        grouped.setdefault(message["id"], []).append(event)
    assert event_seqs == sorted(set(event_seqs)), event_seqs
    streaming_ids = {
        message_id
        for message_id, events in grouped.items()
        if any(event["data"]["message"].get("streaming") is True for event in events)
    }
    for message_id in streaming_ids:
        assert any(
            event["event"] == "message.updated"
            and event["data"]["message"].get("streaming") is False
            for event in grouped[message_id]
        ), grouped[message_id]


def assert_visible_text_blocks(messages: list[dict[str, Any]]) -> None:
    for message in messages:
        fallback = message.get("fallback_text", "")
        blocks = message.get("blocks", [])
        text_blocks = [block for block in blocks if block.get("type") == "text"]
        if fallback and text_blocks:
            assert all(block.get("markdown", "") for block in text_blocks), message
            if len(text_blocks) == len(blocks):
                assert "".join(block["markdown"] for block in text_blocks) == fallback, message


def rpc_chat_history(home: Path, chat_id: str) -> list[dict[str, Any]]:
    # Filled by acceptance through the module-level RPC closure.
    return []


def acceptance(args: argparse.Namespace) -> None:
    base = args.url.rstrip("/")
    suffix = uuid.uuid4().hex[:8]
    provider_server, provider, provider_url = start_provider(suffix)
    daemon = RestartDaemon(args)
    try:
        daemon.start()
        # Create the null-model worker before any provider/model is registered;
        # later settings updates must not retroactively assign it a model.
        missing_bot = rpc(
            base,
            args.password,
            "bot.create",
            {
                "name": f"conversation-no-default-{suffix}",
                "model": None,
                "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
            },
        )["bot"]
        assert missing_bot["model"] is None, missing_bot
        created = rpc(base, args.password, "provider.create", {"name": f"conversation-{suffix}", "api_kind": "openai-completions", "base_url": provider_url, "api_key": TOKEN, "client_request_id": f"conversation-provider:{suffix}"})["provider"]
        refreshed = rpc(base, args.password, "model.refresh", {"provider_id": created["id"]})
        assert any(item["model_id"] == "conversation-fake" for item in refreshed["models"]), refreshed
        model = rpc(base, args.password, "model.upsert", {"provider_id": created["id"], "model_id": "conversation-fake", "display_name": "conversation fake", "caps": {"vision": False, "tools": True, "reasoning": False}, "client_request_id": f"conversation-model:{suffix}"})["model"]["ref"]
        worker = rpc(base, args.password, "bot.create", {"name": f"conversation-worker-{suffix}", "model": model, "tools": {"files": True, "bash": True, "browser": False, "subagent": False, "web": False, "mcp": False}})["bot"]
        chat_id = worker["dm_chat_id"]
        sent = rpc(base, args.password, "chat.send", {"chat_id": chat_id, "text": "MULTI_TOOLS", "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "MULTI_TOOLS"}], "client_request_id": f"conversation-first:{suffix}"})
        first = wait_assignment(base, args.password, chat_id, "MULTI_TOOLS")
        wait_until(lambda: any(item.get("tool") == "write" for item in pending(base, args.password)), "write approval", 30)
        standalone_result = rpc(base, args.password, "approval.request", {"bot_id": "main", "assignment_id": None, "chat_id": "chat_main", "tool": "browser.act", "risk": "exec", "summary": "conversation standalone approval", "detail": "null assignment"})
        standalone = standalone_result.get("approval", standalone_result)
        wait_until(lambda: any(item.get("kind") == "approval" and item.get("approval", {}).get("id") == standalone["id"] and item.get("approval", {}).get("assignment_id") is None for item in workbench_waiting(base, args.password)), "null-assignment workbench approval", 10)
        assert bot_waiting_user(base, args.password, worker["id"])
        decide(base, args.password, standalone)
        wait_until(lambda: not any(item.get("kind") == "approval" and item.get("approval", {}).get("id") == standalone["id"] for item in workbench_waiting(base, args.password)), "standalone approval removed", 10)
        write_approval = next(item for item in pending(base, args.password) if item.get("tool") == "write")
        decide(base, args.password, write_approval)
        def read_finished() -> bool:
            items = trace_items(base, args.password, first.get("id"), chat_id)
            read_calls = {
                item.get("data", {}).get("call_id")
                for item in items
                if item.get("type") == "tool.start" and item.get("data", {}).get("name") == "read"
            }
            return any(
                item.get("type") == "tool.end"
                and item.get("data", {}).get("call_id") in read_calls
                and not item.get("data", {}).get("is_error", False)
                for item in items
            )

        wait_until(read_finished, "read executes without approval", 30)
        assert not any(item.get("tool") == "read" for item in pending(base, args.password)), pending(base, args.password)
        wait_until(lambda: any(item.get("tool") == "bash" for item in pending(base, args.password)), "bash approval", 30)
        decide(base, args.password, next(item for item in pending(base, args.password) if item.get("tool") == "bash"))
        wait_until(lambda: execution_done(base, args.password, first, chat_id), "multi-tool completion", 40)
        wait_until(lambda: not workbench_waiting(base, args.password), "workbench approvals clear", 20)
        first_trace = trace_items(base, args.password, first.get("id"), chat_id)
        unique_tools: list[str] = []
        seen_calls: set[str] = set()
        for item in first_trace:
            if item.get("type") != "tool.start":
                continue
            data = item.get("data", {})
            call_id = data.get("call_id")
            if call_id in seen_calls:
                continue
            seen_calls.add(call_id)
            unique_tools.append(data.get("name"))
        assert unique_tools == ["write", "read", "bash"], first_trace

        history = rpc(base, args.password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"]
        assert [item["seq"] for item in history] == sorted(item["seq"] for item in history)
        first_user_seq = sent["message"]["seq"]
        assert any(item.get("sender", {}).get("kind") == "bot" and "multi-tools-finished" in item.get("fallback_text", "") for item in history)
        chat = rpc(base, args.password, "chat.get", {"chat_id": chat_id})["chat"]
        assert chat["last_seq"] == history[-1]["seq"]

        second_sent = rpc(base, args.password, "chat.send", {"chat_id": chat_id, "text": "SECOND_PROMPT", "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "SECOND_PROMPT"}], "client_request_id": f"conversation-second:{suffix}"})
        second = wait_assignment(base, args.password, chat_id, "SECOND_PROMPT")
        wait_until(lambda: execution_done(base, args.password, second, chat_id), "second conversation", 40)
        bodies = provider.snapshot()
        second_body = next(body for body in reversed(bodies) if "SECOND_PROMPT" in json.dumps(body.get("messages", []), ensure_ascii=False))
        users = [item.get("content", "") for item in second_body["messages"] if item.get("role") == "user"]
        assert users[-1] == "SECOND_PROMPT" and users.count("SECOND_PROMPT") == 1, users
        second_trace = trace_items(base, args.password, second.get("id"), chat_id)
        assert not any(item.get("type") == "tool.start" for item in second_trace), second_trace
        after_second = rpc(base, args.password, "chat.history", {"chat_id": chat_id, "after_seq": second_sent["message"]["seq"], "limit": 100})["messages"]
        assert any(item.get("sender", {}).get("kind") == "bot" and "second-new-marker" in item.get("fallback_text", "") for item in after_second), after_second
        final_seq_before_restart = max(item["seq"] for item in rpc(base, args.password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"])
        rpc(base, args.password, "chat.mark_read", {"chat_id": chat_id, "seq": final_seq_before_restart})
        assert rpc(base, args.password, "chat.get", {"chat_id": chat_id})["chat"]["last_read_seq"] == final_seq_before_restart

        daemon.restart()
        third_sent, live_frames = capture_live_message_events(
            base,
            args.password,
            chat_id,
            final_seq_before_restart,
            {
                "chat_id": chat_id,
                "text": "POST_RESTART",
                "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "POST_RESTART"}],
                "client_request_id": f"conversation-third:{suffix}",
            },
        )
        third = wait_assignment(base, args.password, chat_id, "POST_RESTART")
        wait_until(lambda: execution_done(base, args.password, third, chat_id), "post-restart conversation", 40)
        final_history = rpc(base, args.password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"]
        assert_visible_text_blocks(final_history)
        assert_visible_text_blocks([message for _event, message in event_messages(args.home, chat_id)])
        seqs = [item["seq"] for item in final_history]
        assert seqs == sorted(set(seqs)), final_history
        assert third_sent["message"]["seq"] > final_seq_before_restart
        assert rpc(base, args.password, "chat.get", {"chat_id": chat_id})["chat"]["last_seq"] == seqs[-1]
        assert any("post-restart-new-marker" in item.get("fallback_text", "") for item in final_history)
        user_texts = [item.get("fallback_text", "") for item in final_history if item.get("sender", {}).get("kind") == "user"]
        assert [text for text in ("MULTI_TOOLS", "SECOND_PROMPT", "POST_RESTART") if text in user_texts] == ["MULTI_TOOLS", "SECOND_PROMPT", "POST_RESTART"], user_texts
        bot_texts = [item.get("fallback_text", "") for item in final_history if item.get("sender", {}).get("kind") == "bot"]
        assert sum("multi-tools-finished" in text for text in bot_texts) == 1
        assert sum("second-new-marker" in text for text in bot_texts) == 1
        assert sum("post-restart-new-marker" in text for text in bot_texts) == 1
        assert_live_message_event_sequences(base, args.password, chat_id, live_frames)

        # A Bot with model=null must not silently inherit the main model or
        # create an assignment when the worker default is also unset.  The
        # same guard applies to the main chat when models.main is null.
        settings = rpc(
            base,
            args.password,
            "settings.update",
            {"patch": {"models": {"bot_default": None, "main": None}}},
        )["settings"]
        assert settings["models"]["bot_default"] is None
        assert settings["models"]["main"] is None
        calls_before_missing = len(provider.snapshot())
        missing_dm_sent = rpc(
            base,
            args.password,
            "chat.send",
            {
                "chat_id": missing_bot["dm_chat_id"],
                "text": f"MISSING_DEFAULT_DM_{suffix}",
                "mentions": [],
                "client_request_id": f"conversation-missing-dm:{suffix}",
            },
        )
        missing_dm = wait_missing_model_message(
            base,
            args.password,
            missing_bot["dm_chat_id"],
            missing_dm_sent["message"]["seq"],
        )
        assert missing_dm.get("assignment_id") is None, missing_dm
        assert not any(item.get("type") == "run.start" for item in trace_items(base, args.password, None, missing_bot["dm_chat_id"]))
        assert len(provider.snapshot()) == calls_before_missing

        missing_main_sent = rpc(
            base,
            args.password,
            "chat.send",
            {
                "chat_id": "chat_main",
                "text": f"MISSING_DEFAULT_MAIN_{suffix}",
                "mentions": [],
                "client_request_id": f"conversation-missing-main:{suffix}",
            },
        )
        missing_main = wait_missing_model_message(base, args.password, "chat_main", missing_main_sent["message"]["seq"])
        assert missing_main.get("assignment_id") is None, missing_main
        assert not any(item.get("type") == "run.start" for item in trace_items(base, args.password, None, "chat_main"))
        assert len(provider.snapshot()) == calls_before_missing
        # Bind the helper to this live RPC context for the stream fold check.
        global rpc_chat_history
        rpc_chat_history = lambda _home, cid: rpc(base, args.password, "chat.history", {"chat_id": cid, "limit": 100})["messages"]
        assert_stream_ids(args.home, chat_id, ["multi-tools-finished", "second-new-marker", "post-restart-new-marker"])
        assert_persisted_message_event_sequences(args.home, base, args.password, chat_id)
        provider_evidence = args.home / "provider-bodies.redacted.jsonl"
        write_redacted_provider_bodies(provider_evidence, provider.snapshot())
        print(json.dumps({"ok": True, "chat_id": chat_id, "first_assignment": first.get("id"), "second_assignment": second.get("id"), "third_assignment": third.get("id"), "first_user_seq": first_user_seq, "second_user_seq": second_sent["message"]["seq"], "post_restart_seq": third_sent["message"]["seq"], "live_event_frames": len(live_frames), "missing_dm_message": missing_dm["id"], "missing_main_message": missing_main["id"], "provider_calls": len(provider.snapshot()), "provider_body_evidence": str(provider_evidence), "home": str(args.home)}, ensure_ascii=False))
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
