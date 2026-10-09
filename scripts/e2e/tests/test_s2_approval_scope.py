#!/usr/bin/env python3
"""Unit tests for the content-blind S2 approval scope helper."""

from __future__ import annotations

import json
from pathlib import Path
import shlex
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

E2E_S2 = Path(__file__).resolve().parents[1] / "s2"
sys.path.insert(0, str(E2E_S2))

import approval_scope  # noqa: E402
from approval_scope import check_approval_scope  # noqa: E402


MARKER = "macbot-e2e-s2-approval-scope-test"


def approval(tool: str, risk: str, detail: object, *, state: str = "pending") -> dict[str, object]:
    return {"tool": tool, "risk": risk, "state": state, "detail": json.dumps(detail, ensure_ascii=False)}


class ApprovalScopeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.projects = Path.home() / "MacBot" / "projects"
        self.projects.mkdir(parents=True, exist_ok=True)
        self.temp = Path(tempfile.mkdtemp(prefix=MARKER + "-", dir=self.projects))

    def tearDown(self) -> None:
        shutil.rmtree(self.temp, ignore_errors=True)

    def assert_allowed(self, result: dict[str, object], operation: str) -> None:
        self.assertIs(result.get("authorized"), True)
        self.assertEqual(result.get("operation"), operation)

    def assert_denied(self, result: dict[str, object]) -> None:
        self.assertIs(result.get("authorized"), False)
        self.assertIsInstance(result.get("reason"), str)

    def test_exact_marker_bash_is_allowed_without_detail_echo(self) -> None:
        command = f"mkdir -p e2e && printf '%s' '{MARKER}' > e2e/{MARKER}.txt"
        result = check_approval_scope(approval("bash", "exec", {"command": command}), self.temp, MARKER)
        self.assert_allowed(result, "marker_file")
        self.assertNotIn(MARKER, json.dumps(result))

    def test_read_only_s2_bash_diagnostics_are_exact_and_content_blind(self) -> None:
        self.assert_allowed(
            check_approval_scope(approval("bash", "exec", {"command": "echo $HOME"}), self.temp, MARKER),
            "echo_home",
        )
        command = f"ls ~/MacBot/projects/{self.temp.name}"
        result = check_approval_scope(approval("bash", "exec", {"command": command}), self.temp, MARKER)
        self.assert_allowed(result, "project_home_ls")
        self.assertNotIn("/Users/", json.dumps(result))

    def test_read_only_s2_bash_diagnostics_reject_variants(self) -> None:
        project = f"~/MacBot/projects/{self.temp.name}"
        for command in (
            'echo "$HOME"',
            "echo $HOME/extra",
            "echo $HOME && pwd",
            f"ls -la {project}",
            f"ls '{project}'",
            f'ls "{project}"',
            f"ls ~/MacBot/projects/{MARKER}",
            f"ls ~/MacBot/projects/{self.temp.name}/.",
            f"ls ~/MacBot/projects/{self.temp.name} other",
        ):
            self.assert_denied(
                check_approval_scope(approval("bash", "exec", {"command": command}), self.temp, MARKER)
            )

    def test_unquoted_project_ls_rejects_unsafe_project_home_names(self) -> None:
        # These are real entries in the trusted namespace, but their names
        # cannot safely be interpolated into the deliberately unquoted ls
        # command. This exercises only the approval predicate; no shell runs.
        for suffix in (" with space", ";echo"):
            unsafe_home = self.projects / f"{MARKER}{suffix}"
            unsafe_home.mkdir()
            try:
                command = f"ls ~/MacBot/projects/{unsafe_home.name}"
                result = check_approval_scope(
                    approval("bash", "exec", {"command": command}), unsafe_home, MARKER
                )
                self.assert_denied(result)
                self.assertIn("unsafe", result["reason"])
            finally:
                shutil.rmtree(unsafe_home, ignore_errors=True)

    def test_marker_bash_rejects_e2e_symlink_escape(self) -> None:
        outside = Path(tempfile.mkdtemp(prefix="macbot-s2-marker-outside-"))
        try:
            (self.temp / "e2e").symlink_to(outside, target_is_directory=True)
            command = f"mkdir -p e2e && printf '%s' '{MARKER}' > e2e/{MARKER}.txt"
            self.assert_denied(check_approval_scope(approval("bash", "exec", {"command": command}), self.temp, MARKER))
        finally:
            shutil.rmtree(outside, ignore_errors=True)

    def test_project_home_mkdir_allows_exact_and_quoted_space_path(self) -> None:
        home_token = "~/MacBot/projects/" + self.temp.name + "/"
        self.assert_allowed(
            check_approval_scope(approval("bash", "exec", {"command": f"mkdir -p {home_token}"}), self.temp, MARKER),
            "project_home_mkdir",
        )
        spaced = self.projects / f"{MARKER} with space"
        spaced.mkdir()
        try:
            command = "mkdir -p " + shlex.quote(str(spaced))
            self.assert_allowed(
                check_approval_scope(approval("bash", "exec", {"command": command}), spaced, MARKER),
                "project_home_mkdir",
            )
        finally:
            shutil.rmtree(spaced, ignore_errors=True)

    def test_project_home_mkdir_rejects_shell_metacharacters(self) -> None:
        for suffix in ("$(touch macbot-s2-should-not-run)", "*"):
            evil = self.projects / f"{MARKER}{suffix}"
            evil.mkdir()
            try:
                command = f"mkdir -p ~/MacBot/projects/{evil.name}/"
                self.assert_denied(check_approval_scope(approval("bash", "exec", {"command": command}), evil, MARKER))
            finally:
                shutil.rmtree(evil, ignore_errors=True)

    def test_projects_root_symlink_to_external_tree_is_denied(self) -> None:
        virtual_home = Path(tempfile.mkdtemp(prefix="macbot-s2-home-"))
        external = Path(tempfile.mkdtemp(prefix="macbot-s2-projects-outside-"))
        try:
            (virtual_home / "MacBot").mkdir()
            (virtual_home / "MacBot" / "projects").symlink_to(external, target_is_directory=True)
            project = external / MARKER
            project.mkdir()
            with patch.object(approval_scope.Path, "home", return_value=virtual_home):
                result = check_approval_scope(
                    approval("bash", "exec", {"command": f"mkdir -p {project}"}), project, MARKER
                )
            self.assert_denied(result)
        finally:
            shutil.rmtree(virtual_home, ignore_errors=True)
            shutil.rmtree(external, ignore_errors=True)

    def test_write_and_edit_inside_home_are_allowed(self) -> None:
        write = check_approval_scope(
            approval("write", "write", {"path": "src/README.md", "content": "secret marker content"}),
            self.temp,
            MARKER,
        )
        edit = check_approval_scope(
            approval(
                "edit",
                "write",
                {"path": str(self.temp / "src" / "README.md"), "edits": [{"oldText": "a", "newText": "b"}]},
            ),
            self.temp,
            MARKER,
        )
        self.assert_allowed(write, "project_home_file_mutation")
        self.assert_allowed(edit, "project_home_file_mutation")
        self.assertNotIn("secret", json.dumps(write))

    def test_write_and_edit_reject_runtime_leading_tilde_paths(self) -> None:
        raw_path = f"~/MacBot/projects/{MARKER}/index.html"
        write = check_approval_scope(
            approval("write", "write", {"path": raw_path, "content": "secret"}),
            self.temp,
            MARKER,
        )
        edit = check_approval_scope(
            approval("edit", "write", {"path": raw_path, "edits": [{"oldText": "a", "newText": "b"}]}),
            self.temp,
            MARKER,
        )
        self.assert_denied(write)
        self.assertIn("runtime", write["reason"])
        self.assert_denied(edit)
        self.assertIn("runtime", edit["reason"])

    def test_home_v1_metadata_allows_matching_tilde_path(self) -> None:
        raw_path = f"~/MacBot/projects/{self.temp.name}/index.html"
        resolved = self.temp / "index.html"
        write = check_approval_scope(
            approval(
                "write",
                "write",
                {
                    "path": raw_path,
                    "content": "secret content",
                    "resolved_path": str(resolved),
                    "path_resolution": "home-v1",
                },
            ),
            self.temp,
            MARKER,
        )
        self.assert_allowed(write, "project_home_file_mutation")
        self.assertNotIn("secret", json.dumps(write))

    def test_home_v1_metadata_allows_matching_relative_edit(self) -> None:
        resolved = self.temp / "src" / "README.md"
        result = check_approval_scope(
            approval(
                "edit",
                "write",
                {
                    "path": "src/README.md",
                    "edits": [{"oldText": "a", "newText": "b"}],
                    "resolved_path": str(resolved),
                    "path_resolution": "home-v1",
                },
            ),
            self.temp,
            MARKER,
        )
        self.assert_allowed(result, "project_home_file_mutation")

    def test_home_v1_metadata_requires_exact_pair_and_matching_target(self) -> None:
        raw_path = f"~/MacBot/projects/{self.temp.name}/index.html"
        base = {"path": raw_path, "content": "secret"}
        cases = [
            {**base, "resolved_path": str(self.temp / "index.html")},
            {**base, "path_resolution": "home-v1"},
            {
                **base,
                "resolved_path": str(self.temp / "index.html"),
                "path_resolution": "home-v2",
            },
            {
                **base,
                "resolved_path": "relative/index.html",
                "path_resolution": "home-v1",
            },
            {
                **base,
                "resolved_path": str(self.temp / "other.html"),
                "path_resolution": "home-v1",
            },
            {
                **base,
                "resolved_path": str(self.temp / "index.html"),
                "path_resolution": "home-v1",
                "extra": "unexpected",
            },
        ]
        for detail in cases:
            self.assert_denied(check_approval_scope(approval("write", "write", detail), self.temp, MARKER))

    def test_home_v1_metadata_rejects_symlink_escape_for_both_spellings(self) -> None:
        outside = Path(tempfile.mkdtemp(prefix="macbot-s2-home-v1-outside-"))
        try:
            (self.temp / "linked").symlink_to(outside, target_is_directory=True)
            raw_path = f"~/MacBot/projects/{self.temp.name}/linked/escape.txt"
            result = check_approval_scope(
                approval(
                    "write",
                    "write",
                    {
                        "path": raw_path,
                        "content": "secret",
                        "resolved_path": str(outside / "escape.txt"),
                        "path_resolution": "home-v1",
                    },
                ),
                self.temp,
                MARKER,
            )
            self.assert_denied(result)
        finally:
            shutil.rmtree(outside, ignore_errors=True)

    def test_file_escape_symlink_and_extra_args_are_denied(self) -> None:
        outside = Path(tempfile.mkdtemp(prefix="macbot-s2-outside-"))
        try:
            for path in ("../outside.txt", str(outside / "absolute.txt")):
                self.assert_denied(
                    check_approval_scope(approval("write", "write", {"path": path, "content": "secret"}), self.temp, MARKER)
                )
            (self.temp / "linked").symlink_to(outside, target_is_directory=True)
            self.assert_denied(
                check_approval_scope(
                    approval("write", "write", {"path": "linked/escaped.txt", "content": "secret"}),
                    self.temp,
                    MARKER,
                )
            )
        finally:
            shutil.rmtree(outside, ignore_errors=True)
        self.assert_denied(
            check_approval_scope(
                approval("write", "write", {"path": "a.txt", "content": "x", "mode": 0o777}), self.temp, MARKER
            )
        )
        self.assert_denied(
            check_approval_scope(
                approval("edit", "write", {"path": "a.txt", "edits": [{"oldText": "a", "newText": "b", "mode": "x"}]}),
                self.temp,
                MARKER,
            )
        )

    def test_arbitrary_bash_wrong_risk_nonpending_and_unknown_tool_are_denied(self) -> None:
        for candidate in (
            approval("bash", "exec", {"command": "rm -rf ~/MacBot/projects"}),
            approval("bash", "write", {"command": "mkdir -p " + str(self.temp)}),
            approval("bash", "exec", {"command": "mkdir -p " + str(self.temp)}, state="allowed_once"),
            approval("browser_open", "external", {"url": "https://example.invalid"}),
        ):
            self.assert_denied(check_approval_scope(candidate, self.temp, MARKER))

    def test_project_memory_add_requires_verified_project_id(self) -> None:
        project_id = "01a12164-6b92-732d-9ecb-ed48e8a94ca8"
        detail = {"action": "add", "scope": "project", "project_id": project_id, "content": "private content"}
        result = check_approval_scope(
            approval("memory", "write", detail), self.temp, MARKER, project_id=project_id
        )
        self.assert_allowed(result, "project_memory_add")
        self.assertNotIn("private content", json.dumps(result))

    def test_project_memory_allows_only_matching_optional_kind(self) -> None:
        project_id = "01a12164-6b92-732d-9ecb-ed48e8a94ca8"
        detail = {
            "action": "add", "scope": "project", "project_id": project_id,
            "kind": "project", "content": "private content",
        }
        result = check_approval_scope(
            approval("memory", "write", detail), self.temp, MARKER, project_id=project_id
        )
        self.assert_allowed(result, "project_memory_add")
        self.assertNotIn("private content", json.dumps(result))

    def test_project_memory_rejects_wrong_id_missing_id_broader_scope_and_extra_args(self) -> None:
        project_id = "project-owned"
        base = {"action": "add", "scope": "project", "project_id": project_id, "content": "secret"}
        cases = [
            (base, "other-project"),
            (base, None),
            ({**base, "scope": "global"}, project_id),
            ({**base, "action": "replace"}, project_id),
            ({**base, "kind": "project_status"}, project_id),
            ({**base, "kind": "bot_experience"}, project_id),
            ({**base, "id": "client-chosen-id"}, project_id),
            ({**base, "extra": "unexpected"}, project_id),
        ]
        for detail, verified_id in cases:
            self.assert_denied(
                check_approval_scope(approval("memory", "write", detail), self.temp, MARKER, project_id=verified_id)
            )


if __name__ == "__main__":
    unittest.main()
