#!/usr/bin/env python3
"""S2 API checks for the DESIGN login workflow and parallel project work.

This scenario deliberately starts from ``chat_main``. Calling ``project.create``
directly only creates a project and chat; it is not evidence that the main Bot
understood a user's request or dispatched the first worker.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
from pathlib import Path
from typing import Any, Callable

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    message_text,
    parse_time,
    ready_health,
    require_production_host,
    require_dict,
    require_list,
    run_main,
    safe_error,
    sender_is,
    unique_marker,
    wait_until,
)


from s2.approval_scope import check_approval_scope  # noqa: E402

ROLE_FLOW = ["产品", "编码", "测试"]
TERMINAL = {"done", "failed", "cancelled"}
QUESTION_OPTIONS = ["只做邮箱登录", "改为手机登录"]
_PARTIAL_EVIDENCE: dict[str, Any] = {"partial_id": unique_marker("macbot-e2e-s2-partial"), "requests": [], "projects": []}
_PARTIAL_PATH: Path | None = None


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--product-bot-id", required=True)
    parser.add_argument("--coding-bot-id", required=True)
    parser.add_argument("--test-bot-id", required=True)
    parser.add_argument(
        "--answer-question-option",
        type=int,
        help="Answer only the marker's preset email-only question with this option index; without it the check fails and leaves the question pending",
    )
    parser.add_argument(
        "--approve-test-tools-once",
        action="store_true",
        help="Allow once only the marker bash command, exact project Home mkdir, file writes/edits, or memory additions scoped to this test project; without it approval remains pending",
    )
    parser.add_argument(
        "--resume-partial",
        type=Path,
        help="Resume from a prior partial JSON; existing user requests are verified by chat.history and never resent",
    )
    return parser


def assignment_list(client: Any, project_id: str) -> list[dict[str, Any]]:
    result = require_dict(
        client.call("assignment.list", {"project_id": project_id, "limit": 100}),
        "assignment.list result",
    )
    items = [item for item in require_list(result.get("items"), "assignment.list.items") if isinstance(item, dict)]
    mismatched = [item.get("id") for item in items if item.get("project_id") != project_id]
    if mismatched:
        raise ValueError(f"assignment.list project filter returned unrelated assignments: {mismatched}")
    return items


def project_detail(client: Any, project_id: str) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    result = require_dict(client.call("project.get", {"project_id": project_id}), "project.get result")
    project = require_dict(result.get("project"), "project.get.project")
    announcement = require_dict(result.get("announcement"), "project.get.announcement")
    chat_result = require_dict(client.call("chat.get", {"chat_id": project.get("chat_id")}), "chat.get result")
    chat = require_dict(chat_result.get("chat"), "chat.get.chat")
    return project, announcement, chat


def trace_items(client: Any, assignment_id: str) -> list[dict[str, Any]]:
    result = require_dict(
        client.call("trace.history", {"assignment_id": assignment_id, "tail": True, "limit": 500}),
        "trace.history result",
    )
    return [item for item in require_list(result.get("items"), "trace.history.items") if isinstance(item, dict)]


def interval(item: dict[str, Any]) -> tuple[dt.datetime, dt.datetime] | None:
    start = parse_time(item.get("started_at"))
    if start is None:
        return None
    finish = parse_time(item.get("finished_at")) or dt.datetime.now(dt.timezone.utc)
    return start, finish


def overlaps(left: dict[str, Any], right: dict[str, Any]) -> bool:
    a = interval(left)
    b = interval(right)
    return a is not None and b is not None and max(a[0], b[0]) < min(a[1], b[1])


def bot_role(bot: dict[str, Any], role: str) -> bool:
    name = bot.get("name") if isinstance(bot.get("name"), str) else ""
    label = bot.get("label") if isinstance(bot.get("label"), str) else ""
    return role in name or role in label


def validate_role_setup(state: dict[str, Any], role_ids: dict[str, str]) -> tuple[str, dict[str, dict[str, Any]]]:
    bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
    by_id = {item.get("id"): item for item in bots if isinstance(item.get("id"), str)}
    main = [item for item in bots if item.get("is_main") is True]
    if len(main) != 1 or not isinstance(main[0].get("id"), str):
        raise ValueError("S2 requires exactly one main Bot in bootstrap")
    main_id = main[0]["id"]
    if len(set(role_ids.values())) != 3 or main_id in set(role_ids.values()):
        raise ValueError("S2 requires three distinct non-main role Bot IDs")
    selected: dict[str, dict[str, Any]] = {}
    for role, bot_id in role_ids.items():
        bot = by_id.get(bot_id)
        if not isinstance(bot, dict) or bot.get("is_main") is True:
            raise ValueError(f"{role} Bot {bot_id} is absent or is main")
        if not bot_role(bot, role):
            raise ValueError(f"{role} Bot {bot_id} name/label does not identify role {role}; refusing role-mapping guess")
        selected[role] = bot
    return main_id, selected


def send_main_request(client: Any, main_chat_id: str, text: str) -> dict[str, Any]:
    result = require_dict(
        client.call("chat.send", {"chat_id": main_chat_id, "text": text, "mentions": []}),
        "main chat.send result",
    )
    message = require_dict(result.get("message"), "main request message")
    if not isinstance(message.get("id"), str) or not message["id"]:
        raise ValueError("main request has no message id")
    if not isinstance(message.get("seq"), int) or isinstance(message.get("seq"), bool):
        raise ValueError("main request has no numeric seq")
    return message


def find_project_card(messages: list[dict[str, Any]], *, main_id: str) -> list[dict[str, Any]]:
    cards: list[dict[str, Any]] = []
    for message in messages:
        if not sender_is(message, kind="bot", bot_id=main_id):
            continue
        blocks = message.get("blocks")
        if not isinstance(blocks, list):
            continue
        for block in blocks:
            if not isinstance(block, dict) or block.get("type") != "project_card":
                continue
            project_id = block.get("project_id")
            if isinstance(project_id, str):
                cards.append({"message": message, "project_id": project_id})
    return cards


def fail_on_pre_project_approval(client: Any, *, main_id: str, marker: str) -> None:
    """Stop before the card timeout when main's project creation is awaiting approval.

    Scope by this request's unique marker as well as the Bot/tool pair.
    Older requests may retain pending approvals after a server upgrade;
    they must neither fail this run nor be approved by it.
    """

    result = require_dict(
        client.call("approval.list", {"state": ["pending"]}),
        "approval.list result",
    )
    pending = [
        item
        for item in require_list(result.get("approvals"), "approval.list.approvals")
        if isinstance(item, dict)
        and item.get("state") == "pending"
        and item.get("bot_id") == main_id
        and item.get("assignment_id") is None
        and item.get("tool") in {"create_project", "project.create"}
        and marker in str(item.get("detail", ""))
    ]
    if not pending:
        return
    details = [
        {
            "id": item.get("id"),
            "bot_id": item.get("bot_id"),
            "chat_id": item.get("chat_id"),
            "tool": item.get("tool"),
            "risk": item.get("risk"),
            "summary": item.get("summary"),
            "detail": item.get("detail"),
            "state": item.get("state"),
        }
        for item in pending
    ]
    _PARTIAL_EVIDENCE.setdefault("pre_project_gates", []).append(
        {"marker": marker, "kind": "create_project", "pending": details}
    )
    raise ValueError(
        f"{marker} create_project awaiting approval: "
        f"{json.dumps(details, ensure_ascii=False, sort_keys=True)}"
    )


def wait_project_card(
    client: Any,
    *,
    main_chat_id: str,
    sent_seq: int,
    sent_id: str,
    marker: str,
    main_id: str,
    timeout: float,
    interval: float,
) -> dict[str, Any]:
    def check() -> dict[str, Any] | None:
        fail_on_pre_project_approval(client, main_id=main_id, marker=marker)
        history = chat_history(client, main_chat_id, after_seq=sent_seq)
        cards = find_project_card([item for item in history["messages"] if isinstance(item, dict)], main_id=main_id)
        matches: list[dict[str, Any]] = []
        for card in cards:
            project, announcement, chat = project_detail(client, card["project_id"])
            if (
                marker in message_text(card["message"])
                or marker in str(project.get("name", ""))
                or marker in str(project.get("goal", ""))
                or card["message"].get("reply_to") == sent_id
            ):
                matches.append({"project": project, "announcement": announcement, "chat": chat, "card": card["message"]})
        if len(matches) > 1:
            raise ValueError(f"marker {marker} produced multiple project cards")
        return matches[0] if matches else None

    return wait_until(check, timeout=timeout, interval=interval, description=f"{marker} main project card")


def validate_project(detail: dict[str, Any], *, marker: str, main_id: str, role_ids: dict[str, str]) -> None:
    project = detail["project"]
    chat = detail["chat"]
    announcement = detail["announcement"]
    if project.get("status") != "active":
        raise ValueError(f"{marker} project is not active")
    flow = project.get("flow")
    # PROTOCOL defines display strings, so stage descriptions may accompany
    # each role. Keep the three roles and their order mandatory; actual Bot
    # dispatch and handoff are checked separately below.
    if (
        not isinstance(flow, list)
        or len(flow) != len(ROLE_FLOW)
        or any(
            not isinstance(stage, str)
            or not stage.strip().startswith(role)
            for stage, role in zip(flow, ROLE_FLOW)
        )
    ):
        raise ValueError(f"{marker} project flow does not preserve 产品→编码→测试 order")
    if project.get("lead_bot_id") != main_id:
        raise ValueError(f"{marker} project lead is not the main Bot")
    project_id = project.get("id")
    chat_id = project.get("chat_id")
    if not isinstance(project_id, str) or not isinstance(chat_id, str):
        raise ValueError(f"{marker} project has no canonical id/chat_id")
    if chat.get("id") != chat_id or chat.get("kind") != "project" or chat.get("project_id") != project_id:
        raise ValueError(f"{marker} project chat does not match project")
    members = {
        item.get("bot_id")
        for item in require_list(project.get("members"), f"{marker}.project.members")
        if isinstance(item, dict)
    }
    expected_members = {main_id, *role_ids.values()}
    if not expected_members <= members:
        raise ValueError(f"{marker} project members do not include main/product/coding/test")
    if announcement.get("project_id") != project_id:
        raise ValueError(f"{marker} announcement is for a different project")
    announcement_members = {
        item.get("bot_id")
        for item in require_list(announcement.get("members"), f"{marker}.announcement.members")
        if isinstance(item, dict)
    }
    if not expected_members <= announcement_members:
        raise ValueError(f"{marker} announcement omits a project member")
    if not sender_is(detail["card"], kind="bot", bot_id=main_id):
        raise ValueError(f"{marker} project card was not sent by the main Bot")


def opening_and_first_dispatch(
    client: Any,
    *,
    project: dict[str, Any],
    marker: str,
    main_id: str,
    product_id: str,
) -> dict[str, Any] | None:
    chat_id = project["chat_id"]
    messages = [item for item in chat_history(client, chat_id)["messages"] if isinstance(item, dict)]
    openings = []
    for message in messages:
        if not sender_is(message, kind="bot", bot_id=main_id):
            continue
        mentions = message.get("mentions")
        if not isinstance(mentions, list) or not any(
            isinstance(item, dict) and item.get("kind") == "bot" and item.get("bot_id") == product_id
            for item in mentions
        ):
            continue
        text = message_text(message)
        if marker not in text and project.get("name") not in text:
            continue
        if not isinstance(message.get("id"), str):
            raise ValueError(f"{marker} opening message has no id")
        openings.append(message)
    if len(openings) > 1:
        raise ValueError(f"{marker} has multiple candidate main opening messages")
    if not openings:
        return None
    opening = openings[0]
    assignments = assignment_list(client, project["id"])
    candidates = [
        item
        for item in assignments
        if item.get("project_id") == project["id"]
        and item.get("origin_chat_id") == chat_id
        and item.get("bot_id") == product_id
        and item.get("parent_assignment_id") is None
        and item.get("trigger_message_id") == opening["id"]
        and item.get("status") not in {"failed", "cancelled"}
    ]
    if len(candidates) > 1:
        raise ValueError(f"{marker} opening produced multiple first assignments")
    if not candidates:
        return None
    first = candidates[0]
    if not isinstance(first.get("id"), str):
        raise ValueError(f"{marker} first assignment has no id")
    from_sender = first.get("from")
    if not isinstance(from_sender, dict) or from_sender.get("kind") != "bot" or from_sender.get("bot_id") != main_id:
        raise ValueError(f"{marker} first assignment was not dispatched by the main Bot")
    return {"opening": opening, "product": first}


def coding_working(client: Any, *, project_id: str, coding_id: str, product_assignment_id: str) -> dict[str, Any] | None:
    candidates = [
        item
        for item in assignment_list(client, project_id)
        if item.get("bot_id") == coding_id
        and item.get("parent_assignment_id") == product_assignment_id
        and item.get("status") == "working"
    ]
    if len(candidates) > 1:
        raise ValueError("multiple working coding assignments for one product handoff")
    return candidates[0] if candidates else None


def steer_evidence(
    client: Any,
    *,
    project: dict[str, Any],
    assignment: dict[str, Any],
    coding_id: str,
    marker: str,
    timeout: float,
    interval: float,
    gate_poll: Callable[[], Any] | None = None,
) -> dict[str, Any]:
    chat_id = project["chat_id"]
    assignment_id = assignment["id"]
    result = require_dict(
        client.call(
            "chat.send",
            {
                "chat_id": chat_id,
                "text": f"{marker}: 只实现邮箱登录，不实现手机号登录。",
                "mentions": [{"kind": "bot", "bot_id": coding_id, "instruction": None}],
            },
        ),
        "S2 steer chat.send result",
    )
    message = require_dict(result.get("message"), "S2 steer message")
    message_id = message.get("id")
    message_seq = message.get("seq")
    if not isinstance(message_id, str) or not isinstance(message_seq, int) or isinstance(message_seq, bool):
        raise ValueError("S2 steer message has no canonical id/seq")

    def check() -> dict[str, Any] | None:
        if gate_poll is not None:
            gate_poll()
        current = require_dict(client.call("assignment.get", {"assignment_id": assignment_id}), "assignment.get result")
        current = require_dict(current.get("assignment"), "assignment.get.assignment")
        steers = [
            item
            for item in require_list(current.get("steers"), "assignment.steers")
            if isinstance(item, dict) and item.get("message_id") == message_id
        ]
        history = chat_history(client, chat_id, after_seq=max(0, message_seq - 1))
        matched = next((item for item in history["messages"] if isinstance(item, dict) and item.get("id") == message_id), None)
        deliveries = [
            item
            for item in (matched or {}).get("delivery", [])
            if isinstance(item, dict) and item.get("bot_id") == coding_id
        ]
        trace = trace_items(client, assignment_id)
        if (
            steers
            and steers[-1].get("applied_at") is not None
            and deliveries
            and deliveries[-1].get("state") == "read"
            and any(
                item.get("type") == "steer"
                and isinstance(item.get("data"), dict)
                and item["data"].get("message_id") == message_id
                for item in trace
            )
        ):
            return {"message_id": message_id, "assignment_id": assignment_id, "delivery": deliveries[-1]}
        return None

    value = wait_until(check, timeout=timeout, interval=interval, description=f"{marker} steer applied")
    return require_dict(value, "S2 steer evidence")


def parallel_coding_pair(
    client: Any,
    *,
    first_project_id: str,
    second_project_id: str,
    coding_id: str,
) -> dict[str, Any] | None:
    first = [item for item in assignment_list(client, first_project_id) if item.get("bot_id") == coding_id]
    second = [item for item in assignment_list(client, second_project_id) if item.get("bot_id") == coding_id]
    for left in first:
        for right in second:
            if left.get("status") in TERMINAL or right.get("status") in TERMINAL:
                continue
            if overlaps(left, right):
                return {"first": left, "second": right}
    return None


def subagent_evidence(client: Any, project_ids: list[str]) -> dict[str, Any] | None:
    for project_id in project_ids:
        for assignment in assignment_list(client, project_id):
            assignment_id = assignment.get("id")
            if not isinstance(assignment_id, str):
                continue
            trace = trace_items(client, assignment_id)
            parent_runs = {
                item.get("run_id")
                for item in trace
                if item.get("type") == "run.start"
                and isinstance(item.get("run_id"), str)
                and isinstance(item.get("data"), dict)
                and item["data"].get("phase") != "subagent"
            }
            for item in trace:
                data = item.get("data")
                if (
                    item.get("type") == "run.start"
                    and isinstance(data, dict)
                    and data.get("phase") == "subagent"
                    and isinstance(data.get("parent_run_id"), str)
                    and data["parent_run_id"] in parent_runs
                ):
                    return {"project_id": project_id, "assignment_id": assignment_id, "run_id": item.get("run_id")}
    return None


def pending_items(client: Any, assignment_ids: set[str]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    workbench = require_dict(client.call("workbench.get", {}), "workbench.get result")
    waiting = [item for item in require_list(workbench.get("waiting"), "workbench.waiting") if isinstance(item, dict)]
    approvals: list[dict[str, Any]] = []
    questions: list[dict[str, Any]] = []

    def append_unique(items: list[dict[str, Any]], value: dict[str, Any]) -> None:
        value_id = value.get("id")
        if isinstance(value_id, str) and any(item.get("id") == value_id for item in items):
            return
        items.append(value)

    for item in waiting:
        kind = item.get("kind")
        value = item.get("approval") if kind == "approval" else item.get("question") if kind == "question" else None
        if not isinstance(value, dict) or value.get("assignment_id") not in assignment_ids:
            continue
        if kind == "approval":
            append_unique(approvals, value)
        elif kind == "question":
            append_unique(questions, value)
    state = bootstrap(client)
    pending = require_dict(state.get("pending"), "bootstrap.pending")
    for value in require_list(pending.get("approvals"), "bootstrap.pending.approvals"):
        if isinstance(value, dict) and value.get("assignment_id") in assignment_ids and value.get("state") == "pending":
            append_unique(approvals, value)
    for value in require_list(pending.get("questions"), "bootstrap.pending.questions"):
        if isinstance(value, dict) and value.get("assignment_id") in assignment_ids and value.get("state") == "pending":
            append_unique(questions, value)
    return approvals, questions


def resolve_gates(
    client: Any,
    *,
    assignment_ids: set[str],
    marker: str,
    project_home: str,
    project_id: str,
    args: argparse.Namespace,
    evidence: dict[str, Any],
) -> dict[str, Any] | None:
    approvals, questions = pending_items(client, assignment_ids)
    if questions:
        question = questions[0]
        question_id = question.get("id")
        if not isinstance(question_id, str):
            raise ValueError(f"{marker} pending question has no id")
        if question_id in evidence["question_ids"]:
            questions = []
        else:
            question_text = question.get("text")
            advertised_options = question.get("options")
            evidence.setdefault("questions", []).append(
                {"id": question_id, "text": question_text, "options": advertised_options}
            )
            if not isinstance(question_text, str) or not question_text.strip():
                raise ValueError(f"{marker} pending question has no text")
            free_email_confirmation = (
                advertised_options == []
                and question.get("allow_free_text") is True
                and "邮箱" in question_text
                and "登录" in question_text
                and "方式" in question_text
                and args.answer_question_option == 0
            )
            if advertised_options != QUESTION_OPTIONS and not free_email_confirmation:
                raise ValueError(f"{marker} pending question is not the scoped email-login decision")
            if args.answer_question_option is None:
                raise ValueError(f"{marker} partial: pending question {question_id}; rerun with --answer-question-option")
            options = advertised_options
            if not free_email_confirmation and (not isinstance(options, list) or not 0 <= args.answer_question_option < len(options)):
                raise ValueError(f"{marker} question option index is outside the advertised options")
            answer = {"question_id": question_id, "text": "只做邮箱登录"} if free_email_confirmation else {"question_id": question_id, "option_index": args.answer_question_option}
            result = require_dict(
                client.call("question.answer", answer),
                "question.answer result",
            )
            answered = require_dict(result.get("question"), "question.answer.question")
            if answered.get("id") != question_id or answered.get("state") != "answered":
                raise ValueError(f"{marker} question did not become answered")
            evidence["question_ids"].append(question_id)
    if approvals:
        matching: list[dict[str, Any]] = []
        for candidate in approvals:
            candidate_id = candidate.get("id")
            scope = check_approval_scope(candidate, project_home, marker, project_id=project_id)
            if isinstance(candidate_id, str) and not any(item.get("id") == candidate_id for item in evidence.setdefault("approvals", [])):
                evidence["approvals"].append(
                    {"id": candidate_id, "tool": candidate.get("tool"), "risk": candidate.get("risk"), "scope": scope}
                )
            if scope["authorized"]:
                matching.append(candidate)
            else:
                raise ValueError(f"{marker} pending approval is outside this test project scope: {scope['reason']}")
        if not matching:
            raise ValueError(f"{marker} has no scoped test approval")
        approval = matching[0]
        approval_id = approval.get("id")
        if not isinstance(approval_id, str):
            raise ValueError(f"{marker} pending approval has no id")
        if approval_id not in evidence["approval_ids"]:
            if not args.approve_test_tools_once:
                raise ValueError(f"{marker} partial: pending approval {approval_id}; rerun with --approve-test-tools-once")
            if approval.get("assignment_id") not in assignment_ids:
                raise ValueError(f"{marker} approval is not linked to the scenario assignment")
            result = require_dict(
                client.call("approval.decide", {"approval_id": approval_id, "decision": "allow_once"}),
                "approval.decide result",
            )
            decided = require_dict(result.get("approval"), "approval.decide.approval")
            if decided.get("id") != approval_id or decided.get("state") != "allowed_once":
                raise ValueError(f"{marker} approval did not resolve as allowed_once")
            evidence["approval_ids"].append(approval_id)
    if evidence["approval_ids"] and len(evidence["question_ids"]) == 1:
        return evidence
    return None


def gated_check(gate_poll: Callable[[], Any], check: Callable[[], Any]) -> Callable[[], Any]:
    """Inspect this scenario's pending gates before every polling check."""

    def wrapped() -> Any:
        gate_poll()
        return check()

    return wrapped


