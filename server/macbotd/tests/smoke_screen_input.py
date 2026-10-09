#!/usr/bin/env python3
"""Isolated real /ws/screen input probe.

This deliberately lives beside ``smoke_screen.py`` and does not modify that
acceptance test.  It starts its own daemon, fake provider, headless
agent-browser session, and fixture page, then records the selected-quality frame,
the browser viewport, the generated page events, and the before/after JPEGs.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
import urllib.parse
import uuid

import smoke_screen as smoke


DEFAULT_PORT = 7863
DEFAULT_HOME = Path("/tmp/macbot-screen-input-20261010a")


class InputPageHandler(BaseHTTPRequestHandler):
    marker: str
    report_state: dict[str, Any]

    def log_message(self, _format: str, *_args: Any) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802
        request = urllib.parse.urlsplit(self.path)
        if request.path == "/report":
            payload = urllib.parse.parse_qs(request.query).get("payload", ["{}"]) [0]
            try:
                value = json.loads(payload)
                if isinstance(value, dict):
                    if isinstance(value.get("phase"), str) and isinstance(value.get("events"), list):
                        self.report_state.setdefault("reports", {})[value["phase"]] = value["events"]
                    self.report_state.update(value)
            except json.JSONDecodeError:
                pass
            self.send_response(204)
            self.end_headers()
            return
        body = f"""<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{self.marker}</title>
<style>
html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:#18324a;color:#fff;font:28px sans-serif}}
form{{position:fixed;left:50%;top:50%;transform:translate(-50%,-50%)}}
button{{width:180px;height:72px;font:inherit}}
#result{{position:fixed;left:0;right:0;top:20%;text-align:center}}
</style>
<body><div id="result">{self.marker}</div>
<form id="form"><button id="submit" type="submit">Submit</button></form>
<script>
const marker = {json.dumps(self.marker)};
const phase = new URLSearchParams(location.search).get("phase") || "touch";
const events = [];
const encode = value => btoa(unescape(encodeURIComponent(JSON.stringify(value))));
function record(type, event) {{
  const item = {{type, time: Date.now()}};
  if (event) {{
    for (const key of ["clientX","clientY","screenX","screenY","button","buttons","detail"]) {{
      if (typeof event[key] === "number") item[key] = event[key];
    }}
    if (event.pointerType) item.pointerType = event.pointerType;
    if (event.touches) item.touches = event.touches.length;
    if (event.changedTouches) item.changedTouches = event.changedTouches.length;
    if (event.target) {{ item.targetId = event.target.id || ""; item.targetTag = event.target.tagName || ""; }}
  }}
  events.push(item);
  if (type !== "viewport") document.getElementById("result").textContent = type;
  fetch("/report?payload=" + encodeURIComponent(JSON.stringify({{phase, events}}))).catch(() => {{}});
  location.hash = "events=" + encode(events);
}}
for (const type of ["pointerdown","pointerup","mousedown","mouseup","click","touchstart","touchend"]) {{
  document.addEventListener(type, event => record(type, event), true);
}}
document.getElementById("form").addEventListener("submit", event => {{
  event.preventDefault();
  document.getElementById("result").textContent = "SUBMITTED";
  record("submit", event);
  if (phase === "touch") setTimeout(() => {{
    location.href = location.pathname + "?marker=" + encodeURIComponent(marker) + "&phase=mouse";
  }}, 1500);
}});
const rect = document.getElementById("submit").getBoundingClientRect();
events.push({{type:"viewport", phase, time:Date.now(), innerWidth, innerHeight, devicePixelRatio,
  button:{{left:rect.left, top:rect.top, width:rect.width, height:rect.height}}}});
