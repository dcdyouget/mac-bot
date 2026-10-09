#!/usr/bin/env python3
"""Read-only contract precheck for the deterministic mock Host.

This is a mock API precheck only.  A PASS here is not an S1–S5 integration
result: the desktop/mobile UI, real provider, and end-to-end user flows still
require their stage-specific checks.

The protocol calls the search method ``search`` (section 5.12); there is no
``search.query`` RPC method on the wire.  The search request below therefore
uses the protocol method name and its documented ``query``/``kinds``/``limit``
parameters.
"""

from __future__ import annotations

import argparse
import sys
from typing import Any, Iterable

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parent))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    client_from_args,
    require_dict,
    require_list,
    run_main,
)


LOGIN_CHAT_ID = "chat_login"
LOGIN_PROJECT_ID = "prj_login"
LOGIN_MEMBER_IDS = frozenset({"bot_main", "bot_product", "bot_code", "bot_test"})


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    # The mock's fixed development endpoint must be the default for this
    # precheck; common.password_for_url supplies the matching ``dev`` token.
    add_connection_args(parser, default_url="http://127.0.0.1:7789")
    return parser


def object_by_id(items: Iterable[Any], item_id: str, label: str) -> dict[str, Any]:
    matches = [item for item in items if isinstance(item, dict) and item.get("id") == item_id]
    if len(matches) != 1:
        raise ValueError(f"{label} must contain exactly one {item_id!r} (found {len(matches)})")
    return matches[0]


def member_ids(value: Any, label: str, *, object_items: bool = False) -> set[str]:
    values = require_list(value, label)
    result: set[str] = set()
    for item in values:
        if object_items:
            if not isinstance(item, dict) or not isinstance(item.get("bot_id"), str):
                raise ValueError(f"{label} entries must contain string bot_id")
            bot_id = item["bot_id"]
        else:
            if not isinstance(item, str):
                raise ValueError(f"{label} entries must be strings")
            bot_id = item
        if bot_id in result:
            raise ValueError(f"{label} contains duplicate bot_id {bot_id!r}")
        result.add(bot_id)
    return result


def validate_workbench(value: Any) -> dict[str, Any]:
    workbench = require_dict(value, "workbench.get result")
    required = ("running", "global_limit", "subagents_running", "waiting", "bots", "done_today")
    missing = [key for key in required if key not in workbench]
    if missing:
        raise ValueError("workbench.get result is missing: " + ", ".join(missing))
    for key in ("running", "global_limit", "subagents_running"):
        number = workbench[key]
        if not isinstance(number, (int, float)) or isinstance(number, bool) or number < 0:
            raise ValueError(f"workbench.{key} must be a non-negative number")
    for key in ("waiting", "bots", "done_today"):
        if not isinstance(workbench[key], list):
            raise ValueError(f"workbench.{key} must be an array")
    return workbench


def validate_login_fixture(state: dict[str, Any], client: Any) -> dict[str, Any]:
    chats = require_list(state.get("chats"), "bootstrap.chats")
    projects = require_list(state.get("projects"), "bootstrap.projects")
    bots = require_list(state.get("bots"), "bootstrap.bots")
    login_chat = object_by_id(chats, LOGIN_CHAT_ID, "bootstrap.chats")
    if login_chat.get("kind") != "project" or login_chat.get("project_id") != LOGIN_PROJECT_ID:
        raise ValueError("chat_login must be a project chat for prj_login")
    chat_members = member_ids(login_chat.get("member_bot_ids"), "chat_login.member_bot_ids")
    if chat_members != LOGIN_MEMBER_IDS:
        raise ValueError(f"chat_login must contain the four login fixture members: {sorted(LOGIN_MEMBER_IDS)}")

    bootstrap_project = object_by_id(projects, LOGIN_PROJECT_ID, "bootstrap.projects")
    if bootstrap_project.get("chat_id") != LOGIN_CHAT_ID:
        raise ValueError("bootstrap prj_login.chat_id does not match chat_login")
    project_members = member_ids(bootstrap_project.get("members"), "bootstrap prj_login.members", object_items=True)
    if project_members != chat_members:
        raise ValueError("bootstrap prj_login.members does not match chat_login.member_bot_ids")

    detail = require_dict(client.call("project.get", {"project_id": LOGIN_PROJECT_ID}), "project.get result")
    project = require_dict(detail.get("project"), "project.get.project")
    announcement = require_dict(detail.get("announcement"), "project.get.announcement")
    if project.get("id") != LOGIN_PROJECT_ID or project.get("chat_id") != LOGIN_CHAT_ID:
        raise ValueError("project.get project identity does not match the login fixture")
    detail_members = member_ids(project.get("members"), "project.get.project.members", object_items=True)
    if detail_members != chat_members:
        raise ValueError("project.get.project.members does not match chat_login.member_bot_ids")
    if announcement.get("project_id") != LOGIN_PROJECT_ID:
        raise ValueError("project.get.announcement.project_id does not match prj_login")
    announcement_members = member_ids(announcement.get("members"), "project.get.announcement.members", object_items=True)
    if announcement_members != chat_members:
        raise ValueError("project.get.announcement.members does not match chat_login.member_bot_ids")

    bot_by_id = {bot.get("id"): bot for bot in bots if isinstance(bot, dict) and isinstance(bot.get("id"), str)}
    for bot_id in LOGIN_MEMBER_IDS:
        bot = bot_by_id.get(bot_id)
        if not isinstance(bot, dict) or not isinstance(bot.get("dm_chat_id"), str):
            raise ValueError(f"login fixture bot {bot_id!r} must have a dm_chat_id")
    return {"chat": login_chat, "project": project, "announcement": announcement, "bots": bot_by_id}


