#!/usr/bin/env python3
"""Production file/artifact acceptance test for macbotd.

Starts an isolated daemon on port 7794 and a local fake OpenAI provider. It
checks truncated Bash/Read output through both FileRef and trace/output,
project-id-to-slug resolution, byte ranges, and traversal rejection.
Python 3.9+ and the standard library are sufficient.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Dict, Optional, Tuple
import urllib.error
import urllib.parse
import urllib.request
import uuid


REPO = Path(__file__).resolve().parents[3]
PROVIDER_SECRET = "fake-files-provider-7794"


class FakeProviderHandler(BaseHTTPRequestHandler):
    requests = 0

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def _json(self, status: int, value: Dict[str, Any]) -> None:
        payload = json.dumps(value).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _authorized(self) -> bool:
        return self.headers.get("Authorization") == "Bearer " + PROVIDER_SECRET

    def do_GET(self) -> None:  # noqa: N802
        if self.path.rstrip("/") == "/v1/models" and self._authorized():
            self._json(200, {"data": [{"id": "fake-files", "object": "model"}]})
            return
        self._json(401 if self.path.rstrip("/") == "/v1/models" else 404, {"error": {"message": "not found"}})

    def _stream(self, payloads: Any) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            self.wfile.write(("data: " + json.dumps(payload) + "\n\n").encode("utf-8"))
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or not self._authorized():
            self._json(401, {"error": {"message": "fake provider authorization required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid request"}})
            return
        type(self).requests += 1
        messages = body.get("messages", [])
        prompt = json.dumps(messages, ensure_ascii=False)
        user_messages = [
            message.get("content", "")
            for message in messages
            if isinstance(message, dict) and message.get("role") == "user"
        ]
        latest_user = str(user_messages[-1]) if user_messages else prompt
        last_user_index = max(
            (index for index, message in enumerate(messages) if isinstance(message, dict) and message.get("role") == "user"),
            default=-1,
        )
        has_tool_result = any(
            index > last_user_index and isinstance(message, dict) and message.get("role") == "tool"
            for index, message in enumerate(messages)
        )
        tool = None
        args = None
        if "TRUNCATE_BASH" in latest_user and not has_tool_result:
            tool = "bash"
            args = {"command": "python3 -c 'open(\"read-me.txt\",\"w\").write(\"y\"*60000); print(\"x\"*60000)'"}
        elif "TRUNCATE_READ" in latest_user and not has_tool_result:
            tool = "read"
            args = {"path": "read-me.txt"}
        if tool:
            call_id = "call_files_" + tool
            self._stream(
                [
                    {
                        "choices": [
                            {
                                "delta": {
                                    "tool_calls": [
                                        {
                                            "index": 0,
                                            "id": call_id,
                                            "function": {"name": tool, "arguments": json.dumps(args)},
                                        }
                                    ]
                                },
                                "finish_reason": None,
                            }
                        ]
                    },
                    {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 8, "completion_tokens": 5}},
                ]
            )
            return
        self._stream(
            [
                {"choices": [{"delta": {"content": "file smoke done"}, "finish_reason": None}]},
                {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 8, "completion_tokens": 3}},
            ]
        )


def start_provider() -> Tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    threading.Thread(target=server.serve_forever, name="files-fake-provider", daemon=True).start()
    return server, "http://127.0.0.1:%d/v1" % server.server_port


def http_request(url: str, method: str = "GET", body: Optional[Dict[str, Any]] = None, range_header: Optional[str] = None) -> Tuple[int, bytes]:
    headers = {"Authorization": "Bearer dev"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    if range_header:
        headers["Range"] = range_header
    request = urllib.request.Request(
        url,
        data=None if body is None else json.dumps(body).encode("utf-8"),
        headers=headers,
        method=method,
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def rpc(base: str, method: str, params: Optional[Dict[str, Any]] = None) -> Any:
    request = urllib.request.Request(
        base + "/api/v1/rpc",
        data=json.dumps({"method": method, "params": params or {}}).encode("utf-8"),
        headers={"Content-Type": "application/json", "Authorization": "Bearer dev"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)


def result(base: str, method: str, params: Optional[Dict[str, Any]] = None) -> Any:
    response = rpc(base, method, params)
    if not response.get("ok"):
        raise AssertionError("%s failed: %s" % (method, response.get("error")))
    return response["result"]


def start_daemon(command: str, home: Path, secret_dir: Path) -> subprocess.Popen:
    env = os.environ.copy()
    env.update({"MACBOT_HOME": str(home), "MACBOT_SECRET_BACKEND": "file", "MACBOT_SECRET_DIR": str(secret_dir)})
    process = subprocess.Popen(
        shlex.split(command), cwd=str(REPO), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True
    )
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("macbotd exited during startup")
        try:
            with urllib.request.urlopen("http://127.0.0.1:7794/api/v1/health", timeout=1) as response:
                if json.load(response).get("ok"):
                    return process
        except (OSError, urllib.error.URLError, json.JSONDecodeError):
            pass
        time.sleep(0.15)
    raise RuntimeError("timed out waiting for macbotd")


def stop_daemon(process: Optional[subprocess.Popen]) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=8)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--daemon-command", default="server/target/debug/macbotd --port 7794 --password dev")
    parser.add_argument("--home", type=Path, default=Path("/tmp/macbot-files-smoke-20261009"))
    args = parser.parse_args()
    home = args.home.resolve()
    secret_dir = home.parent / (home.name + "-secrets")
    shutil.rmtree(str(home), ignore_errors=True)
    shutil.rmtree(str(secret_dir), ignore_errors=True)
    home.parent.mkdir(parents=True, exist_ok=True)
    provider, provider_url = start_provider()
    process = None
    base = "http://127.0.0.1:7794"
    try:
        process = start_daemon(args.daemon_command, home, secret_dir)
        result(base, "bootstrap")
        provider_id = result(
            base,
            "provider.create",
            {"name": "files-fake", "api_kind": "openai-completions", "base_url": provider_url, "api_key": PROVIDER_SECRET, "client_request_id": "files-provider"},
        )["provider"]["id"]
        result(base, "model.refresh", {"provider_id": provider_id, "client_request_id": "files-refresh"})
        model_ref = result(
            base,
            "model.upsert",
            {"provider_id": provider_id, "model_id": "fake-files", "display_name": "Files fake", "caps": {"vision": False, "tools": True, "reasoning": False}, "price": None, "client_request_id": "files-model"},
        )["model"]["ref"]
        bot = result(
            base,
            "bot.create",
            {"name": "files-worker", "model": model_ref, "tools": {"files": True, "bash": True, "browser": False, "subagent": False, "web": False, "mcp": False}, "client_request_id": "files-bot"},
        )["bot"]
        project = result(
            base,
            "project.create",
            {"name": "Files Slug Project", "goal": "file smoke", "member_bot_ids": [bot["id"]], "client_request_id": "files-project"},
        )
        chat_id = project["chat"]["id"]
        project_dir = home / "projects" / project["project"]["slug"] if "project" in project else home / "projects" / project["slug"]
        project_dir.mkdir(parents=True, exist_ok=True)
        (project_dir / "artifact.md").write_text("project artifact", encoding="utf-8")
        result(
            base,
            "settings.update",
            {"patch": {"approvals": {"mode": "always_allow", "rules": []}}, "client_request_id": "files-approvals"},
        )

        for text, request_id in [("TRUNCATE_BASH", "files-bash"), ("TRUNCATE_READ", "files-read")]:
            result(base, "chat.send", {"chat_id": chat_id, "text": text, "mentions": [{"kind": "bot", "bot_id": bot["id"], "instruction": text}], "client_request_id": request_id})
            deadline = time.monotonic() + 30
            while FakeProviderHandler.requests < (2 if request_id == "files-bash" else 4) and time.monotonic() < deadline:
                for approval in result(base, "approval.list", {}).get("approvals", []):
                    if approval.get("state") == "pending":
                        result(base, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once"})
                time.sleep(0.25)
            assert FakeProviderHandler.requests >= (2 if request_id == "files-bash" else 4), FakeProviderHandler.requests

        assignments = result(base, "assignment.list", {}).get("items", [])
        traces = []
        for assignment in assignments:
            if assignment.get("origin_chat_id") == chat_id:
                traces.extend(result(base, "trace.history", {"assignment_id": assignment["id"], "limit": 500})["items"])
        outputs = []
        for item in traces:
            data = item.get("data", {})
            if item.get("type") == "tool.end" and data.get("full_output"):
                outputs.append((item, data["full_output"]))
        assert len(outputs) >= 2, outputs
        downloaded = []
        for item, file_ref in outputs:
            data = item["data"]
            assert file_ref["root"] == "bot"
            assert "/" not in file_ref["root_id"] and ".." not in file_ref["path"]
            query = urllib.parse.urlencode({"root": file_ref["root"], "root_id": file_ref["root_id"], "path": file_ref["path"]})
            status, body = http_request(base + "/api/v1/files?" + query)
            assert status == 200 and len(body) == file_ref["size"], (status, file_ref, len(body))
            ranged_status, ranged = http_request(base + "/api/v1/files?" + query, range_header="bytes=0-9")
            assert ranged_status == 206 and len(ranged) == 10, (ranged_status, len(ranged))
            trace_query = urllib.parse.urlencode({"run_id": item["run_id"], "call_id": data["call_id"]})
            trace_status, trace_body = http_request(base + "/api/v1/trace/output?" + trace_query)
            assert trace_status == 200 and trace_body == body, (trace_status, len(trace_body), len(body))
            downloaded.append({"call_id": data["call_id"], "size": len(body)})

        project_query = urllib.parse.urlencode({"root": "project", "root_id": project["project"]["id"], "path": "artifact.md"})
        project_status, project_body = http_request(base + "/api/v1/files?" + project_query)
        assert project_status == 200 and project_body == b"project artifact"
        traversal_status, _ = http_request(base + "/api/v1/files?root=bot&root_id=../escape&path=x")
        path_status, _ = http_request(base + "/api/v1/files?root=bot&root_id=%s&path=../escape" % bot["id"])
        assert traversal_status == 400 and path_status == 400, (traversal_status, path_status)
        print(json.dumps({"ok": True, "home": str(home), "port": 7794, "provider_requests": FakeProviderHandler.requests, "truncated_outputs": downloaded, "project_slug": project["project"]["slug"], "project_artifact": True, "range": True, "traversal_rejected": True}))
    finally:
        stop_daemon(process)
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