fetch("/report?payload=" + encodeURIComponent(JSON.stringify({{phase, events}}))).catch(() => {{}});
location.hash = "probe-ready";
setTimeout(() => {{ location.hash = "events=" + encode(events); }}, 700);
</script></body>""".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def start_input_page(marker: str) -> tuple[ThreadingHTTPServer, str, dict[str, Any]]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), InputPageHandler)
    InputPageHandler.marker = marker
    InputPageHandler.report_state = {}
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/?marker={urllib.parse.quote(marker)}&phase=touch", InputPageHandler.report_state


def build_browser_wrapper(home: Path, browser_bin: str) -> str:
    browser_root = home / "browser-test"
    browser_root.mkdir(parents=True, exist_ok=True)
    config = browser_root / "config.json"
    config.write_text("{}", encoding="utf-8")
    wrapper = browser_root / "agent-browser-wrapper"
    namespace = f"macbot-screen-input-{uuid.uuid4().hex[:10]}"
    socket_dir = Path("/tmp/m")
    socket_dir.mkdir(parents=True, exist_ok=True)
    log = browser_root / "agent-browser.log"
    wrapper.write_text(
        "#!/bin/sh\n"
        f"export AGENT_BROWSER_SOCKET_DIR={shlex.quote(str(socket_dir))}\n"
        f"log={shlex.quote(str(log))}\n"
        "printf '%s\\n' \"argv:$*\" >>\"$log\"\n"
        f"output=$( {shlex.quote(str(Path(browser_bin).resolve()))} "
        f"--namespace {shlex.quote(namespace)} "
        f"--config {shlex.quote(str(config))} --headed false \"$@\" 2>>\"$log\")\n"
        "rc=$?\nprintf '%s rc=%s\\n' \"$output\" \"$rc\" >>\"$log\"\n"
        "printf '%s' \"$output\"\nexit $rc\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)
    return str(wrapper)


class IsolatedDaemon:
    def __init__(self, command: str, home: Path, browser_bin: str, evidence: Path) -> None:
        self.command = command
        self.home = home
        self.browser_bin = browser_bin
        self.evidence = evidence
        self.process: subprocess.Popen[bytes] | None = None

    def start(self, url: str) -> None:
        env = os.environ.copy()
        env["MACBOT_HOME"] = str(self.home)
        env["MACBOT_SECRET_BACKEND"] = "file"
        env["MACBOT_SECRET_DIR"] = str(self.home / "secrets")
        env["MACBOT_BROWSER_BIN"] = self.browser_bin
        log = (self.evidence / "daemon.log").open("wb")
        self.process = subprocess.Popen(
            shlex.split(self.command),
            cwd=smoke.REPO,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        smoke.wait_until(
            lambda: smoke.http_json(f"{url.rstrip('/')}/api/v1/health").get("ok") is True,
            45,
            "isolated macbotd startup",
        )

    def close(self) -> None:
        if self.process is None or self.process.poll() is not None:
            return
        try:
            os.killpg(self.process.pid, signal.SIGTERM)
            self.process.wait(timeout=10)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            self.process.kill()


class StrictProbeFailure(AssertionError):
    def __init__(self, message: str, result: dict[str, Any]) -> None:
        super().__init__(message)
        self.result = result


def browser_json(binary: str, bot_id: str, *args: str) -> dict[str, Any]:
    result = subprocess.run(
        [binary, "--session", f"macbot-{bot_id}", "--json", *args],
        check=True,
        capture_output=True,
        text=True,
        timeout=20,
    )
    return json.loads(result.stdout)


def decode_events(url: str) -> list[dict[str, Any]]:
    fragment = urllib.parse.urlsplit(url).fragment
    if not fragment.startswith("events="):
        return []
    encoded = fragment.removeprefix("events=")
    try:
        return json.loads(base64.b64decode(encoded).decode())
    except (ValueError, UnicodeDecodeError, json.JSONDecodeError):
        return []


def decode_viewport(value: dict[str, Any]) -> dict[str, Any]:
    result = value.get("data", {}).get("result")
    if isinstance(result, str):
        try:
            parsed = json.loads(result)
            if isinstance(parsed, dict):
                return parsed
        except json.JSONDecodeError:
            pass
    return {}


async def read_sidecar_first_frame(
    port: int, evidence: Path, timeout: float = 15
) -> dict[str, Any]:
    """Read one raw stream frame without using the gateway connection."""
    import websockets

    url = f"ws://127.0.0.1:{port}/?pacing=ack&maxFps=8"
    async with websockets.connect(url, proxy=None, max_size=32 * 1024 * 1024) as ws:
        deadline = time.monotonic() + timeout
        current_url = ""
        while time.monotonic() < deadline:
            raw = await asyncio.wait_for(ws.recv(), max(0.1, deadline - time.monotonic()))
            if isinstance(raw, bytes):
                continue
            value = json.loads(raw)
            if value.get("type") == "url":
                current_url = value.get("url", current_url)
                continue
            if value.get("type") != "frame":
                continue
            jpeg = base64.b64decode(value.get("data", ""))
            metadata = value.get("metadata", {})
            record = {
                "seq": value.get("seq"),
                "metadata": metadata,
                "url": current_url,
                "jpeg_dimensions": smoke.jpeg_dimensions(jpeg),
                "payload_bytes": len(jpeg),
            }
            (evidence / "sidecar-first-frame.json").write_text(
                json.dumps(record, indent=2, ensure_ascii=False), encoding="utf-8"
            )
            (evidence / "sidecar-first-frame.jpg").write_bytes(jpeg)
            await ws.send(json.dumps({"type": "ack", "seq": value.get("seq")}))
            return record
    raise AssertionError(f"timed out reading sidecar frame on port {port}")


async def wait_for_page_phase(ws: Any, phase: str, timeout: float = 15) -> tuple[str, list[dict[str, Any]]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            kind, value = await smoke.recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        except asyncio.TimeoutError:
            break
        if kind == "frame":
            await ws.send(json.dumps({"type": "ack", "seq": value[0]["seq"]}))
            continue
        if value.get("type") == "error":
            raise AssertionError(f"screen error: {value}")
        if value.get("type") != "state":
            continue
        tabs = value.get("state", {}).get("tabs", [])
        if not tabs:
            continue
        url = tabs[0].get("url", "")
        events = decode_events(url)
        if any(item.get("type") == "viewport" and item.get("phase") == phase for item in events):
            return url, events
    raise AssertionError(f"timed out waiting for page phase {phase!r}")


async def wait_for_report_phase(report: dict[str, Any], phase: str, timeout: float = 15) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if report.get("phase") == phase:
            return dict(report)
        await asyncio.sleep(0.05)
    raise AssertionError(f"timed out waiting for DOM report phase {phase!r}: {report}")


def report_events(report: dict[str, Any], phase: str) -> list[dict[str, Any]]:
    reports = report.get("reports", {})
    if isinstance(reports, dict) and isinstance(reports.get(phase), list):
        return list(reports[phase])
    if report.get("phase") == phase and isinstance(report.get("events"), list):
        return list(report["events"])
    return []


async def collect_report_events(
    ws: Any,
    report: dict[str, Any],
    evidence: Path,
    name: str,
    phase: str,
    required_types: set[str],
    timeout: float = 10,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout
    last_frame: bytes | None = None
    satisfied_at: float | None = None
    while time.monotonic() < deadline:
        events = report_events(report, phase)
        if required_types.issubset({item.get("type") for item in events}):
            if satisfied_at is None:
                satisfied_at = time.monotonic()
            elif time.monotonic() - satisfied_at >= 0.8:
                if last_frame is not None:
                    (evidence / f"{name}.jpg").write_bytes(last_frame)
                return list(events)
        try:
            kind, value = await smoke.recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        except asyncio.TimeoutError:
            break
        if kind == "frame":
            _header, last_frame = value
            await ws.send(json.dumps({"type": "ack", "seq": _header["seq"]}))
        elif value.get("type") == "error":
            raise AssertionError(f"screen error: {value}")
    events = report_events(report, phase)
    if last_frame is not None:
        (evidence / f"{name}.jpg").write_bytes(last_frame)
    return list(events)


async def collect_events(
    ws: Any,
    evidence: Path,
    name: str,
    required_types: set[str],
    timeout: float = 8,
) -> tuple[str, list[dict[str, Any]]]:
    deadline = time.monotonic() + timeout
    last_frame: bytes | None = None
    last_url = ""
    last_events: list[dict[str, Any]] = []
    satisfied_at: float | None = None
    while time.monotonic() < deadline:
        try:
            kind, value = await smoke.recv_screen_message(ws, max(0.1, deadline - time.monotonic()))
        except asyncio.TimeoutError:
            break
        if kind == "frame":
            header, jpeg = value
            last_frame = jpeg
            last_url = header.get("url", last_url)
            await ws.send(json.dumps({"type": "ack", "seq": header["seq"]}))
            continue
        if value.get("type") == "error":
            raise AssertionError(f"screen error: {value}")
        if value.get("type") == "state":
            tabs = value.get("state", {}).get("tabs", [])
            if tabs:
                last_url = tabs[0].get("url", last_url)
                events = decode_events(last_url)
                if events:
                    last_events = events
                    if required_types.issubset({item.get("type") for item in events}):
                        if satisfied_at is None:
                            satisfied_at = time.monotonic()
                        elif time.monotonic() - satisfied_at >= 0.8:
                            if last_frame is not None:
                                (evidence / f"{name}.jpg").write_bytes(last_frame)
                            return last_url, events
    if last_frame is not None:
        (evidence / f"{name}.jpg").write_bytes(last_frame)
    return last_url, last_events


async def acceptance(
    args: argparse.Namespace,
    fake_url: str,
    fake: smoke.FakeProviderState,
    daemon: IsolatedDaemon,
    evidence: Path,
    page_report: dict[str, Any],
) -> dict[str, Any]:
    import websockets

    base = args.url.rstrip("/")
    password = args.password
    smoke.rpc(base, password, "bootstrap")
    provider_id = smoke.rpc(base, password, "provider.create", {
        "name": "screen-input-fake-provider",
        "api_kind": "openai-completions",
        "base_url": fake_url,
        "api_key": smoke.DUMMY_TOKEN,
        "client_request_id": "screen-input-provider-create",
    })["provider"]["id"]
    smoke.rpc(base, password, "model.refresh", {"provider_id": provider_id})
    model = smoke.rpc(base, password, "model.upsert", {
        "provider_id": provider_id,
        "model_id": "fake-screen",
        "display_name": "Screen input fake",
        "caps": {"vision": False, "tools": True, "reasoning": False},
        "price": {"input_per_mtok": 1.0, "output_per_mtok": 1.0, "cache_read_per_mtok": 0.1, "cache_write_per_mtok": 0.1},
        "client_request_id": "screen-input-model-upsert",
    })["model"]["ref"]
    worker = smoke.rpc(base, password, "bot.create", {
        "name": "screen-input-worker",
        "model": model,
        "tools": {"files": False, "bash": False, "browser": True, "subagent": False, "web": False, "mcp": False},
        "client_request_id": "screen-input-worker-create",
    })["bot"]

    project = smoke.rpc(base, password, "project.create", {
        "name": "screen-input-project",
        "goal": "Open a real input probe page",
        "member_bot_ids": [worker["id"]],
        "client_request_id": "screen-input-project-create",
    })
    chat_id = project["chat"]["id"]
    marker = urllib.parse.parse_qs(urllib.parse.urlsplit(fake.local_page_url).query)["marker"][0]
    smoke.rpc(base, password, "chat.send", {
        "chat_id": chat_id,
        "text": f"SCREEN_OPEN_{marker}",
        "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": "open a browser tab"}],
        "client_request_id": "screen-input-chat-send",
    })

    def settle() -> bool:
        pending = smoke.rpc(base, password, "bootstrap").get("pending", {})
        for approval in pending.get("approvals", []):
            if approval.get("state") == "pending":
                smoke.rpc(base, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once"})
        return False

    smoke.wait_until(
        lambda: any(item.get("origin_chat_id") == chat_id for item in smoke.rpc(base, password, "assignment.list")["items"]),
        20,
        "input probe assignment",
    )
    assignment = smoke.assignment_for(base, password, chat_id)
    smoke.wait_until(
        lambda: settle() or smoke.trace_has_browser_open(base, password, assignment["id"]),
        45,
        "input probe browser_open",
    )
    smoke.wait_until(
        lambda: (settle() or True) and next(item for item in smoke.rpc(base, password, "assignment.list")["items"] if item["id"] == assignment["id"]).get("status") == "done",
        45,
        "input probe assignment completion",
    )

    ws_url = base.replace("http://", "ws://").replace("https://", "wss://")
    screen_url = f"{ws_url}/ws/screen?bot_id={worker['id']}&quality={args.quality}"
    headers = {"Authorization": f"Bearer {password}"}
    browser_status = browser_json(args.browser_bin, worker["id"], "stream", "status")
    browser_tabs = browser_json(args.browser_bin, worker["id"], "tab", "list")
    tab_items = browser_tabs.get("data", [])
    if isinstance(tab_items, dict):
        tab_items = tab_items.get("tabs", [])
    page_tab = next((item for item in tab_items if marker in str(item.get("url", ""))), None)
    if page_tab and page_tab.get("id"):
        browser_json(args.browser_bin, worker["id"], "tab", str(page_tab["id"]))
    viewport_raw = browser_json(
        args.browser_bin,
        worker["id"],
        "eval",
        "JSON.stringify({url:location.href,innerWidth,innerHeight,devicePixelRatio,button:document.querySelector('#submit')?.getBoundingClientRect().toJSON()})",
    )
    viewport = decode_viewport(viewport_raw)
    (evidence / "browser-status.json").write_text(json.dumps(browser_status, indent=2), encoding="utf-8")
    (evidence / "browser-tabs.json").write_text(json.dumps(browser_tabs, indent=2), encoding="utf-8")
    (evidence / "browser-viewport.json").write_text(json.dumps(viewport, indent=2), encoding="utf-8")

    result: dict[str, Any] = {"bot_id": worker["id"], "assignment_id": assignment["id"], "marker": marker}
    async with websockets.connect(screen_url, additional_headers=headers, proxy=None) as ws:
        initial = await smoke.recv_state(ws)
        tab = initial["tabs"][0]
        result["tab_id"] = tab["tab_id"]
        header, jpeg = await smoke.recv_frame(ws)
        (evidence / "before-input.jpg").write_bytes(jpeg)
        await ws.send(json.dumps({"type": "ack", "seq": header["seq"]}))
        smoke.rpc(base, password, "takeover.start", {"bot_id": worker["id"]})
        await smoke.recv_state_with_ack(ws, "user")

        status = browser_json(args.browser_bin, worker["id"], "stream", "status")
        port = status.get("data", {}).get("port")
        if not isinstance(port, int):
            raise AssertionError(f"stream status has no sidecar port: {status}")
        sidecar = await read_sidecar_first_frame(port, evidence)
        result["sidecar"] = sidecar
        metadata = sidecar.get("metadata", {})
        viewport_width = metadata.get("deviceWidth")
        viewport_height = metadata.get("deviceHeight")
        if not isinstance(viewport_width, int) or not isinstance(viewport_height, int) or viewport_width <= 0 or viewport_height <= 0:
            raise AssertionError(f"sidecar metadata lacks device dimensions: {metadata}")

        touch_report = await wait_for_report_phase(page_report, "touch")
        page_viewport = next(
            (item for item in touch_report.get("events", []) if item.get("type") == "viewport"),
            None,
        )
        if not page_viewport:
            raise AssertionError(f"touch DOM report has no viewport event: {touch_report}")
        button = page_viewport.get("button", {})
        button_center = {
            "x": float(button.get("left", 0)) + float(button.get("width", 0)) / 2,
            "y": float(button.get("top", 0)) + float(button.get("height", 0)) / 2,
        }
        dom_width = float(page_viewport.get("innerWidth", 0))
        dom_height = float(page_viewport.get("innerHeight", 0))
        if dom_width <= 0 or dom_height <= 0:
            raise AssertionError(f"DOM viewport is invalid: {page_viewport}")
        button_frame = {
            "x": button_center["x"] * header["w"] / dom_width,
            "y": button_center["y"] * header["h"] / dom_height,
        }
        result["metadata_viewport"] = {
            "deviceWidth": viewport_width,
            "deviceHeight": viewport_height,
            "offsetTop": metadata.get("offsetTop"),
            "pageScaleFactor": metadata.get("pageScaleFactor"),
        }
        result["dom_input_viewport"] = {"width": dom_width, "height": dom_height}
        result["input_coordinate_basis"] = "DOM button rect / DOM viewport scaled to gateway JPEG"
        result["page_viewport"] = page_viewport
        result["button_center_viewport"] = button_center
        result["button_frame_point"] = button_frame
        result["frame"] = {key: header.get(key) for key in ("seq", "w", "h", "url", "ts")}
        result["jpeg_dimensions"] = smoke.jpeg_dimensions(jpeg)

        await ws.send(json.dumps({"type": "input", "event": {"type": "touch", "action": "start", "points": [button_frame]}}))
        await ws.send(json.dumps({"type": "input", "event": {"type": "touch", "action": "end", "points": []}}))
        touch_events = await collect_report_events(
            ws, page_report, evidence, "after-touch-empty", "touch", {"pointerdown", "touchstart", "touchend", "submit"}
        )
        result["touch_url"] = sidecar.get("url", "")
        result["touch_events"] = touch_events
        if not any(item.get("type") == "submit" for item in touch_events):
            result.setdefault("failures", []).append("touch did not submit")
            if not args.diagnose:
                raise StrictProbeFailure("touchStart/touchEnd([]) did not submit", result)

        mouse_report = await wait_for_report_phase(page_report, "mouse")
        result["page_viewport_mouse"] = next(
            (item for item in mouse_report.get("events", []) if item.get("type") == "viewport"),
            None,
        )
        await asyncio.sleep(0.8)
        await ws.send(json.dumps({"type": "input", "event": {"type": "mouse", "action": "click", **button_frame, "button": "left", "click_count": 1}}))
        mouse_events = await collect_report_events(
            ws, page_report, evidence, "after-mouse", "mouse", {"mousedown", "mouseup", "click", "submit"}
        )
        result["mouse_url"] = sidecar.get("url", "")
        result["mouse_events"] = mouse_events
        if not any(item.get("type") == "submit" for item in mouse_events):
            result.setdefault("failures", []).append("mouse did not submit")
            if not args.diagnose:
                raise StrictProbeFailure("mouse click did not submit", result)

        await ws.send(json.dumps({"type": "input", "event": {"type": "key", "action": "press", "key": "k", "code": "KeyK", "text": "k", "modifiers": []}}))
        smoke.wait_until(lambda: True, 0.2, "key dispatch settle")

        smoke.rpc(base, password, "takeover.release", {"bot_id": worker["id"], "note": "input probe released"})
        await smoke.recv_state_with_ack(ws, "bot")
    result["checks"] = ["sidecar_metadata_recorded", f"{args.quality}_frame_recorded", "touch_start_end_empty_submit", "mouse_submit"]
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default=f"http://127.0.0.1:{DEFAULT_PORT}")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, default=Path("/tmp/macbot-screen-input-20261010b"))
    parser.add_argument("--browser-bin", default="/tmp/macbot-agent-browser-0.38.2")
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--quality", choices=("low", "high"), default="low")
    parser.add_argument("--diagnose", action="store_true", help="record failed submit checks and exit successfully")
    args = parser.parse_args()
    args.home.mkdir(parents=True, exist_ok=True)
    evidence = args.home / "evidence"
    evidence.mkdir(parents=True, exist_ok=True)
    wrapped_browser = build_browser_wrapper(args.home, args.browser_bin)
    marker = f"SCREEN_INPUT_{uuid.uuid4().hex[:10]}"
    page_server, page_url, page_report = start_input_page(marker)
    fake_server, fake_state, fake_url = smoke.start_fake_provider(page_url)
    daemon = IsolatedDaemon(args.daemon_command, args.home, wrapped_browser, evidence)
    result: dict[str, Any] | None = None
    try:
        daemon.start(args.url)
        result = asyncio.run(acceptance(args, fake_url, fake_state, daemon, evidence, page_report))
        result["evidence_dir"] = str(evidence)
        (evidence / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False))
    except StrictProbeFailure as error:
        result = error.result
        result["ok"] = False
        result["evidence_dir"] = str(evidence)
        (evidence / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
        raise
    finally:
        daemon.close()
        try:
            if result and result.get("bot_id"):
                subprocess.run([wrapped_browser, "--session", "macbot-" + result["bot_id"], "--json", "close"], check=False, timeout=20, capture_output=True)
        except subprocess.SubprocessError:
            pass
        fake_server.shutdown()
        page_server.shutdown()


if __name__ == "__main__":
    main()
