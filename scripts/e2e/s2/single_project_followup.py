#!/usr/bin/env python3
"""Continue one existing S2 project without creating or confirming anything.

Fresh mode sends exactly one user message to the existing project chat and
persists its request UUID before the RPC.  Resume mode is read-only: it never
resends the message, answers old questions, or approves a pending tool.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    ready_health,
    require_dict,
    require_list,
    require_production_host,
    run_main,
    unique_marker,
)
from s2.project_followup import (  # noqa: E402
    FollowupStop,
    MainRunNotReady,
    assign_trace_result,
    bind_main_run,
    canonical_user_instruction,
    complete_chat_history,
    complete_trace_items,
    discover_main_run_id,
    exact_main_run_request,
    persist,
    project_snapshot,
    send_chat,
    tracked_assignments,
    text_digest,
)
from s2.login_feature import validate_approval_checkpoint  # noqa: E402
from s2.approval_scope import check_approval_scope  # noqa: E402


_RUN_ID_RE = re.compile(r"^run_[A-Za-z0-9_-]+$")


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    add_connection_args(p)
    p.add_argument("--source-journal", type=Path, required=True)
    p.add_argument("--project-id", required=True)
    p.add_argument("--journal", type=Path, required=True)
    p.add_argument("--resume-journal", action="store_true", help="Read the existing journal without sending again")
    p.add_argument("--self-test", action="store_true", help="Run local fake checks without contacting a Host")
    return p


def load_json(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise FollowupStop(f"cannot load {label}") from exc
    if not isinstance(value, dict):
        raise FollowupStop(f"{label} must be an object")
    return value


def source_identity(source: dict[str, Any], project_id: str) -> tuple[dict[str, Any], dict[str, str], str]:
    role_ids = source.get("role_ids")
    main_id = source.get("main_bot_id")
    snapshots = source.get("project_snapshots")
    if not isinstance(role_ids, dict) or set(role_ids) != {"产品", "编码", "测试"}:
        raise FollowupStop("source journal has no complete role identity")
    if not isinstance(main_id, str) or not main_id:
        raise FollowupStop("source journal has no main Bot identity")
    if not isinstance(snapshots, list):
        raise FollowupStop("source journal has no project snapshots")
    snapshot = next((item for item in snapshots if isinstance(item, dict) and item.get("project_id") == project_id), None)
    if not isinstance(snapshot, dict):
        raise FollowupStop("project is not present in source journal")
    for key in ("chat_id", "home_path", "scope_marker", "baseline_assignment_ids"):
        if key not in snapshot:
            raise FollowupStop(f"source project snapshot has no {key}")
    return snapshot, {str(key): str(value) for key, value in role_ids.items()}, main_id


def request_record(journal: dict[str, Any]) -> dict[str, Any]:
    requests = journal.get("requests")
    if not isinstance(requests, list):
        raise FollowupStop("single-project journal has no requests list")
    records = [item for item in requests if isinstance(item, dict) and item.get("kind") == "single_followup"]
    if len(records) != 1:
        raise FollowupStop("single-project journal must contain exactly one followup request")
    record = records[0]
    if record.get("status") not in {"sent", "sent_reconciled_readonly"}:
        raise FollowupStop("single-project request is not safely reconciled")
    return record


def prompt(marker: str, project: dict[str, Any], role_ids: dict[str, str]) -> str:
    home = str(Path(os.path.expanduser(str(project["home_path"]))).resolve())
    return (
        f"{marker}：继续这个已有项目，正确 project_id={project['project_id']}，正确 chat_id={project['chat_id']}。"
        f"只在绝对 Home={home}（namespace={project['scope_marker']}）完成原目标的本地邮箱登录演示；"
        "不要创建项目、确认项目、回答历史问题、访问外部网站/生产系统、启动网络服务或执行 git。"
        f"不要只复写已有 PRD：请主 Bot 先在本群 send_msg 说明续办计划，再通过 assign 派发给编码 Bot {role_ids['编码']}（project_id 使用上述真实项目ID），"
        "由编码 Bot 实际生成或修改 login.html，并由测试 Bot 实际检查后写 TEST.md；完成交接使用 intent=done 并引用真实任务。"
        "只读写这个 Home，禁止全局 projects 探索、自派发同 Bot 子任务或修改其他项目。"
    )


def verify_canonical_request(client: Any, project: dict[str, Any], record: dict[str, Any], marker: str) -> None:
    history = complete_chat_history(client, project["chat_id"], after_seq=max(0, int(record["seq"]) - 1))
    matches = [
        item for item in history["messages"]
        if isinstance(item, dict)
        and item.get("id") == record.get("message_id")
        and item.get("seq") == record.get("seq")
        and item.get("chat_id") == project["chat_id"]
        and isinstance(item.get("sender"), dict)
        and item["sender"].get("kind") == "user"
        and marker in str(item.get("fallback_text", ""))
    ]
    if len(matches) != 1:
        raise FollowupStop("single followup request is not one canonical user message")


def select_triggered_main_assignments(
    assignments: list[dict[str, Any]], *, project: dict[str, Any], message_id: str,
    main_id: str, baseline_ids: set[str],
) -> list[dict[str, Any]]:
    """Select the Main root created directly by this canonical user message.

    ``assignment.from`` is a legacy string in some durable snapshots and a
    structured object in newer responses.  Both forms are accepted only for
    the exact Main identity; trigger/message/project/chat checks stay strict.
    """
    selected = []
    for item in assignments:
        if not isinstance(item, dict):
            continue
        source = item.get("from")
        from_user = source == "user" or (
            isinstance(source, dict) and source.get("kind") == "user"
        )
        if (
            isinstance(item.get("id"), str)
            and item["id"] not in baseline_ids
            and item.get("project_id") == project["project_id"]
            and item.get("origin_chat_id") == project["chat_id"]
            and item.get("bot_id") == main_id
            and item.get("trigger_message_id") == message_id
            and item.get("parent_assignment_id") is None
            and from_user
        ):
            selected.append(item)
    if len(selected) > 1:
        raise FollowupStop("canonical message has multiple Main root assignments")
    return selected


def select_triggered_main_runs(
    trace_items: list[dict[str, Any]], run_requests: dict[str, dict[str, Any]], *,
    project: dict[str, Any], main_assignment_id: str, main_id: str, instruction: str,
) -> list[str]:
    """Select the run whose durable request is tied to the Main assignment."""
    selected = []
    seen: set[str] = set()
    for item in trace_items:
        if not isinstance(item, dict) or item.get("type") != "run.start":
            continue
        run_id = item.get("run_id")
        if not isinstance(run_id, str) or _RUN_ID_RE.fullmatch(run_id) is None or run_id in seen:
            continue
        seen.add(run_id)
        request = run_requests.get(run_id)
        if not isinstance(request, dict):
            continue
        if (
            request.get("run_id") == run_id
            and request.get("assignment_id") == main_assignment_id
            and request.get("bot_id") == main_id
            and request.get("project_id") == project["project_id"]
            and request.get("chat_id") == project["chat_id"]
            and request.get("instruction") == instruction
        ):
            selected.append(run_id)
    if len(selected) > 1:
        raise FollowupStop("canonical Main assignment has multiple exact run requests")
    return selected


def project_assignments(client: Any, project_id: str) -> list[dict[str, Any]]:
    """Read every assignment page; truncation is never treated as absence."""
    rows: list[dict[str, Any]] = []
    cursor: str | None = None
    seen: set[str] = set()
    while True:
        params: dict[str, Any] = {"project_id": project_id, "limit": 100}
        if cursor is not None:
            params["cursor"] = cursor
        result = require_dict(client.call("assignment.list", params), "assignment.list result")
        page = require_list(result.get("items"), "assignment.list.items")
        rows.extend(item for item in page if isinstance(item, dict))
        next_cursor = result.get("next_cursor")
        if next_cursor is None:
            if len(page) >= 100:
                raise FollowupStop("assignment.list reached page limit without next_cursor")
            return rows
        if not isinstance(next_cursor, str) or not next_cursor or next_cursor in seen:
            raise FollowupStop("assignment.list returned an invalid or repeating next_cursor")
        seen.add(next_cursor)
        cursor = next_cursor


def discover_triggered_main_run(
    client: Any, *, project: dict[str, Any], request: dict[str, Any], main_id: str,
) -> dict[str, str] | None:
    """Discover the canonical-trigger path before trying the legacy no-root path.

    A user message can create a Main assignment first.  Its run_request then
    carries that assignment ID, so the old ``assignment_id is None`` discovery
    must not be used for this path.  A sibling Coder assignment from a later
    Main ``send_msg`` is intentionally not a candidate because it has a
    different assignment/trace root.
    """
    instruction, _, message = canonical_user_instruction(client, project=project, request=request)
    assignments = project_assignments(client, project["project_id"])
    baseline = project.get("baseline_assignment_ids", set())
    main_roots = select_triggered_main_assignments(
        assignments, project=project, message_id=message["id"], main_id=main_id,
        baseline_ids=set(baseline),
    )
    if not main_roots:
        return None
    main_assignment_id = main_roots[0]["id"]
    trace_items = complete_trace_items(client, main_assignment_id)
    run_ids = {
        item.get("run_id") for item in trace_items
        if item.get("type") == "run.start" and isinstance(item.get("run_id"), str)
    }
    run_requests: dict[str, dict[str, Any]] = {}
    for run_id in sorted(run_ids):
        try:
            run_requests[run_id] = exact_main_run_request(run_id)
        except FollowupStop:
            continue
    candidates = select_triggered_main_runs(
        trace_items, run_requests, project=project, main_assignment_id=main_assignment_id,
        main_id=main_id, instruction=instruction,
    )
    if not candidates:
        raise MainRunNotReady(f"canonical Main assignment has no durable run yet: {main_assignment_id}")
    return {"run_id": candidates[0], "main_assignment_id": main_assignment_id}


def bind_triggered_main_run(
    client: Any, *, project: dict[str, Any], request: dict[str, Any], run_id: str,
    main_assignment_id: str, role_ids: dict[str, str],
) -> dict[str, Any]:
    """Bind canonical-trigger Main run and its one Coder assign result."""
    run_request = exact_main_run_request(run_id)
    if (
        run_request.get("run_id") != run_id
        or run_request.get("assignment_id") != main_assignment_id
        or run_request.get("bot_id") != "main"
        or run_request.get("project_id") != project["project_id"]
        or run_request.get("chat_id") != project["chat_id"]
    ):
        raise FollowupStop(f"triggered Main run_request identity mismatch for {run_id}")
    canonical, fallback, message = canonical_user_instruction(client, project=project, request=request)
    if run_request.get("instruction") != canonical:
        raise FollowupStop(f"triggered Main instruction is not canonical: {run_id}")
    run_items = [item for item in complete_trace_items(client, main_assignment_id)
                 if isinstance(item, dict) and item.get("run_id") == run_id]
    if not any(item.get("type") == "run.start" for item in run_items):
        raise MainRunNotReady(f"triggered Main run has no run.start: {run_id}")
    start, end = assign_trace_result(run_items, run_id)
    start_data = require_dict(start.get("data"), "assign tool.start data")
    end_data = require_dict(end.get("data"), "assign tool.end data")
    details = require_dict(end_data.get("details"), "assign tool.end.details")
    child_id = details.get("id")
    if not isinstance(child_id, str):
        raise MainRunNotReady(f"triggered Main assign has no child ID: {run_id}")
    assignment_result = require_dict(client.call("assignment.get", {"assignment_id": child_id}), "Coder assignment.get result")
    child = require_dict(assignment_result.get("assignment"), "Coder assignment")
    source = child.get("from")
    from_main = source == "main" or (
        isinstance(source, dict) and source.get("kind") == "bot" and source.get("bot_id") == "main"
    )
    if (
        child.get("id") != child_id or child.get("project_id") != project["project_id"]
        or child.get("origin_chat_id") != project["chat_id"] or child.get("bot_id") != role_ids["编码"]
        or child_id in project.get("baseline_assignment_ids", set()) or not from_main
        or details.get("id") != child_id
        or details.get("project_id") != project["project_id"]
        or details.get("bot_id") != role_ids["编码"]
    ):
        raise FollowupStop(f"triggered Main assign result is not the new Coder assignment: {child_id}")
    return {
        "run_id": run_id, "project_id": project["project_id"], "chat_id": project["chat_id"],
        "main_assignment_id": main_assignment_id, "root_assignment_id": child_id,
        "canonical_message_id": message["id"], "canonical_seq": message["seq"],
        "instruction": text_digest(run_request["instruction"]), "fallback_text": text_digest(fallback),
        "knownText_markdown": text_digest(canonical), "instruction_matches_fallback": True,
        "instruction_matches_knownText": True, "canonical_user_matches": True,
        "trace_run_items": len(run_items), "assign_call_id": start_data.get("call_id"),
        "assign_start_aseq": start.get("aseq"), "assign_end_aseq": end.get("aseq"),
        "triggered_main_assignment": True,
    }


def safe_pending_approvals(
    client: Any, project: dict[str, Any], assignments: list[dict[str, Any]], request: dict[str, Any],
) -> list[dict[str, Any]]:
    ids = {item.get("id") for item in assignments if isinstance(item.get("id"), str)}
    if not ids:
        return []
    result = require_dict(client.call("approval.list", {"state": ["pending"]}), "approval.list result")
    pending = require_list(result.get("approvals"), "approval.list.approvals")
    output = []
    request_at = request.get("message_created_at")
    for candidate in pending:
        if not isinstance(candidate, dict) or candidate.get("assignment_id") not in ids:
            continue
        checkpoint = validate_approval_checkpoint(
            client, candidate, project_id=project["project_id"],
            project_chat_id=project["chat_id"], assignment_ids=ids,
        )
        scope = check_approval_scope(
            candidate, project["home_path"], project["scope_marker"], project_id=project["project_id"],
        )
        output.append({
            "approval_id": candidate.get("id"),
            "assignment_id": candidate.get("assignment_id"),
            "tool": candidate.get("tool"),
            "risk": candidate.get("risk"),
            "created_at": candidate.get("created_at"),
            "scope": scope,
            "checkpoint": checkpoint,
            "created_after_request": bool(request_at is None or candidate.get("created_at") is None or candidate.get("created_at") >= request_at),
        })
    return output


def initialize(args: argparse.Namespace, source: dict[str, Any], project: dict[str, Any], role_ids: dict[str, str], main_id: str) -> tuple[Path, dict[str, Any], bool]:
    if args.resume_journal:
        journal = load_json(args.journal, "resume journal")
        if journal.get("project_id") != args.project_id:
            raise FollowupStop("resume journal project ID mismatch")
        if journal.get("chat_id") != project["chat_id"] or journal.get("home_path") != project["home_path"]:
            raise FollowupStop("resume journal project/chat/Home identity changed")
        if journal.get("role_ids") != role_ids or journal.get("main_bot_id") != main_id:
            raise FollowupStop("resume journal role/Main identity changed")
        return args.journal, journal, True
    if args.journal.exists():
        raise FollowupStop("fresh journal already exists; use --resume-journal")
    marker = unique_marker("macbot-e2e-s2-single")
    journal = {
        "version": 1, "scenario": "single existing project followup", "created_at": now(),
        "source_journal": str(args.source_journal), "project_id": args.project_id,
        "chat_id": project["chat_id"], "home_path": project["home_path"],
        "scope_marker": project["scope_marker"], "role_ids": role_ids, "main_bot_id": main_id,
        "marker": marker, "requests": [], "main_run_discovery": None,
        "main_run_binding": None, "approval_observations": [],
        "full_s2_pass": False, "status": "PREPARED",
    }
    persist(journal, args.journal)
    return args.journal, journal, False


def run(args: argparse.Namespace) -> dict[str, Any]:
    source = load_json(args.source_journal, "source journal")
    source_project, role_ids, main_id = source_identity(source, args.project_id)
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    current = project_snapshot(client, args.project_id, role_ids, main_id)
    for key in ("chat_id", "home_path", "scope_marker"):
        if current.get(key) != source_project.get(key):
            raise FollowupStop(f"current project identity differs at {key}")
    journal_path, journal, resumed = initialize(args, source, current, role_ids, main_id)
    project = dict(current)
    project["baseline_assignment_ids"] = {item for item in source_project["baseline_assignment_ids"] if isinstance(item, str)}
    project["marker"] = journal["marker"]

    if not resumed:
        sent = send_chat(
            client, journal=journal, journal_path=journal_path, project=project,
            kind="single_followup", text=prompt(journal["marker"], project, role_ids),
            mentions=[{"kind": "main"}],
        )
        journal["requests"] = [sent]
        persist(journal, journal_path)
    request = request_record(journal)
    verify_canonical_request(client, project, request, journal["marker"])

    deadline = time.monotonic() + args.timeout
    while True:
        triggered_binding: dict[str, str] | None = None
        triggered_not_ready = False
        try:
            triggered_binding = discover_triggered_main_run(
                client, project=project, request=request, main_id=main_id,
            )
            discovered = triggered_binding["run_id"] if triggered_binding else discover_main_run_id(
                client, project=project, request=request, main_id=main_id,
            )
        except MainRunNotReady as exc:
            triggered_not_ready = True
            journal["status"] = "WAITING_MAIN_TRACE"
            journal["main_run_discovery"] = {
                "status": "not_ready", "candidate_count": 0,
                "error_type": type(exc).__name__,
            }
            persist(journal, journal_path)
            discovered = None
        except FollowupStop as exc:
            journal["status"] = "STOP_PARTIAL"
            journal["main_run_discovery"] = {"status": "error", "error_type": type(exc).__name__}
            persist(journal, journal_path)
            raise
        if discovered is None:
            journal["status"] = "WAITING_MAIN_TRACE"
            if not triggered_not_ready:
                journal["main_run_discovery"] = {"status": "not_ready", "candidate_count": 0}
            persist(journal, journal_path)
        else:
            journal["main_run_discovery"] = {"status": "candidate", "run_id": discovered}
            persist(journal, journal_path)
            try:
                if triggered_binding is not None:
                    binding = bind_triggered_main_run(
                        client, project=project, request=request, run_id=discovered,
                        main_assignment_id=triggered_binding["main_assignment_id"], role_ids=role_ids,
                    )
                else:
                    binding = bind_main_run(
                        client, project=project, request=request, run_id=discovered, role_ids=role_ids,
                    )
            except MainRunNotReady:
                journal["status"] = "WAITING_MAIN_ASSIGN"
                persist(journal, journal_path)
            else:
                journal["main_run_binding"] = binding
                journal["main_root_assignment_id"] = binding["root_assignment_id"]
                journal["status"] = "MAIN_BOUND"
                persist(journal, journal_path)
                assignments = tracked_assignments(
                    client, project, request, project["baseline_assignment_ids"], {binding["root_assignment_id"]},
                )
                pending = safe_pending_approvals(client, project, assignments, request)
                journal["approval_observations"] = pending
                journal["status"] = "PENDING_APPROVAL_REVIEW" if pending else "MAIN_BOUND_NO_PENDING_APPROVAL"
                journal["full_s2_pass"] = False
                persist(journal, journal_path)
                return {
                    "scenario": "single existing project followup",
                    "status": journal["status"], "project_id": args.project_id,
                    "chat_id": project["chat_id"], "main_run_id": discovered,
                    "main_run_binding": binding, "pending_approvals": pending,
                    "journal": str(journal_path), "full_s2_pass": False,
                }
        if resumed or time.monotonic() >= deadline:
            return {
                "scenario": "single existing project followup",
                "status": journal["status"], "project_id": args.project_id,
                "chat_id": project["chat_id"], "journal": str(journal_path),
                "full_s2_pass": False,
            }
        time.sleep(min(args.interval, max(0.0, deadline - time.monotonic())))


def fake_tests() -> dict[str, Any]:
    trace = [
        {"type": "run.start", "run_id": "run_single"},
        {"type": "tool.start", "run_id": "run_single", "data": {"name": "assign", "call_id": "call_single"}},
        {"type": "tool.end", "run_id": "run_single", "data": {"call_id": "call_single", "details": {"id": "asg_single"}}},
    ]
    start, end = assign_trace_result(trace, "run_single")
    assert start["data"]["call_id"] == "call_single"
    assert end["data"]["details"]["id"] == "asg_single"
    project = {"project_id": "project_single", "chat_id": "chat_single"}
    main = select_triggered_main_assignments(
        [
            {"id": "main_asg", "project_id": "project_single", "origin_chat_id": "chat_single",
             "bot_id": "main", "trigger_message_id": "msg_single", "parent_assignment_id": None,
             "from": "user"},
            {"id": "sibling", "project_id": "project_single", "origin_chat_id": "chat_single",
             "bot_id": "coder", "trigger_message_id": "msg_single", "parent_assignment_id": None,
             "from": "main"},
        ], project=project, message_id="msg_single", main_id="main", baseline_ids=set(),
    )
    assert [item["id"] for item in main] == ["main_asg"]
    try:
        select_triggered_main_assignments(
            [
                {"id": "main_asg_1", "project_id": "project_single", "origin_chat_id": "chat_single",
                 "bot_id": "main", "trigger_message_id": "msg_single", "parent_assignment_id": None,
                 "from": "user"},
                {"id": "main_asg_2", "project_id": "project_single", "origin_chat_id": "chat_single",
                 "bot_id": "main", "trigger_message_id": "msg_single", "parent_assignment_id": None,
                 "from": "user"},
            ], project=project, message_id="msg_single", main_id="main", baseline_ids=set(),
        )
    except FollowupStop:
        pass
    else:
        raise AssertionError("duplicate canonical Main roots must stop")
    selected = select_triggered_main_runs(
        [{"type": "run.start", "run_id": "run_triggered"}],
        {"run_triggered": {"run_id": "run_triggered", "assignment_id": "main_asg",
                            "bot_id": "main", "project_id": "project_single", "chat_id": "chat_single",
                            "instruction": "exact"}},
        project=project, main_assignment_id="main_asg", main_id="main", instruction="exact",
    )
    assert selected == ["run_triggered"]
    assert select_triggered_main_runs(
        [{"type": "run.start", "run_id": "run_wrong"}],
        {"run_wrong": {"run_id": "run_wrong", "assignment_id": "other_asg", "bot_id": "main",
                        "project_id": "project_single", "chat_id": "chat_single", "instruction": "exact"}},
        project=project, main_assignment_id="main_asg", main_id="main", instruction="exact",
    ) == []
    assert "project.create" not in prompt("m", {"project_id": "p", "chat_id": "c", "home_path": "/tmp/home", "scope_marker": "home"}, {"编码": "coder", "测试": "tester"})
    return {"scenario": "single_project_followup fake checks", "status": "PASS", "network": False}


if __name__ == "__main__":
    if "--self-test" in sys.argv[1:]:
        print(json.dumps(fake_tests(), ensure_ascii=False, sort_keys=True))
        raise SystemExit(0)
    raise SystemExit(run_main(run, parser().parse_args()))
