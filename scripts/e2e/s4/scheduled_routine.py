#!/usr/bin/env python3
"""S4 API check for one real scheduler-triggered routine run.

This scenario creates one temporary routine whose cron expression targets a
near-future minute, then waits for the production scheduler to create and
finish a ``trigger == schedule`` run.  It never calls ``routine.test_run``.
The task permits only the terminal ``send_msg(intent=done)`` report; browser
login, screen takeover, push notifications, and client UI evidence are outside
this API-only check.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import sys
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    message_text,
    ready_health,
    require_dict,
    require_list,
    run_main,
    sender_is,
    unique_marker,
    wait_until,
)


TIMEZONE = "Asia/Shanghai"
TERMINAL_ASSIGNMENT_STATES = {"done", "failed", "cancelled", "blocked"}


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.set_defaults(timeout=180.0)
    parser.add_argument("--bot-id", required=True, help="Bot that owns the temporary routine")
    return parser


def require_production(health: dict[str, Any], base_url: str) -> None:
    if health.get("mock") is True or urlsplit(base_url).port == 7789:
        raise ValueError("S4 scheduled routine requires the production daemon")


def selected_bot(state: dict[str, Any], bot_id: str) -> dict[str, Any]:
    bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
    bot = next((item for item in bots if item.get("id") == bot_id), None)
    if not isinstance(bot, dict):
        raise ValueError("selected Bot is absent from bootstrap")
    return bot


def routine_runs(client: Any, routine_id: str) -> list[dict[str, Any]]:
    result = require_dict(client.call("routine.runs", {"routine_id": routine_id}), "routine.runs result")
    return [item for item in require_list(result.get("runs"), "routine.runs.runs") if isinstance(item, dict)]


def near_future_schedule(now: datetime) -> tuple[str, str]:
    """Return a five-field cron matching one near-future local minute.

    A single exact calendar occurrence avoids the server's five-minute minimum
    interval validation while allowing the 30-second scheduler tick to see it.
    The routine is deleted in ``finally`` before this date can recur.
    """

    try:
        zone = ZoneInfo(TIMEZONE)
    except ZoneInfoNotFoundError as exc:
        raise ValueError(f"Python tzdata has no {TIMEZONE}") from exc
    target = (now + timedelta(minutes=2)).astimezone(zone).replace(second=0, microsecond=0)
    # Day-of-week '*' makes the restricted day-of-month the sole date filter.
    cron = f"{target.minute} {target.hour} {target.day} {target.month} *"
    return cron, target.isoformat()


def trace_evidence(
    client: Any,
    assignment_id: str,
    marker: str,
    bot_id: str,
    bot_chat_id: str,
) -> dict[str, Any] | None:
    result = require_dict(
        client.call("trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    items = [item for item in require_list(result.get("items"), "trace.history.items") if isinstance(item, dict)]
    if not any(
        item.get("type") == "run.end"
        and isinstance(item.get("data"), dict)
        and item["data"].get("status") == "done"
        for item in items
    ):
        return None
    # The routine instruction permits only the terminal send_msg report.  The
    # model response must contain the marker, and every tool call must be that
    # exact report rather than a file, shell, browser, or external action.
    response_items = [
        item
        for item in items
        if item.get("type") == "llm.response" and isinstance(item.get("data"), dict)
    ]
    if not any(marker in str(item["data"].get("text", "")) for item in response_items):
        return None
    tool_starts = [item for item in items if item.get("type") == "tool.start"]
    if any(
        not isinstance(item.get("data"), dict) or item["data"].get("name") != "send_msg"
        for item in tool_starts
    ):
        raise ValueError("scheduled marker run invoked a tool other than send_msg")
    send_events = [item for item in items if item.get("type") == "send_msg"]
    if len(send_events) != 1:
        raise ValueError("scheduled marker run did not produce exactly one send_msg result")
    send_data = send_events[0].get("data")
    if (
        not isinstance(send_data, dict)
        or send_data.get("chat_id") != bot_chat_id
        or send_data.get("intent") != "done"
    ):
        raise ValueError("scheduled send_msg was not a done report to the Bot DM")
    for item in response_items:
        tool_calls = item["data"].get("tool_calls")
        if not isinstance(tool_calls, list):
            continue
        for call in tool_calls:
            if not isinstance(call, dict) or call.get("name") != "send_msg":
                raise ValueError("scheduled marker run returned a non-send_msg tool call")
            call_args = call.get("args")
            if not isinstance(call_args, dict) or call_args.get("intent") != "done":
                raise ValueError("scheduled send_msg tool call was not intent=done")
            if marker not in str(call_args.get("text", "")):
                raise ValueError("scheduled send_msg text did not contain the marker")
            if "chat_id" in call_args and call_args.get("chat_id") != bot_chat_id:
                raise ValueError("scheduled send_msg targeted a different chat")
            if "mentions" in call_args and call_args.get("mentions") not in ([], None):
                raise ValueError("scheduled send_msg included mentions")
            if "artifacts" in call_args and call_args.get("artifacts") not in ([], None):
                raise ValueError("scheduled send_msg included artifacts")
    history = chat_history(client, bot_chat_id)
    delivered = [
        message
        for message in history["messages"]
        if isinstance(message, dict)
        and message.get("chat_id") == bot_chat_id
        and message.get("assignment_id") == assignment_id
        and message.get("intent") == "done"
        and sender_is(message, kind="bot", bot_id=bot_id)
        and marker in message_text(message)
    ]
    if len(delivered) != 1:
        raise ValueError("scheduled done report was not delivered exactly once to the Bot DM")
    message = delivered[0]
    if not isinstance(send_data.get("message_id"), str) or message.get("id") != send_data.get("message_id"):
        raise ValueError("scheduled send_msg trace is not linked to the delivered DM message")
    if message.get("mentions") not in ([], None) or message.get("artifacts") not in ([], None):
        raise ValueError("scheduled done report contained mentions or artifacts")
    run_ids = sorted({item.get("run_id") for item in items if isinstance(item.get("run_id"), str)})
    return {
        "trace_items": len(items),
        "run_ids": run_ids,
        "marker_in_response": True,
        "send_msg": {"chat_id": bot_chat_id, "intent": "done", "message_id": send_data.get("message_id")},
        "delivered_message_id": message.get("id"),
    }


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production(health, client.base_url)
    state = bootstrap(client)
    bot = selected_bot(state, args.bot_id)
    bot_id = bot["id"]
    bot_chat_id = bot.get("dm_chat_id")
    if not isinstance(bot_chat_id, str) or not bot_chat_id:
        raise ValueError("selected Bot has no dm_chat_id")
    marker = unique_marker("macbot-e2e-s4-schedule")
    routine_id: str | None = None
    cleanup_assignment_id: str | None = None
    scenario_succeeded = False
    created_at = datetime.now(timezone.utc)

    def cleanup_own_state() -> list[str]:
        errors: list[str] = []
        if routine_id and not scenario_succeeded and cleanup_assignment_id:
            try:
                result = require_dict(
                    client.call("assignment.get", {"assignment_id": cleanup_assignment_id}),
                    "cleanup assignment.get result",
                )
                assignment = require_dict(result.get("assignment"), "cleanup assignment.get.assignment")
                if (
                    assignment.get("id") == cleanup_assignment_id
                    and assignment.get("bot_id") == bot_id
                    and assignment.get("status") not in TERMINAL_ASSIGNMENT_STATES
                ):
                    client.call("assignment.stop", {"assignment_id": cleanup_assignment_id})
            except Exception as exc:
                errors.append(f"assignment cleanup failed: {exc}")
        if routine_id:
            try:
                client.call("routine.delete", {"routine_id": routine_id})
            except Exception as exc:
                errors.append(f"routine cleanup failed: {exc}")
        return errors

    try:
        cron, target_local = near_future_schedule(created_at)
        created = require_dict(
            client.call(
                "routine.create",
                {
                    "bot_id": bot_id,
                    "name": f"{marker}-routine",
                    "instructions": (
                        f"S4 scheduled API marker {marker}. Reply with exactly {marker}, then call send_msg exactly once "
                        f"with intent=done and text containing {marker} to report this routine result. "
                        "Do not call any other tool, open a browser, modify files, or perform external actions. "
                        "The send_msg report must contain no mentions or artifacts."
                    ),
                    "schedules": [{"cron": cron, "label": "S4 near-future scheduled check"}],
                    "timezone": TIMEZONE,
                },
            ),
            "routine.create result",
        )
        routine = require_dict(created.get("routine"), "routine.create.routine")
        routine_id_value = routine.get("id")
        if not isinstance(routine_id_value, str) or not routine_id_value:
            raise ValueError("routine.create returned no routine id")
        routine_id = routine_id_value
        if routine.get("bot_id") != bot_id or routine.get("enabled") is not True:
            raise ValueError("routine.create returned the wrong Bot or disabled routine")
        if routine.get("timezone") != TIMEZONE or routine.get("schedules") != [{"cron": cron, "label": "S4 near-future scheduled check"}]:
            raise ValueError("routine.create did not preserve the requested schedule/timezone")
        next_run_at = routine.get("next_run_at")
        if not isinstance(next_run_at, str) or not next_run_at:
            raise ValueError("routine.create returned no next_run_at")
        try:
            next_run_time = datetime.fromisoformat(next_run_at.replace("Z", "+00:00"))
        except ValueError as exc:
            raise ValueError("routine.create returned an invalid next_run_at") from exc
        if next_run_time.tzinfo is None or next_run_time > created_at + timedelta(seconds=args.timeout):
            raise ValueError("routine next_run_at is outside this near-future API check window")

        def scheduled_run() -> dict[str, Any] | None:
            nonlocal cleanup_assignment_id
            runs = routine_runs(client, routine_id)
            matching = [
                run
                for run in runs
                if run.get("routine_id") == routine_id
                and run.get("trigger") == "schedule"
            ]
            if any(run.get("trigger") == "test" for run in runs):
                raise ValueError("scheduler scenario observed a test trigger")
            if not matching:
                return None
            if len(matching) != 1:
                raise ValueError("temporary routine produced multiple scheduled runs")
            run = matching[0]
            run_id = run.get("id")
            assignment_id = run.get("assignment_id")
            if not isinstance(run_id, str) or not isinstance(assignment_id, str):
                raise ValueError("scheduled routine run lacks id or assignment_id")
            cleanup_assignment_id = assignment_id
            if run.get("status") in {"failed", "skipped"}:
                raise ValueError(f"scheduled routine ended as {run.get('status')}: {run.get('error')}")
            return {"run": run, "run_id": run_id, "assignment_id": assignment_id}

        scheduled = wait_until(
            scheduled_run,
            timeout=args.timeout,
            interval=args.interval,
            description="scheduler-created routine run",
        )

        def completed() -> dict[str, Any] | None:
            evidence = scheduled_run()
            if evidence is None:
                return None
            runs = routine_runs(client, routine_id)
            run = next((item for item in runs if item.get("id") == evidence["run_id"]), evidence["run"])
            assignment_result = require_dict(
                client.call("assignment.get", {"assignment_id": evidence["assignment_id"]}),
                "assignment.get result",
            )
            assignment = require_dict(assignment_result.get("assignment"), "assignment.get.assignment")
            if assignment.get("id") != evidence["assignment_id"] or assignment.get("bot_id") != bot_id:
                raise ValueError("scheduled run assignment linkage is invalid")
            if assignment.get("origin_chat_id") != bot_chat_id:
                raise ValueError("scheduled run assignment does not target the selected Bot DM")
            if run.get("status") in {"failed", "skipped"} or assignment.get("status") in {"failed", "cancelled", "blocked"}:
                raise ValueError("scheduled routine or assignment failed")
            if run.get("status") != "done" or not isinstance(run.get("finished_at"), str):
                return None
            if assignment.get("status") != "done":
                return None
            trace = trace_evidence(client, evidence["assignment_id"], marker, bot_id, bot_chat_id)
            if trace is None:
                return None
            return {"run": run, "assignment": assignment, "trace": trace}

        completed_result = wait_until(
            completed,
            timeout=args.timeout,
            interval=args.interval,
            description="scheduled routine done/finished trace",
        )
        scenario_succeeded = True
        return {
            "scenario": "S4 scheduled routine API checks",
            "status": "PASS",
            "url": client.base_url,
            "health_version": health.get("version"),
            "bot_id": bot_id,
            "marker": marker,
            "routine": {"id": routine_id, "cron": cron, "timezone": TIMEZONE, "target_local": target_local, "next_run_at": next_run_at},
            "run": completed_result["run"],
            "assignment": {"id": completed_result["assignment"]["id"], "status": completed_result["assignment"].get("status")},
            "trace": completed_result["trace"],
            "note": "API checks only; scheduled notification, desktop/Android UI, browser login, and screen takeover remain manual.",
        }
    finally:
        active_exception = sys.exc_info()[1]
        cleanup_errors = cleanup_own_state()
        if cleanup_errors:
            message = "; ".join(cleanup_errors)
            if active_exception is None:
                raise ValueError(message)
            if hasattr(active_exception, "add_note"):
                active_exception.add_note(message)
            else:
                print(f"Cleanup also failed: {message}", file=sys.stderr)


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
