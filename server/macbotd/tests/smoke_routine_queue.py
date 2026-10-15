#!/usr/bin/env python3
"""Isolated production regression for queued routine identity and usage.

Uses a local gated provider, ephemeral ports and a fresh --home. No real model
or existing daemon is contacted. Run after building server/target/debug/macbotd.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shlex
import socket
import threading
from http.server import ThreadingHTTPServer

from smoke_collaboration import Daemon, FakeProviderHandler, REPO, TOKEN, rpc, wait_until


class GatedProvider(FakeProviderHandler):
    started = [threading.Event(), threading.Event()]
    released = [threading.Event(), threading.Event()]
    calls = 0

    def do_POST(self) -> None:  # noqa: N802
        assert self.headers.get("Authorization") == f"Bearer {TOKEN}"
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with type(self).lock:
            index = type(self).calls
            type(self).calls += 1
            type(self).bodies.append(body)
        assert index < 2, "routine must not repeat a model request"
        type(self).started[index].set()
        assert type(self).released[index].wait(40), "provider gate timed out"
        self._stream_tool("send_msg", {"intent": "done", "text": "routine completed", "mentions": []})


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True, help="new isolated output directory")
    parser.add_argument("--binary", type=Path, default=REPO / "server/target/debug/macbotd")
    args = parser.parse_args()
    args.home = args.home.resolve()
    args.home.mkdir(parents=True, exist_ok=False)
    config = args.home / "config.toml"
    config.write_text('host_name = "Routine queue regression"\n')
    provider = ThreadingHTTPServer(("127.0.0.1", 0), GatedProvider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    args.url = f"http://127.0.0.1:{port}"
    args.password = "isolated-routine-test"
    args.browser_bin = None
    args.daemon_command = shlex.join([
        str(args.binary.resolve()), "--config", str(config), "--home", str(args.home),
        "--port", str(port), "--password", args.password,
    ])
    daemon = Daemon(args)

    def call(method: str, params: dict | None = None) -> dict:
        return rpc(args.url, args.password, method, params or {})

    try:
        daemon.start()
        provider_id = call("provider.create", {
            "name": "gated routine provider", "api_kind": "openai-completions",
            "base_url": f"http://127.0.0.1:{provider.server_port}/v1", "api_key": TOKEN,
        })["provider"]["id"]
        model = call("model.upsert", {
            "provider_id": provider_id, "model_id": "collaboration-fake", "display_name": "fake",
            "caps": {"tools": True, "vision": False, "reasoning": False},
        })["model"]["ref"]
        bot = call("bot.create", {"name": "routine worker", "model": model, "max_parallel": 1})["bot"]
        routine = call("routine.create", {
            "bot_id": bot["id"], "name": "queued routine", "instructions": "complete routine",
            "schedules": [{"cron": "*/5 * * * *", "label": "five minutes"}],
        })["routine"]
        first = call("routine.test_run", {"routine_id": routine["id"]})["run"]
        assert GatedProvider.started[0].wait(20)
        second = call("routine.test_run", {"routine_id": routine["id"]})["run"]
        assert call("assignment.get", {"assignment_id": second["assignment_id"]})["assignment"]["status"] == "queued"
        assert not GatedProvider.started[1].is_set()
        GatedProvider.released[0].set()
        assert GatedProvider.started[1].wait(20), "queued routine did not start"
        latest = next(row for row in call("routine.list")["routines"] if row["id"] == routine["id"])
        assert latest["last_run"]["id"] == second["id"], "older completion replaced latest trigger"
        assert latest["last_run"]["status"] == "running"
        GatedProvider.released[1].set()
        wait_until(lambda: all(run["status"] == "done" for run in call("routine.runs", {"routine_id": routine["id"]})["runs"]), "both routine runs done", 30)
        for run in [first, second]:
            request = json.loads((args.home / "data/run_requests" / f'{run["id"]}.json').read_text())
            assert request["run_id"] == run["id"]
            assert request["assignment_id"] == run["assignment_id"]
            assert request["routine"] is True and request["private"] is False
            assert request["chat_id"] == bot["dm_chat_id"]
        raw = [json.loads(line) for path in (args.home / "data/usage/raw").glob("*.jsonl") for line in path.read_text().splitlines()]
        requests = [row for row in raw if row["requests"] > 0]
        assert len(requests) == 2 and {row["run_id"] for row in requests} == {first["id"], second["id"]}
        assert all(row["routine"] is True for row in requests)
        breakdown = call("usage.breakdown", {"from": "2000-01-01T00:00:00Z", "to": "2999-01-01T00:00:00Z", "dimension": "project"})
        assert next(row for row in breakdown["rows"] if row["key"] == "routine")["usage"]["requests"] == 2
        daemon.close()
        daemon.start()
        latest = next(row for row in call("routine.list")["routines"] if row["id"] == routine["id"])
        assert latest["last_run"]["id"] == second["id"] and latest["last_run"]["status"] == "done"
        assert GatedProvider.calls == 2, "completed routine replayed after restart"
        print(json.dumps({"result": "passed", "home": str(args.home), "runs": [first["id"], second["id"]], "routine_requests": 2}))
    finally:
        for gate in GatedProvider.released:
            gate.set()
        daemon.close()
        provider.shutdown()


if __name__ == "__main__":
    main()
