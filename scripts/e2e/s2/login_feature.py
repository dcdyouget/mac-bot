#!/usr/bin/env python3
"""S2 API checks for the login project, parallel work, and a delivered steer."""

from __future__ import annotations

import argparse
import datetime as dt
import sys
from typing import Any

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    message_text,
    parse_time,
    ready_health,
    require_production_host,
    require_dict,
    require_list,
    run_main,
    sender_is,
    unique_marker,
    wait_until,
)


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--product-bot-id", required=True)
    parser.add_argument("--coding-bot-id", required=True)
    parser.add_argument("--test-bot-id", required=True)
    return parser


def project_get(client: Any, project_id: str) -> tuple[dict[str, Any], dict[str, Any]]:
    result = require_dict(client.call("project.get", {"project_id": project_id}), "project.get result")
    return require_dict(result.get("project"), "project.get.project"), require_dict(result.get("announcement"), "project.get.announcement")


def assignments(client: Any, project_id: str) -> list[dict[str, Any]]:
    result = require_dict(client.call("assignment.list", {"project_id": project_id, "limit": 100}), "assignment.list result")
    return [item for item in require_list(result.get("items"), "assignment.list.items") if isinstance(item, dict)]


def done_chain(items: list[dict[str, Any]], product_id: str, coding_id: str, test_id: str) -> bool:
    bot_ids = {product_id, coding_id, test_id}
    done = [item for item in items if item.get("status") == "done"]
    if not bot_ids <= {item.get("bot_id") for item in done}:
        return False
    # Every downstream assignment must carry a parent, proving a handoff was
    # materialized rather than merely having three unrelated tasks.
    product = [item for item in done if item.get("bot_id") == product_id and isinstance(item.get("id"), str)]
    coding = [item for item in done if item.get("bot_id") == coding_id]
    testing = [item for item in done if item.get("bot_id") == test_id]
    product_ids = {item["id"] for item in product}
    coding_ids = {item["id"] for item in coding if isinstance(item.get("id"), str)}
    return (
        bool(product and coding and testing)
        and any(item.get("parent_assignment_id") in product_ids for item in coding)
        and any(item.get("parent_assignment_id") in coding_ids for item in testing)
    )


def completion_messages(client: Any, chat_id: str, bot_ids: set[str], args: argparse.Namespace) -> bool:
    history = chat_history(client, chat_id)
    completed = {
        message.get("sender", {}).get("bot_id")
        for message in history["messages"]
        if isinstance(message, dict)
        and isinstance(message.get("intent"), str)
        and message.get("intent") == "done"
        and isinstance(message.get("sender"), dict)
        and message["sender"].get("kind") == "bot"
        and any(isinstance(block, dict) and block.get("type") == "completion" for block in message.get("blocks", []))
    }
    return bot_ids <= completed


def interval(item: dict[str, Any]) -> tuple[dt.datetime, dt.datetime] | None:
    start = parse_time(item.get("started_at"))
    if start is None:
        return None
    finish = parse_time(item.get("finished_at")) or dt.datetime.now(dt.timezone.utc)
    return start, finish


