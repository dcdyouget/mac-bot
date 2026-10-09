#!/usr/bin/env python3
"""Shared helpers for the API-only integration scenarios."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import sys
import time
import uuid
from typing import Any, Callable, Iterable, Mapping
from urllib.parse import urlsplit

HERE = pathlib.Path(__file__).resolve()
if str(HERE.parent) not in sys.path:
    sys.path.insert(0, str(HERE.parent))

from rpc import PollTimeout, RpcClient, RpcError, RpcTransportError  # noqa: E402


def add_connection_args(parser: argparse.ArgumentParser, *, default_url: str = "http://127.0.0.1:7788") -> None:
    parser.add_argument("--url", default=os.environ.get("MACBOT_URL", default_url))
    parser.add_argument(
        "--password",
        default=None,
        help="Host password (default: dev for port 7789; otherwise MACBOT_PASSWORD or ~/.macbot-dev-password)",
    )
    parser.add_argument("--timeout", type=float, default=120.0, help="Scenario wait timeout in seconds")
    parser.add_argument("--interval", type=float, default=0.5, help="Polling interval in seconds")
    parser.add_argument("--json", action="store_true", help="Print the result as JSON")


def password_for_url(url: str) -> str | None:
    parsed = urlsplit(url if "://" in url else "http://" + url)
    if parsed.port == 7789:
        return "dev"
    if os.environ.get("MACBOT_PASSWORD") is not None:
        return os.environ["MACBOT_PASSWORD"]
    try:
        return (pathlib.Path.home() / ".macbot-dev-password").read_text(encoding="utf-8").strip() or None
    except FileNotFoundError:
        return None


def client_from_args(args: argparse.Namespace) -> RpcClient:
    return RpcClient(args.url, args.password if args.password is not None else password_for_url(args.url))


def wait_until(
    check: Callable[[], Any],
    *,
    timeout: float,
    interval: float,
    description: str,
) -> Any:
    """Poll an arbitrary collection of RPC calls until it returns a truthy value."""

    if timeout <= 0 or interval < 0:
        raise ValueError("timeout must be positive and interval must be non-negative")
    deadline = time.monotonic() + timeout
    last_transport_error: RpcTransportError | None = None
    while True:
        try:
            value = check()
            if value:
                return value
        except RpcTransportError as exc:
            last_transport_error = exc
        if time.monotonic() >= deadline:
            if last_transport_error:
                raise PollTimeout(f"timed out waiting for {description}: {last_transport_error}") from last_transport_error
            raise PollTimeout(f"timed out waiting for {description}")
        time.sleep(min(interval, max(0.0, deadline - time.monotonic())))


def ready_health(client: RpcClient, args: argparse.Namespace) -> dict[str, Any]:
    return client.poll_health(
        lambda value: value.get("ok") is True
        and value.get("protocol") == 1
        and value.get("setup_required") is False,
        timeout=args.timeout,
        interval=args.interval,
    )


def require_production_host(client: RpcClient, health: dict[str, Any]) -> None:
    if urlsplit(client.base_url).port == 7789 or health.get("mock") is True:
        raise ValueError("S1–S3 require the production daemon; use mock providers there if needed")


def bootstrap(client: RpcClient) -> dict[str, Any]:
    value = client.call("bootstrap", {})
    if not isinstance(value, dict):
        raise ValueError("bootstrap result is not an object")
    for key in ("seq", "hello", "bots", "chats", "projects", "settings", "pending"):
        if key not in value:
            raise ValueError(f"bootstrap result is missing {key}")
    if not isinstance(value["bots"], list) or not isinstance(value["chats"], list):
        raise ValueError("bootstrap bots/chats must be arrays")
    return value


def require_dict(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise ValueError(f"{label} must be an array")
    return value


def message_text(message: Mapping[str, Any]) -> str:
    parts: list[str] = []
    fallback = message.get("fallback_text")
    if isinstance(fallback, str):
        parts.append(fallback)
    for block in message.get("blocks", []) if isinstance(message.get("blocks"), list) else []:
        if not isinstance(block, dict):
            continue
        for key in ("markdown", "text", "summary", "reason"):
            if isinstance(block.get(key), str):
                parts.append(block[key])
    return "\n".join(parts)


def sender_is(message: Mapping[str, Any], *, kind: str, bot_id: str | None = None) -> bool:
    sender = message.get("sender")
    if not isinstance(sender, dict) or sender.get("kind") != kind:
        return False
    return bot_id is None or sender.get("bot_id") == bot_id


def chat_history(client: RpcClient, chat_id: str, *, after_seq: int | None = None) -> dict[str, Any]:
    params: dict[str, Any] = {"chat_id": chat_id, "limit": 100}
    if after_seq is not None:
        params["after_seq"] = after_seq
    result = require_dict(client.call("chat.history", params), "chat.history result")
    require_list(result.get("messages"), "chat.history.messages")
    if not isinstance(result.get("has_more"), bool):
        raise ValueError("chat.history.has_more must be boolean")
    return result


def unique_marker(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:12]}"


def iso_window(hours: int = 24) -> tuple[str, str]:
    end = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
    start = end - dt.timedelta(hours=hours)
    return start.isoformat().replace("+00:00", "Z"), end.isoformat().replace("+00:00", "Z")


def parse_time(value: Any) -> dt.datetime | None:
    if not isinstance(value, str):
        return None
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def json_dump(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def safe_error(args: argparse.Namespace, exc: BaseException) -> str:
    """Keep auth material out of failure output even if a server echoes it."""

    message = str(exc)
    secret = args.password if getattr(args, "password", None) is not None else password_for_url(args.url)
    if secret:
        message = message.replace(secret, "[REDACTED]")
    return message


def run_main(main: Callable[[argparse.Namespace], dict[str, Any]], args: argparse.Namespace) -> int:
    try:
        summary = main(args)
    except (RpcError, RpcTransportError, PollTimeout, ValueError) as exc:
        print(f"API checks: FAIL: {safe_error(args, exc)}", file=sys.stderr)
        return 1
    if args.json:
        print(json_dump(summary))
    else:
        print(f"API checks: PASS {summary.get('scenario', '')} ({summary.get('note', 'API checks only')})")
    return 0
