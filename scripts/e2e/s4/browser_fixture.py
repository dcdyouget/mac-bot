#!/usr/bin/env python3
"""Serve a local browser fixture for the S4 screen and gesture checks.

This page is deliberately local and synthetic.  It proves that the browser
surface can paint an animated page and accept a visible text-input gesture;
it does not emulate an external login, notifications, or any provider.
"""

from __future__ import annotations

import argparse
import html
import json
import signal
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlsplit


DEFAULT_PORT = 52200
DEFAULT_MARKER = "macbot-e2e-s4-browser"
MAX_MARKER_LENGTH = 120


def clean_marker(value: str) -> str:
    """Keep query markers visible without allowing control characters."""

    value = "".join(char for char in value if char >= " " and char != "\x7f")
    return value.strip()[:MAX_MARKER_LENGTH] or DEFAULT_MARKER


def page_html(marker: str) -> bytes:
    escaped_marker = html.escape(clean_marker(marker), quote=True)
    return f"""<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';">
  <title>MacBot S4 验收 · 本地画面</title>
  <style>
    :root {{ color-scheme: dark; font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100vh; background: #101522; color: #edf2ff; display: grid; place-items: center; }}
    main {{ width: min(92vw, 560px); padding: 28px; border: 1px solid #32415f; border-radius: 20px; background: #182238; box-shadow: 0 18px 50px #070b14aa; }}
    h1 {{ margin: 0 0 8px; font-size: clamp(22px, 5vw, 32px); letter-spacing: .01em; }}
    .marker {{ margin: 0 0 22px; color: #a9bce8; overflow-wrap: anywhere; }}
    .stage {{ position: relative; height: 96px; margin: 0 0 22px; overflow: hidden; border-radius: 14px; background: #0e1525; }}
    .orb {{ position: absolute; top: 50%; left: 12%; width: 32px; height: 32px; margin-top: -16px; border-radius: 50%; background: #72d6ff; box-shadow: 0 0 26px #72d6ff; animation: drift 3.2s ease-in-out infinite alternate; }}
    .stage::after {{ content: "动画帧 · screen transport"; position: absolute; inset: 0; display: grid; place-items: center; color: #91a6d2; font-size: 14px; pointer-events: none; }}
    @keyframes drift {{ from {{ transform: translateX(0) scale(.8); opacity: .7; }} to {{ transform: translateX(min(70vw, 360px)) scale(1.35); opacity: 1; }} }}
    label {{ display: block; margin-bottom: 8px; color: #c2d0ed; font-size: 14px; }}
    .row {{ display: flex; gap: 10px; align-items: stretch; }}
    input {{ min-width: 0; flex: 1; border: 1px solid #4a5d83; border-radius: 10px; padding: 12px 13px; background: #0f1728; color: #fff; font: inherit; font-size: 16px; }}
    button {{ border: 0; border-radius: 10px; padding: 0 18px; background: #5b8cff; color: white; font: inherit; font-weight: 700; cursor: pointer; }}
    button:focus-visible, input:focus-visible {{ outline: 3px solid #9bb8ff; outline-offset: 2px; }}
    #result {{ min-height: 24px; margin: 16px 0 0; color: #9ff0c7; overflow-wrap: anywhere; }}
    .note {{ margin: 20px 0 0; color: #8f9fbe; font-size: 12px; line-height: 1.5; }}
  </style>
</head>
<body>
  <main>
    <h1>MacBot S4 浏览器验收</h1>
    <p class="marker">本地 fixture · {escaped_marker}</p>
    <div class="stage" aria-label="动画画面"><span class="orb"></span></div>
    <form id="gesture-form">
      <label for="fixture-input">输入测试文本</label>
      <div class="row">
        <input id="fixture-input" name="text" type="text" autocomplete="off" placeholder="输入后点击提交">
        <button type="submit">提交</button>
      </div>
    </form>
    <p id="result" aria-live="polite">等待输入</p>
    <p class="note">仅用于本机画面、键盘和点击验收；不连接外部登录或通知服务。</p>
  </main>
  <script>
    (() => {{
      const form = document.getElementById('gesture-form');
      const input = document.getElementById('fixture-input');
      const result = document.getElementById('result');
      form.addEventListener('submit', (event) => {{
        event.preventDefault();
        result.textContent = '收到：' + input.value;
      }});
    }})();
  </script>
</body>
</html>
""".encode("utf-8")


class FixtureServer(ThreadingHTTPServer):
    """HTTP server with only in-memory state and a deliberately quiet logger."""

    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address: tuple[str, int], marker: str) -> None:
        super().__init__(address, FixtureHandler)
        self.default_marker = clean_marker(marker)
        self.access_count = 0
        self._access_lock = threading.Lock()

    def record_access(self) -> int:
        with self._access_lock:
            self.access_count += 1
            return self.access_count


class FixtureHandler(BaseHTTPRequestHandler):
    server: FixtureServer

    def log_message(self, _format: str, *_args: Any) -> None:
        # Do not log URLs, query markers, or submitted form text.
        return

    def _write(self, body: bytes, content_type: str, status: int = 200) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        access_count = self.server.record_access()
        parsed = urlsplit(self.path)
        if parsed.path == "/health":
            body = json.dumps(
                {"status": "ok", "access_count": access_count},
                separators=(",", ":"),
            ).encode("ascii")
            self._write(body, "application/json; charset=utf-8")
            return
        if parsed.path in {"/", "/index.html"}:
            query_marker = parse_qs(parsed.query, keep_blank_values=True).get("marker", [self.server.default_marker])[0]
            self._write(page_html(query_marker), "text/html; charset=utf-8")
            return
        self._write(b"not found\n", "text/plain; charset=utf-8", status=404)


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help=f"local port (default: {DEFAULT_PORT})")
    parser.add_argument("--marker", default=DEFAULT_MARKER, help="marker rendered in the local page")
    return parser


def run(args: argparse.Namespace) -> int:
    if not 1 <= args.port <= 65535:
        raise SystemExit("--port must be between 1 and 65535")
    server = FixtureServer(("127.0.0.1", args.port), args.marker)
    stop_started = threading.Event()

    def stop(_signum: int, _frame: Any) -> None:
        if stop_started.is_set():
            return
        stop_started.set()
        # shutdown() must run outside serve_forever()'s thread.
        threading.Thread(target=server.shutdown, name="browser-fixture-shutdown", daemon=True).start()

    previous: dict[int, Any] = {}
    for signum in (signal.SIGINT, signal.SIGTERM):
        previous[signum] = signal.getsignal(signum)
        signal.signal(signum, stop)
    try:
        print(f"MacBot S4 browser fixture: http://127.0.0.1:{args.port}/", flush=True)
        server.serve_forever(poll_interval=0.2)
    finally:
        server.server_close()
        for signum, handler in previous.items():
            signal.signal(signum, handler)
    return 0


if __name__ == "__main__":
    raise SystemExit(run(args_parser().parse_args()))
