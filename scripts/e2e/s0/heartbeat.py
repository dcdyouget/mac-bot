#!/usr/bin/env python3
"""Verify HTTP and fallback RPC heartbeats on one production WebSocket."""

from __future__ import annotations

import argparse
import datetime as dt
from pathlib import Path
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import add_connection_args, client_from_args, ready_health, require_dict, require_production_host, run_main
from connection_replay import MiniWebSocket, WsError


def validate_ping(result: dict) -> str:
    value = result.get("server_time")
    if not isinstance(value, str):
        raise WsError("ping.server_time is not a timestamp")
    try:
        timestamp = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise WsError("ping.server_time is not an RFC3339 timestamp") from exc
    if timestamp.tzinfo is None:
        raise WsError("ping.server_time lacks a timezone")
    return value


def scenario(args: argparse.Namespace) -> dict:
    if args.duration < 60 or not 0 < args.heartbeat_interval <= 20:
        raise ValueError("duration must be >=60s and heartbeat interval must be in (0,20]s")
    http = client_from_args(args)
    require_production_host(http, ready_health(http, args))
    http_time = validate_ping(require_dict(http.call("ping", {}), "HTTP ping"))
    ws = MiniWebSocket(http.base_url, http.password, timeout=args.timeout)
    samples = []
    try:
        hello_frame = ws.recv_json()
        if hello_frame.get("kind") != "evt" or hello_frame.get("event") != "hello":
            raise WsError("first WebSocket frame is not hello")
        hello = require_dict(hello_frame.get("data"), "hello")
        if hello.get("protocol") != 1 or not isinstance(hello.get("last_seq"), int):
            raise WsError("hello protocol/cursor is invalid")
        resumed, frames = ws.request("session.resume", {
            "last_seq": hello["last_seq"],
            "client": {"platform": "android", "app_version": "e2e-heartbeat",
                       "device_name": "macbot-e2e-heartbeat", "device_id": str(uuid.uuid4())},
        }, timeout=args.timeout)
        if resumed.get("mode") not in {"reset", "replay"}:
            raise WsError("session.resume mode is invalid")
        ws.until_sync_done(frames, timeout=args.timeout)
        start = time.monotonic()
        # No reconnect path: every sample must complete on this same socket.
        while True:
            result, _ = ws.request("ping", {}, timeout=args.timeout)
            elapsed = time.monotonic() - start
            samples.append({"elapsed_seconds": round(elapsed, 3), "server_time": validate_ping(result)})
            if elapsed >= args.duration:
                break
            time.sleep(min(args.heartbeat_interval, args.duration - elapsed))
    finally:
        ws.close()
    return {"scenario": "production-rpc-heartbeat", "status": "PASS_API_TRANSPORT",
            "http_server_time": http_time, "node_id": hello.get("node_id"), "samples": samples,
            "connection_count": 1, "reconnects": 0, "full_s1_pass": False,
            "note": "No model/tool calls; independent transport probe, not installed Android notification or UI acceptance."}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--duration", type=float, default=60)
    parser.add_argument("--heartbeat-interval", type=float, default=20)
    raise SystemExit(run_main(scenario, parser.parse_args()))
