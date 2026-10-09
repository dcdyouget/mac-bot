#!/usr/bin/env python3
"""Repeatable transport/CLI smoke test for a running macbotd.

The daemon must already be running.  This test deliberately uses only Python's
standard library so it can run on a freshly installed Mac Bot host.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from typing import Dict, Optional, Tuple


def fail(message: str) -> None:
    raise AssertionError(message)


def request(base: str, method: str, path: str, password: Optional[str] = None,
            body: Optional[bytes] = None, content_type: Optional[str] = None,
            range_header: Optional[str] = None) -> Tuple[int, Dict[str, str], bytes]:
    headers = {"Accept": "application/json"}
    if password is not None:
        if path.startswith("/admin"):
            token = base64.b64encode(f"admin:{password}".encode()).decode()
            headers["Authorization"] = f"Basic {token}"
        else:
            headers["Authorization"] = f"Bearer {password}"
    if content_type:
        headers["Content-Type"] = content_type
    if range_header:
        headers["Range"] = range_header
    req = urllib.request.Request(base.rstrip("/") + path, data=body,
                                 headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=10) as response:
            return response.status, dict(response.headers), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers), error.read()


def json_request(base: str, method: str, path: str, password: str,
                 value: object) -> tuple[int, dict, dict[str, str]]:
    if value is None:
        status, headers, raw = request(base, method, path, password)
    else:
        payload = json.dumps(value, separators=(",", ":")).encode()
        status, headers, raw = request(base, method, path, password, payload,
                                       "application/json")
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as error:
        fail(f"{method} {path}: expected JSON, got {raw[:200]!r}: {error}")
    if not isinstance(parsed, dict):
        fail(f"{method} {path}: response is not an object")
    return status, parsed, headers


def assert_status(actual: int, expected: int, what: str) -> None:
    if actual != expected:
        fail(f"{what}: expected HTTP {expected}, got {actual}")


def cli(binary: str, home: Path, *args: str) -> str:
    result = subprocess.run([binary, "--home", str(home), *args],
                            text=True, capture_output=True, timeout=15)
    if result.returncode != 0:
        fail(f"CLI {' '.join(args)} failed ({result.returncode}): {result.stderr}")
    return result.stdout


def multipart(file_name: str, payload: bytes) -> tuple[bytes, str]:
    boundary = "----macbot-smoke-" + uuid.uuid4().hex
    body = (
        f"--{boundary}\r\n"
        f"Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n"
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode() + payload + f"\r\n--{boundary}--\r\n".encode()
    return body, f"multipart/form-data; boundary={boundary}"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7789")
    parser.add_argument("--home", type=Path,
                        default=Path(os.environ.get("MACBOT_HOME", "~/MacBot")).expanduser())
    parser.add_argument("--password", default="dev")
    parser.add_argument("--binary", default=os.environ.get("MACBOTD", "macbotd"))
    parser.add_argument("--skip-cli", action="store_true")
    args = parser.parse_args()

    status, headers, body = request(args.url, "GET", "/api/v1/health")
    assert_status(status, 200, "health")
    health = json.loads(body)
    if health.get("ok") is not True:
        fail(f"health response is not healthy: {health}")

    status, headers, _ = request(args.url, "GET", "/admin/status")
    assert_status(status, 401, "admin auth challenge")

    status, admin, _ = json_request(args.url, "GET", "/admin/status", args.password, None)
    assert_status(status, 200, "admin status")
    for key in ("running", "host_name", "port", "seq"):
        if key not in admin:
            fail(f"admin status missing {key}: {admin}")

    # Persist a no-op settings update and verify the response is a real write.
    status, settings, _ = json_request(
        args.url, "POST", "/admin/settings", args.password,
        {"host_name": admin["host_name"], "port": admin["port"]})
    assert_status(status, 200, "admin settings")
    if settings.get("ok") is not True:
        fail(f"admin settings failed: {settings}")

    status, _, _ = request(args.url, "GET", "/admin/logs", args.password)
    assert_status(status, 200, "admin logs")

    payload = b"macbot gateway upload\n"
    multipart_body, multipart_type = multipart("smoke.txt", payload)
    status, _, raw = request(args.url, "POST", "/api/v1/uploads", args.password,
                             multipart_body, multipart_type)
    assert_status(status, 200, "upload")
    upload = json.loads(raw)
    upload_id = upload.get("upload_id")
    if not isinstance(upload_id, str) or not upload_id:
        fail(f"upload did not return upload_id: {upload}")

    query = urllib.parse.urlencode({"root": "upload", "root_id": upload_id, "path": ""})
    status, file_headers, file_bytes = request(
        args.url, "GET", f"/api/v1/files?{query}", args.password, range_header="bytes=0-3")
    assert_status(status, 206, "file range")
    if file_bytes != payload[:4] or {k.lower(): v for k, v in file_headers.items()}.get("content-range") != f"bytes 0-3/{len(payload)}":
        fail(f"file range mismatch: headers={file_headers}, body={file_bytes!r}")

    # Output is intentionally seeded in the selected data directory. This
    # checks the endpoint's path safety and gives installs a deterministic test.
    output_path = args.home / "runs" / "smoke-run"
    output_path.mkdir(parents=True, exist_ok=True)
    (output_path / "call-1.txt").write_text("smoke output\n", encoding="utf-8")
    query = urllib.parse.urlencode({"run_id": "smoke-run", "call_id": "call-1"})
    status, _, output = request(args.url, "GET", f"/api/v1/trace/output?{query}", args.password)
    assert_status(status, 200, "trace output")
    if output != b"smoke output\n":
        fail(f"trace output mismatch: {output!r}")

    status, csv_headers, csv = request(args.url, "GET", "/api/v1/usage/export.csv", args.password)
    assert_status(status, 200, "usage CSV")
    csv_columns = set(csv.splitlines()[0].decode().split(","))
    if not {"input_tokens", "output_tokens", "requests"}.issubset(csv_columns) or "text/csv" not in {k.lower(): v for k, v in csv_headers.items()}.get("content-type", ""):
        fail(f"usage CSV is malformed: headers={csv_headers}, body={csv[:200]!r}")

    if not args.skip_cli:
        if shutil.which(args.binary) is None and not Path(args.binary).exists():
            fail(f"macbotd binary not found: {args.binary!r} (use --skip-cli or --binary)")
        cli_status = json.loads(cli(args.binary, args.home, "status"))
        if not isinstance(cli_status, dict):
            fail(f"CLI status is not JSON object: {cli_status!r}")
        cli(args.binary, args.home, "settings", "--host-name", str(admin["host_name"]))
        cli(args.binary, args.home, "logs")

    print("smoke_gateway: health/admin/settings/logs/upload/range/output/csv/cli passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, urllib.error.URLError) as error:
        print(f"smoke_gateway: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
