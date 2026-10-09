#!/usr/bin/env python3
"""Production routine scheduler acceptance.

The daemon is the system under test.  A local OpenAI-compatible fake provider
is used only to make the scheduled Bot emit one durable ``send_msg`` notice.
The routine snapshot is moved to the past while the daemon is stopped; this
is an isolated, controlled-clock fixture and does not bypass the daemon's
30-second scheduler tick or its execution provider.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import tempfile
import time
from typing import Any
import uuid
from zoneinfo import ZoneInfo

from smoke_collaboration import (
    Daemon,
    FakeProviderHandler,
    TOKEN,
    http_json,
    rpc,
    wait_until,
)


class RoutineProviderHandler(FakeProviderHandler):
    """Return one send_msg call when the scheduled instruction is presented."""

    @classmethod
    def _routine_tool(cls, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        prompt = json.dumps(messages, ensure_ascii=False)
        marker = cls.scenario.get("routine_marker")
        if not marker or marker not in prompt:
            return None
        if "send_msg" in cls._called_tools(messages):
            return None
        return "send_msg", {
            "intent": "done",
            "text": "定时任务已完成，通知用户检查结果",
            "mentions": [],
        }

    def _scripted_tool(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        return type(self)._routine_tool(messages)


def start_provider() -> tuple[Any, str]:
    from http.server import ThreadingHTTPServer
    import threading

    server = ThreadingHTTPServer(("127.0.0.1", 0), RoutineProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def set_routine_clock(home: Path, routine_id: str, *, enabled: bool) -> None:
    snapshot_path = home / "data" / "orchestrator" / "state.json"
    state = json.loads(snapshot_path.read_text())
    routine = state["routines"][routine_id]
    routine["enabled"] = enabled
    routine["next_run_at"] = (datetime.now(timezone.utc) - timedelta(minutes=1)).isoformat()
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=snapshot_path.parent, delete=False
    ) as handle:
        json.dump(state, handle, ensure_ascii=False, separators=(",", ":"))
        handle.flush()
        os.fsync(handle.fileno())
        replacement = Path(handle.name)
    os.replace(replacement, snapshot_path)

    # Production recovery intentionally prefers the newest committed
    # operation snapshot over state.json. Keep this controlled-clock fixture
    # consistent with that WAL source of truth as well.
    operations_path = home / "data" / "orchestrator" / "operations.jsonl"
    operations = [json.loads(line) for line in operations_path.read_text().splitlines() if line]
    for operation in operations:
        operation_snapshot = operation.get("snapshot")
        if isinstance(operation_snapshot, dict) and routine_id in operation_snapshot.get("routines", {}):
            fixture_routine = operation_snapshot["routines"][routine_id]
            fixture_routine["enabled"] = enabled
            fixture_routine["next_run_at"] = routine["next_run_at"]
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=operations_path.parent, delete=False
    ) as handle:
        for operation in operations:
            handle.write(json.dumps(operation, ensure_ascii=False, separators=(",", ":")) + "\n")
        handle.flush()
        os.fsync(handle.fileno())
        replacement = Path(handle.name)
    os.replace(replacement, operations_path)


def assert_cron_next(routine: dict[str, Any], timezone: str) -> None:
    """Check that the server selected a real five-minute cron boundary."""
    next_run = datetime.fromisoformat(routine["next_run_at"].replace("Z", "+00:00"))
    local = next_run.astimezone(ZoneInfo(timezone))
    assert local.minute % 5 == 0, (routine, local.isoformat())


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser()
    p.add_argument("--url", default="http://127.0.0.1:7797")
    p.add_argument("--password", default="dev")
    p.add_argument("--home", type=Path, required=True)
    p.add_argument("--daemon-command", required=True)
    p.add_argument("--browser-bin", default=None)
    return p


def main() -> None:
    args = parser().parse_args()
    server, provider_url = start_provider()
    daemon = Daemon(args)
    base = args.url.rstrip("/")
    suffix = uuid.uuid4().hex[:8]
    marker = f"routine-model-notify-{suffix}"
    RoutineProviderHandler.scenario = {"routine_marker": marker}
    try:
        daemon.start()
        boot = rpc(base, args.password, "bootstrap")
        assert any(bot.get("is_main") for bot in boot["bots"])

        provider = rpc(
            base,
            args.password,
            "provider.create",
            {
                "name": f"routine-fake-{suffix}",
                "api_kind": "openai-completions",
                "base_url": provider_url,
                "api_key": TOKEN,
                "client_request_id": f"routine-provider-{suffix}",
            },
        )["provider"]
        refreshed = rpc(base, args.password, "model.refresh", {"provider_id": provider["id"]})
        assert any(item["model_id"] == "collaboration-fake" for item in refreshed["models"])
        model_ref = rpc(
            base,
            args.password,
            "model.upsert",
            {
                "provider_id": provider["id"],
                "model_id": "collaboration-fake",
                "display_name": "Routine fake",
                "caps": {"vision": False, "tools": True, "reasoning": False},
                "client_request_id": f"routine-model-{suffix}",
            },
        )["model"]["ref"]
        bot = rpc(
            base,
            args.password,
            "bot.create",
            {
                "name": f"定时 Bot-{suffix}",
                "model": model_ref,
                "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
            },
        )["bot"]
        routine = rpc(
            base,
            args.password,
            "routine.create",
            {
                "bot_id": bot["id"],
                "name": f"定时通知-{suffix}",
                "instructions": marker,
                "schedules": [{"cron": "*/5 * * * *", "label": "every five minutes"}],
                "timezone": "Asia/Shanghai",
                "client_request_id": f"routine-create-{suffix}",
            },
        )["routine"]
        assert routine["enabled"] is True and routine["next_run_at"]
        assert_cron_next(routine, "Asia/Shanghai")
        routine_id = routine["id"]

        # routine.test_run must create a real assignment and enter the same
        # runtime/provider path as a scheduled run.
        calls_before_test = RoutineProviderHandler.calls
        test = rpc(
            base,
            args.password,
            "routine.test_run",
            {"routine_id": routine_id, "client_request_id": f"routine-test-{suffix}"},
        )
        test_run = test["run"]
        assert test_run["trigger"] == "test" and test_run["assignment_id"]
        test_assignment_id = test_run["assignment_id"]

        def test_execution_finished() -> bool:
            trace = rpc(base, args.password, "trace.history", {"assignment_id": test_assignment_id, "limit": 500})["items"]
            return any(
                item.get("type") == "run.end" and item.get("data", {}).get("status") == "done"
                for item in trace
            )

        wait_until(test_execution_finished, "routine test_run execution completion", 90)
        assert RoutineProviderHandler.calls > calls_before_test
        test_history = rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
        assert any(item.get("id") == test_run["id"] and item.get("trigger") == "test" for item in test_history)

        # The production create path intentionally enforces the five-minute
        # spacing. Move only this isolated snapshot's clock backwards while
        # stopped, then let the real daemon scheduler observe it. The
        # post-tick next_run_at assertion verifies cron calculation.
        daemon.close()
        set_routine_clock(args.home, routine_id, enabled=True)
        daemon.start()

        def scheduled_run() -> dict[str, Any] | None:
            runs = rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
            return next((run for run in runs if run.get("trigger") == "schedule"), None)

        wait_until(lambda: scheduled_run() is not None, "durable scheduled routine run", 90)
        run = scheduled_run()
        assert run and run["assignment_id"]
        assignment_id = run["assignment_id"]
        current_routines = rpc(base, args.password, "routine.list", {"bot_id": bot["id"]})["routines"]
        scheduled_routine = next(item for item in current_routines if item["id"] == routine_id)
        assert_cron_next(scheduled_routine, "Asia/Shanghai")

        def execution_finished() -> bool:
            trace = rpc(base, args.password, "trace.history", {"assignment_id": assignment_id, "limit": 500})["items"]
            return any(item.get("type") == "run.end" and item.get("data", {}).get("status") == "done" for item in trace)

        wait_until(execution_finished, "scheduled execution completion", 90)
        trace = rpc(base, args.password, "trace.history", {"assignment_id": assignment_id, "limit": 500})["items"]
        assert any(item.get("type") == "routine.run" or item.get("type") == "send_msg" for item in trace)
        assert any(item.get("type") == "send_msg" for item in trace), trace
        routine_chat = f"routine:{routine_id}"
        history = rpc(base, args.password, "chat.history", {"chat_id": routine_chat, "limit": 50})["messages"]
        assert any("定时任务已完成" in item.get("fallback_text", "") for item in history), history

        # Paused routines remain due in the controlled clock, but the daemon
        # must not create another schedule run while enabled=false.
        before = len(rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"])
        daemon.close()
        set_routine_clock(args.home, routine_id, enabled=False)
        daemon.start()
        time.sleep(35)
        after_runs = rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
        assert len(after_runs) == before, after_runs
        current = rpc(base, args.password, "routine.list", {"bot_id": bot["id"]})["routines"]
        assert next(item for item in current if item["id"] == routine_id)["enabled"] is False
        print(json.dumps({"ok": True, "routine_id": routine_id, "test_run_id": test_run["id"], "run_id": run["id"], "assignment_id": assignment_id, "provider_calls": RoutineProviderHandler.calls, "home": str(args.home)}, ensure_ascii=False))
    finally:
        daemon.close()
        server.shutdown()


if __name__ == "__main__":
    main()
