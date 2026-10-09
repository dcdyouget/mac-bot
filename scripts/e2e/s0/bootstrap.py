#!/usr/bin/env python3
"""S0 acceptance: connect to a Host and read the bootstrap session list."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
from typing import Any


HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import password_for_url  # noqa: E402
from rpc import PollTimeout, RpcClient, RpcError, RpcTransportError  # noqa: E402


def safe_error(args: argparse.Namespace, exc: BaseException) -> str:
    message = str(exc)
    secret = args.password if args.password is not None else password_for_url(args.url)
    if secret:
        message = message.replace(secret, "[REDACTED]")
    return message


def validate_bootstrap(result: Any) -> dict[str, Any]:
    """Validate the stable S0 portion of ``bootstrap`` and return a summary."""

    if not isinstance(result, dict):
        raise ValueError("bootstrap result is not an object")
    required = ("seq", "hello", "bots", "chats", "projects", "settings", "pending")
    missing = [key for key in required if key not in result]
    if missing:
        raise ValueError("bootstrap result is missing: " + ", ".join(missing))
    if not isinstance(result["seq"], int) or isinstance(result["seq"], bool) or result["seq"] < 0:
        raise ValueError("bootstrap.seq must be a non-negative integer")
    hello = result["hello"]
    if not isinstance(hello, dict):
        raise ValueError("bootstrap.hello is not an object")
    if hello.get("protocol") != 1:
        raise ValueError(f"unsupported protocol in bootstrap.hello: {hello.get('protocol')!r}")
    for key in ("bots", "chats", "projects"):
        if not isinstance(result[key], list):
            raise ValueError(f"bootstrap.{key} must be an array")
    if not result["chats"]:
        raise ValueError("bootstrap.chats must contain the main session")
    main_bots = [bot for bot in result["bots"] if isinstance(bot, dict) and bot.get("is_main") is True]
    if len(main_bots) != 1:
        raise ValueError(f"bootstrap must contain exactly one main bot (found {len(main_bots)})")
    main_dm_chat_id = main_bots[0].get("dm_chat_id")
    chat_ids = {chat.get("id") for chat in result["chats"] if isinstance(chat, dict)}
    if not isinstance(main_dm_chat_id, str) or not main_dm_chat_id or main_dm_chat_id not in chat_ids:
        raise ValueError("main bot dm_chat_id must identify a chat in bootstrap.chats")
    if not isinstance(result["pending"], dict):
        raise ValueError("bootstrap.pending must be an object")
    # `chats` is the protocol's session list. It is valid for a fresh Host to
    # have zero sessions, so presence and type are checked rather than count.
    return {
        "seq": result["seq"],
        "protocol": hello["protocol"],
        "server_version": hello.get("server_version"),
        "node_id": hello.get("node_id"),
        "bots": len(result["bots"]),
        "main_bot_id": main_bots[0].get("id"),
        "main_dm_chat_id": main_dm_chat_id,
        "sessions": len(result["chats"]),
        "projects": len(result["projects"]),
    }


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--url",
        default=os.environ.get("MACBOT_URL", "http://127.0.0.1:7789"),
        help="Host address (default: MACBOT_URL or 127.0.0.1:7789)",
    )
    parser.add_argument(
        "--password",
        default=None,
        help="Host password (default: dev for port 7789; otherwise MACBOT_PASSWORD or ~/.macbot-dev-password)",
    )
    parser.add_argument("--timeout", type=float, default=20.0, help="RPC readiness timeout in seconds")
    parser.add_argument("--interval", type=float, default=0.5, help="Polling interval in seconds")
    parser.add_argument("--json", action="store_true", help="Print the result summary as JSON")
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> dict[str, Any]:
    client = RpcClient(args.url, args.password if args.password is not None else password_for_url(args.url))
    health = client.poll_health(
        lambda value: value.get("ok") is True
        and value.get("protocol") == 1
        and value.get("setup_required") is False,
        timeout=args.timeout,
        interval=args.interval,
    )
    result = client.poll(
        "bootstrap",
        {},
        lambda value: True,
        timeout=args.timeout,
        interval=args.interval,
        description="bootstrap readiness",
    )
    summary = validate_bootstrap(result)
    summary["url"] = client.base_url
    summary["health_version"] = health.get("version")
    return summary


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    try:
        summary = run(args)
    except (RpcError, RpcTransportError, PollTimeout, ValueError) as exc:
        print(f"S0 bootstrap: FAIL: {safe_error(args, exc)}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(summary, ensure_ascii=False, sort_keys=True))
    else:
        print(
            "S0 bootstrap: PASS "
            f"url={summary['url']} protocol={summary['protocol']} "
            f"bots={summary['bots']} sessions={summary['sessions']} projects={summary['projects']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