def make_gate_poll(
    client: Any,
    projects: list[dict[str, Any]],
    markers: list[str],
    args: argparse.Namespace,
) -> tuple[Callable[[], Any], dict[str, dict[str, Any]]]:
    states: dict[str, dict[str, Any]] = {
        marker: {"approval_ids": [], "question_ids": [], "questions": [], "approvals": []}
        for marker in markers
    }

    def poll() -> dict[str, dict[str, Any]] | None:
        complete = True
        for detail, marker in zip(projects, markers):
            assignment_ids = {
                item.get("id")
                for item in assignment_list(client, detail["project"]["id"])
                if isinstance(item.get("id"), str)
            }
            resolve_gates(
                client,
                assignment_ids=assignment_ids,
                marker=marker,
                project_home=detail["project"].get("home_path"),
                project_id=detail["project"]["id"],
                args=args,
                evidence=states[marker],
            )
            if not states[marker]["approval_ids"] or len(states[marker]["question_ids"]) != 1:
                complete = False
        return states if complete else None

    return poll, states


def completion_for_chain(
    client: Any,
    *,
    project: dict[str, Any],
    first: dict[str, Any],
    role_ids: dict[str, str],
    main_id: str,
    marker: str,
) -> dict[str, Any] | None:
    items = assignment_list(client, project["id"])
    product = first["product"]
    product_id = product["id"]
    product_done = next((item for item in items if item.get("id") == product_id and item.get("status") == "done"), None)
    if product_done is None:
        return None
    coding_candidates = [
        item for item in items
        if item.get("bot_id") == role_ids["编码"]
        and item.get("parent_assignment_id") == product_id
        and item.get("status") == "done"
    ]
    if len(coding_candidates) > 1:
        raise ValueError(f"{marker} product→coding handoff is ambiguous")
    if not coding_candidates:
        return None
    coding = coding_candidates[0]
    coding_id = coding["id"]
    testing_candidates = [
        item for item in items
        if item.get("bot_id") == role_ids["测试"]
        and item.get("parent_assignment_id") == coding_id
        and item.get("status") == "done"
    ]
    if len(testing_candidates) > 1:
        raise ValueError(f"{marker} coding→test handoff is ambiguous")
    if not testing_candidates:
        return None
    testing = testing_candidates[0]
    messages = [item for item in chat_history(client, project["chat_id"])["messages"] if isinstance(item, dict)]
    by_id = {item.get("id"): item for item in messages if isinstance(item.get("id"), str)}

    def check_result(assignment: dict[str, Any], recipient: str | None) -> None:
        result_id = assignment.get("result_message_id")
        if not isinstance(result_id, str):
            raise ValueError(f"{marker} done assignment has no result_message_id")
        message = by_id.get(result_id)
        if not isinstance(message, dict):
            raise ValueError(f"{marker} result message {result_id} is absent from the project history")
        if message.get("assignment_id") != assignment.get("id") or message.get("intent") != "done":
            raise ValueError(f"{marker} result message does not reference its done assignment")
        if not any(isinstance(block, dict) and block.get("type") == "completion" for block in message.get("blocks", [])):
            raise ValueError(f"{marker} result message has no completion block")
        mentions = message.get("mentions")
        if recipient is not None and (not isinstance(mentions, list) or not any(
            isinstance(mention, dict)
            and ((recipient == main_id and mention.get("kind") == "main") or (mention.get("kind") == "bot" and mention.get("bot_id") == recipient))
            for mention in mentions
        )):
            raise ValueError(f"{marker} result message does not mention the next recipient")

    check_result(product_done, role_ids["编码"])
    check_result(coding, role_ids["测试"])
    check_result(testing, main_id)
    return {"product": product_done, "coding": coding, "testing": testing}


