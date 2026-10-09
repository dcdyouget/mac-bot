#!/usr/bin/env python3
"""Continue work in two existing S2 project chats without creating projects.

This is an API-check helper for a separately authorized run.  It never calls
``project.create`` or ``project.confirm_done`` and never answers an existing
question.  A journal is written before every ``chat.send`` so an unknown HTTP
outcome cannot be retried accidentally.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import sys
import tempfile
import uuid
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    parse_time,
    ready_health,
    require_production_host,
    require_dict,
    require_list,
    run_main,
    unique_marker,
)
from s2.approval_scope import check_approval_scope  # noqa: E402
from s2.login_feature import validate_approval_checkpoint  # noqa: E402


class FollowupStop(ValueError):
    """A strict API check stopped and left the project state for review."""


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    add_connection_args(p)
    p.add_argument("--project-id", action="append", required=True, help="Existing project ID; pass exactly twice")
    p.add_argument("--product-bot-id", required=True)
    p.add_argument("--coding-bot-id", required=True)
    p.add_argument("--test-bot-id", required=True)
    p.add_argument(
        "--main-run-id", action="append",
        help="Optional exact Main run_request IDs in project order, for trace-based binding during resume",
    )
    p.add_argument("--journal", type=Path, help="New journal path; must not already exist")
    p.add_argument("--resume-journal", type=Path, help="Resume only the requests recorded in this journal")
    p.add_argument("--approve-test-tools-once", action="store_true", help="Allow only exact, journal-scoped tool approvals")
    p.add_argument("--self-test", action="store_true", help="Run local fake trace-selection checks; never contacts a Host")
    return p


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def atomic_write(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def persist(journal: dict[str, Any], path: Path) -> None:
    atomic_write(path, journal)


def safe_message(message: dict[str, Any], marker: str) -> dict[str, Any]:
    blocks = []
    for block in message.get("blocks", []) if isinstance(message.get("blocks"), list) else []:
        if not isinstance(block, dict):
            continue
        item = {key: block.get(key) for key in ("type", "state", "project_id", "question_id", "assignment_id") if key in block}
        if isinstance(block.get("options"), list):
            item["options_count"] = len(block["options"])
        blocks.append(item)
    sender = message.get("sender") if isinstance(message.get("sender"), dict) else {}
    return {
        "id": message.get("id"),
        "seq": message.get("seq"),
        "created_at": message.get("created_at"),
        "sender_kind": sender.get("kind"),
        "sender_bot_id": sender.get("bot_id"),
        "assignment_id": message.get("assignment_id"),
        "intent": message.get("intent"),
        "reply_to": message.get("reply_to"),
        "marker_present": marker in str(message.get("fallback_text", "")),
        "blocks": blocks,
        "mentions": [
            {key: item.get(key) for key in ("kind", "bot_id") if key in item}
            for item in message.get("mentions", [])
            if isinstance(item, dict)
        ],
    }


def project_snapshot(client: Any, project_id: str, role_ids: dict[str, str], main_id: str) -> dict[str, Any]:
    result = require_dict(client.call("project.get", {"project_id": project_id}), "project.get result")
    project = require_dict(result.get("project"), "project.get.project")
    chat_result = require_dict(client.call("chat.get", {"chat_id": project.get("chat_id")}), "chat.get result")
    chat = require_dict(chat_result.get("chat"), "chat.get.chat")
    if project.get("id") != project_id or project.get("chat_id") != chat.get("id"):
        raise FollowupStop("project/chat identity mismatch")
    if chat.get("kind") != "project" or chat.get("project_id") != project_id:
        raise FollowupStop("existing chat is not the requested project chat")
    members = {
        item.get("bot_id")
        for item in require_list(project.get("members"), "project.members")
        if isinstance(item, dict)
    }
    expected = {main_id, *role_ids.values()}
    if not expected <= members:
        raise FollowupStop("existing project members do not contain main/product/coding/test IDs")
    home = project.get("home_path")
    if not isinstance(home, str) or not home:
        raise FollowupStop("existing project has no Home path")
    scope_marker = Path(os.path.expanduser(home)).name
    if not scope_marker:
        raise FollowupStop("existing project Home has no namespace marker")
    return {
        "project_id": project_id,
        "chat_id": project["chat_id"],
        "home_path": home,
        "scope_marker": scope_marker,
        "name": project.get("name"),
        "goal": project.get("goal"),
        "status": project.get("status"),
        "flow": project.get("flow"),
        "member_bot_ids": sorted(str(item) for item in members),
        "baseline_assignments": assignment_snapshot(client, project_id),
    }


def assignment_snapshot(client: Any, project_id: str) -> list[dict[str, Any]]:
    rows = []
    cursor: str | None = None
    seen_cursors: set[str] = set()
    while True:
        params: dict[str, Any] = {"project_id": project_id, "limit": 100}
        if cursor is not None:
            params["cursor"] = cursor
        result = require_dict(client.call("assignment.list", params), "assignment.list result")
        page_items = require_list(result.get("items"), "assignment.list.items")
        for assignment in page_items:
            if not isinstance(assignment, dict):
                continue
            if assignment.get("project_id") not in (None, project_id):
                raise FollowupStop("assignment.list returned an unrelated project assignment")
            rows.append({
                key: assignment.get(key)
                for key in (
                    "id", "project_id", "bot_id", "status", "parent_assignment_id", "trigger_message_id",
                    "from", "origin_chat_id", "started_at", "finished_at", "result_message_id",
                )
            })
        next_cursor = result.get("next_cursor")
        if next_cursor is None:
            if len(page_items) >= 100:
                raise FollowupStop("assignment.list reached page limit without next_cursor")
            break
        if not isinstance(next_cursor, str) or not next_cursor or next_cursor in seen_cursors:
            raise FollowupStop("assignment.list returned an invalid or repeating next_cursor")
        seen_cursors.add(next_cursor)
        cursor = next_cursor
    return rows


def complete_chat_history(client: Any, chat_id: str, *, after_seq: int | None = None) -> dict[str, Any]:
    history = chat_history(client, chat_id, after_seq=after_seq)
    if history.get("has_more") is True:
        raise FollowupStop("chat.history is truncated; refusing to infer canonical routing from partial history")
    return history


_RUN_ID_RE = re.compile(r"^run_[A-Za-z0-9_-]+$")


def text_digest(value: str) -> dict[str, Any]:
    return {"len": len(value), "sha256": hashlib.sha256(value.encode("utf-8")).hexdigest()}


def exact_main_run_request(run_id: str) -> dict[str, Any]:
    if not isinstance(run_id, str) or _RUN_ID_RE.fullmatch(run_id) is None:
        raise FollowupStop("main run ID has an unsafe format")
    path = Path.home() / "MacBot" / "data" / "run_requests" / f"{run_id}.json"
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise FollowupStop(f"main run_request is missing or invalid: {run_id}") from exc
    if not isinstance(value, dict):
        raise FollowupStop(f"main run_request is not an object: {run_id}")
    return value


class MainRunNotReady(FollowupStop):
    """A strict candidate exists, but its assign trace is not complete yet."""


def canonical_user_instruction(
    client: Any, *, project: dict[str, Any], request: dict[str, Any]
) -> tuple[str, str, dict[str, Any]]:
    """Return the one canonical user instruction without exposing its text."""

    history = complete_chat_history(client, project["chat_id"])
    canonical = [
        item for item in history["messages"]
        if isinstance(item, dict)
        and item.get("id") == request.get("message_id")
        and item.get("seq") == request.get("seq")
        and item.get("chat_id", project["chat_id"]) == project["chat_id"]
        and isinstance(item.get("sender"), dict)
        and item["sender"].get("kind") == "user"
    ]
    if len(canonical) != 1:
        raise FollowupStop(f"canonical followup message is not unique for {project['project_id']}")
    message = canonical[0]
    fallback = message.get("fallback_text")
    markdowns = [
        block.get("markdown")
        for block in message.get("blocks", [])
        if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("markdown"), str)
    ]
    exact_fallbacks = [
        item for item in history["messages"]
        if isinstance(item, dict)
        and isinstance(item.get("sender"), dict)
        and item["sender"].get("kind") == "user"
        and item.get("chat_id", project["chat_id"]) == project["chat_id"]
        and item.get("fallback_text") == fallback
    ]
    if (
        not isinstance(fallback, str)
        or len(exact_fallbacks) != 1
        or exact_fallbacks[0].get("id") != message.get("id")
        or len(markdowns) != 1
        or fallback != markdowns[0]
    ):
        raise FollowupStop("canonical followup is not the unique fallback/knownText message")
    return fallback, fallback, message


def select_main_run_candidates(
    trace_items: list[dict[str, Any]],
    run_requests: dict[str, dict[str, Any]],
    *,
    project_id: str,
    chat_id: str,
    main_id: str,
    instruction: str,
) -> list[str]:
    """Select exact run.start candidates; no time, adjacency, or marker guessing."""

    seen: set[str] = set()
    candidates: list[str] = []
    for item in trace_items:
        if not isinstance(item, dict) or item.get("type") != "run.start":
            continue
        run_id = item.get("run_id")
        if not isinstance(run_id, str) or _RUN_ID_RE.fullmatch(run_id) is None or run_id in seen:
            continue
        seen.add(run_id)
        run_request = run_requests.get(run_id)
        if not isinstance(run_request, dict):
            continue
        if run_request.get("bot_id") != main_id or run_request.get("assignment_id") is not None:
            continue
        if run_request.get("instruction") != instruction:
            continue
        identity = (
            run_request.get("run_id") == run_id
            and run_request.get("bot_id") == main_id
            and run_request.get("assignment_id") is None
            and run_request.get("project_id") == project_id
            and run_request.get("chat_id") == chat_id
        )
        if not identity:
            raise FollowupStop(f"run.start candidate has conflicting Main/run_request identity: {run_id}")
        candidates.append(run_id)
    if len(candidates) > 1:
        raise FollowupStop("multiple exact canonical Main run candidates; refusing ambiguous binding")
    return candidates


def assign_trace_result(run_items: list[dict[str, Any]], run_id: str) -> tuple[dict[str, Any], dict[str, Any]]:
    """Pair the one assign start/end and return its safe start/end metadata."""

    assign_starts = [
        item for item in run_items
        if item.get("type") == "tool.start"
        and isinstance(item.get("data"), dict)
        and item["data"].get("name") == "assign"
    ]
    if len(assign_starts) > 1:
        raise FollowupStop(f"main run {run_id} has multiple assign traces; refusing ambiguous binding")
    if not assign_starts:
        raise MainRunNotReady(f"main run {run_id} has no unique assign trace")
    start = assign_starts[0]
    start_data = require_dict(start.get("data"), "main assign tool.start data")
    call_id = start_data.get("call_id")
    if not isinstance(call_id, str):
        raise MainRunNotReady(f"main assign trace has no call_id: {run_id}")
    ends = [
        item for item in run_items
        if item.get("type") == "tool.end"
        and isinstance(item.get("data"), dict)
        and item["data"].get("call_id") == call_id
    ]
    if len(ends) != 1:
        if len(ends) > 1:
            raise FollowupStop(f"main assign call has duplicate tool.end entries: {run_id}")
        raise MainRunNotReady(f"main assign call is not paired with exactly one tool.end: {run_id}")
    end_data = require_dict(ends[0].get("data"), "main assign tool.end data")
    details = require_dict(end_data.get("details"), "main assign tool.end.details")
    if not isinstance(details.get("id"), str):
        raise MainRunNotReady(f"main assign result has no assignment ID: {run_id}")
    return start, ends[0]


def bind_main_run(
    client: Any,
    *,
    project: dict[str, Any],
    request: dict[str, Any],
    run_id: str,
    role_ids: dict[str, str],
) -> dict[str, Any]:
    run_request = exact_main_run_request(run_id)
    if (
        run_request.get("run_id") != run_id
        or run_request.get("bot_id") != "main"
        or run_request.get("assignment_id") is not None
        or run_request.get("project_id") != project["project_id"]
        or run_request.get("chat_id") != project["chat_id"]
    ):
        raise FollowupStop(f"main run_request identity mismatch for {run_id}")
    instruction = run_request.get("instruction")
    if not isinstance(instruction, str):
        raise FollowupStop(f"main run_request has no instruction: {run_id}")
    canonical_instruction, fallback, message = canonical_user_instruction(
        client, project=project, request=request
    )
    markdowns = [
        block.get("markdown")
        for block in message.get("blocks", [])
        if isinstance(block, dict) and block.get("type") == "text" and isinstance(block.get("markdown"), str)
    ]
    if instruction != canonical_instruction:
        raise FollowupStop(f"main run instruction is not the canonical followup for {run_id}")
    trace_result = require_dict(
        client.call("trace.history", {"chat_id": project["chat_id"], "tail": True, "limit": 500}),
        "main chat trace.history result",
    )
    if trace_result.get("has_more_before") is True:
        raise FollowupStop(f"main chat trace is truncated for {run_id}")
    run_items = [
        item for item in require_list(trace_result.get("items"), "main chat trace.items")
        if isinstance(item, dict) and item.get("run_id") == run_id
    ]
    start, end = assign_trace_result(run_items, run_id)
    start_data = require_dict(start.get("data"), "main assign tool.start data")
    call_id = start_data["call_id"]
    ends = [end]
    end_data = require_dict(end.get("data"), "main assign tool.end data")
    details = require_dict(end_data.get("details"), "main assign tool.end.details")
    root_id = details.get("id")
    if not isinstance(root_id, str):
        raise FollowupStop(f"main assign result has no assignment ID: {run_id}")
    assignment_result = require_dict(client.call("assignment.get", {"assignment_id": root_id}), "main root assignment.get result")
    assignment = require_dict(assignment_result.get("assignment"), "main root assignment")
    root_from = assignment.get("from") if isinstance(assignment.get("from"), dict) else {}
    if (
        assignment.get("id") != root_id
        or assignment.get("project_id") != project["project_id"]
        or assignment.get("origin_chat_id") != project["chat_id"]
        or assignment.get("bot_id") != role_ids["编码"]
        or assignment.get("parent_assignment_id") is not None
        or assignment.get("trigger_message_id") is not None
        or root_from.get("kind") != "bot"
        or root_from.get("bot_id") != "main"
        or root_id in project["baseline_assignment_ids"]
        or details.get("project_id") != project["project_id"]
        or details.get("bot_id") != role_ids["编码"]
    ):
        raise FollowupStop(f"main assign result does not exactly bind a new project root: {root_id}")
    return {
        "run_id": run_id,
        "project_id": project["project_id"],
        "chat_id": project["chat_id"],
        "assignment_id": None,
        "canonical_message_id": message["id"],
        "canonical_seq": message["seq"],
        "instruction": text_digest(instruction),
        "fallback_text": text_digest(fallback),
        "knownText_markdown": text_digest(markdowns[0]),
        "instruction_matches_fallback": True,
        "instruction_matches_knownText": True,
        "canonical_user_matches": True,
        "trace_run_items": len(run_items),
        "assign_call_id": call_id,
        "assign_start_aseq": start.get("aseq"),
        "assign_end_aseq": end.get("aseq"),
        "root_assignment_id": root_id,
    }


def resolve_main_run_ids(args: argparse.Namespace, journal: dict[str, Any], resumed: bool) -> list[str] | None:
    supplied = args.main_run_id
    if supplied is not None:
        if len(supplied) != 2 or len(set(supplied)) != 2:
            raise FollowupStop("pass exactly two distinct --main-run-id values in project order")
        if any(_RUN_ID_RE.fullmatch(item) is None for item in supplied):
            raise FollowupStop("main run IDs have an unsafe format")
        if resumed and journal.get("main_run_ids") is not None and journal.get("main_run_ids") != supplied:
            raise FollowupStop("resume journal main run IDs do not match CLI")
        return supplied
    saved = journal.get("main_run_ids")
    if saved is None:
        return None
    if not isinstance(saved, list) or len(saved) != 2 or len(set(saved)) != 2 or any(not isinstance(item, str) or _RUN_ID_RE.fullmatch(item) is None for item in saved):
        raise FollowupStop("journal main run IDs are invalid")
    return saved


def bind_main_runs(
    client: Any,
    *,
    journal: dict[str, Any],
    journal_path: Path,
    projects: list[dict[str, Any]],
    request_by_project: dict[str, dict[str, Any]],
    main_run_ids: list[str],
    role_ids: dict[str, str],
) -> None:
    bindings_by_project = {
        item.get("project_id"): item
        for item in journal.get("main_run_bindings", [])
        if isinstance(item, dict) and isinstance(item.get("project_id"), str)
    }
    for project, run_id in zip(projects, main_run_ids):
        try:
            binding = bind_main_run(
                client,
                project=project,
                request=request_by_project[project["project_id"]],
                run_id=run_id,
                role_ids=role_ids,
            )
        except Exception as exc:
            journal.setdefault("main_binding_failures", []).append({
                "project_id": project["project_id"],
                "run_id": run_id,
                "status": "STOP/PARTIAL",
                "error_type": type(exc).__name__,
            })
            persist(journal, journal_path)
            raise FollowupStop(f"main run binding STOP/PARTIAL for project {project['project_id']}")
        project["main_root_assignment_ids"] = {binding["root_assignment_id"]}
        bindings_by_project[project["project_id"]] = binding
        journal["main_run_bindings"] = [
            bindings_by_project[key]
            for key in (item["project_id"] for item in projects)
            if key in bindings_by_project
        ]
        journal["main_run_ids"] = main_run_ids
        journal["main_root_assignment_ids"] = {
            item["project_id"]: [item["root_assignment_id"]]
            for item in journal["main_run_bindings"]
        }
        persist(journal, journal_path)


def discover_main_run_id(
    client: Any,
    *,
    project: dict[str, Any],
    request: dict[str, Any],
    main_id: str,
) -> str | None:
    """Find one exact Main root from this project's trace, or return not-ready.

    A run.start is only a candidate.  The corresponding local run_request must
    prove the Main/project/chat/assignment identity and exact canonical
    instruction before the candidate can be bound.  No timestamps, sender
    adjacency, marker substring, or assignment proximity are used.
    """

    instruction, _, _ = canonical_user_instruction(client, project=project, request=request)
    result = require_dict(
        client.call("trace.history", {"chat_id": project["chat_id"], "tail": True, "limit": 500}),
        "main discovery trace.history result",
    )
    if result.get("has_more_before") is True:
        raise FollowupStop("main discovery trace is truncated; refusing to infer a run")
    trace_items = [item for item in require_list(result.get("items"), "main discovery trace.items") if isinstance(item, dict)]
    run_ids = {
        item.get("run_id")
        for item in trace_items
        if item.get("type") == "run.start"
        and isinstance(item.get("run_id"), str)
        and _RUN_ID_RE.fullmatch(item["run_id"])
    }
    run_requests: dict[str, dict[str, Any]] = {}
    for run_id in sorted(run_ids):
        try:
            run_requests[run_id] = exact_main_run_request(run_id)
        except FollowupStop:
            # A trace can precede durable run_request materialization.  It is
            # not evidence of identity and remains a not-ready candidate.
            continue
    candidates = select_main_run_candidates(
        trace_items,
        run_requests,
        project_id=project["project_id"],
        chat_id=project["chat_id"],
        main_id=main_id,
        instruction=instruction,
    )
    return candidates[0] if candidates else None


def auto_bind_main_runs(
    client: Any,
    *,
    journal: dict[str, Any],
    journal_path: Path,
    projects: list[dict[str, Any]],
    request_by_project: dict[str, dict[str, Any]],
    role_ids: dict[str, str],
    main_id: str,
) -> None:
    """Resume-safe discovery/binding for journals without explicit run IDs."""

    bindings_by_project = {
        item.get("project_id"): item
        for item in journal.get("main_run_bindings", [])
        if isinstance(item, dict) and isinstance(item.get("project_id"), str)
    }
    discovery_by_project = {
        item.get("project_id"): item
        for item in journal.get("main_run_discoveries", [])
        if isinstance(item, dict) and isinstance(item.get("project_id"), str)
    }
    for project in projects:
        project_id = project["project_id"]
        binding = bindings_by_project.get(project_id)
        if isinstance(binding, dict) and isinstance(binding.get("root_assignment_id"), str):
            project["main_root_assignment_ids"] = {binding["root_assignment_id"]}
            continue
        candidate = discover_main_run_id(
            client,
            project=project,
            request=request_by_project[project_id],
            main_id=main_id,
        )
        if candidate is None:
            discovery_by_project[project_id] = {
                "project_id": project_id,
                "status": "not_ready",
                "candidate_count": 0,
            }
            journal["main_run_discoveries"] = list(discovery_by_project.values())
            persist(journal, journal_path)
            continue
        discovery_by_project[project_id] = {
            "project_id": project_id,
            "run_id": candidate,
            "status": "candidate",
        }
        journal["main_run_discoveries"] = list(discovery_by_project.values())
        persist(journal, journal_path)
        try:
            binding = bind_main_run(
                client,
                project=project,
                request=request_by_project[project_id],
                run_id=candidate,
                role_ids=role_ids,
            )
        except MainRunNotReady as exc:
            discovery_by_project[project_id]["status"] = "not_ready"
            discovery_by_project[project_id]["error_type"] = type(exc).__name__
            journal["main_run_discoveries"] = list(discovery_by_project.values())
            persist(journal, journal_path)
            continue
        bindings_by_project[project_id] = binding
        project["main_root_assignment_ids"] = {binding["root_assignment_id"]}
        journal["main_run_bindings"] = [
            bindings_by_project[item["project_id"]]
            for item in projects
            if item["project_id"] in bindings_by_project
        ]
        journal["main_root_assignment_ids"] = {
            item["project_id"]: [item["root_assignment_id"]]
            for item in journal["main_run_bindings"]
        }
        discovery_by_project[project_id]["status"] = "bound"
        discovery_by_project[project_id]["root_assignment_id"] = binding["root_assignment_id"]
        journal["main_run_discoveries"] = list(discovery_by_project.values())
        persist(journal, journal_path)
    if len(bindings_by_project) == len(projects):
        ordered = [bindings_by_project[item["project_id"]]["run_id"] for item in projects]
        if len(set(ordered)) != len(ordered):
            raise FollowupStop("auto-discovered Main run IDs are not distinct")
        journal["main_run_ids"] = ordered
        persist(journal, journal_path)


def load_or_initialize(args: argparse.Namespace, project_ids: list[str]) -> tuple[Path, dict[str, Any], bool]:
    if args.resume_journal is not None:
        if args.journal is not None and args.journal != args.resume_journal:
            raise FollowupStop("--journal and --resume-journal must identify the same path")
        path = args.resume_journal
        try:
            journal = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise FollowupStop(f"cannot load resume journal: {exc}") from exc
        if not isinstance(journal, dict) or journal.get("project_ids") != project_ids:
            raise FollowupStop("resume journal project IDs do not match CLI")
        return path, journal, True
    if args.journal is None:
        raise FollowupStop("fresh run requires --journal; this prevents accidental evidence overwrite")
    if args.journal.exists():
        raise FollowupStop("fresh --journal already exists; use --resume-journal")
    markers = [unique_marker("macbot-e2e-s2-followup") for _ in project_ids]
    journal = {
        "version": 1,
        "created_at": now(),
        "project_ids": project_ids,
        "markers": markers,
        "requests": [],
        "steers": [],
        "approval_observations": [],
        "observations": [],
        "full_s2_pass": False,
    }
    persist(journal, args.journal)
    return args.journal, journal, False


def verify_sent_request(client: Any, chat_id: str, record: dict[str, Any], marker: str) -> dict[str, Any]:
    history = complete_chat_history(client, chat_id, after_seq=max(0, int(record["seq"]) - 1))
    matches = [
        item for item in history["messages"]
        if isinstance(item, dict)
        and item.get("id") == record.get("message_id")
        and item.get("seq") == record.get("seq")
        and isinstance(item.get("sender"), dict)
        and item["sender"].get("kind") == "user"
        and item.get("chat_id", chat_id) == chat_id
    ]
    if len(matches) != 1 or marker not in str(matches[0].get("fallback_text", "")):
        raise FollowupStop("journal request does not match exactly one canonical chat message")
    return safe_message(matches[0], marker)


def reconcile_unknown_send(client: Any, journal: dict[str, Any], journal_path: Path, project: dict[str, Any], record: dict[str, Any]) -> None:
    """Read-only reconciliation for a timeout; never retries the write RPC."""

    history = complete_chat_history(client, project["chat_id"])
    matches = [
        item for item in history["messages"]
        if isinstance(item, dict)
        and record.get("marker") in str(item.get("fallback_text", ""))
        and isinstance(item.get("id"), str)
        and isinstance(item.get("seq"), int)
        and isinstance(item.get("sender"), dict)
        and item["sender"].get("kind") == "user"
        and item.get("chat_id", project["chat_id"]) == project["chat_id"]
    ]
    sent_at = parse_time(record.get("created_at"))
    if sent_at is not None:
        matches = [item for item in matches if parse_time(item.get("created_at")) is None or parse_time(item.get("created_at")) >= sent_at]
    if len(matches) != 1:
        raise FollowupStop(
            f"unknown chat.send {record.get('client_request_id')} has {len(matches)} canonical marker matches; refusing to resend"
        )
    message = matches[0]
    record.update({
        "status": "sent_reconciled_readonly",
        "message_id": message["id"],
        "seq": message["seq"],
        "message_created_at": message.get("created_at"),
    })
    journal.setdefault("reconciliations", []).append({
        "project_id": project["project_id"],
        "client_request_id": record.get("client_request_id"),
        "message_id": message["id"],
        "seq": message["seq"],
        "status": "canonical_match_readonly",
    })
    persist(journal, journal_path)


def send_chat(
    client: Any,
    *,
    journal: dict[str, Any],
    journal_path: Path,
    project: dict[str, Any],
    kind: str,
    text: str,
    mentions: list[dict[str, Any]],
    assignment_id: str | None = None,
) -> dict[str, Any]:
    marker = project["marker"]
    request_id = str(uuid.uuid4())
    record = {
        "project_id": project["project_id"],
        "marker": marker,
        "kind": kind,
        "client_request_id": request_id,
        "status": "pending_send",
        "created_at": now(),
    }
    if assignment_id is not None:
        record["assignment_id"] = assignment_id
    journal.setdefault("requests", []).append(record)
    persist(journal, journal_path)
    try:
        result = require_dict(
            client.call(
                "chat.send",
                {
                    "chat_id": project["chat_id"],
                    "text": text,
                    "mentions": mentions,
                    "client_request_id": request_id,
                },
            ),
            f"{kind} chat.send result",
        )
    except Exception:
        record["status"] = "unknown_result"
        persist(journal, journal_path)
        raise
    message = require_dict(result.get("message"), f"{kind} message")
    if not isinstance(message.get("id"), str) or not isinstance(message.get("seq"), int):
        record["status"] = "invalid_result"
        persist(journal, journal_path)
        raise FollowupStop(f"{kind} chat.send returned no canonical id/seq")
    record.update({"status": "sent", "message_id": message["id"], "seq": message["seq"], "message_created_at": message.get("created_at")})
    persist(journal, journal_path)
    return record


def request_text(marker: str, project: dict[str, Any], role_ids: dict[str, str]) -> str:
    goal = project.get("goal") if isinstance(project.get("goal"), str) and project.get("goal") else "项目已有目标"
    home = str(Path(os.path.expanduser(project["home_path"])).resolve())
    return (
        f"{marker}：这是既有项目的继续执行请求，请以服务端记录的原目标为准：{goal}。"
        f"正确目标群 chat_id={project['chat_id']}；项目 ID={project['project_id']} 不是 chat_id。"
        f"只允许直接在本项目绝对 Home={home}（namespace={project['scope_marker']}）内读写演示产物，不探索全局 projects 目录。"
        f"不要只发 PRD 或口头完成；请主 Bot 先 send_msg 在正确群内说明开场计划（mentions=[]），再调用 assign 将本项目任务派给编码 Bot（{role_ids['编码']}），不要自派发同 Bot 子任务，"
        f"再让测试 Bot（{role_ids['测试']}）实际验证并写入测试报告；完成交接时使用 intent=done 并 @下一角色，且必须引用真实 assignment 和结果。"
        "使用本项目绝对路径直接 read/write/edit 产物；不要调用 Bash、sleep 或探索全局目录。产物须包含本项目 namespace 和本次 marker，便于核验。只允许本机静态/本地演示与测试，不启动网络服务，不访问外部网站或生产系统，不执行 git commit/push 或部署。"
        f"本次 marker={marker}；沿用历史决定，不回答旧 Question，不调用 project.create 或 project.confirm_done。"
    )


def ensure_requests(client: Any, journal: dict[str, Any], journal_path: Path, projects: list[dict[str, Any]], role_ids: dict[str, str]) -> None:
    followups = [
        item for item in journal.get("requests", [])
        if isinstance(item, dict) and item.get("kind") == "followup_request"
    ]
    by_project: dict[str, dict[str, Any]] = {}
    for item in followups:
        project_id = item.get("project_id")
        if not isinstance(project_id, str) or project_id in by_project:
            raise FollowupStop("journal must contain at most one followup chat.send per project")
        by_project[project_id] = item
    for project in projects:
        record = by_project.get(project["project_id"])
        if record is not None:
            if record.get("status") in {"pending_send", "unknown_result"}:
                reconcile_unknown_send(client, journal, journal_path, project, record)
            verify_sent_request(client, project["chat_id"], record, project["marker"])
            continue
        sent = send_chat(
            client,
            journal=journal,
            journal_path=journal_path,
            project=project,
            kind="followup_request",
            text=request_text(project["marker"], project, role_ids),
            mentions=[],
        )
        project["request"] = {key: sent.get(key) for key in ("message_id", "seq", "message_created_at", "client_request_id")}
        persist(journal, journal_path)


def tracked_assignments(
    client: Any,
    project: dict[str, Any],
    request: dict[str, Any],
    baseline_ids: set[str],
    root_ids: set[str] | None = None,
) -> list[dict[str, Any]]:
    rows = assignment_snapshot(client, project["project_id"])
    baseline = set(baseline_ids)
    roots = set(root_ids or set())
    known: set[str] = set(roots)
    result = [row for row in rows if isinstance(row.get("id"), str) and row.get("id") in roots and row.get("id") not in baseline]
    changed = True
    while changed:
        changed = False
        for row in rows:
            row_id = row.get("id")
            if not isinstance(row_id, str) or row_id in baseline or row_id in known:
                continue
            if row.get("trigger_message_id") == request.get("message_id") or row.get("parent_assignment_id") in known:
                known.add(row_id)
                result.append(row)
                changed = True
    return result


def pending_question(client: Any, assignment_ids: set[str]) -> list[dict[str, Any]]:
    state = bootstrap(client)
    pending = require_dict(state.get("pending"), "bootstrap.pending")
    return [
        item for item in require_list(pending.get("questions"), "bootstrap.pending.questions")
        if isinstance(item, dict) and item.get("assignment_id") in assignment_ids and item.get("state") == "pending"
    ]


def run_fake_tests() -> dict[str, Any]:
    """Exercise discovery gates without a network, Host, credentials, or writes."""

    def request(run_id: str, instruction: str, *, project: str = "p1", chat: str = "c1") -> dict[str, Any]:
        return {
            "run_id": run_id,
            "bot_id": "main",
            "assignment_id": None,
            "project_id": project,
            "chat_id": chat,
            "instruction": instruction,
        }

    def start(run_id: str) -> dict[str, Any]:
        return {"type": "run.start", "run_id": run_id}

    checks: list[str] = []
    assert select_main_run_candidates(
        [start("run_not_ready")], {}, project_id="p1", chat_id="c1", main_id="main", instruction="m"
    ) == []
    checks.append("not_ready")
    assert select_main_run_candidates(
        [start("run_wrong")], {"run_wrong": request("run_wrong", "other")},
        project_id="p1", chat_id="c1", main_id="main", instruction="m",
    ) == []
    checks.append("wrong_request")
    child = request("run_child", "m")
    child.update(bot_id="coder", assignment_id="child")
    assert select_main_run_candidates(
        [start("run_old"), start("run_child"), start("run_main")],
        {"run_old": request("run_old", "old", project="old"), "run_child": child, "run_main": request("run_main", "m")},
        project_id="p1", chat_id="c1", main_id="main", instruction="m",
    ) == ["run_main"]
    checks.append("unrelated_history_and_child_excluded")
    try:
        select_main_run_candidates(
            [start("run_conflict")], {"run_conflict": request("run_conflict", "m", chat="wrong")},
            project_id="p1", chat_id="c1", main_id="main", instruction="m",
        )
    except FollowupStop:
        checks.append("exact_instruction_identity_conflict")
    else:
        raise AssertionError("conflicting Main identity was accepted")
    try:
        select_main_run_candidates(
            [start("run_a"), start("run_b")],
            {"run_a": request("run_a", "m"), "run_b": request("run_b", "m")},
            project_id="p1", chat_id="c1", main_id="main", instruction="m",
        )
    except FollowupStop:
        checks.append("duplicate_candidates")
    else:
        raise AssertionError("duplicate exact candidates were accepted")
    run_items = [
        {"type": "run.start", "run_id": "run_real"},
        {"type": "tool.start", "run_id": "run_real", "aseq": 2, "data": {"name": "assign", "call_id": "call_1"}},
        {"type": "tool.end", "run_id": "run_real", "aseq": 3, "data": {"call_id": "call_1", "details": {"id": "asg_real"}}},
    ]
    begin, finish = assign_trace_result(run_items, "run_real")
    assert begin["data"]["call_id"] == "call_1" and finish["data"]["details"]["id"] == "asg_real"
    checks.append("unique_assign")
    explicit = argparse.Namespace(main_run_id=["run_a", "run_b"])
    assert resolve_main_run_ids(explicit, {"main_run_ids": ["run_a", "run_b"]}, True) == ["run_a", "run_b"]
    try:
        resolve_main_run_ids(argparse.Namespace(main_run_id=["run_a", "run_c"]), {"main_run_ids": ["run_a", "run_b"]}, True)
    except FollowupStop:
        checks.append("explicit_immutable")
    else:
        raise AssertionError("explicit resume IDs were changed")
    class FakeClient:
        def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
            chat_id = params.get("chat_id")
            if method == "chat.history":
                return {"messages": [{
                    "id": "m1" if chat_id == "c1" else "m2", "seq": 1,
                    "chat_id": chat_id, "sender": {"kind": "user"},
                    "fallback_text": "m", "blocks": [{"type": "text", "markdown": "m"}],
                }], "has_more": False}
            if method == "trace.history":
                if chat_id == "c1":
                    return {"items": [
                        {"type": "run.start", "run_id": "run_real"},
                        {"type": "tool.start", "run_id": "run_real", "data": {"name": "assign", "call_id": "call_1"}},
                        {"type": "tool.end", "run_id": "run_real", "data": {"call_id": "call_1", "details": {"id": "asg_real", "project_id": "p1", "bot_id": "coder"}}},
                    ], "has_more_before": False}
                return {"items": [], "has_more_before": False}
            if method == "assignment.get":
                return {"assignment": {
                    "id": "asg_real", "project_id": "p1", "origin_chat_id": "c1", "bot_id": "coder",
                    "parent_assignment_id": None, "trigger_message_id": None, "from": {"kind": "bot", "bot_id": "main"},
                }}
            raise AssertionError(f"unexpected fake RPC: {method}")

    original_exact = globals()["exact_main_run_request"]
    globals()["exact_main_run_request"] = lambda run_id: request(run_id, "m", project="p1", chat="c1")
    try:
        partial = {"main_run_bindings": []}
        projects = [
            {"project_id": "p1", "chat_id": "c1", "baseline_assignment_ids": set()},
            {"project_id": "p2", "chat_id": "c2", "baseline_assignment_ids": set()},
        ]
        with tempfile.TemporaryDirectory() as temporary:
            auto_bind_main_runs(
                FakeClient(), journal=partial, journal_path=Path(temporary) / "journal.json",
                projects=projects, request_by_project={"p1": {"message_id": "m1", "seq": 1}, "p2": {"message_id": "m2", "seq": 1}},
                role_ids={"编码": "coder"}, main_id="main",
            )
        assert len(partial["main_run_bindings"]) == 1
        assert partial["main_run_bindings"][0]["root_assignment_id"] == "asg_real"
    finally:
        globals()["exact_main_run_request"] = original_exact
    checks.append("partial_preserved")
    return {"scenario": "project_followup discovery fake tests", "status": "PASS", "checks": checks, "network": False}


def handle_approvals(
    client: Any,
    *,
    journal: dict[str, Any],
    journal_path: Path,
    project: dict[str, Any],
    assignments: list[dict[str, Any]],
    request: dict[str, Any],
    approve: bool,
) -> list[dict[str, Any]]:
    assignment_ids = {item.get("id") for item in assignments if isinstance(item.get("id"), str)}
    if not assignment_ids:
        return []
    pending = require_list(require_dict(client.call("approval.list", {"state": ["pending"]}), "approval.list result").get("approvals"), "approval.list.approvals")
    observations = []
    request_time = parse_time(request.get("message_created_at"))
    for candidate in pending:
        if not isinstance(candidate, dict) or candidate.get("assignment_id") not in assignment_ids:
            continue
        created = parse_time(candidate.get("created_at"))
        if request_time is not None and created is not None and created < request_time:
            continue
        checkpoint = validate_approval_checkpoint(
            client,
            candidate,
            project_id=project["project_id"],
            project_chat_id=project["chat_id"],
            assignment_ids=assignment_ids,
        )
        scope = check_approval_scope(candidate, project["home_path"], project["scope_marker"], project_id=project["project_id"])
        observation = {
            "project_id": project["project_id"],
            "marker": project["marker"],
            "scope_marker": project["scope_marker"],
            "approval_id": candidate.get("id"),
            "tool": candidate.get("tool"),
            "risk": candidate.get("risk"),
            "assignment_id": candidate.get("assignment_id"),
            "scope": scope,
            "checkpoint": checkpoint,
        }
        prior_observation = next(
            (
                item for item in reversed(journal.get("approval_observations", []))
                if isinstance(item, dict) and item.get("approval_id") == candidate.get("id")
            ),
            None,
        )
        observations.append(observation)
        journal.setdefault("approval_observations", []).append(observation)
        persist(journal, journal_path)
        if not scope.get("authorized"):
            raise FollowupStop(f"pending approval outside this project scope: {scope.get('reason')}")
        if not approve:
            raise FollowupStop(f"pending scoped approval {candidate.get('id')}; rerun only after review with --approve-test-tools-once")
        prior = prior_observation
        if isinstance(prior, dict) and prior.get("decision") in {"pending_decision", "decision_unknown"}:
            try:
                all_states = require_list(require_dict(client.call("approval.list", {}), "approval.list reconciliation result").get("approvals"), "approval.list reconciliation approvals")
                current = next((item for item in all_states if isinstance(item, dict) and item.get("id") == candidate.get("id")), None)
                if isinstance(current, dict) and current.get("state") == "allowed_once":
                    observation["decision"] = "allowed_once_reconciled_readonly"
                    persist(journal, journal_path)
                    continue
                observation["decision"] = "decision_unknown"
                observation["state_after_error"] = current.get("state") if isinstance(current, dict) else None
            except Exception:
                observation["decision"] = "decision_unknown"
                observation["state_after_error"] = "unreadable"
            persist(journal, journal_path)
            raise FollowupStop(f"approval decision for {candidate.get('id')} is unresolved; refusing to approve again")
        observation["decision"] = "pending_decision"
        journal.setdefault("approval_observations", []).append(observation)
        persist(journal, journal_path)
        try:
            result = require_dict(client.call("approval.decide", {"approval_id": candidate.get("id"), "decision": "allow_once"}), "approval.decide result")
        except Exception:
            try:
                all_states = require_list(require_dict(client.call("approval.list", {}), "approval.list reconciliation result").get("approvals"), "approval.list reconciliation approvals")
                current = next((item for item in all_states if isinstance(item, dict) and item.get("id") == candidate.get("id")), None)
                observation["decision"] = "allowed_once_reconciled_readonly" if isinstance(current, dict) and current.get("state") == "allowed_once" else "decision_unknown"
                observation["state_after_error"] = current.get("state") if isinstance(current, dict) else None
            except Exception:
                observation["decision"] = "decision_unknown"
                observation["state_after_error"] = "unreadable"
            persist(journal, journal_path)
            raise FollowupStop(f"approval.decide result is unknown for {candidate.get('id')}; refusing to retry")
        resolved = require_dict(result.get("approval"), "approval.decide.approval")
        if resolved.get("id") != candidate.get("id") or resolved.get("state") != "allowed_once":
            raise FollowupStop("approval did not resolve as allowed_once")
        observation["decision"] = "allowed_once"
        persist(journal, journal_path)
    if observations:
        # Entries are persisted before mutation above; this branch records
        # read-only approvals when no decision was attempted.
        for item in observations:
            if not any(existing is item for existing in journal.get("approval_observations", [])):
                journal.setdefault("approval_observations", []).append(item)
        persist(journal, journal_path)
    return observations


def send_steer_if_working(client: Any, journal: dict[str, Any], journal_path: Path, project: dict[str, Any], assignments: list[dict[str, Any]], role_ids: dict[str, str]) -> None:
    coding = [item for item in assignments if item.get("bot_id") == role_ids["编码"] and item.get("status") == "working"]
    if len(coding) > 1:
        raise FollowupStop("multiple working coding assignments in one canonical followup graph")
    if not coding:
        return
    assignment = coding[0]
    existing = next(
        (
            item for item in journal.get("steers", [])
            if isinstance(item, dict)
            and item.get("project_id") == project["project_id"]
            and item.get("assignment_id") == assignment.get("id")
        ),
        None,
    )
    if existing is not None:
        return
    pending_record = next(
        (
            item for item in journal.get("requests", [])
            if isinstance(item, dict)
            and item.get("kind") == "steer"
            and item.get("project_id") == project["project_id"]
            and item.get("assignment_id") == assignment.get("id")
        ),
        None,
    )
    if pending_record is not None:
        if pending_record.get("status") in {"pending_send", "unknown_result"}:
            reconcile_unknown_send(client, journal, journal_path, project, pending_record)
        if pending_record.get("status") not in {"sent", "sent_reconciled_readonly"}:
            raise FollowupStop("steer journal record is not safely resumable")
        journal.setdefault("steers", []).append({
            "project_id": project["project_id"],
            "assignment_id": assignment.get("id"),
            **{key: pending_record.get(key) for key in ("message_id", "seq", "client_request_id")},
            "status": pending_record.get("status"),
        })
        persist(journal, journal_path)
        return
    goal = project.get("goal") if isinstance(project.get("goal"), str) and project.get("goal") else "原项目目标"
    if any(term in goal for term in ("邮箱", "登录", "登陆")):
        detail = "补齐邮箱格式校验、错误密码提示和手机窄屏布局"
    else:
        detail = f"围绕既有目标（{goal}）补齐一个可验证的本机演示与移动窄屏检查"
    text = f"{project['marker']}: {detail}；请通过 send_msg 在群内派发，完成交接时使用 intent=done 并 @下一角色，不扩展到外部网站、生产系统或网络服务。"
    sent = send_chat(
        client,
        journal=journal,
        journal_path=journal_path,
        project=project,
        kind="steer",
        text=text,
        mentions=[{"kind": "bot", "bot_id": role_ids["编码"], "instruction": None}],
        assignment_id=assignment.get("id"),
    )
    journal.setdefault("steers", []).append({"project_id": project["project_id"], "assignment_id": assignment.get("id"), **{key: sent.get(key) for key in ("message_id", "seq", "client_request_id")}, "status": "sent"})
    persist(journal, journal_path)


def project_files(home_path: str) -> list[dict[str, Any]]:
    home = Path(home_path).expanduser().resolve()
    files = []
    if not home.exists():
        return files
    for path in sorted(home.rglob("*")):
        if not path.is_file() or path.is_symlink():
            continue
        try:
            path.resolve().relative_to(home)
        except ValueError:
            continue
        files.append({"path": str(path), "size": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    return files


def trace_summary(client: Any, assignment_id: str) -> list[dict[str, Any]]:
    """Return trace routing metadata only; never persist model/tool content."""

    result = require_dict(
        client.call("trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    if result.get("has_more_before") is True:
        raise FollowupStop("trace.history is truncated; refusing to claim complete run evidence")
    summary = []
    for item in require_list(result.get("items"), "trace.history.items"):
        if not isinstance(item, dict):
            continue
        data = item.get("data") if isinstance(item.get("data"), dict) else {}
        summary.append({
            "type": item.get("type"),
            "seq": item.get("seq"),
            "run_id": item.get("run_id"),
            "at": item.get("at", item.get("created_at")),
            "name": data.get("name"),
            "message_id": data.get("message_id"),
            "is_error": data.get("is_error"),
        })
    return summary


def complete_trace_items(client: Any, assignment_id: str) -> list[dict[str, Any]]:
    result = require_dict(
        client.call("trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    if result.get("has_more_before") is True:
        raise FollowupStop("trace.history is truncated; refusing to verify steer routing")
    return [item for item in require_list(result.get("items"), "trace.history.items") if isinstance(item, dict)]


def steer_evidence(client: Any, project: dict[str, Any], steer: dict[str, Any], coding_id: str) -> dict[str, Any]:
    assignment_id = steer.get("assignment_id")
    if not isinstance(assignment_id, str):
        return {"message_id": steer.get("message_id"), "verified": False, "reason": "missing assignment_id"}
    result = require_dict(client.call("assignment.get", {"assignment_id": assignment_id}), "steer assignment.get result")
    assignment = require_dict(result.get("assignment"), "steer assignment.get.assignment")
    assignment_steers = [
        item for item in require_list(assignment.get("steers", []), "assignment.steers")
        if isinstance(item, dict) and item.get("message_id") == steer.get("message_id")
    ]
    history = complete_chat_history(client, project["chat_id"], after_seq=max(0, int(steer.get("seq", 0)) - 1))
    message = next(
        (item for item in history["messages"] if isinstance(item, dict) and item.get("id") == steer.get("message_id")),
        {},
    )
    deliveries = [
        item for item in message.get("delivery", [])
        if isinstance(item, dict) and item.get("bot_id") == coding_id
    ] if isinstance(message.get("delivery"), list) else []
    trace = complete_trace_items(client, assignment_id)
    applied = bool(assignment_steers and assignment_steers[-1].get("applied_at") is not None)
    delivered = bool(deliveries and deliveries[-1].get("state") == "read")
    traced = any(
        item.get("type") == "steer"
        and isinstance(item.get("data"), dict)
        and item["data"].get("message_id") == steer.get("message_id")
        for item in trace
    )
    return {
        "message_id": steer.get("message_id"),
        "assignment_id": assignment_id,
        "delivery": {key: deliveries[-1].get(key) for key in ("bot_id", "state", "at")} if deliveries else None,
        "applied_at": assignment_steers[-1].get("applied_at") if assignment_steers else None,
        "trace_steer": traced,
        "verified": applied and delivered and traced,
    }


def observe_project(
    client: Any,
    project: dict[str, Any],
    request: dict[str, Any],
    role_ids: dict[str, str],
    main_id: str,
    rows: list[dict[str, Any]] | None = None,
    steers: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    if rows is None:
        rows = tracked_assignments(client, project, request, set(project["baseline_assignment_ids"]))
    current_project = require_dict(
        require_dict(client.call("project.get", {"project_id": project["project_id"]}), "observe project.get result").get("project"),
        "observe project.get.project",
    )
    if current_project.get("id") != project["project_id"]:
        raise FollowupStop("observe project identity mismatch")
    history = complete_chat_history(client, project["chat_id"], after_seq=max(0, int(request["seq"]) - 1))["messages"]
    messages = [safe_message(item, project["marker"]) for item in history if isinstance(item, dict)]
    new_messages = [item for item in messages if isinstance(item.get("seq"), int) and item["seq"] >= request["seq"]]
    coder = [item for item in rows if item.get("bot_id") == role_ids["编码"]]
    tester = [item for item in rows if item.get("bot_id") == role_ids["测试"]]
    main_handoffs = [item["id"] for item in new_messages if item.get("sender_bot_id") == main_id and item.get("marker_present")]
    traces = [
        {"assignment_id": item.get("id"), "items": trace_summary(client, item["id"])}
        for item in rows
        if isinstance(item.get("id"), str)
    ]
    steer_observations = [
        steer_evidence(client, project, item, role_ids["编码"])
        for item in (steers or [])
        if item.get("project_id") == project["project_id"]
    ]
    return {
        "project_id": project["project_id"],
        "marker": project["marker"],
        "status": current_project.get("status"),
        "assignments": rows,
        "main_marker_messages": main_handoffs,
        "coding_assignments": coder,
        "tester_assignments": tester,
        "question_message_ids": [item["id"] for item in new_messages if any(block.get("type") == "question" for block in item["blocks"])],
        "review_message_ids": [item["id"] for item in new_messages if any(block.get("type") == "review_card" for block in item["blocks"])],
        "completion_message_ids": [item["id"] for item in new_messages if any(block.get("type") == "completion" for block in item["blocks"])],
        "trace": traces,
        "steers": steer_observations,
        "home_files": project_files(project["home_path"]),
    }


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    if len(args.project_id) != 2 or len(set(args.project_id)) != 2:
        raise FollowupStop("pass exactly two distinct --project-id values")
    role_ids = {"产品": args.product_bot_id, "编码": args.coding_bot_id, "测试": args.test_bot_id}
    if len(set(role_ids.values())) != 3:
        raise FollowupStop("role Bot IDs must be distinct")
    journal_path, journal, resumed = load_or_initialize(args, args.project_id)
    main_run_ids = resolve_main_run_ids(args, journal, resumed)
    if main_run_ids is not None:
        journal["main_run_ids"] = main_run_ids
    persist(journal, journal_path)
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    mains = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict) and item.get("is_main") is True]
    if len(mains) != 1 or not isinstance(mains[0].get("id"), str):
        raise FollowupStop("bootstrap must contain exactly one main Bot")
    main_id = mains[0]["id"]
    if resumed:
        if journal.get("role_ids") != role_ids or journal.get("main_bot_id") != main_id:
            raise FollowupStop("resume journal role/main identity does not match current bootstrap/CLI")
        if not isinstance(journal.get("markers"), list) or len(journal["markers"]) != 2 or len(set(journal["markers"])) != 2 or any(not isinstance(item, str) or not item for item in journal["markers"]):
            raise FollowupStop("resume journal markers are invalid")
    else:
        journal["role_ids"] = role_ids
        journal["main_bot_id"] = main_id
    projects = []
    for index, project_id in enumerate(args.project_id):
        snapshot = project_snapshot(client, project_id, role_ids, main_id)
        snapshot["marker"] = journal["markers"][index]
        current_baseline = {
            item.get("id") for item in snapshot.pop("baseline_assignments") if isinstance(item.get("id"), str)
        }
        prior_snapshot = next(
            (item for item in journal.get("project_snapshots", []) if isinstance(item, dict) and item.get("project_id") == project_id),
            None,
        )
        if resumed:
            if not isinstance(prior_snapshot, dict):
                raise FollowupStop("resume journal has no authoritative project snapshot")
            for key in ("chat_id", "home_path", "scope_marker", "member_bot_ids"):
                if prior_snapshot.get(key) != snapshot.get(key):
                    raise FollowupStop(f"resume project {project_id} identity changed at {key}")
        prior_ids = prior_snapshot.get("baseline_assignment_ids") if isinstance(prior_snapshot, dict) else None
        if resumed and not isinstance(prior_ids, list):
            raise FollowupStop("resume journal has no immutable baseline assignment IDs")
        snapshot["baseline_assignment_ids"] = (
            {item for item in prior_ids if isinstance(item, str)}
            if resumed and isinstance(prior_ids, list)
            else current_baseline
        )
        projects.append(snapshot)
    if not resumed:
        journal["project_snapshots"] = [
            {
                key: (sorted(value) if key == "baseline_assignment_ids" else value)
                for key, value in item.items()
            }
            for item in projects
        ]
        persist(journal, journal_path)
    # Preserve already verified roots when resuming a partially bound journal.
    for binding in journal.get("main_run_bindings", []):
        if not isinstance(binding, dict):
            continue
        project = next((item for item in projects if item["project_id"] == binding.get("project_id")), None)
        root_id = binding.get("root_assignment_id")
        if project is not None and isinstance(root_id, str):
            project["main_root_assignment_ids"] = {root_id}
    ensure_requests(client, journal, journal_path, projects, role_ids)
    request_by_project = {
        item.get("project_id"): item
        for item in journal.get("requests", [])
        if item.get("kind") == "followup_request"
        and item.get("status") in {"sent", "sent_reconciled_readonly"}
    }
    if len(request_by_project) != 2:
        raise FollowupStop("both existing projects must have one canonical followup request")
    if main_run_ids is not None:
        bind_main_runs(
            client,
            journal=journal,
            journal_path=journal_path,
            projects=projects,
            request_by_project=request_by_project,
            main_run_ids=main_run_ids,
            role_ids=role_ids,
        )
    else:
        auto_bind_main_runs(
            client,
            journal=journal,
            journal_path=journal_path,
            projects=projects,
            request_by_project=request_by_project,
            role_ids=role_ids,
            main_id=main_id,
        )
    deadline = dt.datetime.now(dt.timezone.utc).timestamp() + args.timeout
    observations = []
    while True:
        observations = []
        all_done = True
        if main_run_ids is None and journal.get("main_run_ids") is None:
            auto_bind_main_runs(
                client,
                journal=journal,
                journal_path=journal_path,
                projects=projects,
                request_by_project=request_by_project,
                role_ids=role_ids,
                main_id=main_id,
            )
        for project in projects:
            request = request_by_project[project["project_id"]]
            assignments = tracked_assignments(
                client,
                project,
                request,
                project["baseline_assignment_ids"],
                project.get("main_root_assignment_ids"),
            )
            # Existing assignments are only a baseline. Approval, Question,
            # steer, completion and artifact gates must follow the new
            # canonical request graph, never an old done assignment.
            graph_rows = assignments
            handle_approvals(client, journal=journal, journal_path=journal_path, project=project, assignments=graph_rows, request=request, approve=args.approve_test_tools_once)
            questions = pending_question(client, {item.get("id") for item in graph_rows if isinstance(item.get("id"), str)})
            if questions:
                raise FollowupStop(f"new followup question {questions[0].get('id')} is pending; script never answers questions")
            send_steer_if_working(client, journal, journal_path, project, graph_rows, role_ids)
            observation = observe_project(client, project, request, role_ids, main_id, graph_rows, journal.get("steers", []))
            observations.append(observation)
            has_coder = any(item.get("status") == "done" for item in observation["coding_assignments"])
            has_tester = any(item.get("status") == "done" for item in observation["tester_assignments"])
            has_demo = any(Path(item["path"]).name in {"index.html", "app.html"} for item in observation["home_files"])
            has_report = any(Path(item["path"]).name.lower() in {"test.md", "report.md", "test-report.md"} for item in observation["home_files"])
            has_verified_steer = bool(observation["steers"]) and all(item.get("verified") is True for item in observation["steers"])
            if not (has_coder and has_tester and has_demo and has_report and has_verified_steer):
                all_done = False
        journal["observations"] = observations
        journal["updated_at"] = now()
        persist(journal, journal_path)
        if all_done:
            break
        if dt.datetime.now(dt.timezone.utc).timestamp() >= deadline:
            break
        import time
        time.sleep(min(args.interval, max(0.0, deadline - dt.datetime.now(dt.timezone.utc).timestamp())))
    journal["status"] = "PARTIAL"
    journal["full_s2_pass"] = False
    persist(journal, journal_path)
    return {
        "scenario": "S2 existing project followup",
        "status": "PARTIAL",
        "url": client.base_url,
        "health_version": health.get("version"),
        "project_ids": args.project_id,
        "main_run_ids": journal.get("main_run_ids", main_run_ids),
        "main_run_bindings": journal.get("main_run_bindings", []),
        "journal": str(journal_path),
        "observations": observations,
        "full_s2_pass": False,
        "note": "API checks only; no project creation/confirmation, old question answer, UI, notification, or full S2 claim.",
    }


if __name__ == "__main__":
    if "--self-test" in sys.argv[1:]:
        print(json.dumps(run_fake_tests(), ensure_ascii=False, sort_keys=True))
        raise SystemExit(0)
    parsed = parser().parse_args()
    raise SystemExit(run_main(scenario, parsed))
