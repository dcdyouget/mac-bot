#!/usr/bin/env python3
"""Production Main-Bot project closure through a deterministic local provider.

The daemon, scheduler, file tools, approvals, questions, handoff and project
review all run through production code. Only the OpenAI-compatible provider is
local and scripted. No test artifact is created by this script: the worker's
production ``write`` tool must create it in the isolated project Home.

Example:
  python server/macbotd/tests/smoke_product_closure.py \
    --daemon-binary /tmp/macbot-audit-20261015-iEFvBx/bin/macbotd
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import urllib.error
import urllib.request
import uuid

from smoke_collaboration import http_json, rpc, wait_until


TOKEN = "product-closure-local-provider-token"


class ProductProvider(BaseHTTPRequestHandler):
    lock = threading.Lock()
    requests: list[dict[str, Any]] = []
    project_id = ""
    project_chat_id = ""
    worker_id = ""
    worker_name = ""
    project_name = ""
    home = Path("/")
    project_home = Path("/")
    artifact = ""
    revision_artifact = ""
    initial_instruction = ""
    revision_instruction = ""
    marker = ""
    changes_marker = ""
    steer_marker = ""
    worker_waiting_for_steer = threading.Event()
    steer_sent = threading.Event()
    deferred_decision_retried = False

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: dict[str, Any]) -> None:
        body = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _stream_tool(self, name: str, args: dict[str, Any]) -> None:
        index = len(type(self).requests)
        payloads = [
            {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": f"closure-{index}", "type": "function", "function": {"name": name, "arguments": json.dumps(args, ensure_ascii=False)}}]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 11, "completion_tokens": 7}},
        ]
        self._stream(payloads)

    def _stream_text(self, value: str) -> None:
        self._stream([
            {"choices": [{"delta": {"content": value}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 8, "completion_tokens": 3}},
        ])

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

    @staticmethod
    def _calls(messages: list[dict[str, Any]]) -> list[tuple[str, dict[str, Any]]]:
        calls = []
        for message in messages:
            for call in message.get("tool_calls", []):
                fn = call.get("function", {})
                try:
                    args = json.loads(fn.get("arguments", "{}"))
                except json.JSONDecodeError:
                    args = {}
                calls.append((str(fn.get("name", "")), args))
        return calls

    @staticmethod
    def _tool_results(messages: list[dict[str, Any]]) -> list[str]:
        return [str(message.get("content", "")) for message in messages if message.get("role") == "tool"]

    @staticmethod
    def _project_id_from_results(messages: list[dict[str, Any]]) -> str | None:
        for content in reversed(ProductProvider._tool_results(messages)):
            try:
                value = json.loads(content)
            except json.JSONDecodeError:
                continue
            for key in ("project", "result"):
                project = value.get(key) if isinstance(value, dict) else None
                if isinstance(project, dict) and isinstance(project.get("id"), str):
                    return project["id"]
        return None

    def _decision(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        calls = self._calls(messages)
        names = [name for name, _args in calls]
        latest_user = next((str(m.get("content", "")) for m in reversed(messages) if m.get("role") == "user"), "")
        prompt = json.dumps(messages, ensure_ascii=False)
        is_main = any(m.get("role") == "system" and "秘书长" in str(m.get("content", "")) for m in messages)
        is_worker = any(m.get("role") == "system" and type(self).worker_name in str(m.get("content", "")) for m in messages)

        if is_main and self.marker in latest_user and "create_project" not in names:
            return "create_project", {"name": type(self).project_name, "member_bot_ids": [type(self).worker_id], "announcement": "交付一个可验证的登录方案；编码负责实现，完成后由用户验收。"}
        if is_main and "create_project" in names and type(self).project_id == "":
            project_id = self._project_id_from_results(messages)
            if project_id:
                type(self).project_id = project_id
                for name, args in reversed(calls):
                    if name == "create_project":
                        chat = args.get("chat") or {}
                        if isinstance(chat, dict):
                            type(self).project_chat_id = str(chat.get("id", ""))
                        break
        if is_main and self.marker in latest_user and "assign" not in names and type(self).project_id:
            return "assign", {"project_id": type(self).project_id, "bot_id": type(self).worker_id, "title": "实现闭环产物", "instruction": type(self).initial_instruction}
        if is_main and type(self).changes_marker in latest_user and "assign" not in names:
            # Carry the actual latest user request through Main's assignment;
            # the worker must receive the acceptance change from production context.
            revision_request = latest_user
            assert type(self).changes_marker in revision_request
            return "assign", {"project_id": type(self).project_id, "bot_id": type(self).worker_id, "title": "按验收意见修订", "instruction": f"{type(self).revision_instruction}\n用户验收意见：{revision_request}"}
        if is_main and type(self).changes_marker in latest_user and "assign" in names and "send_msg" not in names:
            return "send_msg", {"intent": "done", "text": "已按用户验收意见重新派发修订任务。"}
        if is_main and type(self).project_id and ("WORKER_INITIAL_DONE" in prompt or "WORKER_REVISION_DONE" in prompt) and "request_review" not in names:
            return "request_review", {"project_id": type(self).project_id, "summary": "工作 Bot 已提交真实文件，请用户核对并验收。"}
        if is_main and type(self).project_id and "request_review" in names and "send_msg" not in names:
            return "send_msg", {"intent": "done", "text": "已提交项目验收，等待用户确认。"}

        # Assignment instructions are a user message in a worker run, whereas
        # they also appear inside Main's historical assign tool result. Match
        # the current user turn so a continuing Main run cannot impersonate a
        # worker and invoke worker-only file tools.
        instruction = type(self).revision_instruction if type(self).revision_instruction in prompt else type(self).initial_instruction if type(self).initial_instruction in prompt else ""
        is_revision = instruction == type(self).revision_instruction
        if is_worker and instruction:
            artifact = type(self).revision_artifact if is_revision else type(self).artifact
            send_count = sum(name == "send_msg" for name, _ in calls)
            write_result_ok = (type(self).project_home / artifact).is_file()
            if write_result_ok:
                marker = "WORKER_REVISION_DONE" if is_revision else "WORKER_INITIAL_DONE"
                return "send_msg", {"intent": "done", "text": marker + "：修订产物已由文件工具写入，交主 Bot 发起验收。" if is_revision else marker + "：实现文件已由文件工具生成，交主 Bot 发起验收。", "artifacts": [{"title": "验收说明", "path_or_url": artifact}], "mentions": ["main"]}
            if "write" not in names:
                if send_count == 0:
                    return "send_msg", {"intent": "ack", "text": "收到，我会在项目目录生成可复核文件，并按群内最新要求处理。"}
                if not is_revision and send_count == 1:
                    type(self).worker_waiting_for_steer.set()
                    type(self).steer_sent.wait(20)
                    return "send_msg", {"intent": "decision", "text": "确认使用独立目录写入实现文件吗？", "options": ["确认", "取消"], "mentions": ["user"]}
                deferred_decision = any(
                    message.get("role") == "tool"
                    and "deferred because new user input arrived during the provider request"
                    in str(message.get("content", ""))
                    for message in messages
                )
                if (
                    not is_revision
                    and deferred_decision
                    and not type(self).deferred_decision_retried
                ):
                    # The explicit @ arrived after this decision request had
                    # already gone to the provider. Its stale tool call was
                    # correctly deferred, so ask the user again on the next turn.
                    type(self).deferred_decision_retried = True
                    type(self).worker_waiting_for_steer.set()
                    type(self).steer_sent.wait(20)
                    return "send_msg", {"intent": "decision", "text": "确认使用独立目录写入实现文件吗？", "options": ["确认", "取消"], "mentions": ["user"]}
                required_marker = type(self).changes_marker if is_revision else type(self).steer_marker
                assert required_marker in prompt, f"worker prompt lacks actual user requirement {required_marker}"
                content = "# Product closure artifact\n\n" + type(self).steer_marker + "\n" + type(self).marker + "\n"
                if is_revision:
                    content += "\n" + type(self).changes_marker + "\n已按用户提出的验收意见加入修订记录。\n"
                return "write", {"path": artifact, "content": content}
            return None
        return None

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self.headers.get("Authorization") == f"Bearer {TOKEN}":
            self._json(200, {"data": [{"id": "product-closure-fake", "object": "model"}]})
            return
        self._json(401, {"error": {"message": "local provider token required"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "local provider token required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        messages = body.get("messages", [])
        with type(self).lock:
            type(self).requests.append(body)
        action = self._decision(messages)
        if action:
            self._stream_tool(*action)
        else:
            self._stream_text("已记录当前任务状态。")


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), ProductProvider)
    threading.Thread(target=server.serve_forever, daemon=True, name="product-closure-provider").start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


class IsolatedDaemon:
    def __init__(self, binary: Path, home: Path, config: Path, port: int, password: str) -> None:
        self.command = [str(binary), "--config", str(config), "--home", str(home), "--port", str(port), "--password", password]
        self.process: subprocess.Popen[bytes] | None = None
        self.url = f"http://127.0.0.1:{port}"
        self.password = password
        self.home = home

    def start(self) -> None:
        env = os.environ.copy()
        env.update({"HOME": str(self.home.parent), "MACBOT_HOME": str(self.home), "MACBOT_SECRET_BACKEND": "file", "MACBOT_SECRET_DIR": str(self.home / "secrets")})
        self.process = subprocess.Popen(self.command, cwd=Path(__file__).resolve().parents[3], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        wait_until(lambda: http_json(self.url + "/api/v1/health").get("ok") is True, "isolated macbotd startup", 30)

    def close(self) -> None:
        if self.process is not None and self.process.poll() is None:
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
                self.process.wait(timeout=10)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                self.process.kill()


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def assignment_by_instruction(base: str, password: str, instruction: str) -> dict[str, Any]:
    items = rpc(base, password, "assignment.list", {"limit": 300})["items"]
    # Main appends the actual user change request to the revision assignment.
    # Match the stable marker while retaining the full production instruction.
    matches = [item for item in items if instruction in str(item.get("instruction", ""))]
    assert len(matches) == 1, {"instruction": instruction, "matches": matches}
    return matches[0]


def wait_assignment(base: str, password: str, instruction: str, statuses: set[str], timeout: float = 45) -> dict[str, Any]:
    result: list[dict[str, Any]] = []
    def ready() -> bool:
        try:
            item = assignment_by_instruction(base, password, instruction)
        except AssertionError as error:
            if "matches" in str(error):
                return False
            raise
        result[:] = [item]
        return item.get("status") in statuses
    wait_until(ready, f"assignment {instruction!r} status in {statuses}", timeout)
    return result[0]


def setup_model(base: str, password: str, provider_url: str, suffix: str) -> tuple[str, dict[str, Any]]:
    provider = rpc(base, password, "provider.create", {"name": f"closure-{suffix}", "api_kind": "openai-completions", "base_url": provider_url, "api_key": TOKEN, "client_request_id": f"closure-provider-{suffix}"})["provider"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider["id"]})
    assert any(item.get("model_id") == "product-closure-fake" for item in refreshed["models"]), refreshed
    model = rpc(base, password, "model.upsert", {"provider_id": provider["id"], "model_id": "product-closure-fake", "display_name": "Product closure fake", "caps": {"vision": False, "tools": True, "reasoning": False}, "client_request_id": f"closure-model-{suffix}"})["model"]["ref"]
    rpc(base, password, "bot.update", {"bot_id": "main", "patch": {"model": model}, "client_request_id": f"closure-main-model-{suffix}"})
    worker = rpc(base, password, "bot.create", {"name": f"Closure worker {suffix}", "model": model, "max_parallel": 1, "tools": {"files": True, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False}, "client_request_id": f"closure-worker-{suffix}"})["bot"]
    return model, worker


def acceptance(args: argparse.Namespace) -> dict[str, Any]:
    temp = Path(args.temp_root).resolve() if args.temp_root else Path(tempfile.mkdtemp(prefix="macbot-product-closure-"))
    temp.mkdir(parents=True, exist_ok=True)
    home = temp / "user-home" / "MacBot"
    home.parent.mkdir(parents=True, exist_ok=True)
    config = temp / "config.toml"
    config.write_text("# isolated smoke configuration\n", encoding="utf-8")
    port = args.port or free_port()
    password = args.password
    daemon = IsolatedDaemon(args.daemon_binary.resolve(), home, config, port, password)
    provider, provider_url = start_provider()
    suffix = uuid.uuid4().hex[:8]
    marker = f"PRODUCT_CLOSURE_{suffix}"
    main_marker = f"MAIN_CREATE_{suffix}"
    changes_marker = f"USER_CHANGE_{suffix}"
    steer_marker = f"USER_STEER_{suffix}"
    initial_instruction = f"WORKER_INITIAL_{suffix}"
    revision_instruction = f"WORKER_REVISION_{suffix}"
    artifact = f"code/closure-{suffix}.md"
    revision_artifact = f"code/revision-{suffix}.md"
    try:
        daemon.start()
        base = daemon.url
        assert http_json(base + "/api/v1/health").get("protocol") == 1
        boot = rpc(base, password, "bootstrap")
        main = next(bot for bot in boot["bots"] if bot.get("is_main"))
        _model, worker = setup_model(base, password, provider_url, suffix)
        ProductProvider.project_id = ""
        ProductProvider.project_chat_id = ""
        ProductProvider.worker_id = worker["id"]
        ProductProvider.worker_name = worker["name"]
        ProductProvider.project_name = f"产品闭环 {suffix}"
        ProductProvider.home = home
        ProductProvider.artifact = artifact
        ProductProvider.revision_artifact = revision_artifact
        ProductProvider.initial_instruction = initial_instruction
        ProductProvider.revision_instruction = revision_instruction
        ProductProvider.marker = main_marker
        ProductProvider.changes_marker = changes_marker
        ProductProvider.steer_marker = steer_marker
        ProductProvider.worker_waiting_for_steer.clear()
        ProductProvider.steer_sent.clear()
        ProductProvider.deferred_decision_retried = False

        sent = rpc(base, password, "chat.send", {"chat_id": main["dm_chat_id"], "text": main_marker + "：请创建项目群，派发编码并提交一个可验收文件。", "mentions": [{"kind": "main"}], "client_request_id": f"closure-start-{suffix}"})
        assert sent["message"]["chat_id"] == main["dm_chat_id"]
        wait_until(lambda: bool(ProductProvider.project_id), "Main Bot create_project model tool", 40)
        created = rpc(base, password, "project.get", {"project_id": ProductProvider.project_id})
        project = created["project"]
        project_chat = project["chat_id"]
        assert project["status"] == "active" and project["lead_bot_id"] == "main"
        assert worker["id"] in {member["bot_id"] for member in project["members"]}
        project_home_text = project["home_path"]
        assert project_home_text.startswith("~/"), project
        home_path = home.parent / project_home_text.removeprefix("~/")
        assert home_path.resolve().is_relative_to(home.resolve()), project
        ProductProvider.project_home = home_path

        first = wait_assignment(base, password, initial_instruction, {"working", "waiting_user", "waiting_approval", "done"}, 45)
        wait_until(ProductProvider.worker_waiting_for_steer.is_set, "worker run before the user steer", 35)
        ordinary = rpc(base, password, "chat.send", {"chat_id": project_chat, "text": f"USER_REVIEW_ONLY_{suffix}", "mentions": [], "client_request_id": f"closure-ordinary-{suffix}"})["message"]
        assert any(item.get("assignment_id") == first["id"] and item.get("state") == "queued" for item in ordinary.get("delivery", [])), ordinary
        inbox_path = home / "data" / "inbox.json"
        inbox_before_mention = json.loads(inbox_path.read_text(encoding="utf-8")) if inbox_path.exists() else []
        assert all(item.get("message_id") != ordinary["id"] for item in inbox_before_mention), inbox_before_mention
        steer = rpc(base, password, "chat.send", {"chat_id": project_chat, "text": steer_marker + " 请在文件里保留这条要求。", "mentions": [{"kind": "bot", "bot_id": worker["id"]}], "client_request_id": f"closure-steer-{suffix}"})
        assert steer["message"]["chat_id"] == project_chat
        ProductProvider.steer_sent.set()

        worker_question: list[dict[str, Any]] = []
        def worker_question_pending() -> bool:
            items = rpc(base, password, "bootstrap")["pending"]["questions"]
            found = next((q for q in items if q.get("state") == "pending" and q.get("bot_id") == worker["id"] and q.get("chat_id") == project_chat), None)
            if found is not None:
                worker_question[:] = [found]
                return True
            return False
        wait_until(worker_question_pending, "worker decision question", 35)
        question = worker_question[0]
        qresult = rpc(base, password, "question.answer", {"question_id": question["id"], "option_index": 0, "client_request_id": f"closure-answer-{suffix}"})
        assert qresult

        def pending_write() -> dict[str, Any] | None:
            approvals = rpc(base, password, "bootstrap")["pending"]["approvals"]
            return next((item for item in approvals if item.get("state") == "pending" and item.get("tool") == "write"), None)
        wait_until(lambda: pending_write() is not None, "production write approval", 45)
        approval = pending_write()
        assert approval is not None
        decided = rpc(base, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once", "client_request_id": f"closure-approval-{suffix}"})
        assert decided
        first = wait_assignment(base, password, initial_instruction, {"done"}, 60)
        target = home_path / artifact
        assert target.is_file(), f"production write tool did not create {target}"
        content = target.read_text(encoding="utf-8")
        assert main_marker in content and steer_marker in content, content
        steer_message = next(message for message in rpc(base, password, "chat.history", {"chat_id": project_chat, "after_seq": 0, "limit": 500})["messages"] if message.get("id") == steer["message"]["id"])
        deliveries = steer_message.get("delivery", [])
        assert any(item.get("bot_id") == worker["id"] and item.get("assignment_id") == first["id"] and item.get("state") == "read" for item in deliveries), steer_message
        assert steer_message.get("fallback_text") == steer_marker + " 请在文件里保留这条要求。", steer_message
        ordinary_message = next(message for message in rpc(base, password, "chat.history", {"chat_id": project_chat, "after_seq": 0, "limit": 500})["messages"] if message.get("id") == ordinary["id"])
        assert any(item.get("assignment_id") == first["id"] and item.get("state") == "read" for item in ordinary_message.get("delivery", [])), ordinary_message

        def trace_has(assignment_id: str, name: str) -> bool:
            items = rpc(base, password, "trace.history", {"assignment_id": assignment_id, "limit": 500})["items"]
            return any(item.get("type") == "tool.start" and item.get("data", {}).get("name") == name for item in items)
        assert trace_has(first["id"], "send_msg")
        assert trace_has(first["id"], "write")
        steer_trace = [
            item for item in rpc(base, password, "trace.history", {"assignment_id": first["id"], "limit": 500})["items"]
            if item.get("type") == "steer" and item.get("data", {}).get("message_id") == steer["message"]["id"]
        ]
        assert len(steer_trace) == 1 and steer_trace[0]["data"].get("text") == steer["message"]["fallback_text"], steer_trace
        review_trace = [
            item for item in rpc(base, password, "trace.history", {"assignment_id": first["id"], "limit": 500})["items"]
            if item.get("type") == "queue.review" and ordinary["id"] in item.get("data", {}).get("message_ids", [])
        ]
        assert len(review_trace) == 1, review_trace
        first_messages = rpc(base, password, "chat.history", {"chat_id": project_chat, "after_seq": 0, "limit": 500})["messages"]
        worker_messages = [message for message in first_messages if message.get("sender", {}).get("bot_id") == worker["id"]]
        assert {message.get("intent") for message in worker_messages} >= {"ack", "decision", "done"}, worker_messages

        wait_until(lambda: rpc(base, password, "project.get", {"project_id": project["id"]})["project"]["status"] == "review", "automatic Main review transition", 45)
        review = rpc(base, password, "project.get", {"project_id": project["id"]})
        assert any(item.get("path_or_url") == artifact for item in review["announcement"].get("artifacts", [])), review["announcement"]
        main_history = rpc(base, password, "chat.history", {"chat_id": main["dm_chat_id"], "after_seq": 0, "limit": 500})["messages"]
        review_cards = [block for msg in main_history for block in msg.get("blocks", []) if block.get("type") == "review_card" and block.get("project_id") == project["id"]]
        assert review_cards and review_cards[-1].get("state") == "pending", review_cards
        assert any(item.get("sender", {}).get("bot_id") == worker["id"] and item.get("chat_id") == project_chat and item.get("intent") == "done" for item in rpc(base, password, "chat.history", {"chat_id": project_chat, "after_seq": 0, "limit": 500})["messages"])

        change = rpc(base, password, "project.request_changes", {"project_id": project["id"], "text": changes_marker + " 请在修订文件中注明人工复核意见。", "client_request_id": f"closure-change-{suffix}"})
        assert change["message"]["chat_id"] == project_chat
        active = rpc(base, password, "project.get", {"project_id": project["id"]})["project"]
        assert active["status"] == "active", active
        revised = wait_assignment(base, password, revision_instruction, {"waiting_approval", "working", "done"}, 60)
        if revised.get("status") != "done":
            wait_until(lambda: any(a.get("state") == "pending" and a.get("tool") == "write" for a in rpc(base, password, "bootstrap")["pending"]["approvals"]), "revision write approval", 45)
            approval2 = next(a for a in rpc(base, password, "bootstrap")["pending"]["approvals"] if a.get("state") == "pending" and a.get("tool") == "write")
            rpc(base, password, "approval.decide", {"approval_id": approval2["id"], "decision": "allow_once", "client_request_id": f"closure-approval-revision-{suffix}"})
        revised = wait_assignment(base, password, revision_instruction, {"done"}, 60)
        revision_file = home_path / revision_artifact
        assert revision_file.is_file(), revision_file
        assert changes_marker in revision_file.read_text(encoding="utf-8")
        assert trace_has(revised["id"], "write") and trace_has(revised["id"], "send_msg")
        wait_until(lambda: rpc(base, password, "project.get", {"project_id": project["id"]})["project"]["status"] == "review", "revised project review", 45)

        # Review is intentionally not completion. Only the explicit public
        # project.confirm_done RPC may make the project done.
        before_confirm = rpc(base, password, "project.get", {"project_id": project["id"]})["project"]
        assert before_confirm["status"] == "review", before_confirm
        confirmed = rpc(base, password, "project.confirm_done", {"project_id": project["id"], "client_request_id": f"closure-confirm-{suffix}"})["project"]
        assert confirmed["status"] == "done", confirmed
        done = rpc(base, password, "project.get", {"project_id": project["id"]})
        assert done["project"]["status"] == "done"
        assert {artifact, revision_artifact}.issubset({a.get("path_or_url") for a in done["announcement"].get("artifacts", [])})
        final_cards = [block for msg in rpc(base, password, "chat.history", {"chat_id": main["dm_chat_id"], "after_seq": 0, "limit": 500})["messages"] for block in msg.get("blocks", []) if block.get("type") == "review_card" and block.get("project_id") == project["id"]]
        assert final_cards and final_cards[-1].get("state") == "confirmed", final_cards
        confirmed_artifacts = {item.get("path_or_url") for item in final_cards[-1].get("artifacts", [])}
        assert {artifact, revision_artifact}.issubset(confirmed_artifacts), final_cards[-1]
        project_messages = rpc(base, password, "chat.history", {"chat_id": project_chat, "after_seq": 0, "limit": 500})["messages"]
        completion_cards = [block for message in project_messages for block in message.get("blocks", []) if block.get("type") == "completion" and message.get("id") == f"msg_project_completion_{project['id']}"]
        assert completion_cards, project_messages
        completion_artifacts = {item.get("path_or_url") for item in completion_cards[-1].get("artifacts", [])}
        assert {artifact, revision_artifact}.issubset(completion_artifacts), completion_cards[-1]
        final_tasks = rpc(base, password, "assignment.list", {"project_id": project["id"], "limit": 100})["items"]
        assert final_tasks and all(item.get("status") == "done" for item in final_tasks), final_tasks
        return {"project_id": project["id"], "chat_id": project_chat, "first_assignment_id": first["id"], "revision_assignment_id": revised["id"], "status": "done", "artifacts": [artifact, revision_artifact], "home": str(home), "provider_requests": len(ProductProvider.requests)}
    finally:
        provider.shutdown()
        provider.server_close()
        daemon.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon-binary", type=Path, required=True)
    parser.add_argument("--temp-root", type=Path)
    parser.add_argument("--port", type=int, help="isolated daemon port; defaults to an available ephemeral port")
    parser.add_argument("--password", default="product-closure-smoke-password")
    args = parser.parse_args()
    result = acceptance(args)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
