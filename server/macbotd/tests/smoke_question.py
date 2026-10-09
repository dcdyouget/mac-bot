#!/usr/bin/env python3
"""Production private ask_user acceptance with a local fake provider.

The scenario exercises the real daemon and runtime: a private Bot DM asks a
question, the public question id is answered over RPC, and the same durable
run resumes to ``run.end``.  It also checks the no-assignment scope does not
create a schedulable Assignment or double-count Bot/workbench waiting state.

Dependencies: Python 3.9+ and ``websockets``.
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
from typing import Any, Optional
import urllib.error

from smoke_collaboration import Daemon, rpc, wait_until


REPO = Path(__file__).resolve().parents[3]
TOKEN = "macbot-question-fake-token"


class FakeQuestionProvider(BaseHTTPRequestHandler):
    lock = threading.Lock()
    requests: list[dict[str, Any]] = []

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
            self.wfile.write(f"data: {json.dumps(payload, ensure_ascii=False)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self.headers.get("Authorization") == f"Bearer {TOKEN}":
            self._json(200, {"data": [{"id": "question-fake", "object": "model"}]})
            return
        self._json(401, {"error": {"message": "fake token required"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "fake token required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid JSON"}})
            return
        with type(self).lock:
            type(self).requests.append(body)
        has_tool_result = any(item.get("role") == "tool" for item in body.get("messages", []))
        if not has_tool_result:
            self._stream(
                [
                    {
                        "choices": [
                            {
                                "delta": {
                                    "tool_calls": [
                                        {
                                            "index": 0,
                                            "id": "question-fake-call",
                                            "type": "function",
                                            "function": {
                                                "name": "question",
                                                "arguments": json.dumps({"question": "请选择生产环境"}),
                                            },
                                        }
                                    ]
                                },
                                "finish_reason": None,
                            }
                        ]
                    },
                    {
                        "choices": [{"delta": {}, "finish_reason": "tool_calls"}],
                        "usage": {"prompt_tokens": 12, "completion_tokens": 5},
                    },
                ]
            )
            return
        self._stream(
            [
                {"choices": [{"delta": {"content": "已选择生产环境"}, "finish_reason": None}]},
                {
                    "choices": [{"delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 14, "completion_tokens": 4},
                },
            ]
        )


def start_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeQuestionProvider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


async def receive_until(ws: Any, predicate: Any, timeout: float = 30) -> list[dict[str, Any]]:
    frames: list[dict[str, Any]] = []
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        frame = json.loads(await asyncio.wait_for(ws.recv(), max(0.1, deadline - time.monotonic())))
        frames.append(frame)
        if predicate(frame):
            return frames
    raise AssertionError("timed out waiting for websocket frame")


async def acceptance(args: argparse.Namespace, provider_url: str) -> None:
    import websockets

    base = args.url.rstrip("/")
    password = args.password
    boot = rpc(base, password, "bootstrap")
    main = next(bot for bot in boot["bots"] if bot["is_main"])
    suffix = os.getpid()
    provider = rpc(
        base,
        password,
        "provider.create",
        {
            "name": f"question-fake-{suffix}",
            "api_kind": "openai-completions",
            "base_url": provider_url,
            "api_key": TOKEN,
            "client_request_id": f"question-provider-{suffix}",
        },
    )["provider"]
    provider_id = provider["id"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider_id})
    assert any(model["model_id"] == "question-fake" for model in refreshed["models"])
    model = rpc(
        base,
        password,
        "model.upsert",
        {
            "provider_id": provider_id,
            "model_id": "question-fake",
            "display_name": "Question fake",
            "caps": {"vision": False, "tools": True, "reasoning": False},
            "client_request_id": f"question-model-{suffix}",
        },
    )["model"]["ref"]
    worker = rpc(
        base,
        password,
        "bot.create",
        {"name": f"Question worker {suffix}", "model": model, "client_request_id": f"question-bot-{suffix}"},
    )["bot"]
    dm_chat = worker["dm_chat_id"]
    rpc(
        base,
        password,
        "bot.update",
        {"bot_id": worker["id"], "patch": {"model": model}, "client_request_id": f"question-bot-model-{suffix}"},
    )

    ws_url = base.replace("http://", "ws://").replace("https://", "wss://")
    async with websockets.connect(
        ws_url + "/ws",
        additional_headers={"Authorization": f"Bearer {password}"},
        proxy=None,
    ) as ws:
        hello = json.loads(await ws.recv())
        assert hello.get("event") == "hello", hello
        resume_id = f"question-resume-{suffix}"
        await ws.send(
            json.dumps(
                {
                    "v": 1,
                    "kind": "req",
                    "id": resume_id,
                    "method": "session.resume",
                    "params": {
                        "last_seq": 0,
                        "client": {"platform": "smoke", "app_version": "question", "device_name": "question", "device_id": f"question-{suffix}"},
                    },
                }
            )
        )
        await receive_until(ws, lambda frame: frame.get("event") == "sync.done")
        sent = rpc(
            base,
            password,
            "chat.send",
            {"chat_id": dm_chat, "text": "PRIVATE_ASK_USER", "mentions": [], "client_request_id": f"question-chat-{suffix}"},
        )
        assert sent["message"]["chat_id"] == dm_chat

        question_frames: list[dict[str, Any]] = []
        question_id: Optional[str] = None
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline and question_id is None:
            frame = json.loads(await asyncio.wait_for(ws.recv(), max(0.1, deadline - time.monotonic())))
            if frame.get("event") == "question.asked":
                question_frames.append(frame)
                public_question = frame.get("data", {}).get("question") or frame.get("data", {})
                question_id = public_question.get("id")
            elif frame.get("event") in {"message.created", "message.updated"}:
                message = frame.get("data", {}).get("message", {})
                if message.get("chat_id") == dm_chat:
                    block = next((item for item in message.get("blocks", []) if item.get("type") == "question"), None)
                    # The message card is only the durable UI representation.
                    # Wait for the public question.asked event before using its
                    # id, otherwise a snapshot/card can race the question RPC.
                    assert not block or block.get("question_id")
        assert question_id, "private question was not published"
        assert len(question_frames) == 1, question_frames
        question = rpc(base, password, "bootstrap")["pending"]["questions"]
        pending = next(item for item in question if item["id"] == question_id)
        assert pending["assignment_id"] == f"dm_{dm_chat}"
        assert not any(item["id"] == pending["assignment_id"] for item in rpc(base, password, "assignment.list", {"limit": 200})["items"])
        bot = next(item for item in rpc(base, password, "bot.list", {})["bots"] if item["id"] == worker["id"])
        assert bot["status"]["summary"] == "waiting_user"
        assert bot["status"]["waiting"] == 1
        workbench = rpc(base, password, "workbench.get", {})
        workbench = workbench.get("workbench", workbench)
        assert sum(item.get("kind") == "question" and item.get("question", {}).get("id") == question_id for item in workbench.get("waiting", [])) == 1

        rpc(base, password, "question.answer", {"question_id": question_id, "text": "生产环境", "client_request_id": f"question-answer-{suffix}"})
        done = False
        trace_items: list[dict[str, Any]] = []
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            trace_items = rpc(base, password, "trace.history", {"chat_id": dm_chat, "limit": 500})["items"]
            done = any(item.get("type") == "run.end" and item.get("data", {}).get("status") == "done" for item in trace_items)
            if done:
                break
            await asyncio.sleep(0.2)
        assert done, "private run did not resume to run.end"
        starts = [item for item in trace_items if item.get("type") == "run.start"]
        ends = [item for item in trace_items if item.get("type") == "run.end" and item.get("data", {}).get("status") == "done"]
        assert starts and ends, trace_items
        run_id = starts[-1].get("data", {}).get("run_id") or starts[-1].get("run_id")
        end_run_id = ends[-1].get("data", {}).get("run_id") or ends[-1].get("run_id")
        assert run_id and run_id == end_run_id, (run_id, end_run_id)
        history = rpc(base, password, "chat.history", {"chat_id": dm_chat, "limit": 100})["messages"]
        assert any("已选择生产环境" in item.get("fallback_text", "") for item in history)
        bot = next(item for item in rpc(base, password, "bot.list", {})["bots"] if item["id"] == worker["id"])
        assert bot["status"]["summary"] == "idle"
        assert bot["status"]["waiting"] == 0


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7841")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--browser-bin", default=None)
    args = parser.parse_args()
    if args.home.resolve().is_relative_to(REPO.resolve()):
        parser.error("--home must be outside the repository")
    shutil.rmtree(args.home, ignore_errors=True)
    args.home.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.environ["MACBOT_SECRET_BACKEND"] = "file"
    os.environ["MACBOT_SECRET_DIR"] = str(args.home / "secrets")
    daemon = Daemon(args)
    provider, provider_url = start_provider()
    try:
        daemon.start()
        asyncio.run(acceptance(args, provider_url))
        print("private question smoke passed: public question scope, one question.asked, same-run resume, Bot/workbench waiting counts")
    finally:
        daemon.close()
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
