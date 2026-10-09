#!/usr/bin/env python3
"""Observe Android system notification receipt without changing notifications."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import time


PACKAGE = "bot.mac.mobile"


def timestamp() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def adb_command(adb: str, serial: str, remote: str) -> tuple[int, str]:
    # adb forwards a command to a remote shell; local argv quoting alone does
    # not protect notification keys containing pipe characters.
    try:
        result = subprocess.run(
            [adb, "-s", serial, "shell", remote],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, timeout=15, check=False,
        )
    except subprocess.TimeoutExpired:
        return 124, ""
    return result.returncode, result.stdout


def notification_snapshot(adb: str, serial: str, marker: str) -> dict:
    code, raw = adb_command(adb, serial, "cmd notification list")
    if code != 0:
        raise RuntimeError("Android notification list failed; absence cannot be inferred")
    keys = [line.strip() for line in raw.splitlines() if f"|{PACKAGE}|" in line]
    observations = []
    failures = []
    for key in keys:
        code, detail = adb_command(adb, serial, "cmd notification get " + shlex.quote(key))
        if code != 0 or not detail.strip():
            failures.append({"key": key, "exit_code": code})
            continue
        # Inspect the output internally; persist no notification text, action
        # payloads, host passwords, or other applications' notifications.
        if marker in detail:
            observations.append({"key": key, "marker_present": True})
    return {
        "at": timestamp(), "app_active_count": len(keys),
        "keys": keys, "matching_notifications": observations,
        "query_failures": failures,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default="emulator-5554")
    parser.add_argument("--marker", required=True, help="Unique marker expected in a new notification")
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--interval", type=float, default=5)
    parser.add_argument("--output", type=Path, required=True, help="New evidence path; never overwrite")
    args = parser.parse_args()
    if not re.fullmatch(r"macbot-e2e-[A-Za-z0-9._-]{3,110}", args.marker):
        parser.error("marker must be a unique macbot-e2e-* identifier")
    if args.timeout < 0 or args.interval <= 0:
        parser.error("timeout must be nonnegative and interval positive")
    if args.output.exists():
        parser.error("output exists; choose a new evidence path")
    sdk = Path(os.environ.get("ANDROID_HOME", str(Path.home() / "Library/Android/sdk")))
    adb = shutil.which("adb") or str(sdk / "platform-tools/adb")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    evidence = {
        "scenario": "Android notification receipt", "serial": args.serial,
        "package": PACKAGE, "marker": args.marker, "started_at": timestamp(),
        "read_only": True, "full_s4_pass": False, "snapshots": [],
        "status": "OBSERVING",
        "note": "System receipt only; no notification cancellation, app restart, permission change, or ledger inference.",
    }
    # Exclusive creation prevents accidental overwrite of an earlier result.
    with args.output.open("x", encoding="utf-8") as handle:
        json.dump(evidence, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    deadline = time.monotonic() + args.timeout
    exit_code = 1
    try:
        while True:
            snapshot = notification_snapshot(adb, args.serial, args.marker)
            evidence["snapshots"].append(snapshot)
            if snapshot["matching_notifications"]:
                evidence["status"] = "OBSERVED"
                exit_code = 0
                break
            if time.monotonic() >= deadline:
                evidence["status"] = "INCOMPLETE" if any(
                    item["query_failures"] for item in evidence["snapshots"]
                ) else "NOT_OBSERVED"
                break
            time.sleep(min(args.interval, max(0, deadline - time.monotonic())))
    except (RuntimeError, OSError) as exc:
        evidence["status"] = "ERROR"
        # Exception text is controlled locally; raw adb output is never used.
        evidence["error"] = type(exc).__name__
    finally:
        evidence["finished_at"] = timestamp()
        temporary = args.output.with_name(args.output.name + f".{os.getpid()}.tmp")
        temporary.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, args.output)
    print(json.dumps({"status": evidence["status"], "output": str(args.output), "full_s4_pass": False}))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
