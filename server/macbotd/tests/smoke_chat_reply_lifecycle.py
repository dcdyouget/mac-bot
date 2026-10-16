#!/usr/bin/env python3
"""Bounded production smoke for server-bound chat reply lifecycles.

The daemon, durable runtime, RPCs, questions, traces, and message store are real.
Only the OpenAI-compatible provider is local and scripted. Every invocation uses
a fresh home and random port, and never reads or contacts a real credential.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import Daemon, REPO, http_json, rpc, wait_until


TOKEN = "macbot-chat-reply-lifecycle-fake-token"
MODEL_A = "chat-reply-lifecycle-a"
MODEL_B = "chat-reply-lifecycle-b"
MODEL_MAIN = "chat-reply-lifecycle-main"


class ProviderState:
    """Thread-safe request recorder and gates for deterministic scenarios."""

    def __init__(self) -> None:
        self.lock = threading.Condition()
        self.models = {MODEL_A, MODEL_B, MODEL_MAIN}
        self.markers: dict[str, tuple[str, str]] = {}
        self.requests: list[dict[str, Any]] = []
        self.counts: dict[tuple[str, str], int] = {}
        self.gates: dict[tuple[str, str, int], threading.Event] = {}
        self.forged_chat_id: str | None = None

    def configure(self, markers: dict[str, tuple[str, str]]) -> None:
        with self.lock:
            self.markers = dict(markers)
            self.requests.clear()
            self.counts.clear()
            self.forged_chat_id = None
            self.gates = {
                ("choice", MODEL_MAIN, 1): threading.Event(),
                ("correction", MODEL_A, 1): threading.Event(),
                ("repeat", MODEL_A, 1): threading.Event(),
                ("group_shared", MODEL_A, 1): threading.Event(),
                ("group_shared", MODEL_B, 1): threading.Event(),
                ("scope", MODEL_A, 1): threading.Event(),
                ("steer", MODEL_A, 1): threading.Event(),
                ("recovery", MODEL_A, 1): threading.Event(),
            }

    def record(self, body: dict[str, Any]) -> tuple[str | None, str, int]:
        model = str(body.get("model", ""))
        messages = body.get("messages", [])
        scenario: str | None = None
        marker_name = ""
        for message in reversed(messages if isinstance(messages, list) else []):
            if not isinstance(message, dict) or message.get("role") != "user":
                continue
            content = str(message.get("content", ""))
            with self.lock:
                match = next(
                    (
                        (name, scenario, marker)
                        for name, (scenario, marker) in self.markers.items()
                        if marker in content
                    ),
                    None,
                )
            if match is not None:
                marker_name, scenario, _marker = match
                break
        if scenario is None:
            return None, model, 0
        with self.lock:
            key = (scenario, model)
            count = self.counts.get(key, 0) + 1
            self.counts[key] = count
            self.requests.append({"scenario": scenario, "marker_name": marker_name, "model": model, "count": count, "body": body})
            self.lock.notify_all()
        return scenario, model, count

    def count(self, scenario: str, model: str) -> int:
        with self.lock:
            return self.counts.get((scenario, model), 0)

    def request(self, scenario: str, model: str, count: int) -> dict[str, Any]:
        with self.lock:
            for request in self.requests:
                if (request["scenario"], request["model"], request["count"]) == (scenario, model, count):
                    return request
        raise AssertionError(f"missing provider request {scenario}/{model} #{count}")

    def wait_count(self, scenario: str, model: str, count: int, timeout: float = 35) -> None:
        deadline = time.monotonic() + timeout
        with self.lock:
            while self.counts.get((scenario, model), 0) < count:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(f"provider request {scenario}/{model} #{count} not observed; counts={self.counts}")
                self.lock.wait(remaining)

    def release(self, scenario: str, model: str, count: int = 1) -> None:
        self.gates[(scenario, model, count)].set()

    def rearm(self, scenario: str, model: str, count: int) -> None:
        with self.lock:
            self.gates[(scenario, model, count)] = threading.Event()

    def release_all(self) -> None:
        for gate in self.gates.values():
            gate.set()


STATE = ProviderState()


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
            # Expected for a provider request whose daemon was killed in the
            # recovery scenario.
            return

    def _text(self, text: str) -> None:
        self._stream([
            {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 7, "completion_tokens": 4}},
        ])

    def _tool(self, args: dict[str, Any], content: str | None = None) -> None:
        delta: dict[str, Any] = {
            "tool_calls": [{
                "index": 0,
                "id": f"lifecycle-{uuid.uuid4().hex}",
                "type": "function",
                "function": {"name": "send_msg", "arguments": json.dumps(args, ensure_ascii=False)},
            }],
        }
        if content is not None:
            delta["content"] = content
        self._stream([
            {"choices": [{"delta": delta, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 9, "completion_tokens": 5}},
        ])

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
        scenario, model, count = self.state.record(body)
        if scenario is None:
            self._text("unrouted lifecycle call")
            return
        gate = self.state.gates.get((scenario, model, count))
        if gate is not None:
            gate.wait(60)

        if scenario == "choice":
            if count == 1:
                self._tool({
                    "intent": "decision",
                    "text": "请选择本次实现使用的环境。",
                    "options": ["测试环境", "生产环境"],
                    "allow_free_text": False,
                }, content="INTERNAL_CHOICE_REASONING")
            else:
                self._tool({
                    "intent": "done",
                    "text": "summary: 已按选择完成。 location: docs/release-report.md. how to use: 打开报告并按步骤部署。",
                }, content="INTERNAL_CHOICE_FINAL")
            return

        if scenario == "correction":
            if count == 1:
                self._text("CORRECTION_NATIVE_ONLY: this raw completion must stay private")
            else:
                self._tool({"intent": "done", "text": "summary: corrected report. location: reports/final.md. how to use: open the report."})
            return

        if scenario == "repeat":
            self._tool({"intent": "done", "text": f"summary: REPEAT_DONE_{count} completed. location: reports/repeat-{count}.md. how to use: open the report."})
            return

        if scenario == "group_shared":
            bot = "A" if model == MODEL_A else "B"
            self._tool({"intent": "done", "text": f"summary: GROUP_{bot}_DONE completed. location: reports/group-{bot.lower()}.md. how to use: open the report."}, content=f"GROUP_{bot}_PRIVATE_MODEL_TEXT")
            return

        if scenario == "scope":
            if count == 1:
                # Deliberately forge a destination that the model schema must
                # not expose. The server must refuse it against the immutable
                # execution context and the provider then retries in-scope.
                target = self.state.forged_chat_id
                if target is None:
                    self._json(500, {"error": {"message": "scope target was not configured"}})
                    return
                self._tool({
                    "intent": "done",
                    "text": "FORGED_SCOPE_MUST_NOT_PUBLISH",
                    "chat_id": target,
                    "bot_id": "forged-bot-id",
                    "message_id": "forged-message-id",
                })
            else:
                self._tool({"intent": "done", "text": "summary: SCOPE_BOUND_FINAL completed in the bound chat. location: reports/scope.md. how to use: open the report."})
            return

        if scenario == "steer":
            if count == 1:
                self._text("STEER_NATIVE_ONLY: private model text before correction")
            else:
                self._tool({"intent": "done", "text": "summary: STEER_FINAL incorporated the queued instruction. location: reports/steer.md. how to use: open the report."})
            return

        if scenario == "recovery":
            if count == 1:
                self._tool({"intent": "done", "text": "STALE_RECOVERY_RESPONSE_MUST_NOT_APPEAR"})
            else:
                self._tool({"intent": "done", "text": "summary: RECOVERED_FINAL completed. location: reports/recovered.md. how to use: open the report."})
            return

        self._json(500, {"error": {"message": f"no scripted response for {scenario}/{model} #{count}"}})


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), LifecycleProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def history(base: str, password: str, chat_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 200})["messages"]


def traces(base: str, password: str, chat_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"chat_id": chat_id, "tail": True, "limit": 500})["items"]


def assignment_traces(base: str, password: str, assignment_id: str) -> list[dict[str, Any]]:
    return rpc(base, password, "trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500})["items"]


def bot_messages(messages: list[dict[str, Any]], bot_id: str | None = None) -> list[dict[str, Any]]:
    return [
        message for message in messages
        if message.get("sender", {}).get("kind") == "bot"
        and (bot_id is None or message.get("sender", {}).get("bot_id") == bot_id)
    ]


def wait_message(base: str, password: str, chat_id: str, text: str, timeout: float = 45) -> list[dict[str, Any]]:
    found: list[dict[str, Any]] = []

    def ready() -> bool:
        found[:] = history(base, password, chat_id)
        return any(text in message.get("fallback_text", "") for message in bot_messages(found))

    wait_until(ready, f"public message containing {text}", timeout)
    return found


def assert_ack(messages: list[dict[str, Any]], source_id: str, bot_id: str) -> dict[str, Any]:
    acknowledgements = [
        message for message in bot_messages(messages, bot_id)
        if message.get("source_message_id") == source_id and message.get("intent") == "ack"
    ]
    assert len(acknowledgements) == 1, (source_id, bot_id, acknowledgements, messages)
    ack = acknowledgements[0]
    assert ack.get("fallback_text", "").strip() == "收到，我来处理。", ack
    assert ack.get("streaming") is False, ack
    return ack


def assert_chat_messages_are_complete(messages: list[dict[str, Any]]) -> None:
    public = bot_messages(messages)
    ids = [message.get("id") for message in public]
    assert all(isinstance(value, str) and value for value in ids), public
    assert len(ids) == len(set(ids)), public
    for message in public:
        text = message.get("fallback_text", "").strip()
        assert text, message
        assert message.get("streaming") is False, message
        if message.get("intent") == "done":
            lowered = text.lower()
            for label in ("summary:", "location:", "how to use:"):
                assert label in lowered, message
    seqs = [message.get("seq") for message in public]
    assert all(isinstance(value, int) for value in seqs), public
    assert seqs == sorted(seqs) and len(seqs) == len(set(seqs)), public


def assert_send_msg_schema_is_server_scoped(body: dict[str, Any]) -> None:
    tools = body.get("tools", [])
    schema = next((
        tool.get("function", {}).get("parameters", {})
        for tool in tools
        if tool.get("function", {}).get("name") == "send_msg"
    ), None)
    assert isinstance(schema, dict), body.get("tools")
    properties = schema.get("properties", {})
    for field in ("chat_id", "message_id", "bot_id", "assignment_id", "to"):
        assert field not in properties, (field, properties)


def wait_provider(state: ProviderState, scenario: str, model: str, count: int = 1, timeout: float = 35) -> dict[str, Any]:
    state.wait_count(scenario, model, count, timeout)
    return state.request(scenario, model, count)


class RestartableDaemon(Daemon):
    def start(self) -> None:
        env = os.environ.copy()
        if self.args.browser_bin:
            env["MACBOT_BROWSER_BIN"] = self.args.browser_bin
        if self.args.home:
            env["MACBOT_HOME"] = str(self.args.home)
            env["MACBOT_SECRET_BACKEND"] = "file"
            env["MACBOT_SECRET_DIR"] = str(self.args.home / "secrets")
        log_path = self.args.home / "macbotd.log"
        with log_path.open("ab") as log:
            self.process = subprocess.Popen(
                shlex.split(self.args.daemon_command),
                cwd=REPO,
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        wait_until(
            lambda: http_json(f"{self.args.url}/api/v1/health").get("ok") is True,
            "macbotd startup",
            45,
        )

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
    marker_values = {
        "choice": f"LIFECYCLE_CHOICE_{suffix}",
        "correction": f"LIFECYCLE_CORRECTION_{suffix}",
        "repeat": f"LIFECYCLE_REPEAT_{suffix}",
        "group_shared": f"LIFECYCLE_GROUP_SHARED_{suffix}",
        "scope": f"LIFECYCLE_SCOPE_{suffix}",
        "steer_start": f"LIFECYCLE_STEER_START_{suffix}",
        "steer_input": f"LIFECYCLE_STEER_INPUT_{suffix}",
        "recovery": f"LIFECYCLE_RECOVERY_{suffix}",
    }
    STATE.configure({
        "choice": ("choice", marker_values["choice"]),
        "correction": ("correction", marker_values["correction"]),
        "repeat": ("repeat", marker_values["repeat"]),
        "group_shared": ("group_shared", marker_values["group_shared"]),
        "scope": ("scope", marker_values["scope"]),
        "steer_start": ("steer", marker_values["steer_start"]),
        "steer_input": ("steer", marker_values["steer_input"]),
        "recovery": ("recovery", marker_values["recovery"]),
    })
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

        def add_model(model_id: str) -> str:
            result = rpc(base, password, "model.upsert", {
                "provider_id": provider_result["id"],
                "model_id": model_id,
                "display_name": model_id,
                "caps": {"vision": False, "tools": True, "reasoning": False},
                "client_request_id": f"reply-lifecycle-model:{model_id}:{suffix}",
            })
            return result["model"]["ref"]

        model_a = add_model(MODEL_A)
        model_b = add_model(MODEL_B)
        model_main = add_model(MODEL_MAIN)
        rpc(base, password, "bot.update", {
            "bot_id": "main",
            "patch": {"model": model_main, "max_parallel": 2},
            "client_request_id": f"reply-lifecycle-main-model:{suffix}",
        })
        no_tools = {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False}
        bot_a = rpc(base, password, "bot.create", {
            "name": f"Lifecycle A {suffix}", "model": model_a, "max_parallel": 4, "tools": no_tools,
        })["bot"]
        bot_b = rpc(base, password, "bot.create", {
            "name": f"Lifecycle B {suffix}", "model": model_b, "max_parallel": 3, "tools": no_tools,
        })["bot"]
        dm_chat = bot_a["dm_chat_id"]

        # A group message with no @ defaults to Main. This run has no
        # assignment_id, so its decision answer must still resume the same run.
        choice_project = rpc(base, password, "project.create", {
            "name": f"Lifecycle Main choice {suffix}",
            "goal": "Verify an unassigned Main group question",
            "member_bot_ids": [bot_a["id"]],
            "client_request_id": f"reply-lifecycle-choice-project:{suffix}",
        })
        choice_chat = choice_project["chat"]["id"]

        # A complete acknowledgement is persisted before the first provider
        # request is allowed to return any model output.
        choice_source = rpc(base, password, "chat.send", {
            "chat_id": choice_chat,
            "text": marker_values["choice"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-choice:{suffix}",
        })["message"]
        choice_request = wait_provider(STATE, "choice", MODEL_MAIN)
        assert_send_msg_schema_is_server_scoped(choice_request["body"])
        before_choice = history(base, password, choice_chat)
        choice_ack = assert_ack(before_choice, choice_source["id"], "main")
        assert_chat_messages_are_complete(before_choice)
        STATE.release("choice", MODEL_MAIN)

        choice_question: dict[str, Any] = {}

        def question_ready() -> bool:
            questions = rpc(base, password, "question.list", {"chat_id": choice_chat})["questions"]
            pending = [item for item in questions if item.get("state") == "pending"]
            if not pending:
                return False
            choice_question.update(pending[-1])
            return True

        wait_until(question_ready, "choice question from send_msg(decision)", 35)
        assert choice_question.get("chat_id") == choice_chat, choice_question
        # The protocol requires Question.assignment_id even for a Main run
        # without a real assignment. The orchestrator represents that scope
        # as dm_<chat_id>; the run and public message must still remain
        # unassigned.
        assert choice_question.get("bot_id") == "main", choice_question
        assert choice_question.get("assignment_id") == f"dm_{choice_chat}", choice_question
        decision_messages = history(base, password, choice_chat)
        decision_message = next(
            message for message in bot_messages(decision_messages, "main")
            if any(block.get("question_id") == choice_question["id"] for block in message.get("blocks", []))
        )
        assert decision_message.get("intent") == "decision", decision_message
        assert decision_message.get("source_message_id") == choice_source["id"], decision_message
        assert decision_message.get("assignment_id") is None, decision_message
        assert decision_message["id"] != choice_ack["id"], decision_message
        trace_wait = traces(base, password, choice_chat)
        decision_run = next(item.get("run_id") for item in trace_wait if item.get("type") == "run.start")
        assert any(item.get("type") == "run.wait" and item.get("data", {}).get("reason") == "decision" for item in trace_wait), trace_wait
        assert STATE.count("choice", MODEL_MAIN) == 1, STATE.count("choice", MODEL_MAIN)

        answered = rpc(base, password, "question.answer", {
            "question_id": choice_question["id"], "option_index": 1,
            "client_request_id": f"reply-lifecycle-choice-answer:{suffix}",
        })["question"]
        assert answered.get("state") == "answered" and answered.get("answer", {}).get("option_index") == 1, answered
        choice_resume = wait_provider(STATE, "choice", MODEL_MAIN, 2)
        assert_send_msg_schema_is_server_scoped(choice_resume["body"])
        wait_message(base, password, choice_chat, "how to use: 打开报告并按步骤部署。")
        choice_history = history(base, password, choice_chat)
        choice_messages = [message for message in bot_messages(choice_history, "main") if message.get("source_message_id") == choice_source["id"]]
        choice_done = next(message for message in choice_messages if "summary: 已按选择完成" in message.get("fallback_text", ""))
        assert choice_done.get("intent") == "done" and choice_done.get("id") not in {choice_ack["id"], decision_message["id"]}, choice_done
        assert "docs/release-report.md" in choice_done.get("fallback_text", ""), choice_done
        assert "INTERNAL_CHOICE" not in json.dumps(choice_history, ensure_ascii=False)
        choice_trace = traces(base, password, choice_chat)
        choice_runs = {item.get("run_id") for item in choice_trace if item.get("type") in {"run.start", "run.end"}}
        assert choice_runs == {decision_run}, (decision_run, choice_runs, choice_trace)
        assert sum(item.get("type") == "run.start" and item.get("run_id") == decision_run for item in choice_trace) == 1, choice_trace
        assert_chat_messages_are_complete(choice_history)

        # A native-only answer is withheld from chat; the engine asks once for
        # a send_msg report, then publishes only the corrected done message.
        correction_source = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker_values["correction"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-correction:{suffix}",
        })["message"]
        first_correction = wait_provider(STATE, "correction", MODEL_A)
        assert_send_msg_schema_is_server_scoped(first_correction["body"])
        correction_ack = assert_ack(history(base, password, dm_chat), correction_source["id"], bot_a["id"])
        STATE.release("correction", MODEL_A)
        correction_retry = wait_provider(STATE, "correction", MODEL_A, 2)
        correction_prompt = json.dumps(correction_retry["body"].get("messages", []), ensure_ascii=False)
        assert "send_msg" in correction_prompt and "报告" in correction_prompt, correction_retry
        assert_send_msg_schema_is_server_scoped(correction_retry["body"])
        wait_message(base, password, dm_chat, "corrected report")
        correction_history = history(base, password, dm_chat)
        correction_messages = [message for message in bot_messages(correction_history, bot_a["id"]) if message.get("source_message_id") == correction_source["id"]]
        assert_ack(correction_history, correction_source["id"], bot_a["id"])
        assert any(message.get("intent") == "done" and "reports/final.md" in message.get("fallback_text", "") for message in correction_messages), correction_messages
        assert "CORRECTION_NATIVE_ONLY" not in json.dumps(correction_history, ensure_ascii=False)
        assert correction_ack["id"] != next(message for message in correction_messages if message.get("intent") == "done")["id"]
        assert_chat_messages_are_complete(correction_history)

        # Two identical user texts remain separate sources, each with its own
        # one-time ack and final message IDs.
        repeated_sources: list[dict[str, Any]] = []
        for index in (1, 2):
            if index > 1:
                STATE.rearm("repeat", MODEL_A, index)
            source = rpc(base, password, "chat.send", {
                "chat_id": dm_chat,
                "text": marker_values["repeat"],
                "mentions": [],
                "client_request_id": f"reply-lifecycle-repeat-{index}:{suffix}",
            })["message"]
            repeated_sources.append(source)
            repeat_request = wait_provider(STATE, "repeat", MODEL_A, index)
            assert_send_msg_schema_is_server_scoped(repeat_request["body"])
            repeat_waiting = history(base, password, dm_chat)
            assert_ack(repeat_waiting, source["id"], bot_a["id"])
            assert_chat_messages_are_complete(repeat_waiting)
            STATE.release("repeat", MODEL_A, index)
            wait_message(base, password, dm_chat, f"REPEAT_DONE_{index}")
        repeat_history = history(base, password, dm_chat)
        repeated_ack_ids: set[str] = set()
        for source in repeated_sources:
            ack = assert_ack(repeat_history, source["id"], bot_a["id"])
            repeated_ack_ids.add(ack["id"])
            replies = [message for message in bot_messages(repeat_history, bot_a["id"]) if message.get("source_message_id") == source["id"]]
            assert any(message.get("intent") == "done" and f"REPEAT_DONE_{repeated_sources.index(source) + 1}" in message.get("fallback_text", "") for message in replies), replies
        assert len(repeated_ack_ids) == 2, repeated_ack_ids
        assert_chat_messages_are_complete(repeat_history)

        # One Bot works in two group chats at once. Both independent Bots in
        # the first group get separate acknowledgements. The second group run
        # tries to forge a chat destination and must stay in its bound chat.
        project_one_result = rpc(base, password, "project.create", {
            "name": f"Lifecycle group one {suffix}",
            "goal": "Verify independent Bot group replies",
            "member_bot_ids": [bot_a["id"], bot_b["id"]],
            "client_request_id": f"reply-lifecycle-project-one:{suffix}",
        })
        project_one = project_one_result["project"]
        group_one = project_one_result["chat"]["id"]
        project_two_result = rpc(base, password, "project.create", {
            "name": f"Lifecycle group two {suffix}",
            "goal": "Verify immutable chat scope",
            "member_bot_ids": [bot_a["id"]],
            "client_request_id": f"reply-lifecycle-project-two:{suffix}",
        })
        project_two = project_two_result["project"]
        group_two = project_two_result["chat"]["id"]
        STATE.forged_chat_id = group_one
        group_source = rpc(base, password, "chat.send", {
            "chat_id": group_one,
            "text": marker_values["group_shared"],
            "mentions": [
                {"kind": "bot", "bot_id": bot_a["id"], "instruction": marker_values["group_shared"]},
                {"kind": "bot", "bot_id": bot_b["id"], "instruction": marker_values["group_shared"]},
            ],
            "client_request_id": f"reply-lifecycle-group-one:{suffix}",
        })["message"]
        group_a_request = wait_provider(STATE, "group_shared", MODEL_A)
        group_b_request = wait_provider(STATE, "group_shared", MODEL_B)
        assert_send_msg_schema_is_server_scoped(group_a_request["body"])
        assert_send_msg_schema_is_server_scoped(group_b_request["body"])
        group_one_before = history(base, password, group_one)
        group_ack_a = assert_ack(group_one_before, group_source["id"], bot_a["id"])
        group_ack_b = assert_ack(group_one_before, group_source["id"], bot_b["id"])
        assert group_ack_a["id"] != group_ack_b["id"], (group_ack_a, group_ack_b)
        assert_chat_messages_are_complete(group_one_before)

        scope_source = rpc(base, password, "chat.send", {
            "chat_id": group_two,
            "text": marker_values["scope"],
            "mentions": [{"kind": "bot", "bot_id": bot_a["id"], "instruction": marker_values["scope"]}],
            "client_request_id": f"reply-lifecycle-scope:{suffix}",
        })["message"]
        scope_request = wait_provider(STATE, "scope", MODEL_A)
        assert_send_msg_schema_is_server_scoped(scope_request["body"])
        group_two_before = history(base, password, group_two)
        scope_ack = assert_ack(group_two_before, scope_source["id"], bot_a["id"])
        assert_chat_messages_are_complete(group_two_before)
        assert all(message.get("source_message_id") != scope_source["id"] for message in bot_messages(group_one_before)), group_one_before
        STATE.release("scope", MODEL_A)
        scope_retry = wait_provider(STATE, "scope", MODEL_A, 2)
        assert_send_msg_schema_is_server_scoped(scope_retry["body"])
        scope_retry_messages = json.dumps(scope_retry["body"].get("messages", []), ensure_ascii=False)
        assert "FORGED_SCOPE_MUST_NOT_PUBLISH" in scope_retry_messages, scope_retry_messages
        assert "fixed by this execution's chat scope" in scope_retry_messages, scope_retry_messages
        wait_message(base, password, group_two, "SCOPE_BOUND_FINAL")
        scope_history = history(base, password, group_two)
        assert "FORGED_SCOPE_MUST_NOT_PUBLISH" not in json.dumps(scope_history, ensure_ascii=False)
        assert assert_ack(scope_history, scope_source["id"], bot_a["id"])["id"] == scope_ack["id"]
        assert_chat_messages_are_complete(scope_history)

        # Resolve the first group's two model calls only after the second
        # group has completed, proving both runs stayed in their own chat.
        STATE.release("group_shared", MODEL_A)
        STATE.release("group_shared", MODEL_B)
        wait_message(base, password, group_one, "GROUP_A_DONE")
        wait_message(base, password, group_one, "GROUP_B_DONE")
        group_one_history = history(base, password, group_one)
        group_one_a = [message for message in bot_messages(group_one_history, bot_a["id"]) if message.get("source_message_id") == group_source["id"]]
        group_one_b = [message for message in bot_messages(group_one_history, bot_b["id"]) if message.get("source_message_id") == group_source["id"]]
        assert sum(message.get("intent") == "ack" for message in group_one_a) == 1, group_one_a
        assert sum(message.get("intent") == "ack" for message in group_one_b) == 1, group_one_b
        assert any("GROUP_A_DONE" in message.get("fallback_text", "") for message in group_one_a), group_one_a
        assert any("GROUP_B_DONE" in message.get("fallback_text", "") for message in group_one_b), group_one_b
        assert "GROUP_A_PRIVATE_MODEL_TEXT" not in json.dumps(group_one_history, ensure_ascii=False)
        assert "GROUP_B_PRIVATE_MODEL_TEXT" not in json.dumps(group_one_history, ensure_ascii=False)
        assert "SCOPE_BOUND_FINAL" not in json.dumps(group_one_history, ensure_ascii=False)
        assert "FORGED_SCOPE_MUST_NOT_PUBLISH" not in json.dumps(group_one_history, ensure_ascii=False)
        assert_chat_messages_are_complete(group_one_history)
        assert_chat_messages_are_complete(history(base, password, group_two))

        # A targeted steer is separately sourced and acknowledged when it is
        # accepted into the in-flight Bot run, before the provider gate opens.
        steer_project_result = rpc(base, password, "project.create", {
            "name": f"Lifecycle steer {suffix}",
            "goal": "Verify per-source steer acknowledgement",
            "member_bot_ids": [bot_a["id"]],
            "client_request_id": f"reply-lifecycle-project-steer:{suffix}",
        })
        steer_chat = steer_project_result["chat"]["id"]
        steer_initial = rpc(base, password, "chat.send", {
            "chat_id": steer_chat,
            "text": marker_values["steer_start"],
            "mentions": [{"kind": "bot", "bot_id": bot_a["id"], "instruction": marker_values["steer_start"]}],
            "client_request_id": f"reply-lifecycle-steer-start:{suffix}",
        })["message"]
        steer_request = wait_provider(STATE, "steer", MODEL_A)
        assert_send_msg_schema_is_server_scoped(steer_request["body"])
        initial_steer_history = history(base, password, steer_chat)
        initial_steer_ack = assert_ack(initial_steer_history, steer_initial["id"], bot_a["id"])
        steer_assignments = rpc(base, password, "assignment.list", {"limit": 200})["items"]
        steer_assignment = next(
            item for item in steer_assignments
            if item.get("project_id") == steer_project_result["project"]["id"]
            and item.get("bot_id") == bot_a["id"]
            and item.get("origin_chat_id") == steer_chat
            and marker_values["steer_start"] in item.get("instruction", "")
        )
        steer_trace_before = assignment_traces(base, password, steer_assignment["id"])
        steer_run_starts = [item for item in steer_trace_before if item.get("type") == "run.start"]
        assert len(steer_run_starts) == 1, steer_trace_before
        steer_run_id = steer_run_starts[0].get("run_id")
        steer_message = rpc(base, password, "chat.send", {
            "chat_id": steer_chat,
            "text": marker_values["steer_input"],
            "mentions": [{"kind": "bot", "bot_id": bot_a["id"], "instruction": marker_values["steer_input"]}],
            "client_request_id": f"reply-lifecycle-steer-input:{suffix}",
        })["message"]
        steer_waiting_history: list[dict[str, Any]] = []

        def steer_ack_ready() -> bool:
            steer_waiting_history[:] = history(base, password, steer_chat)
            return any(
                message.get("source_message_id") == steer_message["id"]
                and message.get("intent") == "ack"
                for message in bot_messages(steer_waiting_history, bot_a["id"])
            )

        wait_until(steer_ack_ready, "steer acknowledgement while provider gate is closed", 10)
        steer_ack = assert_ack(steer_waiting_history, steer_message["id"], bot_a["id"])
        assert_chat_messages_are_complete(steer_waiting_history)
        assert steer_ack["id"] != initial_steer_ack["id"], (initial_steer_ack, steer_ack)
        assert STATE.count("steer", MODEL_A) == 1, STATE.count("steer", MODEL_A)
        STATE.release("steer", MODEL_A)
        steer_resume = wait_provider(STATE, "steer", MODEL_A, 2)
        assert_send_msg_schema_is_server_scoped(steer_resume["body"])
        steer_resume_prompt = json.dumps(steer_resume["body"].get("messages", []), ensure_ascii=False)
        assert marker_values["steer_input"] in steer_resume_prompt, steer_resume_prompt
        assert "STEER_NATIVE_ONLY" in steer_resume_prompt and "send_msg" in steer_resume_prompt, steer_resume_prompt
        wait_message(base, password, steer_chat, "STEER_FINAL")
        steer_history = history(base, password, steer_chat)
        assert_ack(steer_history, steer_initial["id"], bot_a["id"])
        assert_ack(steer_history, steer_message["id"], bot_a["id"])
        assert "STEER_NATIVE_ONLY" not in json.dumps(steer_history, ensure_ascii=False)
        assert sum(message.get("id") == steer_ack["id"] for message in bot_messages(steer_history)) == 1, steer_history
        assert any(message.get("intent") == "done" and message.get("source_message_id") in {steer_initial["id"], steer_message["id"]} for message in bot_messages(steer_history)), steer_history
        steer_trace_after = assignment_traces(base, password, steer_assignment["id"])
        steer_run_starts_after = [item for item in steer_trace_after if item.get("type") == "run.start"]
        assert len(steer_run_starts_after) == 1 and steer_run_starts_after[0].get("run_id") == steer_run_id, steer_trace_after
        assert_chat_messages_are_complete(steer_history)

        # Kill the daemon after a persisted acknowledgement and before the
        # provider responds. Recovery must retain one run and one ack.
        traces_before_recovery = traces(base, password, dm_chat)
        recovery_source = rpc(base, password, "chat.send", {
            "chat_id": dm_chat,
            "text": marker_values["recovery"],
            "mentions": [],
            "client_request_id": f"reply-lifecycle-recovery:{suffix}",
        })["message"]
        recovery_request = wait_provider(STATE, "recovery", MODEL_A)
        assert_send_msg_schema_is_server_scoped(recovery_request["body"])
        recovery_before = history(base, password, dm_chat)
        recovery_ack = assert_ack(recovery_before, recovery_source["id"], bot_a["id"])
        assert_chat_messages_are_complete(recovery_before)
        recovery_trace_before = traces(base, password, dm_chat)
        recovery_run_ids = {
            item.get("run_id")
            for item in recovery_trace_before
            if item.get("type") == "run.start"
        } - {
            item.get("run_id")
            for item in traces_before_recovery
            if item.get("type") == "run.start"
        }
        assert len(recovery_run_ids) == 1, recovery_trace_before
        daemon.kill9()
        daemon.start()
        resumed_recovery = wait_provider(STATE, "recovery", MODEL_A, 2, 60)
        assert_send_msg_schema_is_server_scoped(resumed_recovery["body"])
        wait_message(base, password, dm_chat, "RECOVERED_FINAL", 60)
        STATE.release("recovery", MODEL_A)
        recovery_after = history(base, password, dm_chat)
        assert_ack(recovery_after, recovery_source["id"], bot_a["id"])
        assert sum(message.get("id") == recovery_ack["id"] for message in bot_messages(recovery_after)) == 1, recovery_after
        recovery_replies = [message for message in bot_messages(recovery_after, bot_a["id"]) if message.get("source_message_id") == recovery_source["id"]]
        assert len(recovery_replies) == 2, recovery_replies  # one ack and one final report
        assert any(message.get("intent") == "done" and "RECOVERED_FINAL" in message.get("fallback_text", "") for message in recovery_replies), recovery_replies
        assert "STALE_RECOVERY_RESPONSE_MUST_NOT_APPEAR" not in json.dumps(recovery_after, ensure_ascii=False)
        recovery_trace_after = traces(base, password, dm_chat)
        recovery_starts = [item for item in recovery_trace_after if item.get("type") == "run.start"]
        assert len(recovery_starts) == 4, recovery_starts  # correction, two repeats, recovery
        assert recovery_starts[-1].get("run_id") in recovery_run_ids, (recovery_starts, recovery_run_ids)
        assert STATE.count("recovery", MODEL_A) == 2, STATE.count("recovery", MODEL_A)
        assert_chat_messages_are_complete(recovery_after)

        print(json.dumps({
            "ok": True,
            "home": str(args.home),
            "choice_chat_without_assignment": choice_chat,
            "choice_source": choice_source["id"],
            "choice_question": choice_question["id"],
            "choice_done": choice_done["id"],
            "group_one": group_one,
            "group_two": group_two,
            "steer_sources": [steer_initial["id"], steer_message["id"]],
            "recovery_source": recovery_source["id"],
            "recovery_run_id": next(iter(recovery_run_ids)),
        }, ensure_ascii=False))
        return {"ok": True}
    except BaseException:
        log_path = args.home / "macbotd.log"
        if log_path.exists():
            lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
            print(f"daemon log tail ({log_path}):\n" + "\n".join(lines[-80:]), file=sys.stderr)
        raise
    finally:
        STATE.release_all()
        daemon.close()
        provider.shutdown()
        provider.server_close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True, help="fresh isolated home under /tmp")
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
