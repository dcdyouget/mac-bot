#!/usr/bin/env python3
"""Inspect a built pkg and exercise installed updates in a temporary app."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile


SERVER = Path(__file__).resolve().parents[2]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inspect_pkg(pkg, destination):
    subprocess.run(["pkgutil", "--expand-full", str(pkg), str(destination)], check=True)
    apps = list(destination.glob("**/Applications/MacBot Server.app"))
    assert len(apps) == 1, apps
    app = apps[0]
    for relative in (
        "Contents/MacOS/macbotd", "Contents/MacOS/agent-browser",
        "Contents/Resources/agent-browser.LICENSE",
        "Contents/Resources/update-installed.sh",
        "Contents/Resources/update-manifest.json",
        "Contents/Resources/com.macbot.server.plist",
    ):
        assert (app / relative).is_file(), relative
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    assert info["LSUIElement"] and info["CFBundleExecutable"] == "macbotd"
    assert list(destination.glob("**/Scripts/postinstall"))
    daemon = app / "Contents/MacOS/macbotd"
    sidecar = app / "Contents/MacOS/agent-browser"
    assert subprocess.check_output([str(daemon), "--version"]).startswith(b"macbotd ")
    assert subprocess.check_output([str(sidecar), "--version"]).strip() == b"agent-browser 0.38.2"
    assert digest(sidecar) in {
        "8168b86ab5d94be8f670992dfe4fe1445016518a864b48bda105e64142e7cbf9",
        "787cb40e086a188d0bb13ff29a99a0b2380aff3aa5e8600b8f8131a0b98ca69c",
    }
    manifest = json.loads((app / "Contents/Resources/update-manifest.json").read_text())
    assert manifest["version"] == info["CFBundleVersion"]
    return daemon


def inspect_updater(daemon, directory):
    app = directory / "MacBot Server.app"
    resources = app / "Contents/Resources"
    bin_dir = app / "Contents/MacOS"
    resources.mkdir(parents=True)
    bin_dir.mkdir()
    script = resources / "update-installed.sh"
    script.write_bytes((SERVER / "macbotd/packaging/update-installed.sh").read_bytes())
    script.chmod(0o755)
    binary = bin_dir / "macbotd"
    sentinel = b"old-binary-sentinel"
    binary.write_bytes(sentinel)
    manifest = resources / "update-manifest.json"
    env = dict(os.environ, MACBOT_LAUNCH_LABEL="com.macbot.server.isolated-update-smoke")

    def attempt(url, checksum, expected):
        manifest.write_text(json.dumps({"version": "test", "url": url, "sha256": checksum}))
        result = subprocess.run([str(script)], env=env, capture_output=True, text=True)
        assert result.returncode == expected, (result.returncode, result.stderr)

    attempt("", "", 2)
    assert binary.read_bytes() == sentinel
    attempt(daemon.as_uri(), "0" * 64, 3)
    assert binary.read_bytes() == sentinel
    invalid = directory / "invalid-artifact"
    invalid.write_bytes(b"#!/bin/sh\necho invalid\n")
    attempt(invalid.as_uri(), digest(invalid), 4)
    assert binary.read_bytes() == sentinel
    attempt(daemon.as_uri(), digest(daemon), 0)
    assert digest(binary) == digest(daemon)
    assert not (bin_dir / "macbotd.new").exists()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pkg", type=Path)
    parser.add_argument("--daemon", type=Path, default=SERVER / "target/debug/macbotd")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="macbot-package-smoke-") as temporary:
        root = Path(temporary)
        daemon = args.daemon.resolve()
        if args.pkg:
            daemon = inspect_pkg(args.pkg.resolve(), root / "expanded")
        inspect_updater(daemon, root / "update")
    print("package smoke passed: bundle contents, sidecar digest and atomic update failure/success")


if __name__ == "__main__":
    main()
