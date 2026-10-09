#!/usr/bin/env python3
"""S3 API checks: skill CRUD, usage/search responses, and cross-chat memory."""

from __future__ import annotations

import argparse
import sys
from typing import Any

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    iso_window,
    message_text,
    ready_health,
    require_production_host,
    require_dict,
    require_list,
    RpcError,
    run_main,
    sender_is,
    unique_marker,
    wait_until,
)


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--bot-id", help="Non-main Bot used for cross-chat memory; default first non-main")
    return parser


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    bots = [bot for bot in require_list(state["bots"], "bootstrap.bots") if isinstance(bot, dict)]
    bot = next((item for item in bots if item.get("id") == args.bot_id), None) if args.bot_id else None
    if args.bot_id and bot is None:
        raise ValueError("requested Bot is absent from bootstrap")
    if bot is None:
        bot = next((item for item in bots if item.get("is_main") is False), None)
    main = next((item for item in bots if item.get("is_main") is True), None)
    if not isinstance(bot, dict) or not isinstance(main, dict):
        raise ValueError("S3 needs a main Bot and a non-main Bot")
    bot_chat_id = bot.get("dm_chat_id")
    main_chat_id = main.get("dm_chat_id")
    if not isinstance(bot_chat_id, str) or not isinstance(main_chat_id, str) or bot_chat_id == main_chat_id:
        raise ValueError("S3 requires distinct main and non-main DM sessions")
    chats = [chat for chat in require_list(state["chats"], "bootstrap.chats") if isinstance(chat, dict)]
    chats_by_id = {chat.get("id"): chat for chat in chats}
    bot_chat = chats_by_id.get(bot_chat_id)
    main_chat = chats_by_id.get(main_chat_id)
    if not isinstance(bot_chat, dict) or bot_chat.get("kind") != "direct":
        raise ValueError("selected non-main Bot dm_chat_id does not identify a direct session")
    if not isinstance(main_chat, dict) or main_chat.get("kind") != "main":
        raise ValueError("main Bot dm_chat_id does not identify the main session")
    marker = unique_marker("macbot-e2e-s3")

    # CRUD a uniquely named user skill and verify every documented transition.
    skill_name = marker.lower()
    content_v1 = (f"---\nname: {skill_name}\ndescription: Integration CRUD validation skill\n---\n\n"
                  f"# {skill_name}\n\nUse marker {marker} when validating this skill.\n")
    content_v2 = content_v1 + "\nUpdated by the S3 CRUD check.\n"
    created = require_dict(client.call("skill.create", {"name": skill_name, "content": content_v1}), "skill.create result")
    created_skill = require_dict(created.get("skill"), "skill.create.skill")
    if created_skill.get("name") != skill_name or created_skill.get("source") != "user":
        raise ValueError("skill.create did not return a user skill")
    detail = require_dict(client.call("skill.get", {"name": skill_name}), "skill.get result").get("skill")
    detail = require_dict(detail, "skill.get.skill")
    if detail.get("content") != content_v1:
        raise ValueError("skill.get did not return the created content")
    updated = require_dict(client.call("skill.update", {"name": skill_name, "content": content_v2}), "skill.update result")
    if require_dict(updated.get("skill"), "skill.update.skill").get("name") != skill_name:
        raise ValueError("skill.update returned a different skill")
    updated_detail = require_dict(client.call("skill.get", {"name": skill_name}), "skill.get after update result").get("skill")
    updated_detail = require_dict(updated_detail, "skill.get after update.skill")
    if updated_detail.get("content") != content_v2:
        raise ValueError("skill.get after update did not return the updated content")
    disabled = require_dict(client.call("skill.set_enabled", {"name": skill_name, "enabled": False}), "skill.set_enabled result")
    if require_dict(disabled.get("skill"), "skill.set_enabled.skill").get("enabled") is not False:
        raise ValueError("skill.set_enabled(false) was not reflected")
    enabled = require_dict(client.call("skill.set_enabled", {"name": skill_name, "enabled": True}), "skill.set_enabled result")
    if require_dict(enabled.get("skill"), "skill.set_enabled.skill").get("enabled") is not True:
        raise ValueError("skill.set_enabled(true) was not reflected")
    client.call("skill.delete", {"name": skill_name})
    try:
        client.call("skill.get", {"name": skill_name})
    except RpcError as exc:
        if exc.code != "not_found":
            raise ValueError(f"deleted skill did not return not_found: {exc}") from exc
    else:
        raise ValueError("deleted skill remained readable")

    start, end = iso_window()
    summary = require_dict(client.call("usage.summary", {"from": start, "to": end}), "usage.summary result")
    for key in ("current", "previous"):
        current = require_dict(summary.get(key), f"usage.summary.{key}")
        for field in ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "requests", "tasks_done"):
            if not isinstance(current.get(field), (int, float)) or isinstance(current.get(field), bool):
                raise ValueError(f"usage.summary.{key}.{field} is not numeric")
        if current.get("cost") is not None and (
            not isinstance(current.get("cost"), (int, float)) or isinstance(current.get("cost"), bool)
        ):
            raise ValueError(f"usage.summary.{key}.cost must be numeric or null")
    heatmap = require_dict(
        client.call("usage.heatmap", {"mode": "calendar", "from": start, "to": end, "metric": "tokens"}),
        "usage.heatmap result",
    )
    if not isinstance(heatmap.get("days"), list) or not isinstance(heatmap.get("thresholds"), list) or len(heatmap["thresholds"]) != 3:
        raise ValueError("usage.heatmap does not match the calendar response shape")
    if any(not isinstance(value, (int, float)) or isinstance(value, bool) for value in heatmap["thresholds"]):
        raise ValueError("usage.heatmap.thresholds must contain three numbers")
    for day in heatmap["days"]:
        day = require_dict(day, "usage.heatmap.days entry")
        if not isinstance(day.get("date"), str):
            raise ValueError("usage.heatmap day.date must be a string")
        for field in ("value", "tokens", "requests"):
            if not isinstance(day.get(field), (int, float)) or isinstance(day.get(field), bool):
                raise ValueError(f"usage.heatmap day.{field} is not numeric")
        if day.get("cost") is not None and (
            not isinstance(day.get("cost"), (int, float)) or isinstance(day.get("cost"), bool)
        ):
            raise ValueError("usage.heatmap day.cost must be numeric or null")
        if day.get("top_bot_id") is not None and not isinstance(day.get("top_bot_id"), str):
            raise ValueError("usage.heatmap day.top_bot_id must be string or null")
    timeseries = require_dict(
        client.call("usage.timeseries", {"from": start, "to": end, "granularity": "auto", "dimension": "bot", "metric": "tokens"}),
        "usage.timeseries result",
    )
    if timeseries.get("granularity") not in {"hour", "day", "week"} or not isinstance(timeseries.get("buckets"), list) or not isinstance(timeseries.get("series"), list):
        raise ValueError("usage.timeseries does not match the response shape")
    breakdown = require_dict(
        client.call("usage.breakdown", {"from": start, "to": end, "dimension": "bot"}),
        "usage.breakdown result",
    )
    if not isinstance(breakdown.get("rows"), list):
        raise ValueError("usage.breakdown.rows must be an array")

    search_message = require_dict(
        client.call("chat.send", {"chat_id": main_chat_id, "text": f"S3 search marker {marker}", "mentions": []}),
        "search marker chat.send result",
    ).get("message")
    search_message = require_dict(search_message, "search marker message")
    results = wait_until(
        lambda: next(
            (
                result
                for result in require_list(
                    require_dict(client.call("search", {"query": marker, "kinds": ["message"], "limit": 20}), "search result").get("results"),
                    "search.results",
                )
                if isinstance(result, dict) and marker in f"{result.get('title', '')} {result.get('snippet', '')}"
            ),
            None,
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S3 message search index",
    )

    preference = f"{marker}-PREFERENCE-email-only"
    remembered = require_dict(
        client.call(
            "chat.send",
            {"chat_id": bot_chat_id, "text": f"Remember this user preference exactly: {preference}.", "mentions": []},
        ),
        "preference chat.send result",
    ).get("message")
    remembered = require_dict(remembered, "preference message")
    remembered_seq = remembered.get("seq")
    if not isinstance(remembered_seq, int):
        raise ValueError("preference message has no seq")
    wait_until(
        lambda: next(
            (
                message
                for message in chat_history(client, bot_chat_id, after_seq=remembered_seq)["messages"]
                if isinstance(message, dict) and sender_is(message, kind="bot", bot_id=bot["id"]) and preference in message_text(message)
            ),
            None,
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S3 preference acknowledgement",
    )
    recall = require_dict(
        client.call("chat.send", {"chat_id": main_chat_id, "text": "What exact preference did I ask you to remember?", "mentions": []}),
        "recall chat.send result",
    ).get("message")
    recall = require_dict(recall, "recall message")
    recall_seq = recall.get("seq")
    if not isinstance(recall_seq, int):
        raise ValueError("recall message has no seq")
    recall_result = wait_until(
        lambda: next(
            (
                {"message_id": message.get("id"), "text": message_text(message)}
                for message in chat_history(client, main_chat_id, after_seq=recall_seq)["messages"]
                if isinstance(message, dict) and sender_is(message, kind="bot", bot_id=main["id"]) and preference in message_text(message)
            ),
            None,
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S3 cross-chat preference recall",
    )
    return {
        "scenario": "S3 skills usage search memory",
        "status": "PASS",
        "url": client.base_url,
        "health_version": health.get("version"),
        "marker": marker,
        "skill": {"name": skill_name, "deleted": True},
        "usage": {"summary": True, "heatmap_days": len(heatmap["days"]), "timeseries_series": len(timeseries["series"]), "breakdown_rows": len(breakdown["rows"])},
        "search": results,
        "cross_chat_memory": recall_result,
        "note": "API checks only; dashboard parity, skills UI, and mobile/desktop screenshots remain manual.",
    }


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
