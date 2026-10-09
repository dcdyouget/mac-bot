#!/usr/bin/env python3
"""Deploy each new local main commit; retain logs without touching other worktrees."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
STATE = Path.home() / "Library/Caches/MacBot/integrator/watch"
LABEL = "bot.mac.integrator.watch"


def run_once() -> int:
    STATE.mkdir(parents=True, exist_ok=True)
    lock = STATE / "lock"
    # flock is unavailable on stock macOS; fcntl locks also recover after crashes.
    import fcntl
    with lock.open("a") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return 0
        sha = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "main"], text=True).strip()
        previous_file = STATE / "latest.json"
        previous = json.loads(previous_file.read_text()) if previous_file.exists() else {}
        if previous.get("sha") == sha and previous.get("integration_exit") == 0:
            return 0
        # Retry a failed revision at most every five minutes.
        now = datetime.now(timezone.utc)
        if previous.get("sha") == sha and previous.get("at"):
            if (now - datetime.fromisoformat(previous["at"])).total_seconds() < 300:
                return 0
        log_file = STATE / (now.strftime("%Y%m%d-%H%M%S") + "-" + sha[:10] + ".log")
        result = {"sha": sha, "at": now.isoformat(), "log": str(log_file)}
        deploy_env = dict(os.environ, MACBOT_DEPLOY_SHA=sha)
        with log_file.open("w") as log:
            code = subprocess.call([str(ROOT / "scripts/dev/deploy.sh")], stdout=log, stderr=log, env=deploy_env)
            result["deploy_exit"] = code
            # A mock RPC check is preliminary evidence, not a two-client sign-off.
            mock = subprocess.call([str(ROOT / "scripts/dev/mock.sh")], stdout=log, stderr=log, env=deploy_env)
            result["mock_exit"] = mock
            result["s0_api_status"] = "blocked: no mock source" if mock == 2 else "not run"
            if mock == 0:
                result["s0_api_exit"] = subprocess.call(
                    [sys.executable, str(ROOT / "scripts/e2e/s0/bootstrap.py"), "--timeout", "5", "--json"],
                    stdout=log, stderr=log,
                )
                result["s0_api_status"] = "passed" if result["s0_api_exit"] == 0 else "failed"
            result["integration_exit"] = int(code != 0 or mock not in (0, 2)
                                               or result.get("s0_api_exit", 0) != 0)
            provider_marker = STATE / "provider-configured.json"
            if code == 0 and not provider_marker.exists():
                # Configure once when a real deployed service becomes available.
                from urllib.request import urlopen
                try:
                    with urlopen("http://127.0.0.1:7788/api/v1/health", timeout=2) as health_response:
                        health = json.load(health_response)
                    production_ready = health.get("ok") is True and health.get("setup_required") is False
                except Exception:
                    production_ready = False
                if production_ready:
                    provider = subprocess.run(
                        [sys.executable, str(ROOT / "scripts/dev/provider.py"), "--set-defaults"],
                        stdout=subprocess.PIPE, stderr=log, text=True,
                    )
                    result["provider_exit"] = provider.returncode
                    if provider.returncode == 0:
                        provider_marker.write_text(provider.stdout)
                        log.write("MiniMax provider configured from local Keychain.\n")
                    else:
                        result["integration_exit"] = 1
        temporary = previous_file.with_suffix(".tmp")
        temporary.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
        temporary.replace(previous_file)
        print(json.dumps(result, ensure_ascii=False))
        return result["integration_exit"]


def install() -> None:
    STATE.mkdir(parents=True, exist_ok=True)
    plist_path = Path.home() / "Library/LaunchAgents" / (LABEL + ".plist")
    plist_path.parent.mkdir(parents=True, exist_ok=True)
    uid = str(os.getuid())
    path = ":".join([str(Path.home() / ".cargo/bin"), "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"])
    payload = {"Label": LABEL, "ProgramArguments": [sys.executable, str(Path(__file__).resolve()), "--once"],
               "RunAtLoad": True, "StartInterval": 60, "EnvironmentVariables": {"PATH": path},
               "StandardOutPath": str(STATE / "watch.out.log"), "StandardErrorPath": str(STATE / "watch.err.log")}
    plist_path.write_bytes(plistlib.dumps(payload))
    plist_path.chmod(0o600)
    subprocess.run(["launchctl", "bootout", f"gui/{uid}/{LABEL}"], capture_output=True)
    subprocess.run(["launchctl", "bootstrap", f"gui/{uid}", str(plist_path)], check=True)
    print(f"Watching local main every 60 seconds; evidence: {STATE}")


def main() -> int:
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--once", action="store_true")
    mode.add_argument("--install", action="store_true")
    mode.add_argument("--uninstall", action="store_true")
    args = parser.parse_args()
    if args.install:
        install()
        return 0
    if args.uninstall:
        subprocess.run(["launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}"], capture_output=True)
        (Path.home() / "Library/LaunchAgents" / (LABEL + ".plist")).unlink(missing_ok=True)
        return 0
    return run_once()


if __name__ == "__main__":
    raise SystemExit(main())