def review_card(client: Any, main_chat_id: str, project_id: str, main_id: str, after_seq: int) -> dict[str, Any] | None:
    history = chat_history(client, main_chat_id, after_seq=after_seq)
    for message in history["messages"]:
        if not isinstance(message, dict) or not sender_is(message, kind="bot", bot_id=main_id):
            continue
        for block in message.get("blocks", []) if isinstance(message.get("blocks"), list) else []:
            if isinstance(block, dict) and block.get("type") == "review_card" and block.get("project_id") == project_id and block.get("state") == "pending":
                return {"message_id": message.get("id"), "project_id": project_id}
    return None


def final_summary(client: Any, project: dict[str, Any], main_id: str, marker: str) -> dict[str, Any] | None:
    for message in chat_history(client, project["chat_id"])["messages"]:
        if (
            isinstance(message, dict)
            and sender_is(message, kind="bot", bot_id=main_id)
            and message.get("intent") == "done"
            and isinstance(message.get("id"), str)
            and isinstance(message.get("seq"), int)
        ):
            return {"message_id": message.get("id"), "seq": message.get("seq")}
    return None


def load_partial(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot load --resume-partial {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError("--resume-partial must contain a JSON object")
    markers = value.get("markers")
    if not isinstance(markers, list) or len(markers) != 2 or any(not isinstance(item, str) or not item for item in markers):
        raise ValueError("--resume-partial requires exactly two non-empty markers")
    if len(set(markers)) != 2 or any(not item.startswith("macbot-e2e-s2-") for item in markers):
        raise ValueError("--resume-partial markers must be two distinct S2 markers")
    requests = value.get("requests")
    if not isinstance(requests, list) or not requests or len(requests) > 2:
        raise ValueError("--resume-partial requires one or two existing requests")
    seen: set[str] = set()
    seen_ids: set[str] = set()
    seen_seqs: set[int] = set()
    marker_set = set(markers)
    for request in requests:
        if not isinstance(request, dict):
            raise ValueError("--resume-partial request must be an object")
        marker = request.get("marker")
        if marker not in marker_set or marker in seen:
            raise ValueError("--resume-partial requests must reference unique known markers")
        message_id = request.get("message_id")
        if not isinstance(message_id, str) or not message_id or message_id in seen_ids:
            raise ValueError("--resume-partial request has no message_id")
        seq = request.get("seq")
        if not isinstance(seq, int) or isinstance(seq, bool) or seq < 0 or seq in seen_seqs:
            raise ValueError("--resume-partial request has no valid numeric seq")
        seen.add(marker)
        seen_ids.add(message_id)
        seen_seqs.add(seq)
    projects = value.get("projects", [])
    if not isinstance(projects, list):
        raise ValueError("--resume-partial projects must be an array")
    project_markers: set[str] = set()
    for project in projects:
        if not isinstance(project, dict):
            raise ValueError("--resume-partial project must be an object")
        marker = project.get("marker")
        if marker not in marker_set or marker in project_markers:
            raise ValueError("--resume-partial projects must reference unique known markers")
        for key in ("project_id", "chat_id", "card_message_id"):
            if not isinstance(project.get(key), str) or not project[key]:
                raise ValueError(f"--resume-partial project has no {key}")
        project_markers.add(marker)
    return value


def request_prompt(
    *,
    marker: str,
    name: str,
    role_ids: dict[str, str],
    selected: dict[str, dict[str, Any]],
) -> str:
    return (
        f"{marker}：请完成本地邮箱登录 demo‘{name}’，只在新项目 Home（服务端返回的 project.home_path）内操作，"
        "不要访问外部网站、生产系统或网络服务，不要 git commit/push，不要部署服务。请由主 Bot 建一个群，严格使用流程 产品→编码→测试，成员必须是"
        f"产品 Bot id={role_ids['产品']} name={selected['产品'].get('name')}、"
        f"编码 Bot id={role_ids['编码']} name={selected['编码'].get('name')}、"
        f"测试 Bot id={role_ids['测试']} name={selected['测试'].get('name')}。"
        "调用 create_project 时 member_bot_ids 必须逐字使用这三个真实 ID（不要把 name 放入 member_bot_ids）。"
        f"群名必须是‘{name}’，目标必须包含‘{marker}’和‘邮箱登录’。产品写本地 PRD，编码实现本地 demo，测试验证；"
        "每个交接必须用 send_msg(done) @ 下一位，最后 @ 主 Bot。"
        f"本次集成必须启动至少一个 subagent，并提出包含‘{marker}’的 decision question，选项必须严格为："
        f"{QUESTION_OPTIONS!r}；只有选项 0（只做邮箱登录）可由脚本回答。"
        f"审批测试只允许请求精确命令：mkdir -p e2e && printf '%s' '{marker}' > e2e/{marker}.txt。"
    )


def verify_resume_requests(
    client: Any,
    *,
    main_chat_id: str,
    partial: dict[str, Any],
    markers: list[str],
    role_ids: dict[str, str],
) -> dict[str, dict[str, Any]]:
    by_marker: dict[str, dict[str, Any]] = {}
    expected_names = {
        markers[0]: f"{markers[0]}-登录功能",
        markers[1]: f"{markers[1]}-并行官网改版",
    }
    for request in partial["requests"]:
        marker = request["marker"]
        seq = request["seq"]
        history = chat_history(client, main_chat_id, after_seq=max(0, seq - 1))
        matches = [
            item
            for item in history["messages"]
            if isinstance(item, dict)
            and item.get("id") == request["message_id"]
            and item.get("seq") == seq
        ]
        if len(matches) != 1:
            raise ValueError(f"resume request {marker} must match exactly one chat.history id/seq")
        message = matches[0]
        if not sender_is(message, kind="user"):
            raise ValueError(f"resume request {marker} is not a user message")
        text = message_text(message)
        if marker not in text:
            raise ValueError(f"resume request {marker} text does not contain its marker")
        if request.get("name") not in (None, expected_names[marker]):
            raise ValueError(f"resume request {marker} has an unexpected name")
        if expected_names[marker] not in text:
            raise ValueError(f"resume request {marker} text has an unexpected project name")
        for role, bot_id in role_ids.items():
            if f"{role} Bot id={bot_id}" not in text:
                raise ValueError(f"resume request {marker} does not contain the current {role} Bot ID")
        if "member_bot_ids 必须逐字使用这三个真实 ID" not in text:
            raise ValueError(f"resume request {marker} is an old names-only prompt")
        by_marker[marker] = {
            "marker": marker,
            "name": request.get("name") or expected_names[marker],
            "sent": {"id": message["id"], "seq": message["seq"]},
            "message": message,
        }
    return by_marker


def record_project_evidence(detail: dict[str, Any], marker: str) -> None:
    record = {
        "marker": marker,
        "project_id": detail["project"].get("id"),
        "chat_id": detail["project"].get("chat_id"),
        "card_message_id": detail["card"].get("id"),
    }
    existing = next((item for item in _PARTIAL_EVIDENCE["projects"] if item.get("marker") == marker), None)
    if existing is not None and any(existing.get(key) != record[key] for key in ("project_id", "chat_id", "card_message_id")):
        raise ValueError(f"{marker} partial project/card does not match current project card")
    if existing is None:
        _PARTIAL_EVIDENCE["projects"].append(record)


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    global _PARTIAL_EVIDENCE, _PARTIAL_PATH
    partial = load_partial(args.resume_partial) if args.resume_partial is not None else None
    _PARTIAL_PATH = args.resume_partial
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    role_ids = {"产品": args.product_bot_id, "编码": args.coding_bot_id, "测试": args.test_bot_id}
    main_id, selected = validate_role_setup(state, role_ids)
    main_chats = [item for item in require_list(state.get("chats"), "bootstrap.chats") if isinstance(item, dict) and item.get("kind") == "main"]
    if len(main_chats) != 1 or not isinstance(main_chats[0].get("id"), str):
        raise ValueError("S2 requires exactly one chat_main in bootstrap")
    main_chat_id = main_chats[0]["id"]
    if partial is None:
        markers = [unique_marker("macbot-e2e-s2-login"), unique_marker("macbot-e2e-s2-parallel")]
        existing_requests: dict[str, dict[str, Any]] = {}
        _PARTIAL_EVIDENCE = {"partial_id": _PARTIAL_EVIDENCE["partial_id"], "markers": markers, "requests": [], "projects": []}
    else:
        markers = [str(item) for item in partial["markers"]]
        existing_requests = verify_resume_requests(
            client,
            main_chat_id=main_chat_id,
            partial=partial,
            markers=markers,
            role_ids=role_ids,
        )
        _PARTIAL_EVIDENCE = dict(partial)
        _PARTIAL_EVIDENCE["markers"] = markers
        _PARTIAL_EVIDENCE.setdefault("requests", [])
        _PARTIAL_EVIDENCE.setdefault("projects", [])
        _PARTIAL_EVIDENCE.pop("status", None)
        _PARTIAL_EVIDENCE.pop("error", None)
    names = [f"{markers[0]}-登录功能", f"{markers[1]}-并行官网改版"]
    requests: list[dict[str, Any]] = []
    projects: list[dict[str, Any]] = []
    for marker, name in zip(markers, names):
        prior = existing_requests.get(marker)
        if prior is not None:
            sent = prior["sent"]
            requests.append({"marker": marker, "name": name, "sent": sent})
        else:
            text = request_prompt(marker=marker, name=name, role_ids=role_ids, selected=selected)
            sent = send_main_request(client, main_chat_id, text)
            requests.append({"marker": marker, "name": name, "sent": sent})
            _PARTIAL_EVIDENCE["requests"].append({"marker": marker, "name": name, "message_id": sent["id"], "seq": sent["seq"]})
        detail = wait_project_card(
            client,
            main_chat_id=main_chat_id,
            sent_seq=sent["seq"],
            sent_id=sent["id"],
            marker=marker,
            main_id=main_id,
            timeout=args.timeout,
            interval=args.interval,
        )
        record_project_evidence(detail, marker)
        validate_project(detail, marker=marker, main_id=main_id, role_ids=role_ids)
        projects.append(detail)
    gate_poll, gate_states = make_gate_poll(client, projects, markers, args)
    first_dispatch = wait_until(
        gated_check(
            gate_poll,
            lambda: opening_and_first_dispatch(client, project=projects[0]["project"], marker=markers[0], main_id=main_id, product_id=role_ids["产品"]),
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S2 first main opening and product dispatch",
    )
    second_dispatch = wait_until(
        gated_check(
            gate_poll,
            lambda: opening_and_first_dispatch(client, project=projects[1]["project"], marker=markers[1], main_id=main_id, product_id=role_ids["产品"]),
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S2 second main opening and product dispatch",
    )
    _PARTIAL_EVIDENCE["first_dispatch"] = {"opening_message_id": first_dispatch["opening"].get("id"), "assignment_id": first_dispatch["product"].get("id")}
    _PARTIAL_EVIDENCE["second_dispatch"] = {"opening_message_id": second_dispatch["opening"].get("id"), "assignment_id": second_dispatch["product"].get("id")}
    first_coding = wait_until(
        gated_check(
            gate_poll,
            lambda: coding_working(client, project_id=projects[0]["project"]["id"], coding_id=role_ids["编码"], product_assignment_id=first_dispatch["product"]["id"]),
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S2 coding assignment working",
    )
    parallel = wait_until(
        gated_check(
            gate_poll,
            lambda: parallel_coding_pair(client, first_project_id=projects[0]["project"]["id"], second_project_id=projects[1]["project"]["id"], coding_id=role_ids["编码"]),
        ),
        timeout=args.timeout,
        interval=args.interval,
        description="S2 same coding Bot parallel assignments",
    )
    _PARTIAL_EVIDENCE["parallel_coding"] = {"first_assignment_id": parallel["first"].get("id"), "second_assignment_id": parallel["second"].get("id")}
    steer = steer_evidence(
        client,
        project=projects[0]["project"],
        assignment=first_coding,
        coding_id=role_ids["编码"],
        marker=markers[0],
        timeout=args.timeout,
        interval=args.interval,
        gate_poll=gate_poll,
    )
    _PARTIAL_EVIDENCE["steer"] = steer
    subagent = wait_until(
        gated_check(gate_poll, lambda: subagent_evidence(client, [item["project"]["id"] for item in projects])),
        timeout=args.timeout,
        interval=args.interval,
        description="S2 subagent trace",
    )
    _PARTIAL_EVIDENCE["subagent"] = subagent
    gates = wait_until(
        gate_poll,
        timeout=args.timeout,
        interval=args.interval,
        description="S2 pending question and approval",
    )
    _PARTIAL_EVIDENCE["gates"] = gates
    chains: list[dict[str, Any]] = []
    for detail, dispatch, marker in zip(projects, [first_dispatch, second_dispatch], markers):
        chains.append(
            wait_until(
                gated_check(
                    gate_poll,
                    lambda detail=detail, dispatch=dispatch, marker=marker: completion_for_chain(client, project=detail["project"], first=dispatch, role_ids=role_ids, main_id=main_id, marker=marker),
                ),
                timeout=args.timeout,
                interval=args.interval,
                description=f"{marker} handoff chain",
            )
        )
    reviews: list[dict[str, Any]] = []
    for detail, request in zip(projects, requests):
        def review_ready(detail=detail, request=request) -> dict[str, Any] | None:
            project, _, _ = project_detail(client, detail["project"]["id"])
            if project.get("status") != "review":
                return None
            card = review_card(client, main_chat_id, project["id"], main_id, request["sent"]["seq"])
            return {"project": project, "review_card": card} if card else None
        reviews.append(wait_until(gated_check(gate_poll, review_ready), timeout=args.timeout, interval=args.interval, description="S2 review card"))
    confirmed: list[dict[str, Any]] = []
    for detail, marker in zip(projects, markers):
        result = require_dict(client.call("project.confirm_done", {"project_id": detail["project"]["id"]}), "project.confirm_done result")
        project = require_dict(result.get("project"), "project.confirm_done.project")
        if project.get("status") != "done" or not isinstance(project.get("done_at"), str):
            raise ValueError(f"{marker} project.confirm_done did not produce done")
        summary = wait_until(
            gated_check(gate_poll, lambda project=project, marker=marker: final_summary(client, project, main_id, marker)),
            timeout=args.timeout,
            interval=args.interval,
            description=f"{marker} main final summary",
        )
        confirmed.append({"project_id": project["id"], "summary": summary})
    return {
        "scenario": "S2 login feature",
        "status": "PASS",
        "url": client.base_url,
        "health_version": health.get("version"),
        "main_chat_id": main_chat_id,
        "role_bots": {role: {"id": bot.get("id"), "name": bot.get("name"), "label": bot.get("label")} for role, bot in selected.items()},
        "project_ids": [detail["project"]["id"] for detail in projects],
        "project_cards": [{"message_id": detail["card"].get("id"), "project_id": detail["project"]["id"]} for detail in projects],
        "first_dispatch": {"opening_message_id": first_dispatch["opening"].get("id"), "assignment_id": first_dispatch["product"].get("id")},
        "second_dispatch": {"opening_message_id": second_dispatch["opening"].get("id"), "assignment_id": second_dispatch["product"]["id"]},
        "parallel_coding": {"first_assignment_id": parallel["first"].get("id"), "second_assignment_id": parallel["second"].get("id")},
        "steer": steer,
        "subagent": subagent,
        "gates": gates,
        "handoff_chains": [{key: value.get("id") for key, value in chain.items()} for chain in chains],
        "reviews": reviews,
        "confirmed": confirmed,
        "note": "API checks only; desktop/Android UI, screenshots, push delivery, and browser login remain manual.",
    }


def scenario_with_partial(args: argparse.Namespace) -> dict[str, Any]:
    """Persist non-secret IDs on failure so a strict failure is still actionable."""

    global _PARTIAL_EVIDENCE, _PARTIAL_PATH
    try:
        return scenario(args)
    except Exception as exc:
        error = safe_error(args, exc)
        _PARTIAL_EVIDENCE["status"] = "FAIL"
        _PARTIAL_EVIDENCE["error"] = error
        path = _PARTIAL_PATH or Path("/tmp") / f"{_PARTIAL_EVIDENCE['partial_id']}.json"
        try:
            path.write_text(json.dumps(_PARTIAL_EVIDENCE, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        except OSError as write_error:
            raise ValueError(f"{error}; partial evidence write failed: {write_error}") from exc
        raise ValueError(f"{error}; partial evidence: {path}") from exc


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario_with_partial, parser.parse_args()))
