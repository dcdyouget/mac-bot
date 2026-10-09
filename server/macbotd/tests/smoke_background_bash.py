#!/usr/bin/env python3
"""Real background Bash approval, live WS output, cancellation and no replay."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import signal
import threading
import time
from http.server import ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import rpc, wait_until
from smoke_decision_migration import DecisionProvider, ProviderState, RestartDaemon, TOKEN, traces
from smoke_invalid_tool_recovery import InvalidProvider
from smoke_trace import connect_ws, ws_request


class BackgroundProvider(DecisionProvider):
    tools = InvalidProvider.tools

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "unauthorized"}})
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.state.record(body)
        messages = body.get("messages", [])
        marker = next((marker for marker in ("LEGACY_BG", "FRESH_BG_PRIMARY", "FRESH_BG_OTHER")
            if any(message.get("role") == "user" and marker in str(message.get("content", "")) for message in messages)), None)
        if marker is None:
            self._text("No background work requested.")
            return
        results = {message.get("tool_call_id"): str(message.get("content", ""))
            for message in messages if message.get("role") == "tool"}
        count = 2 if marker == "FRESH_BG_PRIMARY" else 1
        if f"{marker}-1" not in results:
            calls = []
            for index in range(1, count + 1):
                name = f"{marker}-{index}"
                command = (f"printf 'start\\n' >> {name}.starts; printf '%s\\n' $$ > {name}.pid; "
                    f"while [ ! -e {name}.stop ]; do printf '{name}-live\\n'; sleep 0.05; done")
                if marker == "LEGACY_BG":
                    # Retain a silent inherited output FD so old gateway EOF
                    # waiting stalls; log ticks to a file so kill9 of daemon
                    # cannot terminate this fixture through a broken pipe.
                    command = f"exec 3>&1; exec > {name}.live 2>&1; " + command
                calls.append((name, "bash", {"command": command, "background": True}))
            self.tools(calls)
        else:
            self.tools([(f"{marker}-decision", "send_msg", {"intent": "decision",
                "text": f"{marker}: keep background jobs alive until cancellation",
                "mentions": [], "options": ["继续", "停止"]})])


def job_for(home: Path, run_id: str) -> dict[str, Any]:
    jobs = [json.loads(path.read_text()) for path in (home / "data/jobs").glob("*.json")]
    matching = [job for job in jobs if job["checkpoint"].get("run_id") == run_id]
    assert len(matching) == 1
    return matching[0]


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def kill_owned(pid: int) -> None:
    if alive(pid):
        assert os.getpgid(pid) == pid, "only kill the isolated Bash process group"
        os.killpg(pid, signal.SIGTERM)


async def wait(predicate: Any, description: str, timeout: float = 15) -> None:
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        if predicate():
            return
        await asyncio.sleep(0.05)
    raise AssertionError(f"timed out: {description}")


async def collect(ws: Any, frames: list[dict[str, Any]]) -> None:
    try:
        async for raw in ws:
            if isinstance(raw, str):
                frames.append(json.loads(raw))
    except Exception:
        return


async def acceptance(args: argparse.Namespace, daemon: RestartDaemon) -> dict[str, Any]:
    daemon.start()
    provider = rpc(args.url, args.password, "provider.create", {"name": "background-fake",
        "api_kind": "openai-completions", "base_url": args.provider_url, "api_key": TOKEN})["provider"]
    rpc(args.url, args.password, "model.refresh", {"provider_id": provider["id"]})
    model = rpc(args.url, args.password, "model.upsert", {"provider_id": provider["id"],
        "model_id": "decision-migration-fake", "display_name": "background fake",
        "caps": {"vision": False, "tools": True, "reasoning": False}})["model"]["ref"]
    worker = rpc(args.url, args.password, "bot.create", {"name": "background-worker",
        "model": model, "max_parallel": 4})["bot"]

    def scene(marker: str) -> tuple[dict[str, Any], dict[str, Any], Path, str]:
        project = rpc(args.url, args.password, "project.create", {"name": marker,
            "goal": "isolated background lifecycle", "member_bot_ids": [worker["id"]]})
        rpc(args.url, args.password, "chat.send", {"chat_id": project["chat"]["id"], "text": marker,
            "mentions": [{"kind": "bot", "bot_id": worker["id"]}], "client_request_id": uuid.uuid4().hex})
        rows: list[dict[str, Any]] = []
        def ready() -> bool:
            rows[:] = rpc(args.url, args.password, "assignment.list", {"project_id": project["project"]["id"]})["items"]
            return len(rows) == 1 and bool(traces(args.url, args.password, rows[0]["id"], project["chat"]["id"]))
        wait_until(ready, "background assignment", 15)
        assignment = rows[0]
        items = traces(args.url, args.password, assignment["id"], project["chat"]["id"])
        run_id = next(item["run_id"] for item in items if item["type"] == "run.start")
        request = json.loads((args.home / "data/run_requests" / f"{run_id}.json").read_text())
        cwd = Path(request["cwd"])
        assert cwd.resolve().is_relative_to(args.home.resolve())
        return project, assignment, cwd, run_id

    def approval(assignment_id: str) -> dict[str, Any] | None:
        rows = [item for item in rpc(args.url, args.password, "bootstrap")["pending"]["approvals"]
            if item["assignment_id"] == assignment_id]
        assert len(rows) <= 1
        return rows[0] if rows else None

    project, assignment, cwd, run_id = scene("LEGACY_BG")
    await wait(lambda: approval(assignment["id"]), "legacy approval")
    card = approval(assignment["id"])
    legacy_allow = asyncio.create_task(asyncio.to_thread(rpc, args.url, args.password, "approval.decide",
        {"approval_id": card["id"], "decision": "allow_once"}))
    pid_path = cwd / "LEGACY_BG-1.pid"
    await wait(pid_path.exists, "legacy background started")
    pid = int(pid_path.read_text())
    args.owned_pids.add(pid)
    await asyncio.sleep(0.3)
    old_job = job_for(args.home, run_id)
    assert old_job["status"] == "running" and old_job["unsafe_replay"] is True
    assert old_job["checkpoint"]["pending_tool"]["call_id"] == "LEGACY_BG-1"
    assert not any(item["type"] == "tool.end" for item in traces(args.url, args.password, assignment["id"], project["chat"]["id"]))
    count = BackgroundProvider.state.count()
    daemon.kill9_stop()
    try:
        await legacy_allow
    except Exception:
        pass  # The old approved HTTP handler is terminated at its known stall.
    daemon.args.daemon_command = args.new_command
    daemon.start()
    await wait(lambda: job_for(args.home, run_id)["status"] == "suspended", "unsafe old call suspended")
    assert BackgroundProvider.state.count() == count
    assert (cwd / "LEGACY_BG-1.starts").read_text().splitlines() == ["start"]
    assert alive(pid)
    legacy_pid = pid
    legacy = {"run_id": run_id, "old_running_unsafe": True, "new_suspended": True,
        "start_count": 1, "provider_calls_unchanged": True, "old_call_not_replayed": True}

    first = scene("FRESH_BG_PRIMARY")
    other = scene("FRESH_BG_OTHER")
    ws_url = args.url.replace("http://", "ws://").replace("https://", "wss://")
    async with await connect_ws(ws_url, args.password) as ws_first, await connect_ws(ws_url, args.password) as ws_other:
        first_stream = (await ws_request(ws_first, "trace.subscribe", {"assignment_id": first[1]["id"], "since_aseq": 0}))["stream"]
        other_stream = (await ws_request(ws_other, "trace.subscribe", {"assignment_id": other[1]["id"], "since_aseq": 0}))["stream"]
        first_frames: list[dict[str, Any]] = []
        other_frames: list[dict[str, Any]] = []
        collectors = [asyncio.create_task(collect(ws_first, first_frames)), asyncio.create_task(collect(ws_other, other_frames))]
        try:
            for marker, scene_value, total in (("FRESH_BG_PRIMARY", first, 2), ("FRESH_BG_OTHER", other, 1)):
                project, assignment, cwd, run_id = scene_value
                for index in range(1, total + 1):
                    call_id = f"{marker}-{index}"
                    await wait(lambda: approval(assignment["id"]), "fresh explicit approval")
                    card = approval(assignment["id"])
                    detail = json.loads(card["detail"])
                    assert detail["background"] is True and call_id in detail["command"]
                    started = time.monotonic()
                    await asyncio.wait_for(asyncio.to_thread(rpc, args.url, args.password, "approval.decide",
                        {"approval_id": card["id"], "decision": "allow_once"}), 3)
                    assert time.monotonic() - started < 3
                    await wait(lambda: (cwd / f"{call_id}.pid").exists(), "fresh child pid")
                    pid = int((cwd / f"{call_id}.pid").read_text())
                    args.owned_pids.add(pid)
                    assert alive(pid)
                    items = traces(args.url, args.password, assignment["id"], project["chat"]["id"])
                    assert any(item["type"] == "tool.end" and item["data"]["call_id"] == call_id and not item["data"]["is_error"] for item in items)
                await wait(lambda: job_for(args.home, run_id)["checkpoint"].get("waiting_reason") == "decision", "next model request and decision wait")
                assert job_for(args.home, run_id)["checkpoint"].get("pending_tool") is None
            first_frames.clear()
            other_frames.clear()
            await wait(lambda: any(frame.get("event") == "trace.tool_output" and frame["data"]["call_id"] == "FRESH_BG_PRIMARY-1" for frame in first_frames), "output after tool.end")
            await wait(lambda: any(frame.get("event") == "trace.tool_output" and frame["data"]["call_id"] == "FRESH_BG_OTHER-1" for frame in other_frames), "other run output")
            for frames, stream, prefix in ((first_frames, first_stream, "FRESH_BG_PRIMARY"), (other_frames, other_stream, "FRESH_BG_OTHER")):
                outputs = [frame["data"] for frame in frames if frame.get("event") == "trace.tool_output"]
                assert outputs and all(item["stream"] == stream and item["call_id"].startswith(prefix) for item in outputs)
                assert all(set(item) == {"stream", "call_id", "chunk"} for item in outputs)
            first_pids = [int((first[2] / f"FRESH_BG_PRIMARY-{index}.pid").read_text()) for index in (1, 2)]
            other_pid = int((other[2] / "FRESH_BG_OTHER-1.pid").read_text())
            rpc(args.url, args.password, "assignment.stop", {"assignment_id": first[1]["id"]})
            await wait(lambda: all(not alive(pid) for pid in first_pids), "cancel first run children")
            assert alive(other_pid)
            other_frames.clear()
            await wait(lambda: any(frame.get("event") == "trace.tool_output" for frame in other_frames), "other output survives cancellation")
            rpc(args.url, args.password, "assignment.stop", {"assignment_id": other[1]["id"]})
            await wait(lambda: not alive(other_pid), "cancel other child")
            for pid in first_pids + [other_pid]:
                args.owned_pids.discard(pid)
            for scene_value in (first, other):
                assert job_for(args.home, scene_value[3])["status"] == "cancelled"
            # Explicit operator cancellation can close the old suspended
            # assignment without executing its side-effecting call again.
            old_assignment_id = json.loads((args.home / "data/run_requests" / f"{legacy['run_id']}.json").read_text())["assignment_id"]
            calls_before_stop = BackgroundProvider.state.count()
            rpc(args.url, args.password, "assignment.stop", {"assignment_id": old_assignment_id})
            await wait(lambda: job_for(args.home, legacy["run_id"])["status"] == "cancelled", "old suspended run converges cancelled")
            assert BackgroundProvider.state.count() == calls_before_stop
            # A new manager cannot own a pre-restart PID from a checkpoint
            # that never persisted its result. Do not claim it was killed.
            assert alive(legacy_pid)
            kill_owned(legacy_pid)
            await wait(lambda: not alive(legacy_pid), "explicit isolated orphan cleanup")
            args.owned_pids.discard(legacy_pid)
            legacy["explicit_stop_cancelled_without_replay"] = True
            legacy["old_pid_not_automatically_adopted_or_killed"] = True
            return {"ok": True, "legacy_upgrade": legacy, "approved_background_calls": 3,
                "same_response_followup": True, "tool_end_before_child_exit": True,
                "live_ws_output_after_tool_end": True, "stream_call_ownership": True,
                "cancellation_isolated_to_run": True, "fresh_runs": [first[3], other[3]]}
        finally:
            for task in collectors:
                task.cancel()
            await asyncio.gather(*collectors, return_exceptions=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7861")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--legacy-command", required=True)
    parser.add_argument("--new-command", required=True)
    parser.add_argument("--browser-bin")
    args = parser.parse_args()
    assert not args.home.exists(), "use a fresh isolated home"
    args.owned_pids = set()
    args.daemon_command = args.legacy_command
    BackgroundProvider.state = ProviderState()
    server = ThreadingHTTPServer(("127.0.0.1", 0), BackgroundProvider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    args.provider_url = f"http://127.0.0.1:{server.server_port}/v1"
    daemon = RestartDaemon(args)
    try:
        print(json.dumps(asyncio.run(acceptance(args, daemon)), indent=2))
    finally:
        for pid in args.owned_pids:
            kill_owned(pid)
        daemon.close()
        server.shutdown()


if __name__ == "__main__":
    main()
