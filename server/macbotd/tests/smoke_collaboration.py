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
    bodies: list[dict[str, Any]] = []
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

    @staticmethod
    def _tool_project_chat_id(messages: list[dict[str, Any]]) -> str | None:
        for message in reversed(messages):
            if message.get("role") != "tool":
                continue
            try:
                value = json.loads(message.get("content", ""))
            except (TypeError, json.JSONDecodeError):
                continue
            chat = value.get("chat") if isinstance(value, dict) else None
            if isinstance(chat, dict) and isinstance(chat.get("id"), str):
                return chat["id"]
            project = value.get("project") if isinstance(value, dict) else None
            if isinstance(project, dict) and isinstance(project.get("chat_id"), str):
                return project["chat_id"]
        return None

    def _scripted_tool(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        scenario = type(self).scenario
        prompt = json.dumps(messages, ensure_ascii=False)
        called = self._called_tools(messages)
        send_msg_count = sum(
            1
            for message in messages
            for call in message.get("tool_calls", [])
            if call.get("function", {}).get("name") == "send_msg"
        )
        latest_user = next(
            (
                message.get("content", "")
                for message in reversed(messages)
                if message.get("role") == "user"
            ),
            "",
        )
        user_messages = [
            message.get("content", "")
            for message in messages
            if message.get("role") == "user"
        ]

        scenario_markers = (
            ("takeover", scenario.get("takeover_marker", "")),
            ("question", scenario.get("question_marker", "")),
            ("notify", scenario.get("notify_marker", "")),
            ("decision", scenario.get("decision_marker", "")),
            ("bot_decision", scenario.get("bot_decision_marker", "")),
            ("blocked", scenario.get("blocked_marker", "")),
        )

        def latest_scenario_marker() -> str | None:
            # A project assignment appends its instruction after the user's
            # marker.  Choose the newest marker in the bounded user window,
            # rather than independently matching every old marker: a later
            # question must not be stolen by an earlier takeover marker.
            candidates = [
                (index, name)
                for index, content in list(enumerate(user_messages))[-3:]
                for name, marker in scenario_markers
                if marker and marker in content
            ]
            return max(candidates, default=(-1, None))[1]
        # The main coordination script is only valid for the main Bot.  Child
        # Bots receive the original marker in their worklog, so matching the
        # marker alone would make every worker try to create another project.
        is_main_bot = any(
            message.get("role") == "system" and "\n总管\n" in message.get("content", "")
            for message in messages
        )
        # A coordinator following a blocked worker has a different task from
        # the worker itself. Do not replay that worker's earlier marker from
        # group history and manufacture another blocked coordinator.
        main_attention = is_main_bot and any(
            "跟进阻塞任务" in message.get("content", "")
            or "主 Bot 请跟进" in message.get("content", "")
            or "主 Bot 跟进任务：" in message.get("content", "")
            for message in [m for m in messages if m.get("role") == "user"][-3:]
        )
        if main_attention:
            if "send_msg" not in called:
                return "send_msg", {"intent":"done", "text":"主 Bot 已记录阻塞并跟进", "mentions":[]}
            return None
        project_id = self._tool_project_id(messages) or scenario.get("project_id")
        if project_id:
            scenario["project_id"] = project_id
        project_chat_id = self._tool_project_chat_id(messages) or scenario.get("project_chat_id")
        if project_chat_id:
            scenario["project_chat_id"] = project_chat_id
        decision_marker = scenario.get("decision_marker", "")
        bot_decision_marker = scenario.get("bot_decision_marker", "")
        blocked_marker = scenario.get("blocked_marker", "")
        active_marker = latest_scenario_marker()
        decision_child_instruction = scenario.get("decision_child_instruction", "")
        decision_child_active = bool(decision_child_instruction) and decision_child_instruction in latest_user
        if decision_child_active and "send_msg" not in called:
            return "send_msg", {
                "intent": "done",
                "text": "决策子任务已快速完成",
                "mentions": [],
                "artifacts": [{
                    "title": "决策子任务回执",
                    "path_or_url": scenario.get("decision_child_artifact_path", ""),
                }],
            }
        bot_decision_active = (active_marker == "bot_decision" and (
            latest_user.strip() == bot_decision_marker or "等待子任务自动恢复" in latest_user
        )) or (
            latest_user.strip() == "决策子任务已快速完成" and bot_decision_marker in prompt
        )
        if bot_decision_active:
            if "send_msg" not in called:
                return "send_msg", {
                    "intent": "decision",
                    "text": "等待决策子任务完成",
                    "mentions": [{
                        "kind": "bot",
                        "bot_id": scenario["decision_child_bot_id"],
                        "instruction": decision_child_instruction,
                    }],
                }
            if "决策子任务已快速完成" in prompt:
                return "send_msg", {
                    "intent": "done",
                    "text": "子任务完成，父任务自动恢复",
                    "mentions": [],
                }
            return None
        decision_active = (active_marker == "decision" and (
            latest_user.strip() == decision_marker or "等待用户确认部署" in latest_user
        )) or (
            latest_user.strip() == "继续" and decision_marker in prompt
        )
        blocked_active = (active_marker == "blocked" and (
            latest_user.strip() == blocked_marker or "等待生产凭据" in latest_user
        )) or (
            "用户已处理阻塞" in latest_user and blocked_marker in prompt
        )
        # Project chat history contains both waiting scenarios.  Use the
        # marker from the latest user turn so an earlier decision does not
        # steal the blocked-message continuation.
        if decision_active and not blocked_active:
            if "send_msg" not in called:
                return "send_msg", {
                    "intent": "decision",
                    "text": "需要用户确认是否继续部署",
                    "options": ["继续", "停止"],
                    "mentions": ["user"],
                }
            if latest_user.strip() == "继续":
                return "send_msg", {
                    "intent": "done",
                    "text": "用户已确认，决策任务完成",
                    "mentions": [],
                }
            return None
        if blocked_active:
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
        takeover_active = active_marker == "takeover"
        if takeover_active and "request_takeover" in called:
            return None
        if takeover_active and "request_takeover" not in called:
            return "request_takeover", {"reason": "smoke 模型需要用户登录"}
        question_active = active_marker == "question"
        if question_active and "question" not in called:
            return "question", {"question": "smoke 请选择登录环境"}
        if question_active and "question" in called:
            return None
        notify_active = active_marker == "notify"
        if notify_active and "notify_user" not in called:
            return "notify_user", {
                "text": "巡检完成，已通知用户",
                "intent": "progress",
            }
        if not is_main_bot:
            # The model-created project is intentionally exercised as a real
            # three-step handoff.  Product and coder each acknowledge receipt
            # before sending a durable done report to the next Bot; Tester
            # sends the final report back to Main.  These calls must come from
            # the provider's tool stream, so the smoke never creates the
            # chain with client-side assignment RPCs.
            chain_instruction = next(
                (
                    instruction
                    for instruction in (
                        scenario.get("product_instruction", ""),
                        scenario.get("coder_instruction", ""),
                        scenario.get("tester_instruction", ""),
                    )
                    if instruction and instruction in latest_user
                ),
                None,
            )
            if chain_instruction is not None:
                if chain_instruction == scenario["product_instruction"]:
                    next_bot_id = scenario["coder_id"]
                    next_instruction = scenario["coder_instruction"]
                    artifact_path = scenario["product_artifact_path"]
                    done_text = "产品阶段已完成，交接编码"
                elif chain_instruction == scenario["coder_instruction"]:
                    next_bot_id = scenario["tester_id"]
                    next_instruction = scenario["tester_instruction"]
                    artifact_path = scenario["coder_artifact_path"]
                    done_text = "编码阶段已完成，交接测试"
                else:
                    next_bot_id = "main"
                    next_instruction = "模型协作最终报告"
                    artifact_path = scenario["tester_artifact_path"]
                    done_text = "测试阶段已完成，报告已写入，已回报主 Bot"
                if send_msg_count == 0:
                    return "send_msg", {
                        "intent": "progress",
                        "text": f"已接收{chain_instruction}",
                        "chat_id": scenario["project_chat_id"],
                        "mentions": [],
                    }
                if send_msg_count == 1:
                    mentions: list[Any]
                    if next_bot_id == "main":
                        mentions = ["main"]
                    else:
                        mentions = [{
                            "kind": "bot",
                            "bot_id": next_bot_id,
                            "instruction": next_instruction,
                        }]
                    return "send_msg", {
                        "intent": "done",
                        "text": done_text,
                        "chat_id": scenario["project_chat_id"],
                        "mentions": mentions,
                        "artifacts": [{
                            "title": f"{chain_instruction}报告",
                            "path_or_url": artifact_path,
                        }],
                    }
                return None
            worker_instruction = next(
                (
                    instruction
                    for instruction in (
                        "实现模型驱动协作 smoke",
                        "验证模型驱动协作 smoke",
                        "接手测试",
                        "重复交接不得创建第二个任务",
                        "读取私信并继续测试",
                    )
                    if instruction in latest_user
                ),
                None,
            )
            if worker_instruction is not None and "send_msg" not in called:
                return "send_msg", {
                    "intent": "done",
                    "text": f"{worker_instruction}已完成，辅助回执",
                    # The main-DM delegate reports to Main.  The auxiliary
                    # project handoff cards are deliberately self-contained;
                    # they must not wake Main and bypass the product chain.
                    "mentions": ["main"] if worker_instruction == "验证模型驱动协作 smoke" else [],
                    "artifacts": [
                        {
                            "title": "登录验收报告",
                            "path_or_url": scenario.get("artifact_path", scenario.get("coder_artifact_path", "")),
                        }
                    ],
                }
        # A review notification may legitimately start a new main-Bot run.
        # In that run the latest user turn is the finish marker; do not replay
        # the original setup marker from the older chat history.
        if is_main_bot and scenario.get("finish_marker") and scenario["finish_marker"] in latest_user and "finish_project" not in called:
            return "finish_project", {
                "project_id": scenario["project_id"],
                "summary": "用户确认后完成项目",
            }
        if is_main_bot and (
            scenario.get("worker_done_marker") in latest_user
            or scenario.get("changes_marker") in latest_user
        ) and "request_review" not in called:
            return "request_review", {
                "project_id": scenario["project_id"],
                "summary": "主 Bot 已汇总最新产物，请用户验收",
            }
        if is_main_bot and scenario.get("main_marker") and scenario["main_marker"] in latest_user:
            if "create_project" not in called:
                return "create_project", {
                    "name": scenario["project_name"],
                    "goal": "模型驱动协作 smoke",
                    "member_bot_ids": [scenario["product_id"], scenario["coder_id"], scenario["tester_id"]],
                    "flow": ["产品", "编码", "测试"],
                }
            if "send_msg" not in called:
                return "send_msg", {
                    "intent": "progress",
                    "text": "目标：模型驱动协作 smoke；流程：产品 → 编码 → 测试。请产品 Bot 开始分析。",
                    "chat_id": scenario["project_chat_id"],
                    "mentions": [{
                        "kind": "bot",
                        "bot_id": scenario["product_id"],
                        "instruction": scenario["product_instruction"],
                    }],
                }
            if "delegate" not in called:
                return "delegate", {
                    "bot_id": scenario["tester_id"],
                    "title": "模型委派测试",
                    "instruction": "验证模型驱动协作 smoke",
                }
            if "send_msg" not in called:
                return "send_msg", {
                    # Keep the initial coordinator run alive for propose_bot.
                    # Review is requested only by the fresh Main run woken by
                    # a worker's done@main handoff.
                    "intent": "progress",
                    "text": "编码完成，请测试并回报主 Bot",
                    "chat_id": scenario["project_chat_id"],
                    "to": {"bot": scenario["tester_id"]},
                    # Duplicate mentions are deliberate: this is the model's
                    # real handoff and verifies the orchestrator deduplicates
                    # one child assignment before the gateway dispatches it.
                    "mentions": [
                        {"kind": "bot", "bot_id": scenario["tester_id"], "instruction": "接手测试"},
                        {"kind": "bot", "bot_id": scenario["tester_id"], "instruction": "重复交接不得创建第二个任务"},
                    ],
                }
            if send_msg_count < 2:
                return "send_msg", {
                    "intent": "progress",
                    "text": "已私信测试 Bot 继续核验",
                    "chat_id": scenario["project_chat_id"],
                    "to": {"bot": scenario["tester_id"]},
                    "mentions": [
                        {
                            "kind": "bot",
                            "bot_id": scenario["tester_id"],
                            "instruction": "读取私信并继续测试",
                        }
                    ],
                }
            if "propose_bot" not in called:
                return "propose_bot", {
                    "name": f"模型提议-{scenario['suffix']}",
                    "label": "smoke",
                    "description": "模型通过工具提出的 Bot",
                }
            # The initial coordinator run stops after handing work to the
            # workers.  The worker's done@main message starts a fresh main
            # run, which is the only run allowed to request review.
        if scenario.get("subagent_marker") and scenario["subagent_marker"] in latest_user and "subagent" not in called:
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
            type(self).bodies.append({
                "model": body.get("model"),
                "messages": body.get("messages", []),
            })
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


def write_provider_bodies(home: Path | None) -> Path | None:
    if home is None:
        return None
    path = home.parent / f"{home.name}.provider-bodies.json"
    path.write_text(
        json.dumps(
            {"requests": FakeProviderHandler.bodies},
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return path


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


def assert_ack(message_result: dict[str, Any], chat_id: str) -> None:
    message = message_result["message"]
    assert message["chat_id"] == chat_id
    assert message.get("intent") == "ack", message


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
    main_chat_id = rpc(base, password, "bot.get", {"bot_id": "main"})["bot"]["dm_chat_id"]

    product = rpc(base, password, "bot.create", {"name": f"产品-{suffix}", "model": model, "max_parallel": 2})["bot"]
    worker_tools = {"files": False, "bash": False, "browser": True, "subagent": True, "web": False, "mcp": False}
    coder = rpc(base, password, "bot.create", {"name": f"编码-{suffix}", "model": model, "max_parallel": 2, "tools": worker_tools})["bot"]
    tester = rpc(base, password, "bot.create", {"name": f"测试-{suffix}", "model": model, "max_parallel": 2})["bot"]

    model_marker = f"model-tool-collaboration-{suffix}"
    FakeProviderHandler.scenario = {
        "main_marker": model_marker,
        "finish_marker": f"model-tool-finish-{suffix}",
        "worker_done_marker": "报告已写入",
        "changes_marker": f"model-request-changes-{suffix}",
        "subagent_marker": f"model-subagent-{suffix}",
        "notify_marker": f"model-notify-{suffix}",
        "takeover_marker": f"model-takeover-{suffix}",
        "question_marker": f"model-question-{suffix}",
        "decision_marker": f"model-decision-{suffix}",
        "bot_decision_marker": f"model-bot-decision-{suffix}",
        "blocked_marker": f"model-blocked-{suffix}",
        "decision_child_instruction": f"决策子任务-{suffix}",
        "decision_child_artifact_path": f"/tmp/macbot-decision-child-{suffix}.md",
        "project_name": f"模型协作-{suffix}",
        "product_id": product["id"],
        "coder_id": coder["id"],
        "tester_id": tester["id"],
        "product_instruction": "产品阶段模型驱动协作 smoke",
        "coder_instruction": "编码阶段模型驱动协作 smoke",
        "tester_instruction": "测试阶段模型驱动协作 smoke",
        "decision_child_bot_id": tester["id"],
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
    assert_ack(dm, product["dm_chat_id"])

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
    artifact_path = f"artifacts/{suffix}/login-smoke.md"
    product_artifact_path = f"artifacts/{suffix}/product-plan.md"
    coder_artifact_path = f"artifacts/{suffix}/implementation.md"
    tester_artifact_path = f"artifacts/{suffix}/test-report.md"
    artifact_file = (args.home or Path("/tmp/macbot-collaboration-smoke")) / artifact_path
    artifact_file.parent.mkdir(parents=True, exist_ok=True)
    for relative_path, title in (
        (artifact_path, "# Login smoke report"),
        (product_artifact_path, "# Product plan"),
        (coder_artifact_path, "# Implementation report"),
        (tester_artifact_path, "# Test report"),
    ):
        target = (args.home or Path("/tmp/macbot-collaboration-smoke")) / relative_path
        target.write_text(
            f"{title}\n\nProduction runtime fake-provider acceptance passed.\n",
            encoding="utf-8",
        )
    FakeProviderHandler.scenario.update({
        # Keep the legacy key pointed at the final Tester report so the
        # completion/memory assertions always refer to a chain-produced
        # artifact rather than the unrelated fixture file.
        "artifact_path": tester_artifact_path,
        "product_artifact_path": product_artifact_path,
        "coder_artifact_path": coder_artifact_path,
        "tester_artifact_path": tester_artifact_path,
    })
    assert project["project"]["status"] == "active", project
    assert project["chat"]["kind"] == "project", project

    def project_card_in_history() -> bool:
        history = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": main_chat_id, "after_seq": 0, "limit": 100},
        )
        return any(
            block.get("type") == "project_card"
            and block.get("project_id") == project_id
            for message in history.get("messages", [])
            for block in message.get("blocks", [])
        )

    wait_until(project_card_in_history, "project card in main Bot DM history", 15)
    initial_project = rpc(base, password, "project.get", {"project_id": project_id})
    initial_announcement = initial_project["announcement"]
    assert initial_announcement["project_id"] == project_id
    assert {member["bot_id"] for member in initial_announcement["members"]} == {
        "main", product["id"], coder["id"], tester["id"]
    }, initial_announcement

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
        # Main coordination starts in the real Main DM.  The model-created
        # project group is selected explicitly in the later handoff message.
        {"chat_id": main_chat_id, "text": model_marker, "mentions": [{"kind": "main"}], "client_request_id": f"model-main-{suffix}"},
    )
    assert_ack(model_start, main_chat_id)

    def model_review_ready() -> bool:
        settle_model_cards()
        projects = rpc(base, password, "project.list")["projects"]
        candidates = [
            item for item in projects if item["name"] == FakeProviderHandler.scenario["project_name"]
        ]
        # Repeated durable chat dispatches can expose the same model project
        # more than once while approvals are replayed; select only the live
        # review instance.  The user confirmation is a separate next step.
        created = next((item for item in candidates if item.get("status") == "review"), None)
        if created is None:
            return False
        FakeProviderHandler.scenario["project_id"] = created["id"]
        FakeProviderHandler.scenario["project_chat_id"] = created["chat_id"]
        main_assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
        return created["status"] == "review" and any(
            item.get("bot_id") == "main"
            and item.get("project_id") == created["id"]
            and item.get("origin_chat_id") == created["chat_id"]
            for item in main_assignments
        )

    wait_until(model_review_ready, "model-driven project review", 45)
    model_project_chat = FakeProviderHandler.scenario["project_chat_id"]
    opening_history = rpc(base, password, "chat.history", {"chat_id":model_project_chat, "after_seq":0, "limit":500})["messages"]
    openings = [message for message in opening_history if message.get("sender", {}).get("bot_id") == "main"
                and any(mention.get("bot_id") == product["id"] for mention in message.get("mentions", []))
                and "模型驱动协作 smoke" in message.get("fallback_text", "")
                and "产品 → 编码 → 测试" in message.get("fallback_text", "")]
    assert len(openings) == 1, openings
    assert all(block.get("type") != "task_card" for block in openings[0]["blocks"]), openings[0]

    def main_review_cards() -> list[dict[str, Any]]:
        history = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": main_chat_id, "after_seq": 0, "limit": 500},
        )
        return [
            block
            for message in history.get("messages", [])
            for block in message.get("blocks", [])
            if block.get("type") == "review_card"
            and block.get("project_id") == FakeProviderHandler.scenario["project_id"]
        ]

    pending_review_cards: list[dict[str, Any]] = []

    def review_card_pending() -> bool:
        nonlocal pending_review_cards
        pending_review_cards = [
            card
            for card in main_review_cards()
            if card.get("state") == "pending"
            and isinstance(card.get("artifacts"), list)
            and bool(card["artifacts"])
        ]
        return bool(pending_review_cards)

    wait_until(review_card_pending, "main DM pending review card with artifacts", 30)
    pending_artifact_ids = {
        artifact.get("artifact_id")
        for artifact in pending_review_cards[-1].get("artifacts", [])
        if artifact.get("artifact_id")
    }
    assert pending_artifact_ids, pending_review_cards[-1]

    # User-requested changes must happen while the project is in review.  The
    # normal lifecycle is review -> request_changes -> review -> finish_project;
    # calling request_changes after completion would exercise an invalid state
    # transition and would hide a regression in the model confirmation path.
    changes = rpc(
        base,
        password,
        "project.request_changes",
        {
            "project_id": FakeProviderHandler.scenario["project_id"],
            "text": FakeProviderHandler.scenario["changes_marker"],
        },
    )
    changes_message = changes["message"]
    assert changes_message["chat_id"] == model_project_chat

    def changes_review_ready() -> bool:
        project_value = rpc(
            base,
            password,
            "project.get",
            {"project_id": FakeProviderHandler.scenario["project_id"]},
        )["project"]
        if project_value.get("status") != "review":
            return False
        assignments = rpc(base, password, "assignment.list", {"limit": 100})["items"]
        main_assignment = next(
            (
                item
                for item in assignments
                if item.get("bot_id") == "main"
                and item.get("project_id") == FakeProviderHandler.scenario["project_id"]
                and item.get("trigger_message_id") == changes_message["id"]
            ),
            None,
        )
        if main_assignment is None:
            return False
        traces = rpc(
            base,
            password,
            "trace.history",
            {"assignment_id": main_assignment["id"], "limit": 500},
        )["items"]
        return any(
            item.get("type") == "tool.start"
            and item.get("data", {}).get("name") == "request_review"
            for item in traces
        )

    wait_until(changes_review_ready, "request changes to Main review", 45)
    wait_until(review_card_pending, "main DM review card after requested changes", 30)

    # The second review is the one the user confirms.  This fresh Main run is
    # deliberately driven through the same public chat path as the first one.
    model_finish = rpc(
        base,
        password,
        "chat.send",
        {"chat_id": model_project_chat, "text": FakeProviderHandler.scenario["finish_marker"], "mentions": [{"kind": "main"}], "client_request_id": f"model-finish-{suffix}"},
    )
    assert_ack(model_finish, model_project_chat)

    def model_project_done() -> bool:
        settle_model_cards()
        return rpc(base, password, "project.get", {"project_id": FakeProviderHandler.scenario["project_id"]})["project"]["status"] == "done"

    wait_until(model_project_done, "model-driven project confirmation", 45)
    finished_project = rpc(base, password, "project.get", {"project_id": FakeProviderHandler.scenario["project_id"]})
    assert finished_project["project"]["status"] == "done", finished_project
    assert finished_project["announcement"]["project_id"] == FakeProviderHandler.scenario["project_id"]

    def review_card_confirmed() -> bool:
        return any(
            card.get("state") == "confirmed"
            and pending_artifact_ids.issubset(
                {
                    artifact.get("artifact_id")
                    for artifact in card.get("artifacts", [])
                    if artifact.get("artifact_id")
                }
            )
            for card in main_review_cards()
        )

    wait_until(review_card_confirmed, "main DM confirmed review card", 30)

    canonical_completion: dict[str, Any] | None = None

    def canonical_completion_ready() -> bool:
        nonlocal canonical_completion
        history = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": model_project_chat, "after_seq": 0, "limit": 500},
        )
        expected_id = f"msg_project_completion_{FakeProviderHandler.scenario['project_id']}"
        for message in history.get("messages", []):
            if message.get("id") != expected_id:
                continue
            sender = message.get("sender")
            if not isinstance(sender, dict) or sender.get("kind") != "bot" or sender.get("bot_id") != "main":
                continue
            for block in message.get("blocks", []):
                if block.get("type") != "completion":
                    continue
                summary = block.get("summary")
                artifacts = block.get("artifacts")
                if not isinstance(summary, str) or not summary.strip() or not isinstance(artifacts, list):
                    continue
                if not any(
                    artifact.get("path_or_url") == FakeProviderHandler.scenario["artifact_path"]
                    for artifact in artifacts
                    if isinstance(artifact, dict)
                ):
                    continue
                canonical_completion = block
                return True
        return False

    wait_until(canonical_completion_ready, "canonical project completion card", 30)
    assert canonical_completion is not None

    # finish_project must use the shared FeatureService and leave both the
    # project summary and Main's worklog in the durable memory snapshot.  Read
    # the on-disk representation so the check covers the restart boundary too.
    assert args.home is not None, "strict collaboration smoke requires --home for durable checks"
    memory_path = args.home / "data/memory/state.json"
    assert memory_path.is_file(), memory_path
    memory_state = json.loads(memory_path.read_text(encoding="utf-8"))
    memory_entries = memory_state.get("entries")
    assert isinstance(memory_entries, list), memory_state
    memory_by_id = {
        entry.get("id"): entry
        for entry in memory_entries
        if isinstance(entry, dict) and isinstance(entry.get("id"), str)
    }
    project_summary = memory_by_id.get(
        f"project-summary:{FakeProviderHandler.scenario['project_id']}"
    )
    main_worklog = memory_by_id.get(
        f"project-summary-worklog:{FakeProviderHandler.scenario['project_id']}:main"
    )
    assert project_summary and main_worklog, memory_by_id.keys()
    assert "模型驱动协作 smoke" in project_summary.get("content", "")
    assert FakeProviderHandler.scenario["artifact_path"] in project_summary.get("content", "")
    assert FakeProviderHandler.scenario["artifact_path"] in main_worklog.get("content", "")

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
    assert_ack(subagent_start, coder["dm_chat_id"])

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
            "chat_id": main_chat_id,
            "text": FakeProviderHandler.scenario["notify_marker"],
            "mentions": [{"kind": "main"}],
            "client_request_id": f"model-notify-{suffix}",
        },
    )
    assert_ack(notify_start, main_chat_id)

    def notify_trace_ready() -> bool:
        traces = rpc(base, password, "trace.history", {"chat_id": main_chat_id, "limit": 500})["items"]
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
    assert_ack(question_start, coder["dm_chat_id"])

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
        assert_ack(takeover_start, project_chat)

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
        assert_ack(start, project_chat)

        decision_question: dict[str, Any] | None = None

        def find_decision_question(target: dict[str, Any]) -> dict[str, Any] | None:
            """Require a decision to be visible in the durable/UI surfaces."""
            if reason != "decision":
                return None
            bootstrap = rpc(base, password, "bootstrap")
            pending_questions = bootstrap.get("pending", {}).get("questions", [])
            pending = next(
                (
                    item
                    for item in pending_questions
                    if item.get("state") == "pending"
                    and item.get("assignment_id") == target["id"]
                    and item.get("options") == ["继续", "停止"]
                ),
                None,
            )
            if pending is None or not isinstance(pending.get("id"), str):
                return None
            question_id = pending["id"]
            history = rpc(
                base,
                password,
                "chat.history",
                {"chat_id": project_chat, "after_seq": 0, "limit": 500},
            )
            question_blocks = [
                block
                for message in history.get("messages", [])
                if message.get("assignment_id") == target["id"]
                for block in message.get("blocks", [])
                if block.get("type") == "question" and block.get("question_id") == question_id
            ]
            if len(question_blocks) != 1:
                return None
            workbench = rpc(base, password, "workbench.get", {})
            workbench = workbench.get("workbench", workbench)
            waiting = [
                item
                for item in workbench.get("waiting", [])
                if item.get("kind") == "question"
                and item.get("question", {}).get("id") == question_id
                and item.get("question", {}).get("assignment_id") == target["id"]
            ]
            if len(waiting) != 1:
                return None
            return pending

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
            if reason == "decision":
                question = find_decision_question(target)
                if question is None:
                    return None
                nonlocal decision_question
                decision_question = question
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

        if reason == "decision":
            assert decision_question is not None
            answered = rpc(
                base,
                password,
                "question.answer",
                {
                    "question_id": decision_question["id"],
                    "option_index": 0,
                    "client_request_id": f"answer-decision-{suffix}",
                },
            )
            answered_question = answered.get("question", answered)
            assert answered_question.get("id") == decision_question["id"], answered
            assert answered_question.get("state") == "answered", answered
        else:
            reply_result = rpc(
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
            assert_ack(reply_result, project_chat)

        def resumed_done() -> bool:
            traces = rpc(base, password, "trace.history", {"assignment_id": target["id"], "limit": 500})["items"]
            resumed = any(item.get("type") == "run.resume" and item.get("run_id") == run_id for item in traces)
            ended = any(
                item.get("type") == "run.end"
                and item.get("run_id") == run_id
                and item.get("data", {}).get("status") == "done"
                for item in traces
            )
            assignments = rpc(base, password, "assignment.list", {"limit": 500})["items"]
            assignment = next((item for item in assignments if item.get("id") == target["id"]), None)
            return resumed and ended and assignment is not None and assignment.get("status") == "done"

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
            "question_id": decision_question["id"] if decision_question is not None else None,
            "answer_method": "question.answer" if reason == "decision" else "chat.send",
            "provider_calls_at_wait": FakeProviderHandler.calls,
            "llm_requests_at_wait": llm_requests_at_wait,
        }

    def exercise_bot_decision(marker: str, instruction: str) -> dict[str, Any]:
        """Verify a no-options Bot decision wakes its parent automatically."""
        start = rpc(
            base,
            password,
            "chat.send",
            {
                "chat_id": project_chat,
                "text": marker,
                "mentions": [{"kind": "bot", "bot_id": coder["id"], "instruction": instruction}],
                "client_request_id": f"bot-decision-{suffix}",
            },
        )
        assert_ack(start, project_chat)
        parent_holder: list[tuple[dict[str, Any], dict[str, Any]]] = []

        def parent_waiting() -> bool:
            assignments = rpc(base, password, "assignment.list", {"limit": 500})["items"]
            parent = next(
                (
                    item
                    for item in assignments
                    if item.get("bot_id") == coder["id"] and item.get("instruction") == instruction
                ),
                None,
            )
            if parent is None:
                return False
            traces = rpc(base, password, "trace.history", {"assignment_id": parent["id"], "limit": 500})["items"]
            wait = next(
                (
                    item
                    for item in traces
                    if item.get("type") == "run.wait"
                    and item.get("data", {}).get("reason") == "decision"
                ),
                None,
            )
            if wait is None:
                return False
            pending = rpc(base, password, "bootstrap").get("pending", {}).get("questions", [])
            if any(item.get("assignment_id") == parent["id"] for item in pending):
                return False
            parent_holder[:] = [(parent, wait)]
            return True

        wait_until(parent_waiting, "Bot decision parent waiting without Question", 45)
        parent, wait = parent_holder[-1]
        run_id = wait["run_id"]
        child_holder: list[dict[str, Any]] = []

        def child_done() -> bool:
            assignments = rpc(base, password, "assignment.list", {"limit": 500})["items"]
            children = [
                item
                for item in assignments
                if item.get("parent_assignment_id") == parent["id"]
                and item.get("bot_id") == FakeProviderHandler.scenario["decision_child_bot_id"]
                and item.get("instruction") == FakeProviderHandler.scenario["decision_child_instruction"]
            ]
            if len(children) != 1 or children[0].get("status") != "done":
                return False
            child = children[0]
            traces = rpc(base, password, "trace.history", {"assignment_id": child["id"], "limit": 500})["items"]
            if not any(
                item.get("type") == "tool.start"
                and item.get("data", {}).get("name") == "send_msg"
                for item in traces
            ) or not any(
                item.get("type") == "run.end"
                and item.get("data", {}).get("status") == "done"
                for item in traces
            ):
                return False
            history = rpc(
                base,
                password,
                "chat.history",
                {"chat_id": project_chat, "after_seq": 0, "limit": 500},
            )
            if not any(
                message.get("assignment_id") == child["id"]
                and message.get("intent") == "done"
                for message in history.get("messages", [])
            ):
                return False
            child_holder[:] = [child]
            return True

        wait_until(child_done, "Bot decision child completion", 45)
        child = child_holder[-1]

        def parent_resumed_done() -> bool:
            traces = rpc(base, password, "trace.history", {"assignment_id": parent["id"], "limit": 500})["items"]
            resumed = any(item.get("type") == "run.resume" and item.get("run_id") == run_id for item in traces)
            ended = any(
                item.get("type") == "run.end"
                and item.get("run_id") == run_id
                and item.get("data", {}).get("status") == "done"
                for item in traces
            )
            assignments = rpc(base, password, "assignment.list", {"limit": 500})["items"]
            current_parent = next((item for item in assignments if item.get("id") == parent["id"]), None)
            return resumed and ended and current_parent is not None and current_parent.get("status") == "done"

        wait_until(parent_resumed_done, "Bot decision parent automatic resume", 45)
        traces = rpc(base, password, "trace.history", {"assignment_id": parent["id"], "limit": 500})["items"]
        pending = rpc(base, password, "bootstrap").get("pending", {}).get("questions", [])
        assert not any(item.get("assignment_id") == parent["id"] for item in pending), pending
        parent_send_calls = sum(
            1
            for item in traces
            if item.get("type") == "tool.start" and item.get("data", {}).get("name") == "send_msg"
        )
        assert parent_send_calls == 2, traces
        return {
            "run_id": run_id,
            "assignment_id": parent["id"],
            "child_assignment_id": child["id"],
            "answer_method": "automatic_child_resume",
            "parent_send_calls": parent_send_calls,
        }

    decision_wait = exercise_waiting_message(
        FakeProviderHandler.scenario["decision_marker"],
        "等待用户确认部署",
        "decision",
        "用户确认继续",
    )
    bot_decision_wait = exercise_bot_decision(
        FakeProviderHandler.scenario["bot_decision_marker"],
        "等待子任务自动恢复",
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
    assert_ack(first, project_chat)

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
        assert_ack(message, created["chat"]["id"])
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
    product_assignment: list[dict[str, Any]] = []
    handoff_assignment: list[dict[str, Any]] = []
    flow_tester_assignment: list[dict[str, Any]] = []
    delegate_assignment: list[dict[str, Any]] = []

    def model_handoff_assignments() -> bool:
        items = current_assignments()
        projects = rpc(base, password, "project.list")["projects"]
        candidates = [
            item for item in projects if item["name"] == FakeProviderHandler.scenario["project_name"]
        ]
        for candidate in candidates:
            target_project_id = candidate.get("id")
            product_match = [
                item
                for item in items
                if item.get("project_id") == target_project_id
                and item.get("origin_chat_id") == model_project_chat
                and item.get("bot_id") == product["id"]
                and item.get("instruction") == FakeProviderHandler.scenario["product_instruction"]
            ]
            coder_match = [
                item
                for item in items
                if item.get("project_id") == target_project_id
                and item.get("origin_chat_id") == model_project_chat
                and item.get("bot_id") == coder["id"]
                and item.get("instruction") == FakeProviderHandler.scenario["coder_instruction"]
            ]
            tester_match = [
                item
                for item in items
                if item.get("project_id") == target_project_id
                and item.get("origin_chat_id") == model_project_chat
                and item.get("bot_id") == tester["id"]
                and item.get("instruction") == FakeProviderHandler.scenario["tester_instruction"]
            ]
            delegate_match = [
                item
                for item in items
                # `delegate` intentionally omits project_id: it is a small
                # Main-DM task and the RPC router must force chat_main.
                if item.get("project_id") is None
                and item.get("origin_chat_id") == main_chat_id
                and item.get("bot_id") == tester["id"]
                and item.get("instruction") == "验证模型驱动协作 smoke"
            ]
            if (
                len(product_match) == 1
                and len(coder_match) == 1
                and len(tester_match) == 1
                and delegate_match
                and all(
                    item.get("status") in {"working", "done"}
                    for item in product_match + coder_match + tester_match
                )
            ):
                product_assignment[:] = [product_match[-1]]
                handoff_assignment[:] = [coder_match[-1]]
                flow_tester_assignment[:] = [tester_match[-1]]
                delegate_assignment[:] = [delegate_match[-1]]
                return True
        return False

    wait_until(model_handoff_assignments, "model-driven handoff assignments", 45)
    assignments = current_assignments()
    coder_assignment = handoff_assignment[-1]
    product_flow_assignment = product_assignment[-1]
    tester_flow_assignment = flow_tester_assignment[-1]
    assert product_flow_assignment["bot_id"] == product["id"], product_flow_assignment
    assert coder_assignment["bot_id"] == coder["id"], coder_assignment
    assert tester_flow_assignment["bot_id"] == tester["id"], tester_flow_assignment
    assert coder_assignment["project_id"] == FakeProviderHandler.scenario["project_id"], coder_assignment
    assert coder_assignment["origin_chat_id"] == model_project_chat, coder_assignment

    model_project = rpc(
        base,
        password,
        "project.get",
        {"project_id": FakeProviderHandler.scenario["project_id"]},
    )["project"]
    assert model_project["flow"] == ["产品", "编码", "测试"], model_project
    assert {member["bot_id"] for member in model_project["members"]} >= {
        product["id"], coder["id"], tester["id"]
    }, model_project

    def worker_artifact_message_ready() -> bool:
        history = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": model_project_chat, "after_seq": 0, "limit": 500},
        )
        expected = {
            product_flow_assignment["id"]: FakeProviderHandler.scenario["product_artifact_path"],
            coder_assignment["id"]: FakeProviderHandler.scenario["coder_artifact_path"],
            tester_flow_assignment["id"]: FakeProviderHandler.scenario["tester_artifact_path"],
        }
        return all(
            any(
                message.get("assignment_id") == assignment_id
                and message.get("intent") == "done"
                and any(
                    artifact.get("path_or_url") == artifact_path
                    for artifact in message.get("artifacts", [])
                )
                for message in history.get("messages", [])
            )
            for assignment_id, artifact_path in expected.items()
        )

    wait_until(worker_artifact_message_ready, "product/coder/tester done artifact messages", 30)

    tester_assignment = next(
        item
        for item in current_assignments()
        if item.get("bot_id") == tester["id"]
        and item.get("project_id") == FakeProviderHandler.scenario["project_id"]
        and item.get("origin_chat_id") == model_project_chat
        and item.get("instruction") == FakeProviderHandler.scenario["tester_instruction"]
    )
    assert delegate_assignment and delegate_assignment[-1]["project_id"] is None
    assert delegate_assignment[-1]["origin_chat_id"] == main_chat_id
    handoff_blocks: list[dict[str, Any]] = []
    bot_dm_ref_chat_id: str | None = None

    def handoff_cards_ready() -> bool:
        nonlocal bot_dm_ref_chat_id, handoff_blocks
        project_history = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": model_project_chat, "after_seq": 0, "limit": 500},
        )
        main_history = rpc(
            base,
            password,
            "chat.history",
            {"chat_id": main_chat_id, "after_seq": 0, "limit": 500},
        )
        handoff_blocks = [
            block
            for message in project_history.get("messages", [])
            for block in message.get("blocks", [])
        ]
        main_blocks = [
            block
            for message in main_history.get("messages", [])
            for block in message.get("blocks", [])
        ]
        refs = [
            block
            for block in handoff_blocks
            if block.get("type") == "bot_dm_ref"
            and str(block.get("chat_id", "")).startswith("bot_dm_")
            and int(block.get("count", 0)) >= 1
        ]
        ready = (
            any(
                block.get("type") == "task_card"
                and block.get("assignment_id") == coder_assignment["id"]
                for block in handoff_blocks
            )
            and any(
                block.get("type") == "delegation"
                and block.get("bot_id") == tester["id"]
                and block.get("assignment_id") == delegate_assignment[-1]["id"]
                for block in main_blocks
            )
            and bool(refs)
        )
        if ready:
            bot_dm_ref_chat_id = refs[-1]["chat_id"]
        return ready

    wait_until(handoff_cards_ready, "task, delegation, and Bot DM reference cards", 30)
    assert bot_dm_ref_chat_id is not None
    bot_dm = rpc(base, password, "chat.get", {"chat_id": bot_dm_ref_chat_id})["chat"]
    assert bot_dm["kind"] == "bot_dm", bot_dm
    assert set(bot_dm["member_bot_ids"]) == {"main", tester["id"]}, bot_dm
    try:
        rpc(
            base,
            password,
            "chat.send",
            {"chat_id": bot_dm_ref_chat_id, "text": "不可写入 Bot DM", "mentions": []},
        )
    except AssertionError as error:
        assert "forbidden" in str(error).lower(), error
    else:
        raise AssertionError("bot_dm must be read-only")

    # Approval/question cards above are emitted by real model tool calls and
    # settled through bootstrap, question.answer, and approval.decide.  Do not
    # call internal request helpers here: they are not public protocol RPCs.
    announcement = rpc(base, password, "project.get", {"project_id": FakeProviderHandler.scenario["project_id"]})["announcement"]
    assert announcement["project_id"] == FakeProviderHandler.scenario["project_id"]
    announcement_artifacts = {
        artifact.get("path_or_url")
        for artifact in announcement.get("artifacts", [])
        if isinstance(artifact, dict)
    }
    assert {
        FakeProviderHandler.scenario["product_artifact_path"],
        FakeProviderHandler.scenario["coder_artifact_path"],
        FakeProviderHandler.scenario["tester_artifact_path"],
    }.issubset(announcement_artifacts), announcement
    usage = rpc(base, password, "usage.summary", {"from": "2000-01-01T00:00:00Z", "to": "2999-01-01T00:00:00Z"})
    assert usage["current"]["requests"] > 0, usage

    # Do not let daemon shutdown hide a still-running attention or handoff
    # assignment.  Every job created by this fresh smoke home must also have
    # reached a durable terminal status before the evidence is emitted.
    assert args.home is not None, "strict collaboration smoke requires --home"
    live_assignment_statuses = {"working", "queued", "blocked", "waiting_user", "waiting_bot"}
    terminal_job_statuses = {"done", "failed", "cancelled"}

    def job_snapshots() -> list[dict[str, Any]]:
        snapshots: list[dict[str, Any]] = []
        for path in (args.home / "data" / "jobs").glob("*.json"):
            try:
                value = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if isinstance(value, dict):
                snapshots.append(value)
        return snapshots

    def all_scene_work_drained() -> bool:
        assignments_now = rpc(base, password, "assignment.list", {"limit": 500})["items"]
        live_assignments = [
            item for item in assignments_now if item.get("status") in live_assignment_statuses
        ]
        live_jobs = [
            job for job in job_snapshots() if job.get("status") not in terminal_job_statuses
        ]
        return not live_assignments and not live_jobs

    wait_until(all_scene_work_drained, "all collaboration assignments and jobs terminal", 60)
    final_assignments = rpc(base, password, "assignment.list", {"limit": 500})["items"]
    final_jobs = job_snapshots()

    def counts(items: list[dict[str, Any]]) -> dict[str, int]:
        result: dict[str, int] = {}
        for item in items:
            status = item.get("status", "unknown")
            result[status] = result.get(status, 0) + 1
        return result

    print(json.dumps({
        "ok": True,
        "project_id": FakeProviderHandler.scenario["project_id"],
        "checks": {
            "three_phase_product_coder_tester": True,
            "main_opening_and_product_dispatch": True,
            "project_cards_and_canonical_completion": True,
            "artifact_summary_memory": True,
            "assignment_project_filter": True,
            "parallel_groups": len(parallel_assignments) == 2,
            "main_delegate_and_bot_dm": True,
            "steer_waiting": True,
            "question_takeover_decision_blocked": True,
            "decision_question_wire": decision_wait["answer_method"] == "question.answer"
            and bool(decision_wait["question_id"]),
            "decision_question_parent_resume": decision_wait["answer_method"] == "question.answer",
            "decision_bot_parent_child_resume": bot_decision_wait["answer_method"] == "automatic_child_resume"
            and bool(bot_decision_wait["child_assignment_id"]),
            "subagent_trace": True,
        },
        "assignment_status_counts": counts(final_assignments),
        "job_status_counts": counts(final_jobs),
        "parallel_assignments": parallel_assignments,
        "decision_wait": decision_wait,
        "bot_decision_wait": bot_decision_wait,
        "blocked_wait": blocked_wait,
        "usage_requests": usage["current"]["requests"],
        "provider_calls": FakeProviderHandler.calls,
    }, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7797")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command")
    parser.add_argument("--home", type=Path)
    parser.add_argument("--browser-bin", default=os.environ.get("MACBOT_BROWSER_BIN"))
    args = parser.parse_args()
    FakeProviderHandler.calls = 0
    FakeProviderHandler.bodies = []
    provider, provider_url = start_provider()
    daemon = Daemon(args)
    try:
        daemon.start()
        acceptance(args, provider_url)
    finally:
        provider_bodies = write_provider_bodies(args.home)
        daemon.close()
        provider.shutdown()
        provider.server_close()
        if provider_bodies is not None:
            print(json.dumps({"provider_bodies": str(provider_bodies)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
