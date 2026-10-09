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


def assert_send_msg_persistence(base: str, password: str, home: Path, trace: list[dict[str, Any]], assignment_id: str) -> None:
    """Tie every executor receipt to the canonical message actually stored.

    A trace receipt alone is insufficient: after a crash/replay the bridge
    must still leave one message with the same id in the target chat.  Done
    receipts additionally have to be the assignment's durable result card.
    """
    receipts = [item for item in trace if item.get("type") == "send_msg"]
    assert receipts, trace
    call_ids = [item.get("data", {}).get("call_id") for item in receipts]
    message_ids = [item.get("data", {}).get("message_id") for item in receipts]
    assert all(call_ids) and len(call_ids) == len(set(call_ids)), receipts
    assert all(message_ids) and len(message_ids) == len(set(message_ids)), receipts
    assignment = next(
        item for item in rpc(base, password, "assignment.list", {"limit": 200})["items"]
        if item.get("id") == assignment_id
    )
    for receipt in receipts:
        data = receipt["data"]
        chat_id = data.get("chat_id")
        assert chat_id, receipt
        messages = rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 100})["messages"]
        matches = [item for item in messages if item.get("id") == data["message_id"]]
        assert len(matches) == 1, (data, messages)
        assert matches[0].get("assignment_id") == assignment_id, (data, matches[0])
        if data.get("intent") == "done":
            assert assignment.get("result_message_id") == data["message_id"], (assignment, data)
    events_path = home / "data" / "events" / "events.jsonl"
    if events_path.exists():
        created = []
        for line in events_path.read_text(encoding="utf-8").splitlines():
            event = json.loads(line)
            if event.get("event") != "message.created":
                continue
            message = event.get("data", {}).get("message", {})
            if message.get("id") in message_ids:
                created.append(message["id"])
        assert len(created) == len(set(created)) == len(message_ids), created


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

        # First exercise the negative default-model route.  A worker with no
        # explicit model must not inherit the main Bot model.
        settings = rpc(
            base,
            args.password,
            "settings.update",
            {"patch": {"models": {"bot_default": None, "main": model_ref}}},
        )["settings"]
        assert settings["models"]["bot_default"] is None
        assert settings["models"]["main"] == model_ref
        missing_bot = rpc(
            base,
            args.password,
            "bot.create",
            {
                "name": f"无默认模型 Bot-{suffix}",
                "model": None,
                "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
            },
        )["bot"]
        assert missing_bot["model"] is None, missing_bot
        missing_routine = rpc(
            base,
            args.password,
            "routine.create",
            {
                "bot_id": missing_bot["id"],
                "name": f"无默认模型巡检-{suffix}",
                "instructions": f"missing-default-{suffix}",
                "schedules": [{"cron": "*/5 * * * *", "label": "every five minutes"}],
                "timezone": "Asia/Shanghai",
                "client_request_id": f"routine-missing-{suffix}",
            },
        )["routine"]
        missing_routine_id = missing_routine["id"]
        missing_response = http_json(
            f"{base}/api/v1/rpc",
            {
                "method": "routine.test_run",
                "params": {"routine_id": missing_routine_id, "client_request_id": f"routine-missing-test-{suffix}"},
            },
            args.password,
        )
        assert missing_response.get("ok"), missing_response
        missing_run = missing_response["result"]["run"]
        assert missing_run["assignment_id"]

        def missing_run_stopped() -> bool:
            runs = rpc(base, args.password, "routine.runs", {"routine_id": missing_routine_id})["runs"]
            return any(
                item.get("id") == missing_run["id"]
                and item.get("status") in {"failed", "blocked"}
                for item in runs
            )

        wait_until(missing_run_stopped, "missing-model routine blocked", 30)
        missing_runs = rpc(base, args.password, "routine.runs", {"routine_id": missing_routine_id})["runs"]
        stopped = next(item for item in missing_runs if item.get("id") == missing_run["id"])
        assert stopped["status"] in {"failed", "blocked"}
        assert "未配置默认模型" in json.dumps(stopped, ensure_ascii=False) or "no model configured" in json.dumps(stopped, ensure_ascii=False)
        missing_assignment = rpc(base, args.password, "assignment.list", {"limit": 100})["items"]
        assignment = next(item for item in missing_assignment if item.get("id") == missing_run["assignment_id"])
        assert assignment["status"] in {"failed", "blocked"}
        assert assignment.get("result_message_id")
        assert assignment["origin_chat_id"] == missing_bot["dm_chat_id"]
        assert assignment.get("project_id") is None
        missing_trace = rpc(base, args.password, "trace.history", {"assignment_id": missing_run["assignment_id"], "limit": 500})["items"]
        assert all(
            item.get("data", {}).get("model")
            for item in missing_trace
            if item.get("type") == "run.start"
        ), missing_trace
        missing_history = rpc(base, args.password, "chat.history", {"chat_id": missing_bot["dm_chat_id"], "limit": 50})["messages"]
        assert any("未配置默认模型" in item.get("fallback_text", "") for item in missing_history), missing_history
        rpc(base, args.password, "routine.set_enabled", {"routine_id": missing_routine_id, "enabled": False})

        # Exercise the positive route without restarting the daemon.  The
        # routine Bot still carries no explicit model; workers now use
        # models.bot_default while the main Bot is independently configured.
        settings = rpc(
            base,
            args.password,
            "settings.update",
            {"patch": {"models": {"bot_default": model_ref, "main": None}}},
        )["settings"]
        assert settings["models"]["bot_default"] == model_ref
        assert settings["models"]["main"] is None
        bot = rpc(
            base,
            args.password,
            "bot.create",
            {
                "name": f"定时 Bot-{suffix}",
                "model": None,
                "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
            },
        )["bot"]
        assert bot["model"] is None, bot
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

        def test_run_finished() -> bool:
            runs = rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
            return any(item.get("id") == test_run["id"] and item.get("status") == "done" for item in runs)

        wait_until(test_run_finished, "routine test_run durable completion", 30)
        test_rpc_run = next(
            item for item in rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
            if item.get("id") == test_run["id"]
        )
        assert test_rpc_run["status"] == "done" and test_rpc_run.get("finished_at")
        disk_state = json.loads((args.home / "data" / "orchestrator" / "state.json").read_text())
        test_disk_run = next(item for item in disk_state["routine_runs"][routine_id] if item.get("id") == test_run["id"])
        assert {
            "status": test_disk_run.get("status"),
            "finished_at": test_disk_run.get("finished_at"),
        } == {
            "status": test_rpc_run.get("status"),
            "finished_at": test_rpc_run.get("finished_at"),
        }
        test_trace = rpc(base, args.password, "trace.history", {"assignment_id": test_assignment_id, "limit": 500})["items"]
        test_assignment = next(
            item for item in rpc(base, args.password, "assignment.list", {"limit": 100})["items"]
            if item.get("id") == test_assignment_id
        )
        assert test_assignment["origin_chat_id"] == bot["dm_chat_id"]
        assert test_assignment.get("project_id") is None
        assert any(item.get("type") == "llm.request" and item.get("data", {}).get("model") == model_ref for item in test_trace), test_trace
        assert any(
            item.get("type") == "llm.response"
            and item.get("data", {}).get("usage", {}).get("requests", 0) > 0
            for item in test_trace
        ), test_trace
        assert_send_msg_persistence(base, args.password, args.home, test_trace, test_assignment_id)

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
        def scheduled_run_finished() -> bool:
            runs = rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
            return any(item.get("id") == run["id"] and item.get("status") == "done" for item in runs)

        wait_until(scheduled_run_finished, "scheduled routine durable completion", 30)
        scheduled_rpc_run = next(
            item for item in rpc(base, args.password, "routine.runs", {"routine_id": routine_id})["runs"]
            if item.get("id") == run["id"]
        )
        assert scheduled_rpc_run["status"] == "done" and scheduled_rpc_run.get("finished_at")
        disk_state = json.loads((args.home / "data" / "orchestrator" / "state.json").read_text())
        scheduled_disk_run = next(item for item in disk_state["routine_runs"][routine_id] if item.get("id") == run["id"])
        assert {
            "status": scheduled_disk_run.get("status"),
            "finished_at": scheduled_disk_run.get("finished_at"),
        } == {
            "status": scheduled_rpc_run.get("status"),
            "finished_at": scheduled_rpc_run.get("finished_at"),
        }
        assert any(item.get("type") == "llm.request" and item.get("data", {}).get("model") == model_ref for item in trace), trace
        assert any(
            item.get("type") == "llm.response"
            and item.get("data", {}).get("usage", {}).get("requests", 0) > 0
            for item in trace
        ), trace
        assert any(item.get("type") == "routine.run" or item.get("type") == "send_msg" for item in trace)
        assert_send_msg_persistence(base, args.password, args.home, trace, assignment_id)
        history = rpc(base, args.password, "chat.history", {"chat_id": bot["dm_chat_id"], "limit": 50})["messages"]
        assert any("定时任务已完成" in item.get("fallback_text", "") for item in history), history

        # A project-bound routine must use the project group as its execution
        # chat.  This covers the routing branch separately from the Bot DM
        # branch above and ensures no routine:<id> pseudo-chat leaks out.
        project = rpc(
            base,
            args.password,
            "project.create",
            {
                "name": f"定时项目-{suffix}",
                "goal": "验证项目定时任务路由",
                "member_bot_ids": [bot["id"]],
                "flow": ["验证"],
                "client_request_id": f"routine-project-{suffix}",
            },
        )
        project_id = project["project"]["id"]
        project_chat = project["chat"]["id"]
        project_routine = rpc(
            base,
            args.password,
            "routine.create",
            {
                "bot_id": bot["id"],
                "project_id": project_id,
                "name": f"项目定时通知-{suffix}",
                "instructions": marker,
                "schedules": [{"cron": "*/5 * * * *", "label": "every five minutes"}],
                "timezone": "Asia/Shanghai",
                "client_request_id": f"routine-project-create-{suffix}",
            },
        )["routine"]
        project_routine_id = project_routine["id"]
        project_test = rpc(
            base,
            args.password,
            "routine.test_run",
            {"routine_id": project_routine_id, "client_request_id": f"routine-project-test-{suffix}"},
        )
        project_run = project_test["run"]
        project_assignment_id = project_run["assignment_id"]

        def project_execution_finished() -> bool:
            trace_items = rpc(base, args.password, "trace.history", {"assignment_id": project_assignment_id, "limit": 500})["items"]
            return any(item.get("type") == "run.end" and item.get("data", {}).get("status") == "done" for item in trace_items)

        wait_until(project_execution_finished, "project routine execution completion", 90)
        project_assignment = next(
            item for item in rpc(base, args.password, "assignment.list", {"limit": 100})["items"]
            if item.get("id") == project_assignment_id
        )
        assert project_assignment["origin_chat_id"] == project_chat
        assert project_assignment.get("project_id") == project_id
        project_history = rpc(base, args.password, "chat.history", {"chat_id": project_chat, "limit": 50})["messages"]
        assert any("定时任务已完成" in item.get("fallback_text", "") for item in project_history), project_history
        project_trace = rpc(base, args.password, "trace.history", {"assignment_id": project_assignment_id, "limit": 500})["items"]
        assert_send_msg_persistence(base, args.password, args.home, project_trace, project_assignment_id)
        rpc(base, args.password, "routine.set_enabled", {"routine_id": project_routine_id, "enabled": False})

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
        events_path = args.home / "data" / "events" / "events.jsonl"
        for line in events_path.read_text(encoding="utf-8").splitlines():
            event = json.loads(line)
            if event.get("event") == "routine.run":
                assert set(event.get("data", {})) == {"run"}, event
        print(json.dumps({"ok": True, "routine_id": routine_id, "project_routine_id": project_routine_id, "test_run_id": test_run["id"], "run_id": run["id"], "assignment_id": assignment_id, "provider_calls": RoutineProviderHandler.calls, "home": str(args.home)}, ensure_ascii=False))
    finally:
        daemon.close()
        server.shutdown()


if __name__ == "__main__":
    main()
