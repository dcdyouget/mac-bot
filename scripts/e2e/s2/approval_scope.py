#!/usr/bin/env python3
"""Strict, content-blind approval matching for the S2 local project.

The server serializes tool arguments in the approval ``detail`` field as JSON
(see ``execution.rs``).  This helper checks only the approval envelope and the
argument shape/path scope.  It never returns command text, file content, or
credentials, so callers can safely persist its decision as test evidence.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shlex
from typing import Any, Mapping


_MARKER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{2,127}$")
_PROJECTS_NAME = "projects"
_SHELL_META_RE = re.compile(r"[$`;&|<>()\r\n*?\[\]]")


def _reject(reason: str, *, tool: Any = None) -> dict[str, Any]:
    result: dict[str, Any] = {"authorized": False, "reason": reason}
    if isinstance(tool, str):
        result["tool"] = tool
    return result


def _allow(tool: str, operation: str) -> dict[str, Any]:
    return {"authorized": True, "reason": "scenario-owned and path-scoped", "tool": tool, "operation": operation}


def _canonical_project_home(project_home: str | os.PathLike[str], marker: str) -> Path | None:
    if not isinstance(project_home, (str, os.PathLike)):
        return None
    lexical = Path(os.path.expanduser(os.fspath(project_home))).absolute()
    root = (Path.home() / "MacBot" / _PROJECTS_NAME).absolute()
    # The requested Home must use the expected local project namespace before
    # resolving existing symlinks; paths elsewhere are never in scope.
    try:
        lexical.relative_to(root)
    except ValueError:
        return None
    if lexical == root or marker not in lexical.name:
        return None
    try:
        root_real = root.resolve(strict=False)
        home_real = lexical.resolve(strict=False)
    except OSError:
        return None
    # The projects namespace is itself a trust boundary. A root symlink could
    # make a lexical ~/MacBot/projects path resolve outside the local tree.
    if root_real != root:
        return None
    try:
        home_real.relative_to(root_real)
    except ValueError:
        return None
    if marker not in home_real.name:
        return None
    return home_real


def _within_home(raw_path: Any, home: Path) -> bool:
    if not isinstance(raw_path, str) or not raw_path or "\x00" in raw_path:
        return False
    candidate = Path(os.path.expanduser(raw_path))
    if not candidate.is_absolute():
        candidate = home / candidate
    try:
        resolved = candidate.resolve(strict=False)
        resolved.relative_to(home)
    except (OSError, ValueError):
        return False
    # A write/edit target must be a descendant file path, not the project
    # directory itself. Existing symlink components are covered by resolve().
    return resolved != home


def _decode_detail(approval: Mapping[str, Any], tool: str) -> dict[str, Any] | None:
    detail = approval.get("detail")
    if not isinstance(detail, str):
        return None
    try:
        value = json.loads(detail)
    except (TypeError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def _exact_bash(detail: dict[str, Any], marker: str, home: Path) -> dict[str, Any]:
    if set(detail) != {"command"} or not isinstance(detail.get("command"), str):
        return _reject("bash arguments are not the exact command shape", tool="bash")
    command = detail["command"]
    expected_marker_command = f"mkdir -p e2e && printf '%s' '{marker}' > e2e/{marker}.txt"
    if command == expected_marker_command:
        if not _within_home(f"e2e/{marker}.txt", home):
            return _reject("marker output path escapes this project Home", tool="bash")
        return _allow("bash", "marker_file")
    if _SHELL_META_RE.search(command):
        return _reject("bash mkdir contains shell metacharacters", tool="bash")
    try:
        tokens = shlex.split(command, posix=True)
    except ValueError:
        return _reject("bash command is not valid shell syntax", tool="bash")
    if len(tokens) != 3 or tokens[:2] != ["mkdir", "-p"] or not isinstance(tokens[2], str):
        return _reject("bash command is outside the scenario allowlist", tool="bash")
    try:
        target = Path(os.path.expanduser(tokens[2])).resolve(strict=False)
    except OSError:
        return _reject("bash project Home cannot be resolved", tool="bash")
    if target != home:
        return _reject("bash mkdir target is not this project Home", tool="bash")
    return _allow("bash", "project_home_mkdir")


def _scoped_file(detail: dict[str, Any], tool: str, home: Path) -> dict[str, Any]:
    expected = {"path", "content"} if tool == "write" else {"path", "edits"}
    if set(detail) != expected:
        return _reject(f"{tool} arguments contain unexpected fields", tool=tool)
    if tool == "write" and not isinstance(detail.get("content"), str):
        return _reject("write content has the wrong type", tool=tool)
    if tool == "edit":
        edits = detail.get("edits")
        if not isinstance(edits, list) or not edits:
            return _reject("edit edits must be a non-empty array", tool=tool)
        for edit in edits:
            if not isinstance(edit, dict) or set(edit) != {"oldText", "newText"}:
                return _reject("edit contains an unexpected edit shape", tool=tool)
            if not isinstance(edit["oldText"], str) or not isinstance(edit["newText"], str):
                return _reject("edit replacement text has the wrong type", tool=tool)
    raw_path = detail.get("path")
    # ToolContext receives these paths as ordinary strings. It does not run
    # shell/path expansion, so a leading ``~`` would be treated as a literal
    # directory under CWD and can land outside the intended target. The
    # project_home configuration itself is still expanded by
    # _canonical_project_home; only tool-supplied write/edit paths are
    # rejected here until the runtime contract is explicit.
    if isinstance(raw_path, str) and raw_path.startswith("~"):
        return _reject(f"{tool} leading-~ path is ambiguous for the runtime path resolver", tool=tool)
    if not _within_home(raw_path, home):
        return _reject(f"{tool} path is outside this project Home", tool=tool)
    return _allow(tool, "project_home_file_mutation")


def _scoped_memory(detail: dict[str, Any], project_id: str | None) -> dict[str, Any]:
    if project_id is None or not isinstance(project_id, str) or not project_id:
        return _reject("caller did not provide the verified project ID", tool="memory")
    if set(detail) != {"action", "scope", "project_id", "content"}:
        return _reject("memory arguments contain unexpected fields", tool="memory")
    if detail.get("action") != "add" or detail.get("scope") != "project":
        return _reject("memory action or scope is outside the project allowlist", tool="memory")
    if detail.get("project_id") != project_id:
        return _reject("memory project ID does not match the verified project", tool="memory")
    if not isinstance(detail.get("content"), str):
        return _reject("memory content has the wrong type", tool="memory")
    return _allow("memory", "project_memory_add")


def check_approval_scope(
    approval: Mapping[str, Any],
    project_home: str | os.PathLike[str],
    marker: str,
    *,
    project_id: str | None = None,
) -> dict[str, Any]:
    """Return a content-blind authorization decision for one approval object.

    ``approval.detail`` must be the server's JSON string of tool arguments.
    Only pending ``bash``/``write``/``edit``/project-scoped ``memory``
    approvals with the required risk and project scope are authorized. The
    result deliberately contains no detail, command, path, or file content.
    """

    if not isinstance(approval, Mapping):
        return _reject("approval is not an object")
    if not isinstance(marker, str) or not _MARKER_RE.fullmatch(marker):
        return _reject("marker is not a safe scenario marker")
    home = _canonical_project_home(project_home, marker)
    if home is None:
        return _reject("project Home is outside ~/MacBot/projects or lacks this marker")
    tool = approval.get("tool")
    if tool not in {"bash", "write", "edit", "memory"}:
        return _reject("tool is outside the scenario allowlist", tool=tool)
    if approval.get("state") != "pending":
        return _reject("approval is not pending", tool=tool)
    expected_risk = "exec" if tool == "bash" else "write"
    if approval.get("risk") != expected_risk:
        return _reject("approval risk does not match the tool", tool=tool)
    detail = _decode_detail(approval, tool)
    if detail is None:
        return _reject("approval detail is not a JSON object", tool=tool)
    if tool == "bash":
        return _exact_bash(detail, marker, home)
    if tool == "memory":
        return _scoped_memory(detail, project_id)
    return _scoped_file(detail, tool, home)


# Short alias for integration callers that prefer a predicate-oriented name.
evaluate_approval_scope = check_approval_scope


__all__ = ["check_approval_scope", "evaluate_approval_scope"]