def validate_pending(state: dict[str, Any], fixture: dict[str, Any], client: Any) -> int:
    pending = require_dict(state.get("pending"), "bootstrap.pending")
    assignment_result = require_dict(client.call("assignment.list", {"limit": 100}), "assignment.list result")
    assignments = [item for item in require_list(assignment_result.get("items"), "assignment.list.items") if isinstance(item, dict)]
    assignment_by_id = {item.get("id"): item for item in assignments if isinstance(item.get("id"), str)}
    bot_by_id = fixture["bots"]
    references = 0
    for key in ("approvals", "questions"):
        for item in require_list(pending.get(key), f"bootstrap.pending.{key}"):
            pending_item = require_dict(item, f"bootstrap.pending.{key} entry")
            assignment_id = pending_item.get("assignment_id")
            bot_id = pending_item.get("bot_id")
            chat_id = pending_item.get("chat_id")
            if not isinstance(assignment_id, str) or assignment_id not in assignment_by_id:
                raise ValueError(f"pending {key} item must reference an assignment.list item")
            assignment = assignment_by_id[assignment_id]
            if not isinstance(bot_id, str) or assignment.get("bot_id") != bot_id:
                raise ValueError(f"pending {key} assignment bot_id does not match")
            bot = bot_by_id.get(bot_id)
            if not isinstance(bot, dict) or chat_id != bot.get("dm_chat_id"):
                raise ValueError(f"pending {key} chat_id must equal the Bot dm_chat_id")
            references += 1
    for review_id in require_list(pending.get("reviews"), "bootstrap.pending.reviews"):
        if not isinstance(review_id, str):
            raise ValueError("bootstrap.pending.reviews entries must be project IDs")
    return references


def validate_search(client: Any) -> dict[str, Any]:
    # PROTOCOL 5.12 uses method ``search``; ``search.query`` is not a wire RPC.
    result = require_dict(
        client.call("search", {"query": "PRD", "kinds": ["artifact"], "limit": 20}),
        "search result",
    )
    hits = require_list(result.get("results"), "search.results")
    if not hits:
        raise ValueError("artifact search for PRD returned no results")
    required = ("kind", "id", "chat_id", "title", "snippet", "at")
    for hit in hits:
        item = require_dict(hit, "search.results entry")
        missing = [key for key in required if key not in item]
        if missing:
            raise ValueError("search result is missing: " + ", ".join(missing))
        if item.get("kind") != "artifact" or not isinstance(item.get("id"), str) or not item["id"]:
            raise ValueError("PRD artifact search returned an invalid artifact identity")
        if not isinstance(item.get("title"), str) or not item["title"]:
            raise ValueError("search artifact title must be a non-empty string")
        if not isinstance(item.get("snippet"), str):
            raise ValueError("search artifact snippet must be a string")
        if item.get("chat_id") is not None and not isinstance(item.get("chat_id"), str):
            raise ValueError("search artifact chat_id must be string or null")
        if item.get("at") is not None and not isinstance(item.get("at"), str):
            raise ValueError("search artifact at must be string or null")
    return {"count": len(hits), "kinds": sorted({item["kind"] for item in hits})}


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    state = bootstrap(client)
    fixture = validate_login_fixture(state, client)
    workbench = validate_workbench(client.call("workbench.get", {}))
    pending_references = validate_pending(state, fixture, client)
    search = validate_search(client)
    return {
        "scenario": "mock API contract precheck",
        "status": "PASS",
        "url": client.base_url,
        "bootstrap": {"chat": LOGIN_CHAT_ID, "project": LOGIN_PROJECT_ID, "members": len(LOGIN_MEMBER_IDS)},
        "workbench": {
            "running": workbench["running"],
            "bots": len(workbench["bots"]),
            "waiting": len(workbench["waiting"]),
            "done_today": len(workbench["done_today"]),
        },
        "pending_references": pending_references,
        "search": search,
        "note": "Mock API precheck only; this does not count as S1–S5 real integration acceptance.",
    }


if __name__ == "__main__":
    parser = args_parser()
    raise SystemExit(run_main(scenario, parser.parse_args()))
