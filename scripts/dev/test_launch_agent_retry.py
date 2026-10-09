#!/usr/bin/env python3
"""Local tests for bounded LaunchAgent activation retries."""

from __future__ import annotations

import os
from pathlib import Path
import shlex
import stat
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
COMMON = REPO / "scripts" / "dev" / "common.sh"

LAUNCHCTL = "\n".join(
    [
        "#!/bin/bash",
        "set -u",
        "log=${MOCK_LOG:?}",
        "cmd=${1-}",
        "echo \"$*\" >> \"$log\"",
        "count_file=\"$MOCK_DIR/${cmd}.count\"",
        "count=0",
        "if [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi",
        "count=$((count + 1))",
        "echo \"$count\" > \"$count_file\"",
        "case \"$cmd\" in",
        "  bootstrap) [ \"$count\" -gt \"${MOCK_BOOTSTRAP_FAILS:-0}\" ];;",
        "  print) [ \"${MOCK_PRINT_OK:-0}\" = 1 ];;",
        "  load) [ \"${MOCK_LOAD_OK:-0}\" = 1 ];;",
        "  kickstart) [ \"$count\" -gt \"${MOCK_KICKSTART_FAILS:-0}\" ];;",
        "  *) exit 97;;",
        "esac",
    ]
) + "\n"

SLEEP = "\n".join(
    [
        "#!/bin/bash",
        "set -u",
        "echo sleep >> \"$MOCK_LOG\"",
        "count_file=\"$MOCK_DIR/sleep.count\"",
        "count=0",
        "if [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi",
        "echo \"$((count + 1))\" > \"$count_file\"",
        "exit 0",
    ]
) + "\n"


class LaunchAgentRetryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = Path(tempfile.mkdtemp(prefix="macbot-launch-agent-test-"))
        self.bin = self.temp / "bin"
        self.bin.mkdir()
        self.log = self.temp / "launchctl.log"
        for name, body in (("launchctl", LAUNCHCTL), ("sleep", SLEEP)):
            path = self.bin / name
            path.write_text(body, encoding="utf-8")
            path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def tearDown(self) -> None:
        for path in sorted(self.temp.rglob("*"), reverse=True):
            if path.is_dir():
                path.rmdir()
            else:
                path.unlink()
        self.temp.rmdir()

    def run_activation(self, **settings: str) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{self.bin}{os.pathsep}{env['PATH']}",
                "MOCK_DIR": str(self.temp),
                "MOCK_LOG": str(self.log),
                **settings,
            }
        )
        script = (
            f"source {shlex.quote(str(COMMON))}\n"
            # Advance the Bash clock for each mocked sleep so a 30s deadline
            # is exercised without waiting 30 wall-clock seconds.
            "sleep() { command sleep \"$@\"; SECONDS=$((SECONDS + 1)); }\n"
            "macbot_activate_launch_agent 501 com.macbot.test "
            f"{shlex.quote(str(self.temp / 'agent.plist'))}\n"
        )
        return subprocess.run(
            ["bash", "-c", script],
            cwd=REPO,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=10,
        )

    def calls(self) -> list[str]:
        return self.log.read_text(encoding="utf-8").splitlines() if self.log.exists() else []

    def count(self, command: str) -> int:
        return sum(1 for line in self.calls() if line.split(maxsplit=1)[0] == command)

    def test_transient_bootstrap_and_kickstart_eventually_succeed(self) -> None:
        result = self.run_activation(MOCK_BOOTSTRAP_FAILS="2", MOCK_KICKSTART_FAILS="3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.count("bootstrap"), 6)
        self.assertEqual(self.count("kickstart"), 4)
        self.assertEqual(self.count("load"), 0)
        self.assertEqual(self.count("sleep"), 5)

    def test_registered_print_stops_bootstrap_retries(self) -> None:
        result = self.run_activation(MOCK_BOOTSTRAP_FAILS="99", MOCK_PRINT_OK="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.count("bootstrap"), 0)
        self.assertEqual(self.count("print"), 1)
        self.assertEqual(self.count("load"), 0)
        self.assertEqual(self.count("kickstart"), 1)

    def test_registration_loss_retries_bootstrap_before_kickstart(self) -> None:
        result = self.run_activation(MOCK_KICKSTART_FAILS="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.count("bootstrap"), 2)
        self.assertEqual(self.count("kickstart"), 2)
        self.assertEqual(self.count("load"), 0)

    def test_permanent_failure_is_nonzero_and_bounded(self) -> None:
        result = self.run_activation(MOCK_BOOTSTRAP_FAILS="2147483647")
        self.assertNotEqual(result.returncode, 0)
        self.assertGreaterEqual(self.count("bootstrap"), 1)
        self.assertLessEqual(self.count("bootstrap"), 30)
        self.assertEqual(self.count("print"), self.count("bootstrap"))
        self.assertEqual(self.count("load"), 0)
        self.assertEqual(self.count("kickstart"), 0)
        self.assertLessEqual(self.count("sleep"), 30)

    def test_permanent_kickstart_failure_does_not_report_success(self) -> None:
        result = self.run_activation(MOCK_KICKSTART_FAILS="2147483647")
        self.assertNotEqual(result.returncode, 0)
        self.assertGreaterEqual(self.count("kickstart"), 1)
        self.assertLessEqual(self.count("kickstart"), 30)
        self.assertEqual(self.count("bootstrap"), self.count("kickstart"))
        self.assertEqual(self.count("load"), 0)
        self.assertLessEqual(self.count("sleep"), 30)


if __name__ == "__main__":
    unittest.main()
