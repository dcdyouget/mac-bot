#!/usr/bin/env python3
"""Production usage/CSV and settings-secret acceptance test.

The test owns an isolated daemon on port 7796 and a local fake OpenAI
provider. It never contacts a real model or writes credentials to Keychain.
Python 3.9+ and only the standard library are required.
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
PROVIDER_SECRET = "fake-provider-secret-7796"
SETTINGS_SECRET = "fake-settings-secret-7796"


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
            self._json(200, {"data": [{"id": "fake-usage", "object": "model"}]})
            return
        self._json(401 if self.path.rstrip("/") == "/v1/models" else 404, {"error": {"message": "not found"}})

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or not self._authorized():
            self._json(401, {"error": {"message": "fake provider authorization required"}})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            json.loads(self.rfile.read(length))
        except (ValueError, json.JSONDecodeError):
            self._json(400, {"error": {"message": "invalid request"}})
            return
        type(self).requests += 1
        chunks = [
            {"choices": [{"delta": {"content": "usage smoke reply"}, "finish_reason": None}]},
            {
                "choices": [{"delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 11, "completion_tokens": 4},
            },
        ]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for chunk in chunks:
            self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode("utf-8"))
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def start_provider() -> Tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    thread = threading.Thread(target=server.serve_forever, name="usage-fake-provider", daemon=True)
    thread.start()
    return server, "http://127.0.0.1:%d/v1" % server.server_port


def http_json(url: str, value: Optional[Dict[str, Any]] = None, password: Optional[str] = None) -> Any:
    headers = {"Content-Type": "application/json"}
    if password:
        headers["Authorization"] = "Bearer " + password
    request = urllib.request.Request(
        url,
        data=None if value is None else json.dumps(value).encode("utf-8"),
        headers=headers,
        method="GET" if value is None else "POST",
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)


def rpc(base: str, method: str, params: Optional[Dict[str, Any]] = None) -> Any:
    return http_json(
        base + "/api/v1/rpc",
        {"method": method, "params": params or {}},
        "dev",
    )


def wait_for_health(base: str, process: subprocess.Popen, timeout: float = 20.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("macbotd exited before becoming healthy")
        try:
            health = http_json(base + "/api/v1/health")
            if health.get("ok") is True:
                return
        except (OSError, urllib.error.URLError, json.JSONDecodeError):
            pass
        time.sleep(0.15)
    raise RuntimeError("timed out waiting for macbotd health")


def start_daemon(command: str, home: Path, secret_dir: Path) -> subprocess.Popen:
    env = os.environ.copy()
    env.update(
        {
            "MACBOT_HOME": str(home),
            "MACBOT_SECRET_BACKEND": "file",
            "MACBOT_SECRET_DIR": str(secret_dir),
        }
    )
    process = subprocess.Popen(
        shlex.split(command),
        cwd=str(REPO),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    wait_for_health("http://127.0.0.1:7796", process)
    return process


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


def get_csv(base: str, query: Dict[str, str]) -> Tuple[int, str]:
    url = base + "/api/v1/usage/export.csv?" + urllib.parse.urlencode(query)
    request = urllib.request.Request(url, headers={"Authorization": "Bearer dev"})
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return response.status, response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode("utf-8", errors="replace")


def assert_rpc_ok(response: Any, method: str) -> Any:
    if not response.get("ok"):
        raise AssertionError("%s failed: %s" % (method, response.get("error")))
    return response["result"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--daemon-command",
        default="server/target/debug/macbotd --port 7796 --password dev",
        help="macbotd command; it must listen on port 7796",
    )
    parser.add_argument(
        "--home",
        type=Path,
        default=Path("/tmp/macbot-usage-smoke-20261009"),
        help="isolated MACBOT_HOME retained as evidence",
    )
    args = parser.parse_args()
    home = args.home.resolve()
    secret_dir = home.parent / (home.name + "-secrets")
    shutil.rmtree(str(home), ignore_errors=True)
    shutil.rmtree(str(secret_dir), ignore_errors=True)
    home.parent.mkdir(parents=True, exist_ok=True)

    provider, provider_url = start_provider()
    process = None
    base = "http://127.0.0.1:7796"
    try:
        process = start_daemon(args.daemon_command, home, secret_dir)
        bootstrap = assert_rpc_ok(rpc(base, "bootstrap"), "bootstrap")
        created = assert_rpc_ok(
            rpc(
                base,
                "provider.create",
                {
                    "name": "usage-fake",
                    "api_kind": "openai-completions",
                    "base_url": provider_url,
                    "api_key": PROVIDER_SECRET,
                    "client_request_id": "usage-provider",
                },
            ),
            "provider.create",
        )
        provider_id = created["provider"]["id"]
        refreshed = assert_rpc_ok(
            rpc(base, "model.refresh", {"provider_id": provider_id, "client_request_id": "usage-refresh"}),
            "model.refresh",
        )
        assert any(item.get("model_id") == "fake-usage" for item in refreshed["models"])
        model_ref = assert_rpc_ok(
            rpc(
                base,
                "model.upsert",
                {
                    "provider_id": provider_id,
                    "model_id": "fake-usage",
                    "display_name": "Usage fake",
                    "caps": {"vision": False, "tools": True, "reasoning": False},
                    "price": {
                        "input_per_mtok": 1.0,
                        "output_per_mtok": 2.0,
                        "cache_read_per_mtok": 0.1,
                        "cache_write_per_mtok": 0.2,
                    },
                    "client_request_id": "usage-model",
                },
            ),
            "model.upsert",
        )["model"]["ref"]
        bot = assert_rpc_ok(
            rpc(
                base,
                "bot.create",
                {
                    "name": "usage-worker",
                    "model": model_ref,
                    "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
                    "client_request_id": "usage-bot",
                },
            ),
            "bot.create",
        )["bot"]
        project = assert_rpc_ok(
            rpc(
                base,
                "project.create",
                {
                    "name": "usage-project",
                    "goal": "usage CSV acceptance",
                    "member_bot_ids": [bot["id"]],
                    "client_request_id": "usage-project",
                },
            ),
            "project.create",
        )
        assert project["chat"]["id"]
        assert_rpc_ok(
            rpc(
                base,
                "chat.send",
                {
                    "chat_id": project["chat"]["id"],
                    "text": "Reply OK.",
                    "mentions": [{"kind": "bot", "bot_id": bot["id"], "instruction": "Reply OK."}],
                    "client_request_id": "usage-chat",
                },
            ),
            "chat.send",
        )

        deadline = time.monotonic() + 30
        summary = {}
        while time.monotonic() < deadline:
            summary = assert_rpc_ok(
                rpc(base, "usage.summary", {"from": "2000-01-01T00:00:00Z", "to": "2999-01-01T00:00:00Z"}),
                "usage.summary",
            )
            if summary.get("current", {}).get("requests", 0) > 0:
                break
            time.sleep(0.25)
        assert summary.get("current", {}).get("requests", 0) > 0, summary

        # An invalid settings timezone must affect CSV export when timezone is
        # omitted; this proves the handler uses settings rather than a constant.
        assert_rpc_ok(
            rpc(base, "settings.update", {"patch": {"timezone": "Invalid/UsageSmoke"}, "client_request_id": "usage-bad-tz"}),
            "settings.update",
        )
        bad_status, _ = get_csv(base, {"from": "2000-01-01T00:00:00Z", "to": "2999-01-01T00:00:00Z", "dimension": "bot"})
        assert bad_status == 400, bad_status
        assert_rpc_ok(
            rpc(base, "settings.update", {"patch": {"timezone": "UTC"}, "client_request_id": "usage-good-tz"}),
            "settings.update",
        )
        status, csv = get_csv(base, {"from": "2000-01-01T00:00:00Z", "to": "2999-01-01T00:00:00Z", "dimension": "bot"})
        lines = [line for line in csv.splitlines() if line.strip()]
        assert status == 200 and len(lines) >= 2, (status, csv)
        assert bot["id"] in csv

        enabled = assert_rpc_ok(
            rpc(
                base,
                "settings.update",
                {
                    "patch": {"web_search": {"provider": "brave", "endpoint": "https://search.example"}},
                    "web_search_key": SETTINGS_SECRET,
                    "client_request_id": "usage-web-key-set",
                },
            ),
            "settings.update(web_search_key)",
        )
        assert enabled["settings"]["web_search"]["has_key"] is True
        cleared = assert_rpc_ok(
            rpc(
                base,
                "settings.update",
                {"patch": {}, "web_search_key": "", "client_request_id": "usage-web-key-clear"},
            ),
            "settings.update(clear web_search_key)",
        )
        assert cleared["settings"]["web_search"]["has_key"] is False
        rejected = rpc(base, "settings.update", {"patch": {"web_search": {"has_key": True}}})
        assert rejected.get("ok") is False and rejected.get("error", {}).get("code") == "invalid_params", rejected

        settings_text = (home / "data" / "settings.json").read_text(encoding="utf-8")
        data_text = "\n".join(path.read_text(encoding="utf-8", errors="replace") for path in (home / "data").rglob("*") if path.is_file())
        assert SETTINGS_SECRET not in settings_text
        assert SETTINGS_SECRET not in data_text
        assert PROVIDER_SECRET not in data_text
        print(
            json.dumps(
                {
                    "ok": True,
                    "home": str(home),
                    "secret_dir": str(secret_dir),
                    "port": 7796,
                    "provider_requests": FakeProviderHandler.requests,
                    "usage_requests": summary["current"]["requests"],
                    "csv_status": status,
                    "csv_lines": len(lines),
                    "timezone_default_checked": True,
                    "web_search_has_key_after_set": enabled["settings"]["web_search"]["has_key"],
                    "web_search_has_key_after_clear": cleared["settings"]["web_search"]["has_key"],
                    "secret_absent_from_settings_wal_events": True,
                },
                ensure_ascii=False,
            )
        )
    finally:
        stop_daemon(process)
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
