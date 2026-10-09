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
    return parser


def trace_evidence(client: Any, chat_id: str, marker: str, args: argparse.Namespace) -> dict[str, Any] | None:
    def check() -> dict[str, Any] | None:
        result = require_dict(
            client.call("trace.history", {"chat_id": chat_id, "tail": True, "limit": 500}),
            "trace.history result",
        )
        items = require_list(result.get("items"), "trace.history.items")
        types = {item.get("type") for item in items if isinstance(item, dict)}
        if not {"run.start", "llm.request", "llm.response", "tool.start", "tool.end", "run.end"} <= types:
            return None
        starts: dict[str, dict[str, Any]] = {}
        ends: dict[str, dict[str, Any]] = {}
        successful: set[str] = set()
        for item in items:
            if not isinstance(item, dict) or not isinstance(item.get("data"), dict):
                continue
            data = item["data"]
            if item.get("type") == "tool.start" and isinstance(data.get("call_id"), str):
                starts[data["call_id"]] = data
            if item.get("type") == "tool.end" and data.get("is_error") is False and isinstance(data.get("call_id"), str):
                successful.add(data["call_id"])
                ends[data["call_id"]] = data
        names: set[str] = set()
        marker_in_tool_args = False
        for call_id, data in starts.items():
            if call_id in successful and isinstance(data.get("name"), str):
                names.add(data["name"])
            if marker in json.dumps(data.get("args", {}), ensure_ascii=False):
                marker_in_tool_args = True
        if not {"read", "write", "bash"} <= names or not marker_in_tool_args:
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
            for item in items
        ):
            return None
        return {"items": len(items), "types": sorted(types), "successful_tools": sorted(names), "live": result.get("live")}

    return wait_until(check, timeout=args.timeout, interval=args.interval, description="S1 trace evidence")


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    bots = [bot for bot in require_list(state["bots"], "bootstrap.bots") if isinstance(bot, dict)]
    bot = next((bot for bot in bots if bot.get("id") == args.bot_id), None) if args.bot_id else None
    if args.bot_id and bot is None:
        raise ValueError("requested Bot is absent from bootstrap")
    if bot is None:
        bot = next((candidate for candidate in bots if candidate.get("is_main") is False), None)
    if not isinstance(bot, dict) or not isinstance(bot.get("id"), str):
        raise ValueError("S1 requires a non-main Bot; pass --bot-id explicitly")
    bot_id = bot["id"]
    chat_id = bot.get("dm_chat_id")
    if not isinstance(chat_id, str) or not chat_id:
        raise ValueError("selected Bot has no dm_chat_id")
    chats = [chat for chat in require_list(state["chats"], "bootstrap.chats") if isinstance(chat, dict)]
    chat = next((candidate for candidate in chats if candidate.get("id") == chat_id), None)
    if not isinstance(chat, dict) or chat.get("kind") != "direct":
        raise ValueError("selected Bot dm_chat_id does not identify a direct session")

    marker = unique_marker("macbot-e2e-s1")
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

    def bot_reply() -> dict[str, Any] | None:
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
        "marker": marker,
        "reply": reply,
        "trace": trace,
        "note": "API checks only; desktop/Android streaming, replay UI, and restart recovery remain manual.",
    }


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
