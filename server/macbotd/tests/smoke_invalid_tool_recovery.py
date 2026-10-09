#!/usr/bin/env python3
"""Upgrade legacy invalid approvals without granting permission or new runs."""

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
from smoke_decision_migration import DecisionProvider, ProviderState, RestartDaemon, TOKEN, traces


class InvalidProvider(DecisionProvider):
    project_id = ""
    project_home = Path("/tmp")

    def tools(self, calls: list[tuple[str, str, dict[str, Any]]]) -> None:
        self._stream([
            {"choices": [{"delta": {"tool_calls": [
                {"index": index, "id": call_id, "type": "function", "function": {
                    "name": name, "arguments": json.dumps(args)}}
                for index, (call_id, name, args) in enumerate(calls)
            ]}, "finish_reason": None}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
             "usage": {"prompt_tokens": 10, "completion_tokens": 5}},
        ])

    def do_POST(self) -> None:  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self._json(401, {"error": {"message": "unauthorized"}})
            return
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.state.record(body)
        messages = body.get("messages", [])
        latest = next((str(m.get("content", "")) for m in reversed(messages) if m.get("role") == "user"), "")
        marker = next((m for m in ("INVALID_MEMORY_OLD", "INVALID_MEMORY_RECEIPT", "INVALID_PATH_OLD", "INVALID_MEMORY_FRESH", "VALID_PATH_FRESH") if m in latest), None)
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
            expected_error = "legacy tilde path resolution changed" if marker == "INVALID_PATH_OLD" else "memory project scope requires project_id"
            assert expected_error in results[invalid]
            if marker == "INVALID_PATH_OLD":
                self.tools([(valid, "write", {"path": str(self.project_home / "corrected.txt"), "content": marker})])
            else:
                self.tools([(valid, "memory", {"scope": "project", "project_id": self.project_id, "action": "add", "content": marker})])
        elif marker == "INVALID_PATH_OLD":
            self.tools([(invalid, "write", {"path": f"~/MacBot-invalid-tools-{self.project_id}/old.txt", "content": marker})])
        elif marker == "VALID_PATH_FRESH":
            relative = self.project_home.relative_to(Path.home()) / "fresh-tilde.txt"
            self.tools([(valid, "write", {"path": f"~/{relative}", "content": marker})])
        else:
            calls = [(invalid, "memory", {"scope": "project", "action": "add", "content": marker})]
            if marker != "INVALID_MEMORY_FRESH":
                calls.append((f"{marker}-deferred", "write", {"path": str(self.project_home / "must-not-execute.txt"), "content": "not authorized"}))
            self.tools(calls)


def pending(base: str, password: str, assignment: str) -> list[dict[str, Any]]:
    return [a for a in rpc(base, password, "bootstrap").get("pending", {}).get("approvals", []) if a.get("assignment_id") == assignment]


def run_case(args: argparse.Namespace, daemon: RestartDaemon, worker: str, project: dict[str, Any], marker: str) -> dict[str, Any]:
    old = marker not in ("INVALID_MEMORY_FRESH", "VALID_PATH_FRESH")
    daemon.close()
    daemon.args.daemon_command = args.legacy_command if old else args.new_command
    daemon.start()
    sent = rpc(args.url, args.password, "chat.send", {"chat_id": project["chat"]["id"], "text": marker,
        "mentions": [{"kind": "bot", "bot_id": worker}], "client_request_id": f"invalid-{uuid.uuid4().hex}"})
    assignment: dict[str, Any] = {}
    def assigned() -> bool:
        rows = rpc(args.url, args.password, "assignment.list", {"project_id": project["project"]["id"]})["items"]
        found = next((a for a in rows if marker in a.get("instruction", "")), None)
        if found:
            assignment.update(found)
        return bool(found)
    wait_until(assigned, "assignment", 30)
    assignment_id = assignment["id"]
    approvals: list[dict[str, Any]] = []
    def approved_card() -> bool:
        approvals[:] = pending(args.url, args.password, assignment_id)
        return len(approvals) == 1
    wait_until(approved_card, "first scoped approval", 45)
    first = approvals[0]
    old_detail = json.loads(first["detail"])
    run_id = next(i["run_id"] for i in traces(args.url, args.password, assignment_id, project["chat"]["id"]) if i["type"] == "run.start")
    request = json.loads((args.home / "data/run_requests" / f"{run_id}.json").read_text())
    assert request["run_id"] == run_id and request["assignment_id"] == assignment_id
    assert request["bot_id"] == worker and request["chat_id"] == project["chat"]["id"]
    assert request["project_id"] == project["project"]["id"]
    assert Path(request["cwd"]) == InvalidProvider.project_home
    mapping_path = args.home / "data/approval-map" / f"{first['id']}.json"
    def approval_checkpoint_committed() -> bool:
        if not mapping_path.exists():
            return False
        mapping = json.loads(mapping_path.read_text())
        jobs = [json.loads(path.read_text()) for path in (args.home / "data/jobs").glob("*.json")]
        return any(job["status"] in ("waiting", "suspended")
            and job["checkpoint"].get("run_id") == run_id
            and job["checkpoint"].get("pending_tool", {}).get("call_id") == mapping["call_id"]
            for job in jobs)
    # Approval publication precedes its checkpoint/map writes. Kill only at
    # the persisted waiting boundary that this upgrade case intends to test.
    wait_until(approval_checkpoint_committed, "mapped waiting checkpoint", 15)
    if old:
        assert "project_id" not in old_detail if marker != "INVALID_PATH_OLD" else "resolved_path" not in old_detail
        daemon.kill9_stop()
        if marker == "INVALID_MEMORY_RECEIPT":
            state_path = args.home / "data/orchestrator/state.json"
            state = json.loads(state_path.read_text())
            state["approvals"][first["id"]]["state"] = "expired"
            state["approvals"][first["id"]]["decided_at"] = "2026-10-10T00:00:00Z"
            state_path.write_text(json.dumps(state))
            operations_path = args.home / "data/orchestrator/operations.jsonl"
            operations = [json.loads(line) for line in operations_path.read_text().splitlines()]
            operations[-1]["snapshot"] = state
            operations_path.write_text("".join(json.dumps(row) + "\n" for row in operations))
            mapping = json.loads(mapping_path.read_text())
            receipt = {"approval_id": first["id"], "run_id": run_id, "call_id": mapping["call_id"], "tool": first["tool"],
                "args": old_detail, "bot_id": first["bot_id"], "chat_id": first["chat_id"], "assignment_id": assignment_id}
            root = args.home / "data/invalid-tool-recovery"
            root.mkdir(parents=True, exist_ok=True)
            (root / f"{first['id']}.json").write_text(json.dumps(receipt))
        daemon.args.daemon_command = args.new_command
        daemon.start()
        def corrected_card() -> bool:
            return approved_card() and approvals[0]["id"] != first["id"]
        wait_until(corrected_card, "new approval for corrected arguments", 45)
        state = json.loads((args.home / "data/orchestrator/state.json").read_text())
        assert state["approvals"][first["id"]]["state"] == "expired"
        assert json.loads(state["approvals"][first["id"]]["detail"]) == old_detail
    corrected = approvals[0]
    detail = json.loads(corrected["detail"])
    if marker in ("INVALID_PATH_OLD", "VALID_PATH_FRESH"):
        target = InvalidProvider.project_home / ("corrected.txt" if old else "fresh-tilde.txt")
        assert Path(detail["path"]).expanduser() == Path(detail["resolved_path"]) == target
        assert detail["path_resolution"] == "home-v1"
        assert not target.exists()
        assert not list(InvalidProvider.project_home.glob("~/MacBot-invalid-tools-*/old.txt"))
    else:
        assert detail["project_id"] == project["project"]["id"]
        assert detail["scope"] == "project"
    assert not (InvalidProvider.project_home / "must-not-execute.txt").exists()
    before_items = traces(args.url, args.password, assignment_id, project["chat"]["id"])
    assert not any(i["type"] == "tool.end" and i["data"].get("call_id") == f"{marker}-valid" for i in before_items)
    rpc(args.url, args.password, "approval.decide", {"approval_id": corrected["id"], "decision": "allow_once"})
    wait_until(lambda: any(i["type"] == "run.end" and i["data"].get("status") == "done" for i in traces(args.url, args.password, assignment_id, project["chat"]["id"])), "same run done", 45)
    items = traces(args.url, args.password, assignment_id, project["chat"]["id"])
    assert not any(i["type"] == "tool.end" and i["data"].get("call_id") == f"{marker}-deferred" and not i["data"].get("is_error") for i in items)
    assert {i["run_id"] for i in items} == {run_id}
    assert sum(i["type"] == "run.start" for i in items) == 1
    if marker != "VALID_PATH_FRESH":
        assert any(i["type"] == "tool.end" and i["data"].get("call_id") == f"{marker}-invalid" and i["data"].get("is_error") for i in items)
    assert not (InvalidProvider.project_home / "must-not-execute.txt").exists()
    if marker in ("INVALID_PATH_OLD", "VALID_PATH_FRESH"):
        assert target.read_text() == marker
        assert not (InvalidProvider.project_home / "~").exists()
    calls = InvalidProvider.state.count()
    daemon.kill9_restart()
    time.sleep(0.2)
    assert InvalidProvider.state.count() == calls
    return {"marker": marker, "run_id": run_id, "assignment_id": assignment_id, "old_approval": first["id"] if old else None,
        "old_approval_expired": old, "old_detail": old_detail if old else None,
        "new_approval": corrected["id"], "new_detail": detail,
        "runtime_cwd": request["cwd"], "actual_target": str(target) if marker in ("INVALID_PATH_OLD", "VALID_PATH_FRESH") else None,
        "same_run_done": True, "no_unapproved_side_effects": True, "restart_idempotent": True, "sent_id": sent["message"]["id"]}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7858")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--legacy-command", required=True)
    parser.add_argument("--new-command", required=True)
    parser.add_argument("--browser-bin")
    args = parser.parse_args()
    assert not args.home.exists(), "use a fresh isolated home"
    args.daemon_command = args.legacy_command
    InvalidProvider.state = ProviderState()
    provider = ThreadingHTTPServer(("127.0.0.1", 0), InvalidProvider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    daemon = RestartDaemon(args)
    try:
        daemon.start()
        created = rpc(args.url, args.password, "provider.create", {"name": "invalid-tools-fake", "api_kind": "openai-completions",
            "base_url": f"http://127.0.0.1:{provider.server_port}/v1", "api_key": TOKEN})["provider"]
        rpc(args.url, args.password, "model.refresh", {"provider_id": created["id"]})
        model = rpc(args.url, args.password, "model.upsert", {"provider_id": created["id"], "model_id": "decision-migration-fake",
            "display_name": "invalid tools fake", "caps": {"vision": False, "tools": True, "reasoning": False}})["model"]["ref"]
        worker = rpc(args.url, args.password, "bot.create", {"name": "invalid-tools-worker", "model": model})["bot"]
        project = rpc(args.url, args.password, "project.create", {"name": "invalid-tools-project", "goal": "Validate invalid tool recovery in isolation", "member_bot_ids": [worker["id"]]})
        InvalidProvider.project_id = project["project"]["id"]
        # The protocol uses a display path under ~/MacBot, while the runtime
        # maps a project's cwd under MACBOT_HOME. Keep every test target in
        # that isolated runtime directory without changing process HOME.
        InvalidProvider.project_home = args.home.resolve() / "projects" / project["project"]["slug"]
        assert InvalidProvider.project_home.is_relative_to(args.home.resolve())
        InvalidProvider.project_home.relative_to(Path.home())
        cases = []
        for marker in ("INVALID_MEMORY_OLD", "INVALID_MEMORY_RECEIPT", "INVALID_PATH_OLD", "INVALID_MEMORY_FRESH", "VALID_PATH_FRESH"):
            case = run_case(args, daemon, worker["id"], project, marker)
            cases.append(case)
            print(json.dumps({"case_pass": case}), flush=True)
        print(json.dumps({"ok": True, "cases": cases, "provider_calls": InvalidProvider.state.count()}, indent=2))
    finally:
        daemon.close()
        provider.shutdown()


if __name__ == "__main__":
    main()
