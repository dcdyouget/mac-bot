#!/usr/bin/env python3
"""Exercise the package installer's bounded admin-page readiness probe."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import threading
import time


SERVER = Path(__file__).resolve().parents[2]
WAITER = SERVER / "macbotd/packaging/wait-for-admin.sh"
POSTINSTALL = SERVER / "macbotd/packaging/pkg-scripts/postinstall"
PACKAGER = SERVER / "macbotd/packaging/build-pkg.sh"


class ReadyAfterRetries(BaseHTTPRequestHandler):
    requests = 0

    def do_GET(self):
        type(self).requests += 1
        self.send_response(503 if type(self).requests < 3 else 401)
        self.end_headers()

    def log_message(self, *_):
        pass


class NeverReady(BaseHTTPRequestHandler):
    requests = 0

    def do_GET(self):
        type(self).requests += 1
        self.send_response(404)
        self.end_headers()

    def log_message(self, *_):
        pass


def serve(handler):
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    return httpd


def probe(httpd, timeout):
    url = f"http://127.0.0.1:{httpd.server_port}/admin"
    return subprocess.run(
        ["/bin/sh", str(WAITER), url, str(timeout)],
        capture_output=True,
        text=True,
        timeout=timeout + 5,
    )


def main():
    assert WAITER.is_file()
    postinstall = POSTINSTALL.read_text()
    assert postinstall.index("wait-for-admin.sh") < postinstall.index('launchctl asuser "$CONSOLE_UID" open')
    assert "wait-for-admin.sh" in PACKAGER.read_text()

    delayed = serve(ReadyAfterRetries)
    try:
        result = probe(delayed, 5)
        assert result.returncode == 0, (result.returncode, result.stderr)
        assert ReadyAfterRetries.requests >= 3, ReadyAfterRetries.requests
    finally:
        delayed.shutdown()
        delayed.server_close()

    unavailable = serve(NeverReady)
    try:
        started = time.monotonic()
        result = probe(unavailable, 2)
        elapsed = time.monotonic() - started
        assert result.returncode == 1, (result.returncode, result.stderr)
        # The helper uses the system's whole-second clock, so rounding can
        # trigger the two-second deadline slightly before two wall seconds.
        assert 1 <= elapsed < 5, elapsed
        assert "timed out after 2s" in result.stderr, result.stderr
        assert "last HTTP status: 404" in result.stderr, result.stderr
        assert NeverReady.requests >= 2, NeverReady.requests
    finally:
        unavailable.shutdown()
        unavailable.server_close()

    print("postinstall readiness smoke passed: retries through 503 to 401; bounded timeout on 404")


if __name__ == "__main__":
    main()
