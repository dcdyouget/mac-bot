#!/usr/bin/env python3
"""Crash an approved routine bash call after its external effect.

The recovered routine must retain its only run, suspend the unsafe checkpoint,
and never create an assignment-derived replacement run or call the provider.
All processes, credentials and external effects are isolated under --home.
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
from http.server import ThreadingHTTPServer

from smoke_collaboration import Daemon, FakeProviderHandler, REPO, TOKEN, rpc, wait_until


class EffectProvider(FakeProviderHandler):
    calls = 0
    effect: Path

    def do_POST(self) -> None:  # noqa: N802
        assert self.headers.get("Authorization") == f"Bearer {TOKEN}"
        json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with type(self).lock:
            type(self).calls += 1
        self._stream_tool("bash", {
            "command": f"printf x >> {shlex.quote(str(type(self).effect))}; sleep 4; echo done",
            "background": False,
        })


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=REPO / "server/target/debug/macbotd")
    args = parser.parse_args()
    args.home = args.home.resolve()
    args.home.mkdir(parents=True, exist_ok=False)
    config = args.home / "config.toml"
    config.write_text('host_name = "Unsafe routine recovery regression"\n')
    EffectProvider.effect = args.home / "effect.txt"
    provider = ThreadingHTTPServer(("127.0.0.1", 0), EffectProvider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    args.url = f"http://127.0.0.1:{port}"
    args.password = "isolated-recovery-test"
    args.browser_bin = None
    args.daemon_command = shlex.join([
        str(args.binary.resolve()), "--config", str(config), "--home", str(args.home),
        "--port", str(port), "--password", args.password,
    ])
    daemon = Daemon(args)

    def call(method: str, params: dict | None = None) -> dict:
        return rpc(args.url, args.password, method, params or {})

    def jobs() -> list[dict]:
        return [json.loads(path.read_text()) for path in (args.home / "data/jobs").glob("*.json")]

    try:
        daemon.start()
        provider_id = call("provider.create", {
            "name": "effect provider", "api_kind": "openai-completions",
            "base_url": f"http://127.0.0.1:{provider.server_port}/v1", "api_key": TOKEN,
        })["provider"]["id"]
        model = call("model.upsert", {
            "provider_id": provider_id, "model_id": "collaboration-fake", "display_name": "fake",
            "caps": {"tools": True, "vision": False, "reasoning": False},
        })["model"]["ref"]
        bot = call("bot.create", {
            "name": "unsafe routine worker", "model": model, "max_parallel": 1,
            "tools": {"subagent": False, "files": True, "bash": True, "browser": False, "web": False, "mcp": False},
        })["bot"]
        routine = call("routine.create", {
            "bot_id": bot["id"], "name": "unsafe routine", "instructions": "write effect",
            "schedules": [{"cron": "*/5 * * * *", "label": "five minutes"}],
        })["routine"]
        run = call("routine.test_run", {"routine_id": routine["id"]})["run"]
        wait_until(lambda: call("approval.list", {"state": ["pending"]})["approvals"], "bash approval", 20)
        approvals = call("approval.list", {"state": ["pending"]})["approvals"]
        assert len(approvals) == 1
        call("approval.decide", {"approval_id": approvals[0]["id"], "decision": "allow_once"})
        wait_until(EffectProvider.effect.exists, "external effect", 20)
        before = jobs()
        assert len(before) == 1 and before[0]["checkpoint"]["run_id"] == run["id"]
        assert before[0]["unsafe_replay"] and before[0]["status"] == "running"
        assert EffectProvider.calls == 1 and EffectProvider.effect.read_text() == "x"
        assert daemon.process is not None
        os.killpg(daemon.process.pid, signal.SIGKILL)
        daemon.process.wait(timeout=5)
        time.sleep(5)  # Let this test's finite external command exit too.
        daemon.start()
        wait_until(lambda: any(job["status"] == "suspended" for job in jobs()), "unsafe suspension", 20)
        time.sleep(2)  # Allow recovery dispatch to expose any replacement run.
        after = jobs()
        assert len(after) == 1, f"replacement run created: {after}"
        assert after[0]["id"] == before[0]["id"]
        assert after[0]["checkpoint"]["run_id"] == run["id"]
        assert after[0]["status"] == "suspended" and after[0]["unsafe_replay"]
        assert EffectProvider.calls == 1, "provider called again across unsafe recovery"
        assert EffectProvider.effect.read_text() == "x"
        request_ids = {path.stem for path in (args.home / "data/run_requests").glob("*.json")}
        assert request_ids == {run["id"]}, request_ids
        pending = call("approval.list", {"state": ["pending"]})["approvals"]
        assert not pending, f"replacement execution requested approval: {pending}"
        print(json.dumps({"result": "passed", "home": str(args.home), "run_id": run["id"], "jobs": 1, "provider_calls": 1, "status": "suspended"}))
    finally:
        daemon.close()
        provider.shutdown()


if __name__ == "__main__":
    main()
