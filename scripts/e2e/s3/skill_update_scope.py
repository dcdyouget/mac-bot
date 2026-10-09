#!/usr/bin/env python3
"""S3 API check for skill.update preserving enabled and per-Bot scope.

The scenario creates one uniquely named user skill, disables it globally and
for one selected Bot, updates only its content, and verifies both scopes are
still present.  It deletes only that skill in ``finally``.  No skill content
is printed; failures point to a local partial-evidence JSON file.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import Any

HERE = pathlib.Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    client_from_args,
    ready_health,
    require_dict,
    require_list,
    require_production_host,
    run_main,
    unique_marker,
)


SKILL_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--bot-id", help="Non-main Bot whose disabled scope is checked; default first non-main Bot")
    return parser


def content_for(skill_name: str, marker: str, revision: str) -> str:
    return (
        "---\n"
        f"name: {skill_name}\n"
        "description: S3 skill update scope regression\n"
        "---\n\n"
        f"# {skill_name}\n\n"
        f"Scope regression marker: {marker}\n"
        f"Revision: {revision}\n"
    )


def disabled_ids(skill: dict[str, Any], label: str) -> set[str]:
    values = require_list(skill.get("disabled_bot_ids"), f"{label}.disabled_bot_ids")
    if any(not isinstance(value, str) or not value for value in values):
        raise ValueError(f"{label}.disabled_bot_ids contains an invalid Bot ID")
    return set(values)


def validate_skill_state(
    skill: dict[str, Any],
    *,
    expected_name: str,
    expected_enabled: bool,
    bot_id: str,
    expected_disabled: bool,
    label: str,
) -> dict[str, Any]:
    if skill.get("name") != expected_name:
        raise ValueError(f"{label} returned a different skill name")
    if skill.get("enabled") is not expected_enabled:
        raise ValueError(f"{label}.enabled changed unexpectedly")
    ids = disabled_ids(skill, label)
    if (bot_id in ids) is not expected_disabled:
        raise ValueError(f"{label}.disabled_bot_ids does not preserve the selected Bot scope")
    return {"enabled": skill.get("enabled"), "bot_disabled": bot_id in ids, "disabled_count": len(ids)}


def partial_path(marker: str) -> pathlib.Path:
    return pathlib.Path("/tmp") / f"{marker}.json"


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    bots = [item for item in require_list(state.get("bots"), "bootstrap.bots") if isinstance(item, dict)]
    selected = next((item for item in bots if item.get("id") == args.bot_id), None) if args.bot_id else None
    if args.bot_id and selected is None:
        raise ValueError("requested Bot is absent from bootstrap")
    if selected is None:
        selected = next((item for item in bots if item.get("is_main") is False), None)
    if not isinstance(selected, dict) or not isinstance(selected.get("id"), str):
        raise ValueError("S3 skill scope check needs a non-main Bot")
    if selected.get("is_main") is True:
        raise ValueError("per-Bot scope check refuses the main Bot")
    bot_id = selected["id"]

    marker = unique_marker("macbot-e2e-s3-skill-scope")
    skill_name = marker.lower()
    if not SKILL_NAME_RE.fullmatch(skill_name):
        raise ValueError("generated skill name does not match the protocol name contract")
    content_v1 = content_for(skill_name, marker, "v1")
    content_v2 = content_for(skill_name, marker, "v2")
    evidence: dict[str, Any] = {
        "scenario": "S3 skill update scope",
        "marker": marker,
        "skill_name": skill_name,
        "bot_id": bot_id,
        "status": "RUNNING",
        "steps": [],
    }
    created = False
    original_error: BaseException | None = None
    cleanup_error: BaseException | None = None
    try:
        result = require_dict(
            client.call("skill.create", {"name": skill_name, "content": content_v1}),
            "skill.create result",
        )
        skill = require_dict(result.get("skill"), "skill.create.skill")
        created = True
        if skill.get("source") != "user":
            raise ValueError("skill.create did not create a user skill")
        validate_skill_state(
            skill,
            expected_name=skill_name,
            expected_enabled=True,
            bot_id=bot_id,
            expected_disabled=False,
            label="skill.create.skill",
        )
        evidence["steps"].append("created_enabled")

        result = require_dict(
            client.call("skill.set_enabled", {"name": skill_name, "enabled": False}),
            "skill.set_enabled global result",
        )
        skill = require_dict(result.get("skill"), "skill.set_enabled global.skill")
        validate_skill_state(
            skill,
            expected_name=skill_name,
            expected_enabled=False,
            bot_id=bot_id,
            expected_disabled=False,
            label="skill.set_enabled global.skill",
        )
        evidence["steps"].append("globally_disabled")

        result = require_dict(
            client.call("skill.set_enabled", {"name": skill_name, "enabled": False, "bot_id": bot_id}),
            "skill.set_enabled per-Bot result",
        )
        skill = require_dict(result.get("skill"), "skill.set_enabled per-Bot.skill")
        validate_skill_state(
            skill,
            expected_name=skill_name,
            expected_enabled=False,
            bot_id=bot_id,
            expected_disabled=True,
            label="skill.set_enabled per-Bot.skill",
        )
        evidence["steps"].append("per_bot_disabled")

        result = require_dict(
            client.call("skill.update", {"name": skill_name, "content": content_v2}),
            "skill.update result",
        )
        updated = require_dict(result.get("skill"), "skill.update.skill")
        if updated.get("name") != skill_name:
            raise ValueError("skill.update returned a different skill")
        detail_result = require_dict(client.call("skill.get", {"name": skill_name}), "skill.get after update result")
        detail = require_dict(detail_result.get("skill"), "skill.get after update.skill")
        if detail.get("content") != content_v2:
            raise ValueError("skill.get after update did not return the new content")
        validate_skill_state(
            detail,
            expected_name=skill_name,
            expected_enabled=False,
            bot_id=bot_id,
            expected_disabled=True,
            label="skill.get after update.skill",
        )
        evidence["steps"].append("updated_content_scope_preserved")
        evidence["status"] = "PASS"
        evidence["conclusion"] = "API PASS: skill.update changed content while preserving global disabled and per-Bot disabled scope."
        return evidence
    except BaseException as exc:
        original_error = exc
        evidence["status"] = "FAIL"
        evidence["error"] = str(exc)
        raise ValueError(f"{exc}; partial evidence: {partial_path(marker)}") from exc
    finally:
        if created:
            try:
                client.call("skill.delete", {"name": skill_name})
                evidence["cleanup"] = "deleted_own_skill"
            except BaseException as exc:
                cleanup_error = exc
                evidence["cleanup"] = "delete_failed"
                evidence["cleanup_error"] = str(exc)
        if original_error is not None or cleanup_error is not None:
            path = partial_path(marker)
            try:
                path.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            except OSError:
                pass
            if original_error is None and cleanup_error is not None:
                raise ValueError(f"skill cleanup failed; partial evidence: {path}") from cleanup_error
            if original_error is not None and cleanup_error is not None:
                message = f"{original_error}; cleanup also failed; partial evidence: {path}"
                raise ValueError(message) from original_error
            if original_error is not None:
                # The original exception is re-raised by the except block;
                # this branch only documents the local path for callers that
                # inspect the process output.
                pass


if __name__ == "__main__":
    raise SystemExit(run_main(scenario, args_parser().parse_args()))
