#!/usr/bin/env python3
"""Exercise the bundled agent-browser CLI without user data or network access.

The test creates two independent named sessions, opens a local data URL in
each, verifies stable tab ids and disjoint CDP targets, then takes a JPEG and
performs a DOM click.  Pass ``--binary`` or set ``MACBOT_BROWSER_BIN``.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid


def run(binary: Path, session: str, namespace: str, config: Path, *args: str) -> dict:
    command = [
        str(binary),
        "--namespace",
        namespace,
        "--config",
        str(config),
        "--headed",
        "false",
        "--session",
        session,
        "--json",
        *args,
    ]
    result = subprocess.run(command, check=True, capture_output=True, text=True)
    value = json.loads(result.stdout)
    assert value.get("success") is True, value
    return value["data"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path(os.environ.get("MACBOT_BROWSER_BIN", "agent-browser")))
    args = parser.parse_args()
    if not args.binary.exists() and args.binary.name == "agent-browser":
        # Let PATH resolution produce the normal executable-not-found error.
        binary = args.binary
    else:
        binary = args.binary.resolve()
    suffix = uuid.uuid4().hex[:8]
    namespace = f"macbot-test-{suffix}"
    first = f"macbot-smoke-a-{suffix}"
    second = f"macbot-smoke-b-{suffix}"
    url_a = 'data:text/html,<button id="a">A</button>'
    url_b = 'data:text/html,<button id="b">B</button>'
    config_dir = tempfile.TemporaryDirectory(prefix="macbot-browser-config-")
    config = Path(config_dir.name) / "config.json"
    config.write_text("{}", encoding="utf-8")
    try:
        a = run(binary, first, namespace, config, "tab", "new", url_a)
        b = run(binary, second, namespace, config, "tab", "new", url_b)
        tabs_a = run(binary, first, namespace, config, "tab", "list")["tabs"]
        tabs_b = run(binary, second, namespace, config, "tab", "list")["tabs"]
        assert a["tabId"] == "t2" and b["tabId"] == "t2"
        assert {tab["targetId"] for tab in tabs_a}.isdisjoint({tab["targetId"] for tab in tabs_b})
        with tempfile.TemporaryDirectory(prefix="macbot-browser-smoke-") as directory:
            screenshot = Path(directory) / "screen.jpg"
            run(binary, first, namespace, config, "screenshot", str(screenshot), "--screenshot-format", "jpeg")
            assert screenshot.read_bytes()[:2] == b"\xff\xd8"
        clicked = run(binary, first, namespace, config, "click", "#a")
        assert clicked["clicked"] == "#a"
        status = run(binary, first, namespace, config, "stream", "status")
        assert status["enabled"] is True
        run(binary, first, namespace, config, "stream", "disable")
        enabled = run(binary, first, namespace, config, "stream", "enable", "--port", "0")
        assert enabled["enabled"] is True and enabled["port"] > 0
        print(json.dumps({"ok": True, "namespace": namespace, "sessions": [first, second], "tab_id": a["tabId"]}))
    finally:
        for session in (first, second):
            subprocess.run(
                [
                    str(binary),
                    "--namespace",
                    namespace,
                    "--config",
                    str(config),
                    "--headed",
                    "false",
                    "--session",
                    session,
                    "--json",
                    "close",
                ],
                check=False,
                capture_output=True,
            )
        config_dir.cleanup()


if __name__ == "__main__":
    main()
