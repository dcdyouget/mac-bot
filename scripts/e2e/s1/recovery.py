#!/usr/bin/env python3
"""S1 durable recovery acceptance for the installed production LaunchAgent.

The destructive part is opt-in.  Without ``--restart-service`` this script
refuses before connecting or creating a task.  With the flag it only kills a
process after launchctl label ``com.macbot.server`` and the single TCP listener
on port 7788 resolve to the same non-mock PID.  The task is a private-chat run
with a unique marker; the local checkpoint is read only from
``$MACBOT_HOME/data/jobs/*.json``.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import time
from typing import Any
from urllib.parse import urlsplit

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    approve_exact_pending,
    bootstrap,
    chat_history,
    client_from_args,
    message_text,
    ready_health,
    require_dict,
    require_list,
    run_main,
    sender_is,
    unique_marker,
    wait_until,
)


SERVICE_LABEL = "com.macbot.server"
SERVICE_PORT = 7788


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--bot-id", help="Existing non-main Bot to test")
    group.add_argument("--create-worker", action="store_true", help="Create a unique non-main Bot for this run")
    parser.add_argument("--home", default=os.environ.get("MACBOT_HOME", str(Path.home() / "MacBot")))
    parser.add_argument("--restart-service", action="store_true", help="Allow killing the verified 7788 LaunchAgent PID")
    parser.add_argument(
        "--approve-test-tools-once",
        "--approve-test-bash-once",
        dest="approve_test_tools_once",
        action="store_true",
        help="Allow once only this marker run's exact bash call (old bash alias kept); otherwise leave approval for manual review",
    )
    parser.add_argument("--restart-timeout", type=float, default=60.0)
    return parser


def command_output(argv: list[str]) -> str:
    try:
        result = subprocess.run(argv, check=False, capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return ""
    return result.stdout


def launchagent_pid() -> int | None:
    uid = str(os.getuid())
    output = command_output(["/bin/launchctl", "print", f"gui/{uid}/{SERVICE_LABEL}"])
    match = re.search(r"(?m)^\s*pid\s*=\s*(\d+)\s*$", output)
    return int(match.group(1)) if match else None


def listener_pids() -> set[int]:
    output = command_output([
        "/usr/sbin/lsof",
        "-nP",
        f"-iTCP:{SERVICE_PORT}",
        "-sTCP:LISTEN",
        "-t",
    ])
    pids: set[int] = set()
    for line in output.splitlines():
        if line.strip().isdigit():
            pids.add(int(line.strip()))
    return pids


def process_command(pid: int) -> str:
    return command_output(["/bin/ps", "-ww", "-p", str(pid), "-o", "command="]).strip()


def valid_server_command(command: str) -> bool:
    try:
        tokens = shlex.split(command)
    except ValueError:
        return False
    if not any(Path(token).name == "macbotd" for token in tokens):
        return False
    if "--mock" in tokens:
        return False
    return any(tokens[index:index + 2] == ["--port", str(SERVICE_PORT)] for index in range(len(tokens) - 1))


def service_identity() -> dict[str, Any]:
    agent_pid = launchagent_pid()
    pids = listener_pids()
    if agent_pid is None:
        raise ValueError(f"LaunchAgent {SERVICE_LABEL} has no running PID")
    if pids != {agent_pid}:
        raise ValueError(f"port {SERVICE_PORT} is not owned exclusively by LaunchAgent {SERVICE_LABEL}")
    command = process_command(agent_pid)
    if not valid_server_command(command):
        raise ValueError("verified 7788 PID is not the non-mock macbotd command")
    return {"pid": agent_pid, "listener_pids": sorted(pids), "executable": "macbotd", "label": SERVICE_LABEL, "port": SERVICE_PORT}


def process_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


def local_checkpoint(home: Path, run_id: str, trigger_time: float) -> dict[str, Any] | None:
    jobs_dir = home / "data" / "jobs"
    try:
        paths = sorted(jobs_dir.glob("*.json"))
    except OSError:
        return None
    for path in paths:
        try:
            if path.stat().st_mtime < trigger_time - 1.0:
                continue
            job = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError):
            continue
        if not isinstance(job, dict) or job.get("status") != "running" or job.get("unsafe_replay") is not False:
            continue
        checkpoint = job.get("checkpoint")
        if not isinstance(checkpoint, dict) or checkpoint.get("run_id") != run_id:
            continue
        if not isinstance(checkpoint.get("messages"), list):
            continue
        return {
            "path": str(path),
            "job_id": job.get("id"),
            "commit_seq": job.get("commit_seq"),
            "mtime": path.stat().st_mtime,
        }
    return None


def trace_items(client: Any, chat_id: str) -> list[dict[str, Any]]:
    result = require_dict(
        client.call("trace.history", {"chat_id": chat_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    return [item for item in require_list(result.get("items"), "trace.history.items") if isinstance(item, dict)]


def approve_marker_bash(
    client: Any,
    chat_id: str,
    bot_id: str,
    marker: str,
    expected_command: str,
    approved_ids: set[str],
) -> list[dict[str, Any]]:
    """Approve the exact bash call in this marker run, never another approval."""

    items = trace_items(client, chat_id)
    marker_runs = {
        item.get("run_id")
        for item in items
        if item.get("type") == "tool.start"
        and isinstance(item.get("run_id"), str)
        and isinstance(item.get("data"), dict)
        and item["data"].get("name") == "bash"
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
        if not isinstance(data, dict) or data.get("name") != "bash":
            continue
        call_id = data.get("call_id")
        args = data.get("args")
        if not isinstance(call_id, str) or not isinstance(args, dict):
            raise ValueError("recovery marker run has bash without call_id/args")
        if call_id in successful_calls:
            continue
        if args.get("command") != expected_command:
            raise ValueError("recovery marker bash command differs from the scripted command")
        expected[call_id] = {"tool": "bash", "risk": "exec", "args": {"command": expected_command}}
    return approve_exact_pending(
        client,
        run_id=run_id,
        bot_id=bot_id,
        chat_id=chat_id,
        expected_calls=expected,
        approved_ids=approved_ids,
    )


def checkpoint_ready(
    client: Any,
    chat_id: str,
    bot_id: str,
    marker: str,
    relative_path: str,
    home: Path,
    trigger_time: float,
) -> dict[str, Any] | None:
    items = trace_items(client, chat_id)
    marker_runs = {
        item.get("run_id")
        for item in items
        if item.get("type") == "tool.start"
        and isinstance(item.get("run_id"), str)
        and isinstance(item.get("data"), dict)
        and item["data"].get("name") == "bash"
        and marker in json.dumps(item["data"].get("args", {}), ensure_ascii=False)
        and "sleep" in json.dumps(item["data"].get("args", {}), ensure_ascii=False)
    }
    if len(marker_runs) != 1:
        if any(
            item.get("type") == "run.end"
            and isinstance(item.get("run_id"), str)
            and marker in json.dumps(item.get("data", {}), ensure_ascii=False)
            for item in items
        ):
            raise ValueError("recovery run finished before a safe checkpoint was observed")
        return None
    run_id = next(iter(marker_runs))
    run_items = [item for item in items if item.get("run_id") == run_id]
    if any(item.get("type") == "run.end" for item in run_items):
        raise ValueError("recovery run finished before restart; refusing to claim recovery")
    tool_starts = [item for item in run_items if item.get("type") == "tool.start" and isinstance(item.get("data"), dict)]
    if any(item["data"].get("name") == "read" for item in tool_starts):
        raise ValueError("Bot read before the restart checkpoint; refusing to kill an unobserved state")
    bash_starts = [item for item in tool_starts if item["data"].get("name") == "bash"]
    successful_calls = {
        item["data"].get("call_id")
        for item in run_items
        if item.get("type") == "tool.end"
        and isinstance(item.get("data"), dict)
        and item["data"].get("is_error") is False
        and isinstance(item["data"].get("call_id"), str)
    }
    bash_call_ids = {
        item["data"].get("call_id")
        for item in bash_starts
        if isinstance(item["data"].get("call_id"), str)
    }
    if not bash_call_ids & successful_calls:
        return None
    checkpoint = local_checkpoint(home, run_id, trigger_time)
    if checkpoint is None:
        return None
    target = home / "bots" / bot_id / relative_path
    if target.is_symlink() or (target.exists() and not target.is_file()):
        raise ValueError("recovery marker path is not a regular file")
    return {"run_id": run_id, "checkpoint": checkpoint, "trace_items": len(run_items), "marker_path": str(target)}


def choose_bot(client: Any, state: dict[str, Any], args: argparse.Namespace, marker: str) -> tuple[dict[str, Any], bool]:
    bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
    created_worker = False
    if args.create_worker:
        created = require_dict(
            client.call("bot.create", {"name": f"macbot-recovery-{marker.rsplit('-', 1)[-1]}", "client_request_id": f"{marker}-bot"}),
            "bot.create result",
        )
        created_bot = require_dict(created.get("bot"), "bot.create.bot")
        created_chat = require_dict(created.get("dm_chat"), "bot.create.dm_chat")
        if created_bot.get("is_main") is not False or created_chat.get("kind") != "direct":
            raise ValueError("bot.create did not return a non-main Bot with a direct DM")
        state = bootstrap(client)
        bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
        bot = next((item for item in bots if item.get("id") == created_bot.get("id")), None)
        created_worker = True
    else:
        bot = next((item for item in bots if item.get("id") == args.bot_id), None)
    if not isinstance(bot, dict) or not isinstance(bot.get("id"), str) or bot.get("is_main") is not False:
        raise ValueError("recovery requires a non-main Bot")
    chat_id = bot.get("dm_chat_id")
    if not isinstance(chat_id, str) or not chat_id:
        raise ValueError("selected Bot has no dm_chat_id")
    chats = [item for item in require_list(state.get("chats"), "bootstrap.chats") if isinstance(item, dict)]
    chat = next((item for item in chats if item.get("id") == chat_id), None)
    if not isinstance(chat, dict) or chat.get("kind") != "direct":
        raise ValueError("selected Bot dm_chat_id is not a direct chat")
    return bot, created_worker


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    if not args.restart_service:
        raise ValueError("refusing to create a recovery task or kill a service; pass --restart-service explicitly")
    home = Path(args.home).expanduser().resolve()
    before = service_identity()
    client = client_from_args(args)
    endpoint = urlsplit(client.base_url)
    if endpoint.hostname not in {"127.0.0.1", "localhost", "::1"} or endpoint.port != SERVICE_PORT:
        raise ValueError("recovery requires production 7788, never mock")
    health = ready_health(client, args)
    if health.get("mock") is True:
        raise ValueError("recovery requires production 7788, never mock")
    state = bootstrap(client)
    marker = unique_marker("macbot-e2e-recovery")
    bot, created_worker = choose_bot(client, state, args, marker)
    bot_id = bot["id"]
    chat_id = bot["dm_chat_id"]
    relative_path = f"e2e/{marker}.txt"
    target = home / "bots" / bot_id / relative_path
    if target.exists() or target.is_symlink():
        raise ValueError("recovery marker path already exists; refusing to overwrite it")
    trigger_time = time.time()
    expected_command = f"sleep 20; mkdir -p e2e; printf '%s\\n' '{marker}' > {relative_path}"
    prompt = (
        f"Durable recovery acceptance marker {marker}. Use exactly one tool call per model turn. "
        f"First call bash with `{expected_command}`. "
        "After that bash call returns, stop and wait for the next model turn; do not call read in the same turn. "
        f"On the next turn call the read tool on {relative_path}, then reply with exactly {marker}."
    )
    sent = require_dict(
        client.call("chat.send", {"chat_id": chat_id, "text": prompt, "mentions": []}),
        "chat.send result",
    )
    sent_message = require_dict(sent.get("message"), "chat.send.message")
    sent_seq = sent_message.get("seq")
    if not isinstance(sent_seq, int):
        raise ValueError("recovery message has no numeric seq")
    approved_ids: set[str] = set()
    approval_evidence: list[dict[str, Any]] = []

    def checkpoint_poll() -> dict[str, Any] | None:
        if args.approve_test_tools_once:
            approval_evidence.extend(
                approve_marker_bash(client, chat_id, bot_id, marker, expected_command, approved_ids)
            )
        return checkpoint_ready(client, chat_id, bot_id, marker, relative_path, home, trigger_time)

    ready = wait_until(
        checkpoint_poll,
        timeout=args.timeout,
        interval=args.interval,
        description="same run active with durable checkpoint",
    )
    current = service_identity()
    if current["pid"] != before["pid"]:
        raise ValueError("verified service PID changed before kill; refusing to kill an unknown process")
    old_pid = current["pid"]
    os.kill(old_pid, signal.SIGKILL)

    def restarted() -> dict[str, Any] | None:
        if process_alive(old_pid):
            return None
        try:
            identity = service_identity()
        except ValueError:
            return None
        if identity["pid"] == old_pid:
            return None
        return identity

    after = wait_until(
        restarted,
        timeout=args.restart_timeout,
        interval=args.interval,
        description="new com.macbot.server PID after KeepAlive restart",
    )
    ready_health(client, args)

    def resumed() -> dict[str, Any] | None:
        items = trace_items(client, chat_id)
        same_run = [item for item in items if item.get("run_id") == ready["run_id"]]
        if not any(item.get("type") == "run.resume" for item in same_run):
            if any(item.get("type") == "run.end" for item in same_run):
                raise ValueError("same run ended without run.resume after restart")
            return None
        if not any(
            item.get("type") == "run.end"
            and isinstance(item.get("data"), dict)
            and item["data"].get("status") == "done"
            for item in same_run
        ):
            return None
        if target.is_symlink() or not target.is_file() or target.read_text(encoding="utf-8").strip() != marker:
            return None
        history = chat_history(client, chat_id, after_seq=sent_seq)
        reply = next(
            (
                item
                for item in history["messages"]
                if isinstance(item, dict)
                and sender_is(item, kind="bot", bot_id=bot_id)
                and marker in message_text(item)
            ),
            None,
        )
        if reply is None:
            return None
        return {"trace_items": len(same_run), "reply_id": reply.get("id"), "marker_path": str(target)}

    recovered = wait_until(
        resumed,
        timeout=args.timeout,
        interval=args.interval,
        description="same run resume, file marker, and done",
    )
    return {
        "scenario": "S1 durable recovery",
        "status": "PASS",
        "url": client.base_url,
        "health_version": health.get("version"),
        "bot_id": bot_id,
        "created_worker": created_worker,
        "chat_id": chat_id,
        "marker": marker,
        "run_id": ready["run_id"],
        "checkpoint": ready["checkpoint"],
        "old_service": before,
        "new_service": after,
        "recovered": recovered,
        "approvals": approval_evidence,
        "note": "API and local durable/PID checks only; desktop/Android replay UI remains manual.",
    }


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
