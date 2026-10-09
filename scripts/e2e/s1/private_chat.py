#!/usr/bin/env python3
"""S1 API checks: private chat, file/bash execution evidence, and trace history."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    approve_exact_pending,
    bootstrap,
    chat_history,
    client_from_args,
    message_text,
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
    parser.add_argument("--bot-id", help="Bot to test; default is the first non-main Bot")
    parser.add_argument(
        "--create-worker",
        action="store_true",
        help="Create a fresh non-main Bot when the production Host has no worker yet",
    )
    parser.add_argument(
        "--approve-test-tools-once",
        "--approve-test-bash-once",
        dest="approve_test_tools_once",
        action="store_true",
        help="Allow once only the marker run's exact write/bash calls (old bash alias kept); otherwise leave approvals for manual review",
    )
    return parser


def approve_marker_calls(
    client: Any,
    chat_id: str,
    bot_id: str,
    marker: str,
    path: str,
    approved_ids: set[str],
) -> list[dict[str, Any]]:
    """Approve only this marker run's exact unsafe file and bash calls."""

    result = require_dict(
        client.call("trace.history", {"chat_id": chat_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    items = [item for item in require_list(result.get("items"), "trace.history.items") if isinstance(item, dict)]
    marker_runs = {
        item.get("run_id")
        for item in items
        if item.get("type") == "tool.start"
        and isinstance(item.get("run_id"), str)
        and isinstance(item.get("data"), dict)
        and marker in json.dumps(item["data"].get("args", {}), ensure_ascii=False)
    }
    if len(marker_runs) != 1:
        return []
    run_id = next(iter(marker_runs))
    successful_calls = {
        item["data"].get("call_id")
        for item in items
        if item.get("run_id") == run_id
        and item.get("type") == "tool.end"
        and isinstance(item.get("data"), dict)
        and item["data"].get("is_error") is False
    }
    expected: dict[str, dict[str, Any]] = {}
    for item in items:
        if item.get("run_id") != run_id or item.get("type") != "tool.start":
            continue
        data = item.get("data")
        if not isinstance(data, dict) or data.get("name") not in {"write", "bash"}:
            continue
        call_id = data.get("call_id")
        args = data.get("args")
        if not isinstance(call_id, str) or not isinstance(args, dict):
            raise ValueError("marker run has an unsafe tool call without call_id/args")
        if call_id in successful_calls:
            continue
        name = data["name"]
        if name == "write":
            expected_args = {"path": path, "content": marker}
            risk = "write"
        else:
            expected_args = {"command": f"cat {path}"}
            risk = "exec"
        if any(args.get(key) != value for key, value in expected_args.items()):
            raise ValueError(f"marker run {name} call does not match the scripted path/command")
        expected[call_id] = {"tool": name, "risk": risk, "args": expected_args}
    return approve_exact_pending(
        client,
        run_id=run_id,
        bot_id=bot_id,
        chat_id=chat_id,
        expected_calls=expected,
        approved_ids=approved_ids,
    )


def trace_evidence(client: Any, chat_id: str, marker: str, args: argparse.Namespace) -> dict[str, Any] | None:
    def check() -> dict[str, Any] | None:
        result = require_dict(
            client.call("trace.history", {"chat_id": chat_id, "tail": True, "limit": 500}),
            "trace.history result",
        )
        items = require_list(result.get("items"), "trace.history.items")
        marker_run_ids = {
            item.get("run_id")
            for item in items
            if isinstance(item, dict)
            and item.get("type") == "tool.start"
            and isinstance(item.get("run_id"), str)
            and isinstance(item.get("data"), dict)
            and marker in json.dumps(item["data"].get("args", {}), ensure_ascii=False)
        }
        if len(marker_run_ids) != 1:
            return None
        marker_run_id = next(iter(marker_run_ids))
        run_items = [item for item in items if isinstance(item, dict) and item.get("run_id") == marker_run_id]
        types = {item.get("type") for item in run_items}
        if not {"run.start", "llm.request", "llm.response", "tool.start", "tool.end", "run.end"} <= types:
            return None
        starts: dict[str, dict[str, Any]] = {}
        ends: dict[str, dict[str, Any]] = {}
        successful: set[str] = set()
        for item in run_items:
            if not isinstance(item, dict) or not isinstance(item.get("data"), dict):
                continue
            data = item["data"]
            if item.get("type") == "tool.start" and isinstance(data.get("call_id"), str):
                starts[data["call_id"]] = data
            if item.get("type") == "tool.end" and data.get("is_error") is False and isinstance(data.get("call_id"), str):
                successful.add(data["call_id"])
                ends[data["call_id"]] = data
        names: set[str] = set()
        for call_id, data in starts.items():
            if call_id in successful and isinstance(data.get("name"), str):
                names.add(data["name"])
        if not {"read", "write", "bash"} <= names:
            return None
        # `read` and `bash` must have returned the marker, proving the tool
        # calls actually observed the file content rather than only receiving
        # a prompt that mentioned it.
        for required_name in ("read", "bash"):
            matching = [call_id for call_id, data in starts.items() if data.get("name") == required_name and call_id in successful]
            if not matching or not any(marker in json.dumps(ends[call_id], ensure_ascii=False) for call_id in matching):
                return None
        if not any(
            isinstance(item, dict)
            and item.get("type") == "run.end"
            and isinstance(item.get("data"), dict)
            and item["data"].get("status") == "done"
            for item in run_items
        ):
            return None
        return {
            "run_id": marker_run_id,
            "items": len(run_items),
            "types": sorted(types),
            "successful_tools": sorted(names),
            "live": result.get("live"),
        }

    return wait_until(check, timeout=args.timeout, interval=args.interval, description="S1 trace evidence")


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    marker = unique_marker("macbot-e2e-s1")
    if args.bot_id and args.create_worker:
        raise ValueError("--bot-id and --create-worker are mutually exclusive")
    bots = [bot for bot in require_list(state["bots"], "bootstrap.bots") if isinstance(bot, dict)]
    created_worker = False
    if args.create_worker:
        worker_name = f"macbot-e2e-s1-{marker.rsplit('-', 1)[-1]}"
        created = require_dict(client.call("bot.create", {"name": worker_name}), "bot.create result")
        created_bot = require_dict(created.get("bot"), "bot.create.bot")
        created_chat = require_dict(created.get("dm_chat"), "bot.create.dm_chat")
        if created_bot.get("is_main") is not False or created_chat.get("kind") != "direct":
            raise ValueError("bot.create did not return a non-main Bot with a direct DM")
        if created_bot.get("dm_chat_id") != created_chat.get("id"):
            raise ValueError("bot.create Bot dm_chat_id does not match dm_chat.id")
        # Re-read bootstrap so the scenario only proceeds after the new Bot and
        # its session are visible through the same contract clients consume.
        state = bootstrap(client)
        bots = [bot for bot in require_list(state["bots"], "bootstrap.bots") if isinstance(bot, dict)]
        bot = next((item for item in bots if item.get("id") == created_bot.get("id")), None)
        created_worker = True
    else:
        bot = next((bot for bot in bots if bot.get("id") == args.bot_id), None) if args.bot_id else None
        if args.bot_id and bot is None:
            raise ValueError("requested Bot is absent from bootstrap")
        if bot is None:
            bot = next((candidate for candidate in bots if candidate.get("is_main") is False), None)
    if not isinstance(bot, dict) or not isinstance(bot.get("id"), str):
        raise ValueError("S1 requires a non-main Bot; pass --bot-id or use --create-worker")
    bot_id = bot["id"]
    chat_id = bot.get("dm_chat_id")
    if not isinstance(chat_id, str) or not chat_id:
        raise ValueError("selected Bot has no dm_chat_id")
    chats = [chat for chat in require_list(state["chats"], "bootstrap.chats") if isinstance(chat, dict)]
    chat = next((candidate for candidate in chats if candidate.get("id") == chat_id), None)
    if not isinstance(chat, dict) or chat.get("kind") != "direct":
        raise ValueError("selected Bot dm_chat_id does not identify a direct session")

    path = f"e2e/{marker}.txt"
    text = (
        f"S1 API evidence marker {marker}. Use the file tools to write EXACTLY {marker} to {path}, "
        f"then use the read tool to read {path}, then use bash to run `cat {path}`. "
        f"Only after all three successful tool calls, reply with {marker} and the path."
    )
    sent = require_dict(
        client.call("chat.send", {"chat_id": chat_id, "text": text, "mentions": []}),
        "chat.send result",
    ).get("message")
    sent = require_dict(sent, "chat.send.message")
    sent_seq = sent.get("seq")
    if not isinstance(sent_seq, int):
        raise ValueError("sent message has no numeric seq")

    approved_ids: set[str] = set()
    approval_evidence: list[dict[str, Any]] = []

    def bot_reply() -> dict[str, Any] | None:
        if args.approve_test_tools_once:
            approval_evidence.extend(
                approve_marker_calls(client, chat_id, bot_id, marker, path, approved_ids)
            )
        history = chat_history(client, chat_id, after_seq=sent_seq)
        for message in history["messages"]:
            if (
                isinstance(message, dict)
                and sender_is(message, kind="bot", bot_id=bot_id)
                and marker in message_text(message)
                and path in message_text(message)
            ):
                return {"history_messages": len(history["messages"]), "reply_id": message.get("id")}
        return None

    reply = wait_until(bot_reply, timeout=args.timeout, interval=args.interval, description="S1 bot reply")
    trace = trace_evidence(client, chat_id, marker, args)
    return {
        "scenario": "S1 private chat",
        "status": "PASS",
        "url": client.base_url,
        "health_version": health.get("version"),
        "bot_id": bot_id,
        "chat_id": chat_id,
        "created_worker": created_worker,
        "marker": marker,
        "reply": reply,
        "trace": trace,
        "approvals": approval_evidence,
        "note": "API checks only; desktop/Android streaming, replay UI, and restart recovery remain manual.",
    }


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
