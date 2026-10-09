#!/usr/bin/env python3
"""S4 API checks for a routine execution and its durable assignment.

This is the RPC portion of S4 only.  Browser screen frames, takeover input,
Chrome login state, and Android notifications still need UI evidence.  The
scenario creates one uniquely named routine for the selected Bot and removes
that routine when the check ends; it does not enumerate or mutate unrelated
routines.
"""

from __future__ import annotations

import argparse
import sys
from typing import Any
from urllib.parse import urlsplit

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    client_from_args,
    ready_health,
    require_dict,
    require_list,
    RpcError,
    run_main,
    unique_marker,
    wait_until,
)


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--bot-id", required=True, help="Bot that owns the temporary routine")
    return parser


def require_production(health: dict[str, Any], base_url: str) -> None:
    if health.get("mock") is True or urlsplit(base_url).port == 7789:
        raise ValueError("S4 requires the production daemon; routine API checks do not count on mock")


def selected_bot(state: dict[str, Any], bot_id: str) -> dict[str, Any]:
    bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
    bot = next((item for item in bots if item.get("id") == bot_id), None)
    if not isinstance(bot, dict):
        raise ValueError("selected Bot is absent from bootstrap")
    return bot


def routine_runs(client: Any, routine_id: str) -> list[dict[str, Any]]:
    result = require_dict(client.call("routine.runs", {"routine_id": routine_id}), "routine.runs result")
    return [item for item in require_list(result.get("runs"), "routine.runs.runs") if isinstance(item, dict)]


def run_for_id(runs: list[dict[str, Any]], run_id: str) -> dict[str, Any] | None:
    return next((item for item in runs if item.get("id") == run_id), None)


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production(health, client.base_url)
    state = bootstrap(client)
    bot = selected_bot(state, args.bot_id)
    bot_id = bot["id"]
    marker = unique_marker("macbot-e2e-s4")
    routine_id: str | None = None

    try:
        created = require_dict(
            client.call(
                "routine.create",
                {
                    "bot_id": bot_id,
                    "name": f"{marker}-routine",
                    "instructions": (
                        f"S4 integration API check {marker}. Reply with the exact marker {marker}. "
                        "Do not open a browser, modify files, or perform external actions."
                    ),
                    "schedules": [{"cron": "*/5 * * * *", "label": "S4 integration check"}],
                    "timezone": "Asia/Shanghai",
                    "client_request_id": f"{marker}-create",
                },
            ),
            "routine.create result",
        )
        routine = require_dict(created.get("routine"), "routine.create.routine")
        candidate_routine_id = routine.get("id")
        if not isinstance(candidate_routine_id, str) or not candidate_routine_id:
            raise ValueError("routine.create returned no routine id")
        routine_id = candidate_routine_id
        if routine.get("bot_id") != bot_id or routine.get("enabled") is not True:
            raise ValueError("routine.create returned the wrong Bot or a disabled routine")
        if not isinstance(routine.get("next_run_at"), str) or not routine.get("next_run_at"):
            raise ValueError("routine.create returned no next_run_at")
        schedules = require_list(routine.get("schedules"), "routine.schedules")
        if not schedules or not all(isinstance(item, dict) for item in schedules):
            raise ValueError("routine.create returned no valid schedule")

        test_result = require_dict(
            client.call(
                "routine.test_run",
                {"routine_id": routine_id, "client_request_id": f"{marker}-test"},
            ),
            "routine.test_run result",
        )
        initial_run = require_dict(test_result.get("run"), "routine.test_run.run")
        run_id = initial_run.get("id")
        if not isinstance(run_id, str) or not run_id:
            raise ValueError("routine.test_run returned no run id")
        if initial_run.get("routine_id") != routine_id or initial_run.get("trigger") != "test":
            raise ValueError("routine.test_run returned the wrong routine or trigger")

        def actual_assignment() -> dict[str, Any] | None:
            run = run_for_id(routine_runs(client, routine_id), run_id)
            if not isinstance(run, dict):
                return None
            assignment_id = run.get("assignment_id")
            if not isinstance(assignment_id, str) or not assignment_id:
                return None
            try:
                assignment_result = require_dict(
                    client.call("assignment.get", {"assignment_id": assignment_id}),
                    "assignment.get result",
                )
            except RpcError as exc:
                if exc.code == "not_found":
                    return None
                raise
            assignment = require_dict(assignment_result.get("assignment"), "assignment.get.assignment")
            if assignment.get("id") != assignment_id:
                raise ValueError("routine run assignment id does not match assignment.get")
            if assignment.get("bot_id") != bot_id:
                raise ValueError("routine assignment belongs to a different Bot")
            return {"run": run, "assignment": assignment}

        wait_until(
            actual_assignment,
            timeout=args.timeout,
            interval=args.interval,
            description="S4 routine assignment linkage",
        )

        def completed_execution() -> dict[str, Any] | None:
            evidence = actual_assignment()
            if evidence is None:
                return None
            run = evidence["run"]
            assignment = evidence["assignment"]
            if run.get("status") in {"failed", "skipped"}:
                raise ValueError(f"routine test run ended as {run.get('status')}: {run.get('error')}")
            if run.get("status") != "done" or assignment.get("status") != "done":
                return None
            trace = require_dict(
                client.call("trace.history", {"assignment_id": assignment["id"], "tail": True, "limit": 500}),
                "trace.history result",
            )
            items = require_list(trace.get("items"), "trace.history.items")
            if not any(
                isinstance(item, dict)
                and item.get("type") == "run.end"
                and isinstance(item.get("data"), dict)
                and item["data"].get("status") == "done"
                for item in items
            ):
                return None
            return {"run": run, "assignment": assignment, "trace_items": len(items)}

        completed = wait_until(
            completed_execution,
            timeout=args.timeout,
            interval=args.interval,
            description="S4 routine assignment completion",
        )
        return {
            "scenario": "S4 routine API checks",
            "status": "PASS",
            "url": client.base_url,
            "health_version": health.get("version"),
            "bot_id": bot_id,
            "marker": marker,
            "routine_id": routine_id,
            "run": completed["run"],
            "assignment": {
                "id": completed["assignment"]["id"],
                "status": completed["assignment"].get("status"),
                "trace_items": completed["trace_items"],
            },
            "screen": {
                "documented_path": "/ws/screen",
                "rpc_screen_get": False,
                "note": "PROTOCOL defines no screen.get/control_take/control_release/tabs RPC; JPEG, takeover UI, and notifications remain manual.",
            },
            "note": "API checks only; Chrome login state, screen JPEG/takeover, Android notification, and desktop/mobile evidence remain manual.",
        }
    finally:
        if routine_id:
            client.call("routine.delete", {"routine_id": routine_id, "client_request_id": f"{marker}-cleanup"})


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
