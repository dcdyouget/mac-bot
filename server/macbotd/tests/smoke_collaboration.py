#!/usr/bin/env python3
"""Production collaboration acceptance.

This script talks to a real macbotd over HTTP.  The only fake component is a
local OpenAI-compatible provider, so model execution remains the production
runtime path while the test never sends a real API key or network request.

Example:
  python server/macbotd/tests/smoke_collaboration.py \
    --daemon-command 'cargo run --manifest-path server/macbotd/Cargo.toml -- --port 7797 --password dev' \
    --home /tmp/macbot-collaboration-smoke
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable
import urllib.error
import urllib.request
import uuid


REPO = Path(__file__).resolve().parents[3]
TOKEN = "macbot-collaboration-fake-token"


class FakeProviderHandler(BaseHTTPRequestHandler):
    calls = 0
    lock = threading.Lock()
    scenario: dict[str, Any] = {}

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _stream(self, text: str) -> None:
        payloads = [
            {"choices": [{"delta": {"content": text}, "finish_reason": None}]},
            {
                "choices": [{"delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 7, "completion_tokens": 3},
            },
        ]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def _stream_tool(self, name: str, arguments: dict[str, Any]) -> None:
        call_id = f"smoke-call-{type(self).calls}"
        payloads = [
            {
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": call_id,
                            "type": "function",
                            "function": {"name": name, "arguments": json.dumps(arguments)},
                        }]
                    },
                    "finish_reason": None,
                }]
            },
            {
                "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 9, "completion_tokens": 5},
            },
        ]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    @staticmethod
    def _called_tools(messages: list[dict[str, Any]]) -> set[str]:
        names: set[str] = set()
        for message in messages:
            for call in message.get("tool_calls", []):
                name = call.get("function", {}).get("name")
                if isinstance(name, str):
                    names.add(name)
        return names

    @staticmethod
    def _tool_project_id(messages: list[dict[str, Any]]) -> str | None:
        for message in reversed(messages):
            if message.get("role") != "tool":
                continue
            try:
                value = json.loads(message.get("content", ""))
            except (TypeError, json.JSONDecodeError):
                continue
            project = value.get("project") if isinstance(value, dict) else None
            if isinstance(project, dict) and isinstance(project.get("id"), str):
                return project["id"]
        return None

    def _scripted_tool(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        scenario = type(self).scenario
        prompt = json.dumps(messages, ensure_ascii=False)
        called = self._called_tools(messages)
        # The main coordination script is only valid for the main Bot.  Child
        # Bots receive the original marker in their worklog, so matching the
        # marker alone would make every worker try to create another project.
        is_main_bot = any(
            message.get("role") == "system" and "\n总管\n" in message.get("content", "")
            for message in messages
        )
        project_id = self._tool_project_id(messages) or scenario.get("project_id")
        decision_at = prompt.rfind(scenario.get("decision_marker", ""))
        blocked_at = prompt.rfind(scenario.get("blocked_marker", ""))
        # Project chat history contains both waiting scenarios.  Use the
        # marker from the latest user turn so an earlier decision does not
        # steal the blocked-message continuation.
        if decision_at >= 0 and decision_at >= blocked_at:
            if "send_msg" not in called:
                return "send_msg", {
                    "intent": "decision",
                    "text": "需要用户确认是否继续部署",
                    "options": ["继续", "停止"],
                }
            if "用户确认继续" in prompt:
                return "send_msg", {
                    "intent": "done",
                    "text": "用户已确认，决策任务完成",
                    "mentions": [],
                }
            return None
        if blocked_at >= 0:
            if "send_msg" not in called:
                return "send_msg", {
                    "intent": "blocked",
                    "text": "缺少生产环境凭据，等待用户处理",
                    "options": ["已处理", "取消"],
                }
            if "用户已处理阻塞" in prompt:
                return "send_msg", {
                    "intent": "done",
                    "text": "阻塞已解除，任务完成",
                    "mentions": [],
                }
            return None
        if scenario.get("takeover_marker") and scenario["takeover_marker"] in prompt and "request_takeover" in called:
            return None
        if scenario.get("takeover_marker") and scenario["takeover_marker"] in prompt and "request_takeover" not in called:
            return "request_takeover", {"reason": "smoke 模型需要用户登录"}
        if scenario.get("question_marker") and scenario["question_marker"] in prompt and "question" not in called:
            return "question", {"question": "smoke 请选择登录环境"}
        if scenario.get("question_marker") and scenario["question_marker"] in prompt and "question" in called:
            return None
        if scenario.get("notify_marker") and scenario["notify_marker"] in prompt and "notify_user" not in called:
            return "notify_user", {
                "text": "巡检完成，已通知用户",
                "intent": "progress",
            }
        if is_main_bot and scenario.get("main_marker") and scenario["main_marker"] in prompt:
            if "create_project" not in called:
                return "create_project", {
                    "name": scenario["project_name"],
                    "goal": "模型驱动协作 smoke",
                    "member_bot_ids": [scenario["coder_id"], scenario["tester_id"]],
                    "flow": ["编码", "测试"],
                }
            if "assign" not in called:
                return "assign", {
                    "bot_id": scenario["coder_id"],
                    "project_id": project_id,
                    "title": "模型派发编码",
                    "instruction": "实现模型驱动协作 smoke",
                }
            if "delegate" not in called:
                return "delegate", {
                    "bot_id": scenario["tester_id"],
                    "project_id": project_id,
                    "title": "模型委派测试",
                    "instruction": "验证模型驱动协作 smoke",
                }
            if "send_msg" not in called:
                return "send_msg", {
                    # Keep the coordinator run alive for the subsequent
                    # propose_bot/request_review calls.  `done` is terminal
                    # by protocol and would correctly stop the model before
                    # it can publish the review card.
                    "intent": "progress",
                    "text": "编码完成，请测试并回报主 Bot",
                    # Duplicate mentions are deliberate: this is the model's
                    # real handoff and verifies the orchestrator deduplicates
                    # one child assignment before the gateway dispatches it.
                    "mentions": [
                        {"kind": "bot", "bot_id": scenario["tester_id"], "instruction": "接手测试"},
                        {"kind": "bot", "bot_id": scenario["tester_id"], "instruction": "重复交接不得创建第二个任务"},
                    ],
                }
            if "propose_bot" not in called:
                return "propose_bot", {
                    "name": f"模型提议-{scenario['suffix']}",
                    "label": "smoke",
                    "description": "模型通过工具提出的 Bot",
                }
            if "request_review" not in called:
                return "request_review", {
                    "project_id": project_id,
                    "summary": "模型已完成编码和测试交接，请用户审核",
                }
        if is_main_bot and scenario.get("finish_marker") and scenario["finish_marker"] in prompt and "finish_project" not in called:
            return "finish_project", {
                "project_id": scenario["project_id"],
                "summary": "用户确认后完成项目",
            }
        if scenario.get("subagent_marker") and scenario["subagent_marker"] in prompt and "subagent" not in called:
            return "subagent", {
                "action": "start",
                "task": "nested-trace child: inspect the assignment",
                "max_turns": 2,
            }
        return None

    def _authorized(self) -> bool:
        return self.headers.get("Authorization") == f"Bearer {TOKEN}"

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self._authorized():
            self._json(200, {"data": [{"id": "collaboration-fake", "object": "model"}]})
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
        if body.get("model") != "collaboration-fake":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        with self.lock:
            type(self).calls += 1
        prompt = json.dumps(body.get("messages", []), ensure_ascii=False)
        if "parallel-collaboration" in prompt:
            time.sleep(1.2)
        scripted = self._scripted_tool(body.get("messages", []))
        if scripted is not None:
            self._stream_tool(*scripted)
            return
        self._stream("production fake execution complete")


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def http_json(url: str, value: dict[str, Any] | None = None, password: str | None = None) -> Any:
    headers = {"Content-Type": "application/json"}
    if password:
        headers["Authorization"] = f"Bearer {password}"
    request = urllib.request.Request(
        url,
        data=None if value is None else json.dumps(value).encode(),
        headers=headers,
        method="GET" if value is None else "POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise AssertionError(f"HTTP {error.code} {url}: {error.read().decode(errors='replace')}") from error


def rpc(base: str, password: str, method: str, params: dict[str, Any] | None = None) -> Any:
    response = http_json(
        f"{base}/api/v1/rpc",
        {"method": method, "params": params or {}},
        password,
    )
    if not response.get("ok"):
        raise AssertionError(f"{method}: {response.get('error')}")
    return response["result"]


def wait_until(predicate: Callable[[], bool], description: str, timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.2)
    raise AssertionError(f"timed out waiting for {description}")


class Daemon:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.process: subprocess.Popen[bytes] | None = None

    def start(self) -> None:
        if not self.args.daemon_command:
            wait_until(lambda: http_json(f"{self.args.url}/api/v1/health").get("ok") is True, "existing macbotd", 15)
            return
        env = os.environ.copy()
        if self.args.browser_bin:
            env["MACBOT_BROWSER_BIN"] = self.args.browser_bin
        if self.args.home:
            env["MACBOT_HOME"] = str(self.args.home)
            # Keep acceptance credentials isolated from the user's keychain and
            # from any concurrently deployed daemon.
            env["MACBOT_SECRET_BACKEND"] = "file"
            env["MACBOT_SECRET_DIR"] = str(self.args.home / "secrets")
        self.process = subprocess.Popen(
            shlex.split(self.args.daemon_command),
            cwd=REPO,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        wait_until(lambda: http_json(f"{self.args.url}/api/v1/health").get("ok") is True, "macbotd startup", 45)

    def close(self) -> None:
        if self.process is None or self.process.poll() is not None:
            return
        try:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            self.process.kill()


def ids(value: list[dict[str, Any]]) -> set[str]:
    return {item["id"] for item in value}


def acceptance(args: argparse.Namespace, provider_url: str) -> None:
    base = args.url.rstrip("/")
    password = args.password
    assert http_json(f"{base}/api/v1/health").get("protocol") == 1
    boot = rpc(base, password, "bootstrap")
    assert any(bot["is_main"] for bot in boot["bots"])

    suffix = uuid.uuid4().hex[:8]
    provider = rpc(
        base,
        password,
        "provider.create",
        {
            "name": f"collaboration-fake-{suffix}",
            "api_kind": "openai-completions",
            "base_url": provider_url,
            "api_key": TOKEN,
            "client_request_id": f"collab-provider-{suffix}",
        },
    )["provider"]
    provider_id = provider["id"]
    assert TOKEN not in json.dumps(provider)
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider_id})
    assert any(model["model_id"] == "collaboration-fake" for model in refreshed["models"])
    model = rpc(
        base,
        password,
        "model.upsert",
        {
            "provider_id": provider_id,
            "model_id": "collaboration-fake",
            "display_name": "Collaboration fake",
            "caps": {"vision": False, "tools": True, "reasoning": False},
            "client_request_id": f"collab-model-{suffix}",
        },
    )["model"]["ref"]
    rpc(base, password, "bot.update", {"bot_id": "main", "patch": {"model": model}, "client_request_id": f"main-model-{suffix}"})

    product = rpc(base, password, "bot.create", {"name": f"产品-{suffix}", "model": model, "max_parallel": 2})["bot"]
    worker_tools = {"files": False, "bash": False, "browser": True, "subagent": True, "web": False, "mcp": False}
    coder = rpc(base, password, "bot.create", {"name": f"编码-{suffix}", "model": model, "max_parallel": 2, "tools": worker_tools})["bot"]
    tester = rpc(base, password, "bot.create", {"name": f"测试-{suffix}", "model": model, "max_parallel": 2})["bot"]

    model_marker = f"model-tool-collaboration-{suffix}"
    FakeProviderHandler.scenario = {
        "main_marker": model_marker,
        "finish_marker": f"model-tool-finish-{suffix}",
        "subagent_marker": f"model-subagent-{suffix}",
        "notify_marker": f"model-notify-{suffix}",
        "takeover_marker": f"model-takeover-{suffix}",
        "question_marker": f"model-question-{suffix}",
        "decision_marker": f"model-decision-{suffix}",
        "blocked_marker": f"model-blocked-{suffix}",
        "project_name": f"模型协作-{suffix}",
        "coder_id": coder["id"],
        "tester_id": tester["id"],
        "suffix": suffix,
    }

    # Team templates are a production RPC and must return usable Bot records.
    templates = rpc(base, password, "bot.templates")
    assert templates["templates"], templates
    template_id = templates["templates"][0]["id"]
    templated = rpc(base, password, "bot.create_from_template", {"template_id": template_id})
    assert templated["bots"] and len(templated["dm_chats"]) <= len(templated["bots"])

    routine = rpc(
        base,
        password,
        "routine.create",
        {
            "bot_id": coder["id"],
            "name": f"登录巡检-{suffix}",
            "instructions": "检查登录回归状态",
            "schedules": [{"cron": "*/5 * * * *", "label": "每五分钟"}],
            "timezone": "America/New_York",
            "client_request_id": f"routine-{suffix}",
        },
    )["routine"]
    assert routine["timezone"] == "America/New_York" and routine["next_run_at"]
    test_run = rpc(base, password, "routine.test_run", {"routine_id": routine["id"]})["run"]
    assert test_run["routine_id"] == routine["id"]
    assert rpc(base, password, "routine.runs", {"routine_id": routine["id"]})["runs"]
    disabled = rpc(base, password, "routine.set_enabled", {"routine_id": routine["id"], "enabled": False})["routine"]
    assert disabled["enabled"] is False

    # A Bot DM is a real private execution route, selected by dm_chat_id.
    dm = rpc(base, password, "chat.send", {"chat_id": product["dm_chat_id"], "text": "DM smoke", "mentions": [], "client_request_id": f"dm-{suffix}"})
    assert dm["message"]["chat_id"] == product["dm_chat_id"]

    project = rpc(
        base,
        password,
        "project.create",
        {
            "name": f"登录功能-{suffix}",
            "goal": "给 App 加邮箱登录，周六前上线",
            "member_bot_ids": [product["id"], coder["id"], tester["id"]],
            "flow": ["产品", "编码", "测试"],
            "client_request_id": f"project-{suffix}",
        },
    )
    project_id = project["project"]["id"]
    project_chat = project["chat"]["id"]

    def settle_model_cards() -> None:
        pending = rpc(base, password, "bootstrap")["pending"]
        for approval in pending["approvals"]:
            if approval["state"] == "pending":
                rpc(base, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once"})
        for question in pending["questions"]:
            if question["state"] == "pending":
                rpc(base, password, "question.answer", {"question_id": question["id"], "option_index": 0})

    # This is the acceptance path for model autonomy. The fake provider emits
    # real OpenAI function-call deltas, while all side effects go through the
    # production runtime and CoordinationTools bridges.
    model_start = rpc(
        base,
        password,
        "chat.send",
        # Run the coordinator in the project group so the production tool
        # allowlist includes send_msg and its Bot mentions can be dispatched.
        {"chat_id": project_chat, "text": model_marker, "mentions": [{"kind": "main"}], "client_request_id": f"model-main-{suffix}"},
    )
    assert model_start["message"]["chat_id"] == project_chat

    def model_review_ready() -> bool:
        settle_model_cards()
        projects = rpc(base, password, "project.list")["projects"]
        candidates = [
            item for item in projects if item["name"] == FakeProviderHandler.scenario["project_name"]
        ]
        # Repeated durable chat dispatches can expose the same model project
        # more than once while approvals are replayed; prefer the live review
        # instance so subsequent handoff assertions use its project id.
        created = next((item for item in candidates if item.get("status") == "review"), None)
        if created is None:
            created = next((item for item in candidates if item.get("status") == "done"), None)
        if created is None:
            return False
        FakeProviderHandler.scenario["project_id"] = created["id"]
        traces = rpc(base, password, "trace.history", {"chat_id": project_chat, "limit": 500})["items"]
        tool_names = {
            item.get("data", {}).get("name")
            for item in traces
            if item.get("type") == "tool.start"
        }
        # request_review is the durable review boundary.  A concurrent
        # scheduler pass may already have advanced the card to done by the
        # time this polling call reads the snapshot, so accept both terminal
        # observations while still requiring the actual tool trace.
        return created["status"] in {"review", "done"} and {
            "create_project", "assign", "delegate", "send_msg", "propose_bot", "request_review"
        }.issubset(tool_names)

    wait_until(model_review_ready, "model-driven project review", 45)
    model_finish = rpc(
        base,
        password,
        "chat.send",
        {"chat_id": project_chat, "text": FakeProviderHandler.scenario["finish_marker"], "mentions": [{"kind": "main"}], "client_request_id": f"model-finish-{suffix}"},
    )
    assert model_finish["message"]["chat_id"] == project_chat

    def model_project_done() -> bool:
        settle_model_cards()
        return rpc(base, password, "project.get", {"project_id": FakeProviderHandler.scenario["project_id"]})["project"]["status"] == "done"

    wait_until(model_project_done, "model-driven project confirmation", 45)

    # A worker model can start a bounded subagent. The continuation is
    # approval-gated, then emits child run trace entries under the same scope.
    subagent_start = rpc(
        base,
        password,
        "chat.send",
        {
            "chat_id": coder["dm_chat_id"],
            "text": FakeProviderHandler.scenario["subagent_marker"],
            "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": "run nested-trace child"}],
            "client_request_id": f"model-subagent-{suffix}",
        },
    )
    assert subagent_start["message"]["chat_id"] == coder["dm_chat_id"]

    def subagent_trace_ready() -> bool:
        settle_model_cards()
        # Subagent trace items are scoped to the durable assignment stream,
        # while the parent model run is scoped to the Bot DM chat stream.
        assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
        for assignment in assignments:
            if assignment.get("bot_id") != coder["id"] or "nested-trace" not in assignment.get("instruction", ""):
                continue
            traces = rpc(base, password, "trace.history", {"assignment_id": assignment["id"], "limit": 500})["items"]
            if any(str(item.get("run_id", "")).startswith("subagent_") for item in traces):
                return True
        return False

    wait_until(subagent_trace_ready, "nested subagent trace", 45)

    # Notifications are emitted by a real model tool call and routed through
    # the same durable send_msg path as handoffs.
    notify_start = rpc(
        base,
        password,
        "chat.send",
        {
            "chat_id": "chat_main",
            "text": FakeProviderHandler.scenario["notify_marker"],
            "mentions": [{"kind": "main"}],
            "client_request_id": f"model-notify-{suffix}",
        },
    )
    assert notify_start["message"]["chat_id"] == "chat_main"

    def notify_trace_ready() -> bool:
        traces = rpc(base, password, "trace.history", {"chat_id": "chat_main", "limit": 500})["items"]
        return any(
            item.get("type") == "tool.start" and item.get("data", {}).get("name") == "notify_user"
            for item in traces
        )

    wait_until(notify_trace_ready, "model notification tool", 45)

    # The generic model question follows the same durable path as takeover:
    # execution emits a question card, the sink registers it in pending
    # questions, and question.answer resumes the original run id.
    question_start = rpc(
        base,
        password,
        "chat.send",
        {
            "chat_id": coder["dm_chat_id"],
            "text": FakeProviderHandler.scenario["question_marker"],
            "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": "询问登录环境"}],
            "client_request_id": f"model-question-{suffix}",
        },
    )
    assert question_start["message"]["chat_id"] == coder["dm_chat_id"]

    model_question_target: dict[str, Any] = {}

    def question_waiting() -> bool:
        assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
        pending_questions = rpc(base, password, "bootstrap")["pending"]["questions"]
        for target in assignments:
            if target.get("bot_id") != coder["id"] or target.get("instruction") != "询问登录环境":
                continue
            traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
            if any(
                item.get("type") == "run.wait"
                and item.get("data", {}).get("reason") == "decision"
                for item in traces
            ):
                question = next(
                    (
                        item
                        for item in pending_questions
                        if item.get("state") == "pending"
                        and item.get("assignment_id") == target["id"]
                        and "登录环境" in item.get("text", "")
                    ),
                    None,
                )
                if question is not None:
                    model_question_target.update(assignment=target, question=question)
                    return True
        return False

    wait_until(question_waiting, "model question waiting card", 45)
    answered_model_question = rpc(
        base,
        password,
        "question.answer",
        {"question_id": model_question_target["question"]["id"], "text": "生产环境"},
    )["question"]
    assert answered_model_question["state"] == "answered"

    # The answer is routed back to the original run asynchronously.  Observe
    # that resume before proceeding; completion is intentionally left to the
    # runtime because a model may ask a follow-up question.
    def model_question_resumed() -> bool:
        target = model_question_target["assignment"]
        traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
        return any(
            item.get("type") == "run.resume"
            for item in traces
        )

    wait_until(model_question_resumed, "model question continuation", 45)

    if args.browser_bin:
        # Full model-driven takeover: the tool call suspends the same run,
        # creates a pending private question, and only the user start/release
        # RPCs change the driver and resume that checkpoint.
        takeover_start = rpc(
            base,
            password,
            "chat.send",
            {
                "chat_id": project_chat,
                "text": FakeProviderHandler.scenario["takeover_marker"],
                "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": "请求用户完成登录接管"}],
                "client_request_id": f"model-takeover-{suffix}",
            },
        )
        assert takeover_start["message"]["chat_id"] == project_chat

        def takeover_waiting() -> bool:
            assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
            target = next(
                (
                    item
                    for item in assignments
                    if item.get("bot_id") == coder["id"]
                    and item.get("instruction") == "请求用户完成登录接管"
                ),
                None,
            )
            if target is None:
                return False
            traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
            pending = rpc(base, password, "bootstrap")["pending"]["questions"]
            return (
                any(
                    item.get("type") == "run.wait"
                    and item.get("data", {}).get("reason") == "takeover"
                    for item in traces
                )
                and any(
                    item.get("state") == "pending"
                    and "接管浏览器" in item.get("text", "")
                    for item in pending
                )
            )

        wait_until(takeover_waiting, "model takeover waiting card", 45)
        # The public protocol deliberately returns an empty authority object;
        # the pending card and subsequent run trace are the observable state.
        assert rpc(base, password, "takeover.start", {"bot_id": coder["id"]}) == {}
        released = rpc(
            base,
            password,
            "takeover.release",
            {"bot_id": coder["id"], "note": "用户已完成登录"},
        )
        assert released == {}

        def takeover_resumed() -> bool:
            assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
            target = next(
                (
                    item
                    for item in assignments
                    if item.get("bot_id") == coder["id"]
                    and item.get("instruction") == "请求用户完成登录接管"
                ),
                None,
            )
            if target is None:
                return False
            traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
            runs = {
                item.get("run_id")
                for item in traces
                if item.get("type") == "tool.start"
                and item.get("data", {}).get("name") == "request_takeover"
            }
            return any(
                item.get("type") == "tool.end"
                and item.get("data", {}).get("is_error") is False
                and item.get("run_id") in runs
                for item in traces
            ) and any(item.get("type") == "run.end" and item.get("run_id") in runs for item in traces)

        wait_until(takeover_resumed, "model takeover continuation", 45)

    def exercise_waiting_message(marker: str, instruction: str, reason: str, reply: str) -> dict[str, Any]:
        start = rpc(
            base,
            password,
            "chat.send",
            {
                "chat_id": project_chat,
                "text": marker,
                "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": instruction}],
                "client_request_id": f"waiting-{reason}-{suffix}",
            },
        )
        assert start["message"]["chat_id"] == project_chat

        def waiting_target() -> tuple[dict[str, Any], dict[str, Any]] | None:
            assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
            target = next(
                (
                    item
                    for item in assignments
                    if item.get("bot_id") == coder["id"] and item.get("instruction") == instruction
                ),
                None,
            )
            if target is None:
                return None
            traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
            wait = next(
                (
                    item
                    for item in traces
                    if item.get("type") == "run.wait"
                    and item.get("data", {}).get("reason") == reason
                ),
                None,
            )
            if wait is None:
                return None
            return target, wait

        holder: list[tuple[dict[str, Any], dict[str, Any]]] = []

        def capture_waiting() -> bool:
            value = waiting_target()
            if value is None:
                return False
            holder[:] = [value]
            return True

        wait_until(capture_waiting, f"model {reason} waiting message", 45)
        target, wait = holder[-1]
        assert target and wait
        run_id = wait["run_id"]
        trace_at_wait = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
        llm_requests_at_wait = sum(
            1 for item in trace_at_wait if item.get("type") == "llm.request" and item.get("run_id") == run_id
        )
        time.sleep(1.0)
        trace_while_waiting = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
        llm_requests_while_waiting = sum(
            1 for item in trace_while_waiting if item.get("type") == "llm.request" and item.get("run_id") == run_id
        )
        assert llm_requests_while_waiting == llm_requests_at_wait, (
            reason,
            llm_requests_at_wait,
            llm_requests_while_waiting,
        )

        rpc(
            base,
            password,
            "chat.send",
            {
                "chat_id": project_chat,
                "assignment_id": target["id"],
                "text": reply,
                "mentions": [],
                "client_request_id": f"reply-{reason}-{suffix}",
            },
        )

        def resumed_done() -> bool:
            traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
            return any(item.get("type") == "run.resume" and item.get("run_id") == run_id for item in traces) and any(
                item.get("type") == "run.end"
                and item.get("run_id") == run_id
                and item.get("data", {}).get("status") == "done"
                for item in traces
            )

        wait_until(resumed_done, f"model {reason} waiting message resume", 45)
        traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
        send_calls = [
            item
            for item in traces
            if item.get("type") == "tool.start" and item.get("data", {}).get("name") == "send_msg"
        ]
        assert len(send_calls) == 2, send_calls
        return {
            "run_id": run_id,
            "assignment_id": target["id"],
            "provider_calls_at_wait": FakeProviderHandler.calls,
            "llm_requests_at_wait": llm_requests_at_wait,
        }

    decision_wait = exercise_waiting_message(
        FakeProviderHandler.scenario["decision_marker"],
        "等待用户确认部署",
        "decision",
        "用户确认继续",
    )
    blocked_wait = exercise_waiting_message(
        FakeProviderHandler.scenario["blocked_marker"],
        "等待生产凭据",
        "blocked",
        "用户已处理阻塞",
    )

    # The first hop is routed from a user message through the production
    # runtime. The fake provider sleeps so both independent groups can overlap.
    first = rpc(
        base,
        password,
        "chat.send",
        {
            "chat_id": project_chat,
            "text": "parallel-collaboration 产品先写邮箱登录 PRD",
            "mentions": [{"kind": "bot", "bot_id": product["id"], "instruction": "parallel-collaboration 写 PRD"}],
            "client_request_id": f"project-first-{suffix}",
        },
    )
    assert first["message"]["chat_id"] == project_chat

    parallel_projects = []
    parallel_assignments = []
    for index in (1, 2):
        created = rpc(
            base,
            password,
            "project.create",
            {"name": f"并行群-{suffix}-{index}", "goal": "并行执行", "member_bot_ids": [coder["id"]], "client_request_id": f"parallel-project-{suffix}-{index}"},
        )
        parallel_projects.append(created)
        message = rpc(
            base,
            password,
            "chat.send",
            {
                "chat_id": created["chat"]["id"],
                "text": f"parallel-collaboration task {index}",
                "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": f"parallel-collaboration task {index}"}],
                "client_request_id": f"parallel-chat-{suffix}-{index}",
            },
        )
        parallel_assignments.append(message["message"]["id"])

    def current_assignments() -> list[dict[str, Any]]:
        return rpc(base, password, "assignment.list", {"limit": 100})["items"]

    def both_parallel() -> bool:
        items = current_assignments()
        matched = [item for item in items if item.get("trigger_message_id") in parallel_assignments]
        return len(matched) == 2 and all(item["status"] in {"working", "done"} for item in matched)

    wait_until(both_parallel, "two independent project assignments scheduled", 20)
    wait_until(lambda: all(item["status"] == "done" for item in current_assignments() if item.get("trigger_message_id") in parallel_assignments), "parallel assignments complete", 30)

    # The model's own assign/delegate/send_msg chain above is the handoff
    # source.  The gateway automatically dispatches those assignments; do not
    # inject a second client-side send_msg chain here.
    handoff_assignment: list[dict[str, Any]] = []

    def model_handoff_assignments() -> bool:
        items = current_assignments()
        projects = rpc(base, password, "project.list")["projects"]
        candidates = [
            item for item in projects if item["name"] == FakeProviderHandler.scenario["project_name"]
        ]
        for candidate in candidates:
            target_project_id = candidate.get("id")
            coder_match = [
                item
                for item in items
                if item.get("project_id") in {target_project_id, None}
                and item.get("origin_chat_id") == project_chat
                and item.get("bot_id") == coder["id"]
                and item.get("instruction") == "实现模型驱动协作 smoke"
            ]
            tester_match = [
                item
                for item in items
                if item.get("project_id") in {target_project_id, None}
                and item.get("origin_chat_id") == project_chat
                and item.get("bot_id") == tester["id"]
                and item.get("instruction") in {"接手测试", "重复交接不得创建第二个任务"}
            ]
            if coder_match and tester_match and all(
                item.get("status") in {"working", "done"} for item in coder_match + tester_match
            ):
                # The daemon may replay the same group trigger while an
                # approval continuation is being restored; select one durable
                # pair, while the duplicate mention itself is deduped by Bot.
                handoff_assignment[:] = [coder_match[-1]]
                return True
        return False

    wait_until(model_handoff_assignments, "model-driven handoff assignments", 45)
    assignments = current_assignments()
    coder_assignment = handoff_assignment[-1]

    # Approval/question cards above are emitted by real model tool calls and
    # settled through bootstrap, question.answer, and approval.decide.  Do not
    # call internal request helpers here: they are not public protocol RPCs.
    announcement = rpc(base, password, "project.get", {"project_id": FakeProviderHandler.scenario["project_id"]})["announcement"]
    assert announcement["project_id"] == FakeProviderHandler.scenario["project_id"]
    print(json.dumps({"ok": True, "project_id": FakeProviderHandler.scenario["project_id"], "parallel_assignments": parallel_assignments, "decision_wait": decision_wait, "blocked_wait": blocked_wait, "provider_calls": FakeProviderHandler.calls}, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7797")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command")
    parser.add_argument("--home", type=Path)
    parser.add_argument("--browser-bin", default=os.environ.get("MACBOT_BROWSER_BIN"))
    args = parser.parse_args()
    provider, provider_url = start_provider()
    daemon = Daemon(args)
    try:
        daemon.start()
        acceptance(args, provider_url)
    finally:
        daemon.close()
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
