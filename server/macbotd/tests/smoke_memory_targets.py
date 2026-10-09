#!/usr/bin/env python3
"""Reject nonexistent memory targets before approval; retire exact legacy calls."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import threading
import time
from http.server import ThreadingHTTPServer
from typing import Any
import uuid

from smoke_collaboration import rpc, wait_until
from smoke_decision_migration import ProviderState, RestartDaemon, TOKEN, traces
from smoke_invalid_tool_recovery import InvalidProvider, pending


CASES = ("TARGET_PROJECT_OLD", "TARGET_BOT_OLD", "TARGET_PROJECT_FRESH",
         "TARGET_BOT_FRESH", "TARGET_SEARCH_FRESH")


class TargetProvider(InvalidProvider):
    worker_id = ""

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "unauthorized"}})
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.state.record(body)
        messages = body.get("messages", [])
        latest = next((str(m.get("content", "")) for m in reversed(messages) if m.get("role") == "user"), "")
        marker = next((m for m in CASES if m in latest), None)
        if marker is None:
            self._text("No work requested.")
            return
        results = {m.get("tool_call_id"): str(m.get("content", "")) for m in messages if m.get("role") == "tool"}
        invalid, valid, done = (f"{marker}-{part}" for part in ("invalid", "valid", "done"))
        if done in results:
            self._text(f"{marker} finished")
        elif valid in results:
            self.tools([(done, "send_msg", {"intent": "done", "text": f"{marker} complete", "mentions": []})])
        elif invalid in results:
            assert "does not exist" in results[invalid], results[invalid]
            if "OLD" in marker:
                assert "not executed" in results[f"{marker}-deferred"]
            self.tools([(valid, "memory", {"scope": "project", "project_id": self.project_id,
                "action": "add", "content": marker})])
        else:
            scope = "bot" if "BOT" in marker else "project"
            args = {"scope": scope, f"{scope}_id": f"nonexistent-{marker}", "content": marker, "action": "add"}
            tool = "memory_search" if "SEARCH" in marker else "memory"
            if tool == "memory_search":
                args = {"scope": scope, f"{scope}_id": f"nonexistent-{marker}", "query": marker}
            calls = [(invalid, tool, args)]
            if "OLD" in marker:
                calls.append((f"{marker}-deferred", "write", {"path": str(self.project_home / f"{marker}-forbidden.txt"), "content": "not authorized"}))
            self.tools(calls)


def latest_job(home: Path, run_id: str) -> dict[str, Any] | None:
    jobs = [json.loads(p.read_text()) for p in (home / "data/jobs").glob("*.json")]
    return next((j for j in jobs if j["checkpoint"].get("run_id") == run_id), None)


def run_case(args: argparse.Namespace, daemon: RestartDaemon, worker: str, project: dict[str, Any], marker: str) -> dict[str, Any]:
    old = marker.endswith("OLD")
    daemon.close()
    daemon.args.daemon_command = args.legacy_command if old else args.new_command
    daemon.start()
    sent = rpc(args.url, args.password, "chat.send", {"chat_id": project["chat"]["id"], "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker}], "client_request_id": uuid.uuid4().hex})
    assignment: dict[str, Any] = {}
    def assigned() -> bool:
        rows = rpc(args.url, args.password, "assignment.list", {"project_id": project["project"]["id"]})["items"]
        row = next((a for a in rows if marker in a.get("instruction", "")), None)
        if row:
            assignment.update(row)
        return bool(row)
    wait_until(assigned, "new assignment", 30)
    assignment_id = assignment["id"]
    cards: list[dict[str, Any]] = []
    def card_ready() -> bool:
        cards[:] = pending(args.url, args.password, assignment_id)
        return len(cards) == 1
    wait_until(card_ready, "memory approval", 45)
    initial = cards[0]
    items = traces(args.url, args.password, assignment_id, project["chat"]["id"])
    run_id = next(i["run_id"] for i in items if i["type"] == "run.start")
    request = json.loads((args.home / "data/run_requests" / f"{run_id}.json").read_text())
    assert request["project_id"] == project["project"]["id"] and request["assignment_id"] == assignment_id
    invalid_call = f"{marker}-invalid"
    original_args = json.loads(initial["detail"]) if old else None
    def persisted_wait() -> bool:
        job = latest_job(args.home, run_id)
        mapping = args.home / "data/approval-map" / f"{cards[0]['id']}.json"
        return bool(job and job["status"] == "waiting" and mapping.exists()
            and job["checkpoint"].get("pending_tool", {}).get("call_id") == json.loads(mapping.read_text())["call_id"])
    wait_until(persisted_wait, "mapped waiting checkpoint", 15)
    before_job = latest_job(args.home, run_id)
    if old:
        assert initial["tool"] == "memory" and "nonexistent-" in json.dumps(original_args)
        assert before_job["checkpoint"]["pending_tool"]["call_id"] == invalid_call
        assert before_job["checkpoint"]["pending_tool"]["args"] == original_args
        daemon.kill9_stop()
        daemon.args.daemon_command = args.new_command
        daemon.start()
        wait_until(lambda: card_ready() and cards[0]["id"] != initial["id"], "new explicit corrected target approval", 45)
        state = json.loads((args.home / "data/orchestrator/state.json").read_text())
        expired = state["approvals"][initial["id"]]
        assert expired["state"] == "expired" and json.loads(expired["detail"]) == original_args
        # Retired authorization cannot be used after startup parameter rejection.
        try:
            rpc(args.url, args.password, "approval.decide", {"approval_id": initial["id"], "decision": "allow_once"})
        except AssertionError as error:
            assert "expired" in str(error) or "decided" in str(error) or "pending" in str(error), str(error)
            pass
        else:
            raise AssertionError("expired invalid authorization was accepted")
    corrected = cards[0]
    detail = json.loads(corrected["detail"])
    assert detail["scope"] == "project" and detail["project_id"] == request["project_id"]
    before = traces(args.url, args.password, assignment_id, project["chat"]["id"])
    assert any(i["type"] == "tool.end" and i["data"].get("call_id") == invalid_call and i["data"].get("is_error") for i in before)
    assert not any(i["type"] == "tool.end" and i["data"].get("call_id") == f"{marker}-valid" for i in before)
    assert not (TargetProvider.project_home / f"{marker}-forbidden.txt").exists()
    state = json.loads((args.home / "data/orchestrator/state.json").read_text())
    related = [a for a in state["approvals"].values() if a.get("assignment_id") == assignment_id]
    assert len(related) == (2 if old else 1), related
    # A second kill9 while corrected approval is pending must neither re-error
    # the old head nor invoke a provider/new side effect.
    calls_before = TargetProvider.state.count()
    errors_before = sum(i["type"] == "tool.end" and i["data"].get("call_id") == invalid_call for i in before)
    daemon.kill9_restart()
    time.sleep(0.25)
    assert TargetProvider.state.count() == calls_before
    after = traces(args.url, args.password, assignment_id, project["chat"]["id"])
    assert sum(i["type"] == "tool.end" and i["data"].get("call_id") == invalid_call for i in after) == errors_before == 1
    assert pending(args.url, args.password, assignment_id)[0]["id"] == corrected["id"]
    rpc(args.url, args.password, "approval.decide", {"approval_id": corrected["id"], "decision": "allow_once"})
    wait_until(lambda: any(i["type"] == "run.end" and i["data"].get("status") == "done" for i in traces(args.url, args.password, assignment_id, project["chat"]["id"])), "same run completes", 45)
    final = traces(args.url, args.password, assignment_id, project["chat"]["id"])
    assert {i["run_id"] for i in final} == {run_id}
    assert sum(i["type"] == "run.start" for i in final) == 1
    assert not (TargetProvider.project_home / f"{marker}-forbidden.txt").exists()
    assert not any(i["type"] == "tool.start" and i["data"].get("call_id") == f"{marker}-deferred" for i in final)
    return {"marker": marker, "run_id": run_id, "assignment_id": assignment_id, "job_id": before_job["id"],
        "old_approval": initial["id"] if old else None, "old_args_unchanged": old, "old_expired": old,
        "new_approval": corrected["id"], "new_target": detail["project_id"], "same_run_done": True,
        "invalid_error_count": 1, "batch_not_executed": True, "restart_idempotent": True, "sent_id": sent["message"]["id"]}


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
    args.daemon_command = args.legacy_command
    TargetProvider.state = ProviderState()
    provider = ThreadingHTTPServer(("127.0.0.1", 0), TargetProvider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    daemon = RestartDaemon(args)
    try:
        daemon.start()
        created = rpc(args.url, args.password, "provider.create", {"name": "memory-target-fake", "api_kind": "openai-completions",
            "base_url": f"http://127.0.0.1:{provider.server_port}/v1", "api_key": TOKEN})["provider"]
        rpc(args.url, args.password, "model.refresh", {"provider_id": created["id"]})
        model = rpc(args.url, args.password, "model.upsert", {"provider_id": created["id"], "model_id": "decision-migration-fake",
            "display_name": "memory target fake", "caps": {"vision": False, "tools": True, "reasoning": False}})["model"]["ref"]
        worker = rpc(args.url, args.password, "bot.create", {"name": "memory-target-worker", "model": model})["bot"]
        project = rpc(args.url, args.password, "project.create", {"name": "memory-target-project", "goal": "Validate actual memory targets", "member_bot_ids": [worker["id"]]})
        TargetProvider.project_id = project["project"]["id"]
        TargetProvider.worker_id = worker["id"]
        TargetProvider.project_home = args.home.resolve() / "projects" / project["project"]["slug"]
        results = []
        for marker in CASES:
            case = run_case(args, daemon, worker["id"], project, marker)
            results.append(case)
            print(json.dumps({"case_pass": case}), flush=True)
        print(json.dumps({"ok": True, "cases": results, "provider_calls": TargetProvider.state.count()}, indent=2))
    finally:
        daemon.close()
        provider.shutdown()


if __name__ == "__main__":
    main()
