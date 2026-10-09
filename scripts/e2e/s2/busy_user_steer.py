#!/usr/bin/env python3
"""Verify one new user chat.send steer on an existing scenario-owned run.

Never creates a project or initial worker, resends an old steer, or approves
an old call. The source journal must bind the target to a canonical Main
request graph. Unknown RPC outcomes stop with the reserved UUID retained.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from common import add_connection_args, client_from_args, ready_health, require_production_host, unique_marker
from s2.project_followup import (
    FollowupStop, complete_trace_items, handle_approvals, persist,
    send_chat, steer_evidence, tracked_assignments,
)
from s2.approval_scope import check_approval_scope
from s2.login_feature import validate_approval_checkpoint

BUSY = {"working", "waiting_user", "waiting_bot", "blocked"}


def selected_delivery_events(message_id: str, assignment_id: str) -> list[dict]:
    """Read only routing metadata for this exact local test message."""
    result = []
    with (Path.home() / "MacBot/data/events/events.jsonl").open(encoding="utf-8") as stream:
        for line in stream:
            if message_id not in line:
                continue
            event = json.loads(line)
            if event.get("event") not in {"message.created", "message.updated"}:
                continue
            message = event.get("data", {}).get("message", {})
            if message.get("id") != message_id:
                continue
            for delivery in message.get("delivery", []):
                if delivery.get("assignment_id") == assignment_id:
                    result.append({"seq": event["seq"], "event": event["event"],
                                   "state": delivery.get("state"), "assignment_id": assignment_id,
                                   "bot_id": delivery.get("bot_id")})
    return result


def run(args: argparse.Namespace) -> dict:
    if args.journal.exists():
        raise FollowupStop("journal already exists; inspect it without resending")
    source = json.loads(args.source_journal.read_text(encoding="utf-8"))
    client = client_from_args(args)
    require_production_host(client, ready_health(client, args))
    target = client.call("assignment.get", {"assignment_id": args.assignment_id})["assignment"]
    project_id = target["project_id"]
    project = dict(next(x for x in source["project_snapshots"] if x["project_id"] == project_id))
    current = client.call("project.get", {"project_id": project_id})["project"]
    if current["chat_id"] != project["chat_id"] or current["home_path"] != project["home_path"]:
        raise FollowupStop("project/chat/Home changed")
    request = next(x for x in source["requests"] if x["project_id"] == project_id and x["kind"] == "followup_request")
    roots = source["main_root_assignment_ids"][project_id]
    rows = tracked_assignments(client, project, request, set(project["baseline_assignment_ids"]), roots)
    if target["id"] not in {x["id"] for x in rows} or target["origin_chat_id"] != project["chat_id"]:
        raise FollowupStop("target is outside the canonical source request graph")
    bot_id = target["bot_id"]
    if bot_id != source["role_ids"]["测试"]:
        raise FollowupStop("this report-check scenario requires the source journal Tester")
    listing = client.call("assignment.list", {"project_id": project_id, "bot_id": bot_id, "limit": 100})
    if listing.get("next_cursor"):
        raise FollowupStop("assignment listing is truncated")
    busy = [x for x in listing["items"] if x["status"] in BUSY and x.get("origin_chat_id") == project["chat_id"]]
    if len(busy) != 1 or busy[0]["id"] != args.assignment_id:
        raise FollowupStop("no unique busy assignment for this Bot/project/chat")
    before = complete_trace_items(client, args.assignment_id)
    run_ids = {x.get("run_id") for x in before if x.get("type") == "run.start"}
    if len(run_ids) != 1:
        raise FollowupStop("target does not have exactly one original run")
    run_id = next(iter(run_ids))
    marker = unique_marker("macbot-e2e-s2-steer")
    project["marker"] = marker
    pending_before = [x for x in client.call("approval.list", {"state": ["pending"]})["approvals"] if x.get("assignment_id") == args.assignment_id]
    if pending_before:
        if len(pending_before) != 1 or pending_before[0]["id"] != args.initial_approval_id:
            raise FollowupStop("initial approval is not the explicitly reviewed unique call")
        candidate = pending_before[0]
        scope = check_approval_scope(candidate, project["home_path"], project["scope_marker"], project_id=project_id)
        if not scope.get("authorized"):
            raise FollowupStop("initial approval is outside the reviewed project scope")
        checkpoint = validate_approval_checkpoint(client, candidate, project_id=project_id,
                                                  project_chat_id=project["chat_id"], assignment_ids={args.assignment_id})
    else:
        checkpoint = None
    journal = {
        "scenario": "new busy user chat.send steer", "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "source_journal": str(args.source_journal), "project_id": project_id,
        "chat_id": project["chat_id"], "assignment_id": args.assignment_id, "bot_id": bot_id,
        "run_id": run_id, "marker": marker, "requests": [], "approval_observations": [],
        "baseline": {"status": target["status"], "run_start_count": 1,
                     "pending_approval_ids": [x["id"] for x in pending_before],
                     "reviewed_checkpoint": checkpoint},
        "full_s2_pass": False, "status": "PREPARED",
    }
    persist(journal, args.journal)
    try:
        text = (
            f"{marker}：这是一条运行中插话，继续本项目原任务，不另建任务或群。"
            "新增验收约束：报告必须区分源代码检查与实际运行；未实际执行不得声称测试通过。"
            f"请优先用 read 读取 {Path(project['home_path']).expanduser() / 'login.html'}，"
            f"再用 write 保存 {Path(project['home_path']).expanduser() / 'TEST.md'}，包含本条 marker。"
            "不要 memory、Bash、启动服务、自派任务或修改其他项目。"
        )
        sent = send_chat(client, journal=journal, journal_path=args.journal, project=project,
                         kind="busy_user_steer", text=text,
                         mentions=[{"kind": "bot", "bot_id": bot_id, "instruction": None}],
                         assignment_id=args.assignment_id)
        steer = {**sent, "assignment_id": args.assignment_id, "project_id": project_id}
        routing_deadline = time.monotonic() + 15
        while True:
            message = client.call("chat.history", {"chat_id": project["chat_id"], "after_seq": sent["seq"] - 1, "limit": 100})["messages"]
            matches = [x for x in message if x["id"] == sent["message_id"]]
            if len(matches) != 1:
                raise FollowupStop("canonical user message is missing or ambiguous; no resend")
            canonical = matches[0]
            mentions = canonical.get("mentions", [])
            if (canonical.get("chat_id") != project["chat_id"]
                    or canonical.get("sender", {}).get("kind") != "user"
                    or canonical.get("fallback_text") != text
                    or len(mentions) != 1 or mentions[0].get("kind") != "bot"
                    or mentions[0].get("bot_id") != bot_id):
                raise FollowupStop("canonical user message/chat/mention mismatch")
            delivery = [x for x in canonical.get("delivery", []) if x.get("bot_id") == bot_id and x.get("assignment_id") == args.assignment_id]
            if len(delivery) == 1 and delivery[0]["state"] in {"queued", "delivered", "read"}:
                break
            if time.monotonic() >= routing_deadline:
                raise FollowupStop("new user steer was not bound to the original assignment")
            time.sleep(args.interval)
        pending = [x["id"] for x in client.call("approval.list", {"state": ["pending"]})["approvals"] if x.get("assignment_id") == args.assignment_id]
        journal["initial_delivery"] = delivery[0]
        journal["approval_wait_not_approved_by_steer"] = pending == journal["baseline"]["pending_approval_ids"]
        persist(journal, args.journal)
        if target["status"] == "waiting_user" and not journal["approval_wait_not_approved_by_steer"]:
            raise FollowupStop("pending approval changed before any explicit scoped decision")
        deadline = time.monotonic() + args.timeout
        initial_decision_sent = False
        while True:
            evidence = steer_evidence(client, project, steer, bot_id)
            events = selected_delivery_events(sent["message_id"], args.assignment_id)
            trace = complete_trace_items(client, args.assignment_id)
            rows = client.call("assignment.list", {"project_id": project_id, "limit": 100})
            if rows.get("next_cursor"):
                raise FollowupStop("assignment listing is truncated after steer")
            new_tasks = [x["id"] for x in rows["items"] if x.get("trigger_message_id") == sent["message_id"]]
            starts = [x for x in trace if x.get("type") == "run.start"]
            journal.update(steer=evidence, new_assignments_from_steer=new_tasks,
                           run_start_count=len(starts), delivery_events=events,
                           updated_at=dt.datetime.now(dt.timezone.utc).isoformat())
            persist(journal, args.journal)
            if new_tasks or len(starts) != 1 or starts[0].get("run_id") != run_id:
                raise FollowupStop("steer created a new assignment/run")
            if evidence.get("verified") and any(x["event"] == "message.updated" and x["state"] == "read" for x in events):
                journal["status"] = "PASS_LOCAL_BUSY_USER_STEER"
                persist(journal, args.journal)
                return {"status": journal["status"], "assignment_id": args.assignment_id,
                        "message_id": sent["message_id"], "full_s2_pass": False}
            pending_now = [x for x in client.call("approval.list", {"state": ["pending"]})["approvals"] if x.get("assignment_id") == args.assignment_id]
            if pending_now:
                if initial_decision_sent or len(pending_now) != 1 or pending_now[0]["id"] != args.initial_approval_id:
                    raise FollowupStop("a new/unreviewed approval needs separate inspection")
                handle_approvals(client, journal=journal, journal_path=args.journal,
                                 project=project, assignments=[target], request=request,
                                 approve=args.approve_test_tools_once)
                initial_decision_sent = True
            if time.monotonic() >= deadline:
                raise FollowupStop("steer delivery/read/applied evidence timed out")
            time.sleep(min(args.interval, max(0, deadline - time.monotonic())))
    except Exception as exc:
        journal["status"] = "BLOCKED"
        journal["failure_type"] = type(exc).__name__
        journal["failure"] = str(exc) if isinstance(exc, FollowupStop) else "RPC/check failed; inspect journal without resending"
        persist(journal, args.journal)
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--source-journal", type=Path, required=True)
    parser.add_argument("--journal", type=Path, required=True)
    parser.add_argument("--assignment-id", required=True)
    parser.add_argument("--initial-approval-id", help="Exact separately reviewed pending call; no later approval is auto-authorized")
    parser.add_argument("--approve-test-tools-once", action="store_true")
    args = parser.parse_args()
    try:
        print(json.dumps(run(args), ensure_ascii=False))
    except Exception as exc:
        print(f"Busy steer checks stopped: {exc}", file=sys.stderr)
        raise SystemExit(1)