def overlap(items_a: list[dict[str, Any]], items_b: list[dict[str, Any]]) -> bool:
    for left in (interval(item) for item in items_a):
        if left is None:
            continue
        for right in (interval(item) for item in items_b):
            if right is not None and max(left[0], right[0]) < min(left[1], right[1]):
                return True
    return False


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    bot_ids = {args.product_bot_id, args.coding_bot_id, args.test_bot_id}
    if len(bot_ids) != 3:
        raise ValueError("S2 needs three distinct product/coding/test Bot IDs")
    known_ids = {bot.get("id") for bot in state["bots"] if isinstance(bot, dict)}
    if not bot_ids <= known_ids:
        raise ValueError("one or more S2 Bot IDs are absent from bootstrap")
    marker = unique_marker("macbot-e2e-s2")
    projects: list[dict[str, Any]] = []
    for suffix in ("login", "parallel"):
        result = require_dict(
            client.call(
                "project.create",
                {
                    "name": f"{marker}-{suffix}",
                    "goal": f"{marker}: implement and validate email login",
                    "member_bot_ids": [args.product_bot_id, args.coding_bot_id, args.test_bot_id],
                    "flow": ["产品", "编码", "测试"],
                },
            ),
            "project.create result",
        )
        project = require_dict(result.get("project"), "project.create.project")
        chat = require_dict(result.get("chat"), "project.create.chat")
        if project.get("status") != "active" or chat.get("kind") != "project":
            raise ValueError("project.create did not produce an active project chat")
        if project.get("chat_id") != chat.get("id"):
            raise ValueError("project.chat_id does not match created project chat")
        projects.append({"project": project, "chat": chat})

    first_id = projects[0]["project"]["id"]
    second_id = projects[1]["project"]["id"]
    def active_pair() -> dict[str, Any] | None:
        first_items = assignments(client, first_id)
        second_items = assignments(client, second_id)
        if not first_items or not second_items or not overlap(first_items, second_items):
            return None
        return {"first": first_items, "second": second_items}

    pair = wait_until(active_pair, timeout=args.timeout, interval=args.interval, description="S2 overlapping project work")
    coding_assignment = wait_until(
        lambda: next((item for item in assignments(client, first_id) if item.get("bot_id") == args.coding_bot_id and item.get("status") == "working"), None),
        timeout=args.timeout,
        interval=args.interval,
        description="S2 working coding assignment for steer",
    )
    first_chat_id = projects[0]["chat"]["id"]
    steer_text = f"{marker}: steer evidence — only implement email login, do not add phone login."
    sent = require_dict(
        client.call(
            "chat.send",
            {
                "chat_id": first_chat_id,
                "text": steer_text,
                "mentions": [{"kind": "bot", "bot_id": args.coding_bot_id, "instruction": None}],
            },
        ),
        "steer chat.send result",
    )
    steer_message = require_dict(sent.get("message"), "steer message")
    steer_id = steer_message.get("id")
    if not isinstance(steer_id, str):
        raise ValueError("steer message has no id")

    def steer_read() -> dict[str, Any] | None:
        steer_seq = steer_message.get("seq")
        if not isinstance(steer_seq, int):
            raise ValueError("steer message has no numeric seq")
        history = chat_history(client, first_chat_id, after_seq=max(0, steer_seq - 1))
        matching = next((item for item in history["messages"] if isinstance(item, dict) and item.get("id") == steer_id), None)
        if not isinstance(matching, dict):
            return None
        deliveries = [item for item in matching.get("delivery", []) if isinstance(item, dict) and item.get("bot_id") == args.coding_bot_id]
        if not deliveries or deliveries[-1].get("state") != "read":
            return None
        assignment = require_dict(client.call("assignment.get", {"assignment_id": coding_assignment.get("id")}), "assignment.get result").get("assignment")
        assignment = require_dict(assignment, "assignment.get.assignment")
        steers = [item for item in assignment.get("steers", []) if isinstance(item, dict) and item.get("message_id") == steer_id]
        if not steers or steers[-1].get("applied_at") is None:
            return None
        trace = require_dict(client.call("trace.history", {"assignment_id": coding_assignment.get("id"), "tail": True, "limit": 500}), "trace.history result")
        if not any(
            isinstance(item, dict)
            and item.get("type") == "steer"
            and isinstance(item.get("data"), dict)
            and item["data"].get("message_id") == steer_id
            for item in require_list(trace.get("items"), "trace.history.items")
        ):
            return None
        return {"message_id": steer_id, "delivery": deliveries[-1], "assignment_id": coding_assignment.get("id")}

    steer = wait_until(steer_read, timeout=args.timeout, interval=args.interval, description="S2 steer read/applied")

    def both_review() -> dict[str, Any] | None:
        first_project, _ = project_get(client, first_id)
        second_project, _ = project_get(client, second_id)
        first_items = assignments(client, first_id)
        second_items = assignments(client, second_id)
        if first_project.get("status") != "review" or second_project.get("status") != "review":
            return None
        if not done_chain(first_items, args.product_bot_id, args.coding_bot_id, args.test_bot_id) or not done_chain(second_items, args.product_bot_id, args.coding_bot_id, args.test_bot_id):
            return None
        if not completion_messages(client, projects[0]["chat"]["id"], bot_ids, args):
            return None
        if not completion_messages(client, projects[1]["chat"]["id"], bot_ids, args):
            return None
        return {"first": first_items, "second": second_items}

    completed = wait_until(both_review, timeout=args.timeout, interval=args.interval, description="S2 handoff chain and review state")
    return {
        "scenario": "S2 login feature",
        "status": "PASS",
        "url": client.base_url,
        "health_version": health.get("version"),
        "marker": marker,
        "project_ids": [first_id, second_id],
        "overlap_assignments": [len(pair["first"]), len(pair["second"])],
        "steer": steer,
        "review_assignments": [len(completed["first"]), len(completed["second"])],
        "note": "API checks only; two-client UI, screenshots, and user review confirmation remain manual.",
    }


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
