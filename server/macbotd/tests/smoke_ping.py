#!/usr/bin/env python3
"""Keep one production WebSocket alive across protocol fallback heartbeats."""

from __future__ import annotations

import argparse
import asyncio
from datetime import datetime, timezone
import json
from pathlib import Path
import time

from smoke_collaboration import Daemon, rpc
from smoke_trace import connect_ws, ws_request


def validate_time(result: dict, before: float) -> str:
    assert set(result) == {"server_time"}, result
    parsed = datetime.fromisoformat(result["server_time"].replace("Z", "+00:00"))
    assert parsed.tzinfo is not None
    assert before - 2 <= parsed.timestamp() <= datetime.now(timezone.utc).timestamp() + 2
    return result["server_time"]


async def acceptance(args: argparse.Namespace) -> dict:
    before = time.time()
    http_time = validate_time(rpc(args.url, args.password, "ping"), before)
    ws_url = args.url.replace("http://", "ws://").replace("https://", "wss://")
    ws = await connect_ws(ws_url, args.password)
    started = time.monotonic()
    times = []
    try:
        # No reconnect helper exists here: every request must use this same
        # connection, including after three 20-second fallback intervals.
        for index in range(args.rounds):
            if index:
                await asyncio.sleep(args.interval)
            before = time.time()
            result = await ws_request(ws, "ping", {})
            times.append(validate_time(result, before))
        boot = await ws_request(ws, "bootstrap", {})
        assert boot["hello"]["node_id"]
        return {"ok": True, "http_server_time": http_time, "ws_server_times": times,
            "same_connection": True, "rounds": args.rounds, "interval_seconds": args.interval,
            "elapsed_seconds": round(time.monotonic() - started, 3), "node_id": boot["hello"]["node_id"]}
    finally:
        await ws.close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7862")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--browser-bin")
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--interval", type=float, default=20)
    args = parser.parse_args()
    assert args.rounds >= 4 and args.interval >= 20
    assert not args.home.exists(), "use a fresh isolated home"
    daemon = Daemon(args)
    try:
        daemon.start()
        print(json.dumps(asyncio.run(acceptance(args)), indent=2), flush=True)
    finally:
        daemon.close()


if __name__ == "__main__":
    main()
