#!/usr/bin/env python3
"""S3 feature acceptance against a local fake provider.

The daemon is always supplied by ``--daemon-command`` and owned by this test.
The script uses an isolated ``--home`` and starts the deterministic provider
from ``smoke_runtime.py``; no real API key or model endpoint is contacted.

This test deliberately exercises the public RPC surface first.  A failure
whose method is ``skill.*`` or ``memory.*`` means gateway wiring is missing,
rather than silently reducing the S3 check to a library-only test.
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
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from typing import Any


DUMMY_TOKEN = "macbot-s3-fake-token"


REPO = Path(__file__).resolve().parents[3]


class FakeProviderState:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests: list[dict[str, Any]] = []
        self.bot_id = ""
        self.project_id = ""
        self.skill_name = ""
        self.compact_failure_seen = False
        self.compact_failure_armed = False
        self.memory_failure_seen = False
        self.request_meta: list[dict[str, Any]] = []

    def record(self, request: dict[str, Any]):
        with self.lock:
            self.requests.append(request)

    def snapshot(self):
        with self.lock:
            return list(self.requests)


class FakeProviderHandler(BaseHTTPRequestHandler):
    state: FakeProviderState

    def log_message(self, *_args):
        return

    def _json(self, status: int, value: dict):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        if self.path.rstrip("/") == "/v1/models":
            self._json(200, {"data": [{"id": "s3-fake", "object": "model"}]})
        else:
            self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions":
            self._json(404, {"error": {"message": "not found"}})
            return
        length = int(self.headers.get("Content-Length", "0"))
        request = json.loads(self.rfile.read(length))
        self.state.record(request)
        prompt = json.dumps(request.get("messages", []), ensure_ascii=False)
        markers = (
            "MEMORY_SUCCESS", "MEMORY_FAILURE", "PROJECT_MEMORY", "MEMORY_SEARCH",
            "SKILL_EXEC", "COMPACT_FAILURE", "COMPACT_SENTINEL",
        )
        # Context assembly includes recent group messages, so an old marker
        # can remain in the prompt after the next run starts.  Drive the fake
        # from the latest marker occurrence, matching the current task.
        marker = max((x for x in markers if x in prompt), key=prompt.rfind, default="")
        if "COMPACT_FAILURE" in prompt:
            with self.state.lock:
                self.state.compact_failure_armed = True
        # Failure is deterministic: the first turn stages a memory write and
        # the next model request fails, forcing the runtime rollback hook.
        has_tool_result = any(message.get("role") == "tool" for message in request.get("messages", []))
        has_tool_error = any(
            message.get("role") == "tool" and message.get("is_error") is True
            for message in request.get("messages", [])
        )
        compact_request = any(
            message.get("role") == "system"
            and "Compact the context" in str(message.get("content", ""))
            for message in request.get("messages", [])
        )
        with self.state.lock:
            self.state.request_meta.append({
                "marker": marker,
                "compact": compact_request,
                "failure_marker": "COMPACT_FAILURE" in prompt,
                "armed": self.state.compact_failure_armed,
            })
        # The daemon may place the newest user instruction after an older
        # context segment, so marker ordering is not reliable here.  A
        # compaction request must fail whenever this run's marker is present.
        if self.state.compact_failure_armed and compact_request:
            with self.state.lock:
                self.state.compact_failure_seen = True
            self._json(500, {"error": {"message": "intentional compact failure"}})
            return
        if marker == "MEMORY_FAILURE" and has_tool_result:
            with self.state.lock:
                self.state.memory_failure_seen = True
            self._json(500, {"error": {"message": "intentional fake provider failure"}})
            return
        if marker == "MEMORY_SUCCESS" and not has_tool_result:
            self._stream_tool("memory", {"scope": "bot", "bot_id": self.state.bot_id, "action": "add", "kind": "bot_experience", "content": "cross-run memory sentinel"})
        elif marker == "MEMORY_FAILURE" and not has_tool_result:
            self._stream_tool("memory", {"scope": "bot", "bot_id": self.state.bot_id, "action": "add", "kind": "bot_experience", "content": "must rollback sentinel"})
        elif marker == "PROJECT_MEMORY" and not has_tool_result:
            self._stream_tool("memory", {"scope": "project", "project_id": self.state.project_id, "action": "add", "kind": "project", "content": "ordinary bot project sentinel"})
        elif marker == "MEMORY_SEARCH" and not has_tool_result:
            self._stream_tool("memory_search", {"query": "cross-run memory sentinel"})
        elif marker == "SKILL_EXEC" and not has_tool_result:
            self._stream_tool("skill", {"text": f"/{self.state.skill_name} use this now"})
        elif marker == "COMPACT_SENTINEL":
            self._stream_text("COMPACT_SUMMARY_SUCCESS" if compact_request else "compact complete", 13, 3)
        elif marker == "MEMORY_SEARCH":
            self._stream_text("memory-visible", 13, 3)
        elif marker == "SKILL_EXEC":
            self._stream_text("skill-disabled" if has_tool_error else "skill-visible", 13, 3)
        else:
            self._stream_text("fake ok", 7, 2)

    def _stream(self, payloads: list[dict]):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in payloads:
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")

    def _stream_text(self, text: str, input_tokens: int, output_tokens: int):
        self._stream([{"choices": [{"delta": {"content": text}, "finish_reason": None}]}, {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": input_tokens, "completion_tokens": output_tokens}}])

    def _stream_tool(self, name: str, args: dict):
        self._stream([{"choices": [{"delta": {"tool_calls": [{"index": 0, "id": f"call_{uuid.uuid4().hex[:8]}", "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": None}]}, {"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 17, "completion_tokens": 5}}])


def start_fake_provider():
    state = FakeProviderState()
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeProviderHandler)
    FakeProviderHandler.state = state
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, state, f"http://127.0.0.1:{server.server_port}/v1"


def request_json(
    url: str,
    value: dict | None = None,
    password: str | None = None,
    timeout: float = 15,
):
    headers = {"Content-Type": "application/json"}
    if password:
        headers["Authorization"] = f"Bearer {password}"
    request = urllib.request.Request(
        url,
        data=None if value is None else json.dumps(value).encode(),
        headers=headers,
        method="GET" if value is None else "POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def rpc(
    base: str,
    password: str,
    method: str,
    params: dict | None = None,
    timeout: float = 15,
):
    try:
        body = request_json(
            f"{base}/api/v1/rpc",
            {"method": method, "params": params or {}},
            password,
            timeout,
        )
    except urllib.error.HTTPError as error:
        raise AssertionError(f"{method} HTTP {error.code}: {error.read().decode()}") from error
    if not body.get("ok"):
        raise AssertionError(f"{method} failed: {body.get('error')}")
    return body["result"]


def wait_until(predicate, description: str, timeout: float = 30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, urllib.error.URLError, json.JSONDecodeError):
            pass
        time.sleep(0.25)
    raise AssertionError(f"timed out waiting for {description}")


class Daemon:
    def __init__(self, command: str | None, home: Path | None, base: str):
        self.command = command
        self.home = home
        self.base = base
        self.process: subprocess.Popen | None = None
        self.log_path = self.home / "smoke-daemon.log" if self.home else None
        self.secret_dir = (
            self.home.parent / f".{self.home.name}-smoke-secrets"
            if self.home
            else None
        )

    def start(self):
        if self.command is None or self.home is None or self.secret_dir is None:
            raise AssertionError("--daemon-command and an isolated --home are required")
        if "MACBOT_SECRET_BACKEND=" in self.command or "MACBOT_SECRET_DIR=" in self.command:
            raise AssertionError("secret backend and directory are controlled by the smoke test")
        # Never attach to a daemon started by another test, deployment, or
        # agent.  Otherwise the first health check could pass against that
        # process and restart/close would kill the wrong service.
        try:
            request_json(f"{self.base}/api/v1/health", timeout=0.5)
        except (OSError, urllib.error.URLError, TimeoutError):
            pass
        else:
            raise AssertionError(f"refusing to use occupied daemon port: {self.base}")
        env = os.environ.copy()
        env["MACBOT_HOME"] = str(self.home)
        env["MACBOT_SECRET_BACKEND"] = "file"
        env["MACBOT_SECRET_DIR"] = str(self.secret_dir)
        self.home.mkdir(parents=True, exist_ok=True)
        self.secret_dir.mkdir(parents=True, exist_ok=True)
        log = open(self.log_path, "w", encoding="utf-8") if self.log_path else subprocess.DEVNULL
        self.process = subprocess.Popen(
            shlex.split(self.command), cwd=REPO, env=env,
            stdout=log, stderr=log,
            start_new_session=True,
        )
        if hasattr(log, "close"):
            log.close()
        try:
            wait_until(lambda: request_json(f"{self.base}/api/v1/health").get("ok"), "daemon startup", timeout=90)
        except AssertionError as error:
            if self.process.poll() is not None:
                raise AssertionError(f"daemon exited with {self.process.returncode}; log: {self.log_path}") from error
            raise

    def restart(self):
        if self.process is None:
            raise AssertionError("--daemon-command is required for restart recovery")
        os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait(timeout=10)
        time.sleep(0.4)
        self.start()

    def close(self):
        if self.process and self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)


def feature_lifecycle(base: str, password: str, home: Path):
    name = f"smoke-skill-{uuid.uuid4().hex[:8]}"
    content = f"---\nname: {name}\ndescription: feature smoke\n---\nSKILL_SENTINEL_BODY"
    created = rpc(base, password, "skill.create", {"name": name, "content": content, "client_request_id": f"create-{name}"})
    assert created["skill"]["name"] == name
    assert any(item["name"] == name for item in rpc(base, password, "skill.list")["skills"])
    detail = rpc(base, password, "skill.get", {"name": name})
    assert "SKILL_SENTINEL_BODY" in detail["skill"]["content"]
    updated_content = content + "\nUpdated."
    updated = rpc(base, password, "skill.update", {"name": name, "content": updated_content, "client_request_id": f"update-{name}"})
    assert "Updated." in rpc(base, password, "skill.get", {"name": name})["skill"]["content"]
    disabled = rpc(base, password, "skill.set_enabled", {"name": name, "enabled": False, "client_request_id": f"disable-{name}"})
    assert disabled["skill"]["enabled"] is False
    enabled = rpc(base, password, "skill.set_enabled", {"name": name, "enabled": True, "client_request_id": f"enable-{name}"})
    assert enabled["skill"]["enabled"] is True
    import_name = f"smoke-import-{uuid.uuid4().hex[:8]}"
    import_dir = home / "smoke-import" / import_name
    import_dir.mkdir(parents=True, exist_ok=True)
    (import_dir / "SKILL.md").write_text(f"---\nname: {import_name}\ndescription: imported\n---\nIMPORTED_SENTINEL\n")
    imported = rpc(base, password, "skill.import", {"source": {"kind": "path", "path": str(import_dir)}, "client_request_id": f"import-{import_name}"})
    assert any(item["name"] == import_name for item in imported["skills"])
    # Drafts are generated by the model/runtime, so the acceptance endpoint is
    # intentionally explicit. It must become invocable only after publish.
    draft_name = f"smoke-draft-{uuid.uuid4().hex[:8]}"
    draft = rpc(base, password, "skill.create_draft", {"name": draft_name, "content": f"---\nname: {draft_name}\ndescription: draft\n---\nDRAFT_SENTINEL", "client_request_id": f"draft-{draft_name}"})
    assert draft["skill"]["source"] == "draft"
    published = rpc(base, password, "skill.publish", {"name": draft_name, "client_request_id": f"publish-{draft_name}"})
    assert published["skill"]["source"] == "user"
    deleted = rpc(base, password, "skill.delete", {"name": import_name, "client_request_id": f"delete-{import_name}"})
    assert deleted == {}
    return name


def memory_entries(home: Path):
    state = home / "data" / "memory" / "state.json"
    if not state.exists():
        # A tool approval is asynchronous: the durable job may be resumed
        # after the approval RPC returns.  Callers polling for the first
        # commit must therefore treat a missing state file as "not yet";
        # explicit checks after the poll still fail if no commit occurred.
        return []
    payload = json.loads(state.read_text())
    entries = payload.get("entries", payload if isinstance(payload, list) else [])
    assert isinstance(entries, list)
    return entries


def model_setup(base: str, password: str, fake_url: str, main: dict):
    provider = rpc(base, password, "provider.create", {
        "name": "s3-feature-fake", "api_kind": "openai-completions",
        "base_url": fake_url, "api_key": DUMMY_TOKEN,
        "client_request_id": "s3-feature-provider",
    })
    assert DUMMY_TOKEN not in json.dumps(provider)
    provider_id = provider["provider"]["id"]
    refreshed = rpc(base, password, "model.refresh", {"provider_id": provider_id, "client_request_id": "s3-feature-refresh"})
    assert any(row["model_id"] == "s3-fake" for row in refreshed["models"])
    model = rpc(base, password, "model.upsert", {
        "provider_id": provider_id, "model_id": "s3-fake", "display_name": "S3 fake",
        "caps": {"vision": False, "tools": True, "reasoning": False},
        "price": {"input_per_mtok": 1.0, "output_per_mtok": 2.0, "cache_read_per_mtok": 0.1, "cache_write_per_mtok": 0.2},
        "client_request_id": "s3-feature-upsert",
    })
    model_ref = model["model"]["ref"]
    worker = rpc(base, password, "bot.create", {
        "name": "s3-feature-worker", "model": model_ref,
        "tools": {"files": True, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
        "client_request_id": "s3-feature-worker",
    })["bot"]
    # Keep the main Bot configured too: this catches a deployment that only
    # wires the worker path while the real settings.main path remains null.
    rpc(base, password, "bot.update", {"bot_id": main["id"], "patch": {"model": model_ref}, "client_request_id": "s3-feature-main-model"})
    return worker


def send_run(base: str, password: str, worker: dict, marker: str, chat_id: str, run_id: str):
    result = rpc(base, password, "chat.send", {
        "chat_id": chat_id,
        "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": marker}],
        "client_request_id": run_id,
    }, timeout=120 if len(marker) > 100_000 else 15)
    assert result["message"]["id"]
    return result


def wait_assignment(base: str, password: str, instruction: str, timeout: float = 45, minimum_matches: int = 1):
    def find():
        rows = rpc(base, password, "assignment.list")["items"]
        matches = [row for row in rows if row.get("instruction") == instruction]
        return max(matches, key=lambda row: row.get("created_at", "")) if len(matches) >= minimum_matches else None
    wait_until(lambda: find() is not None, f"assignment {instruction}", timeout)
    return find()


def pump_approvals(base: str, password: str) -> list[str]:
    """Allow each pending fake run exactly once and resume its durable job."""
    pending = rpc(base, password, "approval.list", {"state": ["pending"]}).get("approvals", [])
    decided = []
    for approval in pending:
        approval_id = approval.get("id")
        if not approval_id:
            continue
        rpc(base, password, "approval.decide", {
            "approval_id": approval_id,
            "decision": "allow_once",
            "client_request_id": f"smoke-approval-{approval_id}",
        })
        decided.append(approval_id)
    return decided


def wait_for_run_evidence(base: str, password: str, home: Path, instruction: str, timeout: float = 45):
    """Return the durable assignment and actual run id, never client_request_id."""
    assignment = wait_assignment(base, password, instruction, timeout)
    assignment_id = assignment["id"]
    run_ids: set[str] = set()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        pump_approvals(base, password)
        trace = rpc(base, password, "trace.history", {
            "assignment_id": assignment_id, "tail": True, "limit": 500,
        })
        for item in trace.get("items", []):
            run_id = item.get("run_id")
            if run_id:
                run_ids.add(run_id)
        for path in (home / "data" / "jobs").glob("*.json"):
            try:
                job = json.loads(path.read_text())
            except (OSError, json.JSONDecodeError):
                continue
            run_id = job.get("checkpoint", {}).get("run_id")
            if run_id:
                run_ids.add(run_id)
        if run_ids:
            return assignment, sorted(run_ids)[-1]
        time.sleep(0.25)
    raise AssertionError(f"no durable run_id for assignment {assignment_id}: trace={trace}")


def chat_text(base: str, password: str, chat_id: str):
    history = rpc(base, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 100})
    return json.dumps(history, ensure_ascii=False)


def chat_messages(base: str, password: str, chat_id: str):
    return rpc(base, password, "chat.history", {"chat_id": chat_id, "after_seq": 0, "limit": 100})["messages"]


def trace_contains(base: str, password: str, assignment_id: str, text: str) -> bool:
    items = rpc(base, password, "trace.history", {
        "assignment_id": assignment_id, "tail": True, "limit": 500,
    }).get("items", [])
    return any(text in json.dumps(item, ensure_ascii=False) for item in items)


def acceptance(args: argparse.Namespace):
    fake_server, fake_state, fake_url = start_fake_provider()
    home = Path(args.home).resolve()
    if REPO == home or REPO in home.parents:
        raise AssertionError(f"--home must be outside repository: {home}")
    if home.exists() and any(home.iterdir()):
        raise AssertionError(f"--home must be a new or empty isolated directory: {home}")
    daemon = Daemon(args.daemon_command, home, args.url.rstrip("/"))
    daemon.start()
    try:
        base = args.url.rstrip("/")
        assert request_json(f"{base}/api/v1/health").get("protocol") == 1
        bootstrap = rpc(base, args.password, "bootstrap")
        main = next(bot for bot in bootstrap["bots"] if bot["is_main"])
        worker = model_setup(base, args.password, fake_url, main)
        fake_state.bot_id = worker["id"]
        skill_name = feature_lifecycle(base, args.password, home)
        fake_state.skill_name = skill_name
        # Verify the mutable enablement metadata survives a real daemon
        # restart before any model run uses the skill.
        daemon.restart()
        assert rpc(base, args.password, "skill.get", {"name": skill_name})["skill"]["enabled"] is True
        project = rpc(base, args.password, "project.create", {
            "name": "s3-feature-project", "goal": "S3 feature smoke",
            "member_bot_ids": [worker["id"]], "client_request_id": "s3-feature-project",
        })
        chat_id = project["chat"]["id"]
        fake_state.project_id = project["project"]["id"]

        # A normal worker Bot can write only a project where the runtime has
        # injected its membership into MemoryAccess.
        send_run(base, args.password, worker, "PROJECT_MEMORY", chat_id, "s3-project-memory")
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and "ordinary bot project sentinel" in json.dumps(memory_entries(home)),
            "ordinary Bot project memory commit",
        )
        project_entry = next(row for row in memory_entries(home) if row["content"] == "ordinary bot project sentinel")
        assert project_entry["target"] == {"scope": "project", "owner_id": fake_state.project_id}

        # The fake model emits a memory tool call. Verify the exact durable
        # entry, owner, source run id and cross-run retrieval through the tool.
        send_run(base, args.password, worker, "MEMORY_SUCCESS", chat_id, "s3-memory-success")
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and "cross-run memory sentinel" in json.dumps(memory_entries(home)),
            "memory success commit",
        )
        success_assignment, success_run_id = wait_for_run_evidence(
            base, args.password, home, "MEMORY_SUCCESS"
        )
        assert success_run_id != "s3-memory-success"
        entries = memory_entries(home)
        durable = next(row for row in entries if row["content"] == "cross-run memory sentinel")
        assert durable["target"] == {"scope": "bot", "owner_id": worker["id"]}
        assert durable["source"]["run_id"] == success_run_id

        send_run(base, args.password, worker, "MEMORY_SEARCH", chat_id, "s3-memory-search")
        search_assignment = wait_assignment(base, args.password, "MEMORY_SEARCH")
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and trace_contains(base, args.password, search_assignment["id"], "memory-visible"),
            "cross-run memory search",
        )

        send_run(base, args.password, worker, "SKILL_EXEC", chat_id, "s3-skill-exec")
        skill_assignment = wait_assignment(base, args.password, "SKILL_EXEC")
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and trace_contains(base, args.password, skill_assignment["id"], "skill-visible"),
            "skill tool execution",
        )
        disabled = rpc(base, args.password, "skill.set_enabled", {"name": skill_name, "enabled": False, "client_request_id": "s3-skill-disable-again"})
        assert disabled["skill"]["enabled"] is False
        daemon.restart()
        assert rpc(base, args.password, "skill.get", {"name": skill_name})["skill"]["enabled"] is False
        send_run(base, args.password, worker, "SKILL_EXEC", chat_id, "s3-skill-disabled")
        disabled_assignment = wait_assignment(base, args.password, "SKILL_EXEC", minimum_matches=2)
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and trace_contains(base, args.password, disabled_assignment["id"], "skill-disabled"),
            "disabled skill rejection",
        )
        assert not trace_contains(base, args.password, disabled_assignment["id"], "skill-visible"), "disabled skill was still exposed to model"

        # A large task crosses the 80% context boundary. The configured
        # maintenance adapter must record the protocol phase as `compact`.
        send_run(base, args.password, worker, (" x" * 210000) + " COMPACT_SENTINEL", chat_id, "s3-compact")
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and any(
                "compact" == row.get("phase")
                for path in (home / "data" / "usage" / "raw").glob("*.jsonl")
                for row in map(json.loads, path.read_text().splitlines())
                if row
            ),
            "compact usage phase",
        )
        wait_until(
            lambda: any(
                row.get("content") == "COMPACT_SUMMARY_SUCCESS"
                for row in memory_entries(home)
            ),
            "successful compact summary commit",
            timeout=90,
        )

        # A provider failure during compaction must not leave a summary in
        # durable Bot memory. The main model request still completes so the
        # daemon can report the failed maintenance call independently.
        before_compact_failure = memory_entries(home)
        before_non_worklog = [
            row for row in before_compact_failure if row.get("kind") != "bot_worklog"
        ]
        before_worklog_ids = {
            row["id"] for row in before_compact_failure if row.get("kind") == "bot_worklog"
        }
        send_run(base, args.password, worker, (" x" * 210000) + " COMPACT_FAILURE", chat_id, "s3-compact-failure")
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and fake_state.compact_failure_seen,
            "failed compact provider call",
            timeout=120,
        )
        time.sleep(1)
        after_compact_failure = memory_entries(home)
        assert [
            row for row in after_compact_failure if row.get("kind") != "bot_worklog"
        ] == before_non_worklog, "failed compact run changed durable non-worklog memory"
        new_worklogs = [
            row for row in after_compact_failure
            if row.get("kind") == "bot_worklog" and row.get("id") not in before_worklog_ids
        ]
        assert not any(row.get("content") == "COMPACT_SUMMARY_SUCCESS" for row in new_worklogs), (
            "failed compaction left a summary in durable worklog: "
            + json.dumps({"entries": new_worklogs, "requests": fake_state.request_meta}, ensure_ascii=False)
        )

        # Keep the provider failure as the final model turn.  A durable job
        # may remain retryable after an upstream 500; stopping our own
        # assignment exercises rollback without starving later scenarios.
        before_failure = memory_entries(home)
        before_failure_non_worklog = [
            row for row in before_failure if row.get("kind") != "bot_worklog"
        ]
        before_failure_worklog_ids = {
            row["id"] for row in before_failure if row.get("kind") == "bot_worklog"
        }
        send_run(base, args.password, worker, "MEMORY_FAILURE", chat_id, "s3-memory-failure")
        failure_assignment = wait_assignment(
            base, args.password, "MEMORY_FAILURE", timeout=120
        )
        wait_until(
            lambda: (pump_approvals(base, args.password) or True)
            and fake_state.memory_failure_seen,
            "failed provider turn",
            timeout=120,
        )
        _, failure_run_id = wait_for_run_evidence(
            base, args.password, home, "MEMORY_FAILURE", timeout=120
        )
        rpc(base, args.password, "assignment.stop", {
            "assignment_id": failure_assignment["id"],
            "client_request_id": "s3-stop-failed-memory",
        })
        assert failure_run_id != "s3-memory-failure"
        time.sleep(1)
        after_failure = memory_entries(home)
        assert [
            row for row in after_failure if row.get("kind") != "bot_worklog"
        ] == before_failure_non_worklog, "failed run committed staged non-worklog memory"
        assert not any(row.get("content") == "must rollback sentinel" for row in after_failure), (
            "failed run committed staged memory"
        )
        new_failure_worklogs = [
            row for row in after_failure
            if row.get("kind") == "bot_worklog" and row.get("id") not in before_failure_worklog_ids
        ]
        assert all(row.get("content") == "fake ok" for row in new_failure_worklogs), (
            "failed run wrote unexpected worklog: " + json.dumps(new_failure_worklogs, ensure_ascii=False)
        )

        daemon.restart()
        after_restart = memory_entries(home)
        durable_after = next(row for row in after_restart if row["content"] == "cross-run memory sentinel")
        assert durable_after["source"]["run_id"] == success_run_id
        requests = fake_state.snapshot()
        assert any("MEMORY_SUCCESS" in json.dumps(row) for row in requests)
        assert any("MEMORY_FAILURE" in json.dumps(row) for row in requests)
        print("S3 feature smoke passed: skill CRUD/import/draft/publish/disable, tool execution, memory commit/rollback/search/restart, compact usage")
    finally:
        daemon.close()
        fake_server.shutdown()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7798")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", required=True, help="isolated MACBOT_HOME")
    parser.add_argument("--daemon-command", required=True)
    acceptance(parser.parse_args())
