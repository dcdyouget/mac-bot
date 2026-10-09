#!/usr/bin/env python3
"""Strict production API regression for one background bash call.

This creates one disposable non-main Bot and sends one uniquely marked request.
It approves only the matching ``bash`` approval whose complete argument object is
``{command, background: true}``.  The script never kills a process or sends a
cleanup model request.  It is API evidence only; it does not prove desktop or
Android rendering.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import select
import sys
import time
import uuid
from typing import Any

HERE = pathlib.Path(__file__).resolve()
ROOT = HERE.parents[3]
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
    require_production_host,
    run_main,
    sender_is,
    unique_marker,
    wait_until,
)
from rpc import RpcClient  # noqa: E402
from s0.connection_replay import MiniWebSocket, WsError  # noqa: E402


EVIDENCE: pathlib.Path | None = None
SLEEP_SECONDS = 12.0


def _digest(value: Any) -> str:
    raw = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()


def _safe_id(value: Any) -> str | None:
    return value if isinstance(value, str) and value else None


def _approval_snapshot(client: RpcClient) -> dict[str, Any]:
    result = require_dict(client.call("approval.list", {"state": ["pending"]}), "approval.list result")
    rows = require_list(result.get("approvals"), "approval.list.approvals")
    out: list[dict[str, Any]] = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        out.append(
            {
                "id": _safe_id(row.get("id")),
                "bot_id": _safe_id(row.get("bot_id")),
                "assignment_id": _safe_id(row.get("assignment_id")),
                "chat_id": _safe_id(row.get("chat_id")),
                "tool": row.get("tool") if isinstance(row.get("tool"), str) else None,
                "risk": row.get("risk") if isinstance(row.get("risk"), str) else None,
                "state": row.get("state") if isinstance(row.get("state"), str) else None,
            }
        )
    return {"count": len(out), "ids": sorted(item["id"] for item in out if item["id"])}


def _new_bot(client: RpcClient, marker: str) -> tuple[str, str, dict[str, Any]]:
    name = f"macbot-e2e-background-{marker.rsplit('-', 1)[-1]}"
    result = require_dict(
        client.call(
            "bot.create",
            {
                "name": name,
                "model": None,
                "tools": {
                    "files": False,
                    "bash": True,
                    "browser": False,
                    "subagent": False,
                    "web": False,
                    "mcp": False,
                },
                "client_request_id": f"{marker}-bot",
            },
        ),
        "bot.create result",
    )
    bot = require_dict(result.get("bot"), "bot.create.bot")
    chat = require_dict(result.get("dm_chat"), "bot.create.dm_chat")
    bot_id = _safe_id(bot.get("id"))
    chat_id = _safe_id(chat.get("id"))
    if not bot_id or not chat_id:
        raise ValueError("bot.create returned no Bot or DM id")
    if bot.get("is_main") is not False or bot.get("model") is not None:
        raise ValueError("background regression Bot is not a non-main model-inheriting Bot")
    expected_tools = {"files": False, "bash": True, "browser": False, "subagent": False, "web": False, "mcp": False}
    if bot.get("tools") != expected_tools:
        raise ValueError("background regression Bot tools are not bash-only")
    if chat.get("id") != bot.get("dm_chat_id") or chat.get("kind") != "direct":
        raise ValueError("background regression Bot does not have its direct dm_chat_id")
    return bot_id, chat_id, {"id": bot_id, "name": bot.get("name"), "model": None, "tools": expected_tools}


def _trace_items(client: RpcClient, chat_id: str) -> list[dict[str, Any]]:
    result = require_dict(
        client.call("trace.history", {"chat_id": chat_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    return [item for item in require_list(result.get("items"), "trace.history.items") if isinstance(item, dict)]


def _run_for_marker(items: list[dict[str, Any]], marker: str) -> tuple[str, list[dict[str, Any]]]:
    runs: set[str] = set()
    for item in items:
        if item.get("type") != "run.start" or not isinstance(item.get("run_id"), str):
            continue
        data = item.get("data")
        if marker in json.dumps(data, ensure_ascii=False, sort_keys=True):
            runs.add(item["run_id"])
    # The user marker is also in the request message, so a run.start may not
    # echo it.  In that case the unique chat-local run is selected only when
    # there is exactly one recent run containing our exact bash command.
    if len(runs) == 1:
        run_id = next(iter(runs))
        return run_id, [item for item in items if item.get("run_id") == run_id]
    command_runs: set[str] = set()
    for item in items:
        if item.get("type") != "tool.start" or not isinstance(item.get("run_id"), str):
            continue
        data = item.get("data")
        if isinstance(data, dict) and data.get("name") == "bash":
            args = data.get("args")
            if isinstance(args, dict) and isinstance(args.get("command"), str) and marker in args["command"]:
                command_runs.add(item["run_id"])
    if len(command_runs) != 1:
        raise ValueError("could not identify exactly one marker background run")
    run_id = next(iter(command_runs))
    return run_id, [item for item in items if item.get("run_id") == run_id]


def _exact_background_start(run_items: list[dict[str, Any]], expected_command: str) -> dict[str, Any]:
    starts: dict[str, dict[str, Any]] = {}
    for item in run_items:
        if item.get("type") != "tool.start" or not isinstance(item.get("data"), dict):
            continue
        data = item["data"]
        args = data.get("args")
        if data.get("name") == "bash" and isinstance(args, dict) and args.get("command") == expected_command:
            call_id = data.get("call_id")
            if isinstance(call_id, str):
                starts[call_id] = data
    if len(starts) != 1:
        raise ValueError(f"expected one exact bash background tool.start, found {len(starts)}")
    start = next(iter(starts.values()))
    args = start.get("args")
    if not isinstance(args, dict) or set(args) != {"command", "background"} or args.get("background") is not True:
        raise ValueError("bash approval candidate does not have exactly command plus background=true")
    if not isinstance(start.get("call_id"), str):
        raise ValueError("background tool.start has no call_id")
    return start


def _approval_for_call(
    client: RpcClient,
    *,
    bot_id: str,
    chat_id: str,
    assignment_id: str | None,
    expected_command: str,
) -> dict[str, Any] | None:
    result = require_dict(client.call("approval.list", {"state": ["pending"]}), "approval.list result")
    rows = require_list(result.get("approvals"), "approval.list.approvals")
    matches: list[dict[str, Any]] = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        if (
            row.get("bot_id") != bot_id
            or row.get("chat_id") != chat_id
            or row.get("assignment_id") != assignment_id
            or row.get("tool") != "bash"
            or row.get("risk") != "exec"
            or row.get("state") != "pending"
        ):
            continue
        detail = row.get("detail")
        try:
            args = json.loads(detail) if isinstance(detail, str) else None
        except json.JSONDecodeError:
            continue
        if isinstance(args, dict) and set(args) == {"command", "background"} and args == {"command": expected_command, "background": True}:
            matches.append(row)
    if len(matches) > 1:
        raise ValueError("multiple exact background approvals matched; refusing ambiguity")
    return matches[0] if matches else None


def _ws_drain(ws: MiniWebSocket, events: list[dict[str, Any]], *, seconds: float, reply_id: str | None = None, marker: str = "") -> bool:
    deadline = time.monotonic() + seconds
    observed = False
    while time.monotonic() < deadline:
        if ws._sock is None:
            break
        ready, _, _ = select.select([ws._sock], [], [], min(0.25, max(0.0, deadline - time.monotonic())))
        if not ready:
            continue
        try:
            frame = ws.recv_json()
        except (WsError, OSError):
            break
        if frame.get("kind") != "evt":
            continue
        event = frame.get("event") if isinstance(frame.get("event"), str) else "unknown"
        record: dict[str, Any] = {"event": event, "seq": frame.get("seq") if isinstance(frame.get("seq"), int) else None}
        data = frame.get("data")
        if isinstance(data, dict):
            message = data.get("message")
            if isinstance(message, dict):
                mid = _safe_id(message.get("id"))
                record["message_id"] = mid
                record["marker_seen"] = marker in message_text(message)
                if reply_id and mid == reply_id and record["marker_seen"]:
                    observed = True
            if isinstance(data.get("item"), dict):
                record["item_type"] = data["item"].get("type") if isinstance(data["item"].get("type"), str) else None
        events.append(record)
    return observed


def _history_reply(client: RpcClient, chat_id: str, bot_id: str, sent_seq: int, marker: str) -> dict[str, Any] | None:
    result = chat_history(client, chat_id, after_seq=sent_seq)
    matches: list[dict[str, Any]] = []
    for message in result["messages"]:
        if not isinstance(message, dict) or not isinstance(message.get("seq"), int) or message["seq"] <= sent_seq:
            continue
        if sender_is(message, kind="bot", bot_id=bot_id) and message.get("streaming") is False and marker in message_text(message):
            matches.append(message)
    if not matches:
        return None
    message = matches[-1]
    text = message_text(message)
    return {
        "id": _safe_id(message.get("id")),
        "seq": message.get("seq"),
        "streaming": message.get("streaming"),
        "blocks": len(message.get("blocks")) if isinstance(message.get("blocks"), list) else 0,
        "markdown_nonempty": bool(text.strip()),
        "marker_seen": marker in text,
        "text_sha256": hashlib.sha256(text.encode()).hexdigest(),
    }


def _job_metadata(run_id: str) -> list[dict[str, Any]]:
    root = pathlib.Path.home() / "MacBot" / "data" / "jobs"
    found: list[dict[str, Any]] = []
    if not root.is_dir():
        return found
    for path in root.glob("*.json"):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            continue
        if not isinstance(data, dict):
            continue
        checkpoint = data.get("checkpoint")
        if not isinstance(checkpoint, dict) or checkpoint.get("run_id") != run_id:
            continue
        found.append({
            "file": path.name,
            "status": data.get("status") if isinstance(data.get("status"), str) else None,
            "checkpoint_keys": sorted(str(key) for key in checkpoint),
            "pending_tools": bool(checkpoint.get("pending_tools")),
            "wait_intent": checkpoint.get("wait_intent") if isinstance(checkpoint.get("wait_intent"), str) else None,
        })
    return found


def _write_evidence(value: dict[str, Any]) -> None:
    if EVIDENCE is None:
        raise ValueError("background evidence path is not initialized")
    EVIDENCE.parent.mkdir(parents=True, exist_ok=True)
    EVIDENCE.write_text(json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--evidence", type=pathlib.Path, help="New evidence file; an existing file is never overwritten")
    return parser


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    global EVIDENCE
    EVIDENCE = args.evidence or ROOT / "docs" / "progress" / "S1" / (unique_marker("background") + ".json")
    if EVIDENCE.exists():
        raise ValueError("evidence file already exists; use a new path")
    client = client_from_args(args)
    started_at = time.time()
    marker = unique_marker("macbot-e2e-background")
    expected_command = f"printf '{marker}-start'; sleep {int(SLEEP_SECONDS)}; printf '{marker}-end'"
    evidence: dict[str, Any] = {
        "scenario": "S1 production background bash regression",
        "status": "FAIL",
        "full_s1_pass": False,
        "url": client.base_url,
        "marker": marker,
        "command_sha256": _digest(expected_command),
        "command_length": len(expected_command),
        "background": True,
        "sleep_seconds": SLEEP_SECONDS,
        "started_at_epoch": started_at,
        "old_pending_before": None,
        "old_pending_after": None,
        "ws_events": [],
    }
    ws: MiniWebSocket | None = None
    try:
        health = ready_health(client, args)
        require_production_host(client, health)
        evidence["health_version"] = health.get("version")
        evidence["old_pending_before"] = _approval_snapshot(client)
        state = bootstrap(client)
        bot_id, chat_id, bot_meta = _new_bot(client, marker)
        evidence["bot"] = bot_meta
        state = bootstrap(client)
        chats = [row for row in require_list(state.get("chats"), "bootstrap.chats") if isinstance(row, dict)]
        direct = next((row for row in chats if row.get("id") == chat_id), None)
        if not isinstance(direct, dict) or direct.get("kind") != "direct":
            raise ValueError("created Bot direct chat is not present in bootstrap")

        password = client.password
        if not password:
            raise ValueError("production WebSocket requires a Host password")
        ws_url = client.base_url.replace("http://", "ws://", 1).replace("https://", "wss://", 1)
        ws = MiniWebSocket(ws_url, password, timeout=min(args.timeout, 15.0))
        hello = ws.recv_json()
        if hello.get("kind") != "evt" or hello.get("event") != "hello":
            raise WsError("main WebSocket did not begin with hello")
        hello_data = require_dict(hello.get("data"), "hello.data")
        if hello_data.get("protocol") != 1 or not isinstance(hello_data.get("last_seq"), int):
            raise WsError("main WebSocket hello has invalid protocol/cursor")
        resumed, frames = ws.request(
            "session.resume",
            {
                "last_seq": hello_data["last_seq"],
                "client": {
                    "platform": "macos",
                    "app_version": "e2e-background",
                    "device_name": "e2e-background",
                    "device_id": f"{marker}-device",
                },
            },
            timeout=min(args.timeout, 20.0),
        )
        frames, sync_seq = ws.until_sync_done(frames, timeout=min(args.timeout, 20.0))
        evidence["ws_baseline"] = {"mode": resumed.get("mode"), "sync_seq": sync_seq}
        evidence["ws_coverage"] = {
            "main_session_resume": True,
            "trace_subscribe": False,
            "tool_output_routing_assertion": False,
            "qualification": "Main /ws event frames only; trace tool.output requires an explicit trace.subscribe stream and is not inferred from this connection.",
        }

        prompt = (
            f"Background regression marker {marker}. Call bash exactly once. "
            f"Use exactly this JSON argument object and no other keys: {{\"command\": \"{expected_command}\", \"background\": true}}. "
            "Do not call files, browser, web, subagent, MCP, or any other tool. Do not alter files, network, environment, or working directory. "
            "After the background command has completed and its stdout is available, reply with a short Markdown sentence containing the marker."
        )
        sent_result = require_dict(
            client.call("chat.send", {"chat_id": chat_id, "text": prompt, "mentions": [], "client_request_id": f"{marker}-chat"}),
            "chat.send result",
        )
        sent = require_dict(sent_result.get("message"), "chat.send.message")
        sent_seq = sent.get("seq")
        sent_id = _safe_id(sent.get("id"))
        if not sent_id or not isinstance(sent_seq, int):
            raise ValueError("chat.send did not return canonical id/seq")
        evidence["sent"] = {"id": sent_id, "seq": sent_seq}

        run_id: str | None = None
        run_items: list[dict[str, Any]] = []
        start: dict[str, Any] | None = None
        approval: dict[str, Any] | None = None

        def find_start() -> dict[str, Any] | None:
            nonlocal run_id, run_items, start
            items = _trace_items(client, chat_id)
            try:
                candidate_run, candidate_items = _run_for_marker(items, marker)
                candidate_start = _exact_background_start(candidate_items, expected_command)
            except ValueError:
                return None
            run_id, run_items, start = candidate_run, candidate_items, candidate_start
            return candidate_start

        wait_until(find_start, timeout=args.timeout, interval=args.interval, description="background bash tool.start")
        assert run_id is not None and start is not None
        evidence["run"] = {"run_id": run_id, "start_call_id": start.get("call_id"), "start_args_keys": ["background", "command"]}
        assignment_id = start.get("assignment_id") if isinstance(start.get("assignment_id"), str) else None
        evidence["run"]["assignment_id"] = assignment_id

        def find_approval() -> dict[str, Any] | None:
            nonlocal approval
            approval = _approval_for_call(
                client,
                bot_id=bot_id,
                chat_id=chat_id,
                assignment_id=assignment_id,
                expected_command=expected_command,
            )
            return approval

        wait_until(find_approval, timeout=args.timeout, interval=args.interval, description="exact background approval")
        assert approval is not None
        approval_id = _safe_id(approval.get("id"))
        if not approval_id:
            raise ValueError("matching background approval has no id")
        decision = require_dict(client.call("approval.decide", {"approval_id": approval_id, "decision": "allow_once"}), "approval.decide result")
        decided = require_dict(decision.get("approval"), "approval.decide.approval")
        if decided.get("id") != approval_id or decided.get("state") != "allowed_once":
            raise ValueError("exact background approval did not become allowed_once")
        evidence["approval"] = {"id": approval_id, "bot_id": bot_id, "chat_id": chat_id, "assignment_id": assignment_id, "tool": "bash", "risk": "exec", "state": "allowed_once"}

        start_monotonic = time.monotonic()
        tool_end: dict[str, Any] | None = None
        run_end: dict[str, Any] | None = None
        reply: dict[str, Any] | None = None
        tool_end_seen_at: float | None = None
        while time.monotonic() - start_monotonic < args.timeout:
            run_items = [item for item in _trace_items(client, chat_id) if item.get("run_id") == run_id]
            for item in run_items:
                data = item.get("data")
                if not isinstance(data, dict):
                    continue
                if item.get("type") == "tool.end" and data.get("call_id") == start.get("call_id"):
                    tool_end = data
                    if tool_end_seen_at is None:
                        tool_end_seen_at = time.monotonic()
                if item.get("type") == "run.end":
                    run_end = data
            reply = _history_reply(client, chat_id, bot_id, sent_seq, marker)
            # Keep the process alive for its whole requested sleep after the
            # trace end is visible. This proves the script did not kill the
            # background PID, while avoiding a false failure when persistence
            # of the trace is slower than the command itself.
            if tool_end_seen_at is not None and time.monotonic() - tool_end_seen_at >= SLEEP_SECONDS:
                break
            _ws_drain(ws, evidence["ws_events"], seconds=min(0.25, args.interval), marker=marker)
            time.sleep(max(0.05, args.interval))
        if tool_end is None:
            raise ValueError("background bash tool.end was not observed")
        evidence["tool_end"] = {
            "call_id": start.get("call_id"),
            "is_error": tool_end.get("is_error"),
            "details_keys": sorted(tool_end.get("details", {}).keys()) if isinstance(tool_end.get("details"), dict) else [],
            "preview_present": isinstance(tool_end.get("preview"), str) and bool(tool_end.get("preview")),
            "duration_ms": tool_end.get("duration_ms") if isinstance(tool_end.get("duration_ms"), (int, float)) else None,
            "trace_observed_elapsed_seconds": round((tool_end_seen_at or time.monotonic()) - start_monotonic, 3),
        }
        if tool_end.get("is_error") is not False:
            raise ValueError("background bash tool.end reported an error")
        duration_ms = evidence["tool_end"]["duration_ms"]
        if isinstance(duration_ms, (int, float)) and duration_ms >= SLEEP_SECONDS * 1000:
            raise ValueError("background bash tool.end duration was not shorter than the 12-second command")
        unexpected: dict[str, dict[str, Any]] = {}
        for item in run_items:
            if item.get("type") != "tool.start" or not isinstance(item.get("data"), dict):
                continue
            data = item["data"]
            call_id = data.get("call_id")
            if not isinstance(call_id, str) or call_id == start.get("call_id"):
                continue
            args = data.get("args")
            unexpected[call_id] = {
                "name": data.get("name") if isinstance(data.get("name"), str) else None,
                "args_keys": sorted(args) if isinstance(args, dict) else [],
                "is_error": next(
                    (
                        end.get("data", {}).get("is_error")
                        for end in run_items
                        if end.get("type") == "tool.end"
                        and isinstance(end.get("data"), dict)
                        and end["data"].get("call_id") == call_id
                    ),
                    None,
                ),
            }
        evidence["unexpected_tools"] = list(unexpected.values())
        if unexpected:
            raise ValueError("marker run invoked an additional tool after the exact background bash call")
        if run_end is None or run_end.get("status") != "done":
            raise ValueError("background run did not reach run.end=done")
        evidence["run"]["end_status"] = run_end.get("status")
        if reply is None or not reply.get("markdown_nonempty") or not reply.get("marker_seen"):
            raise ValueError("final Bot Markdown reply is missing or does not contain the marker")
        evidence["reply"] = reply
        evidence["jobs"] = _job_metadata(run_id)
        if not evidence["jobs"]:
            raise ValueError("no durable job checkpoint was found for the marker run")
        _ws_drain(ws, evidence["ws_events"], seconds=1.0, reply_id=reply.get("id"), marker=marker)
        evidence["main_ws_output_observed"] = any(item.get("marker_seen") for item in evidence["ws_events"])
        if not evidence["main_ws_output_observed"]:
            raise ValueError("main WebSocket did not expose the marker reply after the run completed")
        evidence["status"] = "PASS"
        evidence["note"] = "Strict API/trace/WS evidence only; no S1/S2 full-stage or client UI PASS."
        return evidence
    except Exception as exc:
        evidence["error_type"] = type(exc).__name__
        evidence["error"] = str(exc)[:300]
        evidence["status"] = "FAIL"
        raise
    finally:
        if ws is not None:
            ws.close()
        try:
            evidence["old_pending_after"] = _approval_snapshot(client)
        except Exception:
            evidence["old_pending_after"] = {"unavailable": True}
        evidence["finished_at_epoch"] = time.time()
        _write_evidence(evidence)


def main(args: argparse.Namespace) -> int:
    try:
        summary = scenario(args)
    except Exception as exc:
        print(f"API checks: FAIL: {type(exc).__name__}: {str(exc)[:300]}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(summary, ensure_ascii=False, sort_keys=True))
    else:
        print(f"API checks: {summary.get('status')} background regression (API checks only)")
    return 0 if summary.get("status") == "PASS" else 1


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(main(parser.parse_args()))
