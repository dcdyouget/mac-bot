#!/usr/bin/env python3
"""Read-only summaries of interrupted background calls; never resume or kill."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


def fingerprint(value: Any) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def inspect(home: Path, assignments: list[str]) -> dict[str, Any]:
    jobs: dict[str, dict[str, Any]] = {}
    for path in (home / "data/jobs").glob("*.json"):
        job = json.loads(path.read_text())
        jobs[job["id"]] = job
    # Use the latest durable commit, including the commit-before-snapshot
    # crash boundary. Ignore only an incomplete final JSONL line.
    log = home / "data/jobs/commits.jsonl"
    if log.exists():
        lines = log.read_text().splitlines()
        for index, line in enumerate(lines):
            try:
                job = json.loads(line)["job"]
            except json.JSONDecodeError:
                if index == len(lines) - 1:
                    break
                raise
            current = jobs.get(job["id"])
            if current is None or job["commit_seq"] >= current["commit_seq"]:
                jobs[job["id"]] = job
    wanted = set(assignments)
    requests = {}
    for path in (home / "data/run_requests").glob("*.json"):
        request = json.loads(path.read_text())
        if request.get("assignment_id") in wanted:
            requests[request["run_id"]] = request
    rows = []
    for job in jobs.values():
        checkpoint = job["checkpoint"]
        request = requests.get(checkpoint.get("run_id"))
        if request is None:
            continue
        call = checkpoint.get("pending_tool") or {}
        args = call.get("args") or {}
        pending = checkpoint.get("pending_tools") or []
        matches = [other for other in jobs.values()
            if other["checkpoint"].get("run_id") == request["run_id"]]
        rows.append({
            "assignment_id": request["assignment_id"], "run_id": request["run_id"],
            "job_id": job["id"], "status": job["status"], "commit_seq": job["commit_seq"],
            "unsafe_replay": job["unsafe_replay"], "bot_id": request["bot_id"],
            "chat_id": request["chat_id"], "project_id": request.get("project_id"),
            "cwd": request.get("cwd"), "call_id": call.get("call_id"), "tool": call.get("name"),
            "background": args.get("background"), "args_sha256": fingerprint(args),
            "command_sha256": hashlib.sha256(str(args.get("command", "")).encode()).hexdigest(),
            "pending_tools_count": len(pending),
            "pending_head_matches": not pending or pending[0] == call,
            "unique_job_for_run": len(matches) == 1,
            "automatic_replay_allowed": False,
            "cannot_infer_old_job_id_or_pid_from_checkpoint": True,
        })
    return {"read_only": True, "home": str(home), "calls": sorted(rows, key=lambda row: row["assignment_id"]),
        "missing_assignments": sorted(wanted - {row["assignment_id"] for row in rows})}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--assignment-id", action="append", required=True)
    args = parser.parse_args()
    print(json.dumps(inspect(args.home, args.assignment_id), indent=2))


if __name__ == "__main__":
    main()
