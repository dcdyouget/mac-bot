#!/usr/bin/env python3
"""Small standard-library HTTP RPC client used by the integration scenarios.

The wire format is the HTTP form from PROTOCOL.md section 9:
``POST /api/v1/rpc`` with ``{"method": ..., "params": ...}``, returning
``{"ok": true, "result": ...}`` or ``{"ok": false, "error": ...}``.
"""

from __future__ import annotations

import json
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from dataclasses import dataclass
from typing import Any, Callable, Mapping


class RpcError(RuntimeError):
    """An RPC response with ``ok: false`` or an invalid response body."""

    def __init__(self, message: str, *, error: Mapping[str, Any] | None = None) -> None:
        super().__init__(message)
        self.error = dict(error or {})

    @property
    def code(self) -> str | None:
        value = self.error.get("code")
        return value if isinstance(value, str) else None


class RpcTransportError(RuntimeError):
    """The endpoint could not be reached or returned a non-RPC HTTP error."""

    def __init__(self, message: str, *, status: int | None = None) -> None:
        super().__init__(message)
        self.status = status


class PollTimeout(RuntimeError):
    """A polling operation did not produce a matching result before its deadline."""


def http_base_url(value: str) -> str:
    """Normalize a host:port, HTTP(S), or WS(S) address to an HTTP base URL."""

    value = value.strip()
    if not value:
        raise ValueError("RPC URL is empty")
    if "://" not in value:
        value = "http://" + value
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme in ("ws", "wss"):
        scheme = "https" if parsed.scheme == "wss" else "http"
        parsed = parsed._replace(scheme=scheme)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError("unsupported RPC URL (expected http(s)://host:port)")
    # Credentials and query strings are not part of the protocol and are easy
    # to leak through diagnostics. Tokens must be supplied via Authorization.
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("RPC URL must not contain userinfo")
    if parsed.query or parsed.fragment:
        raise ValueError("RPC URL must not contain a query or fragment")
    # A copied WebSocket endpoint is a useful input for a script, but the HTTP
    # RPC endpoint lives at the host root rather than under /ws.
    path = parsed.path.rstrip("/")
    if path == "/ws":
        path = ""
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, path, "", ""))


@dataclass
class RpcClient:
    """Synchronous RPC client; each call is independent and easy to poll."""

    base_url: str
    password: str | None = None
    timeout: float = 10.0

    def __post_init__(self) -> None:
        self.base_url = http_base_url(self.base_url)

    @property
    def rpc_url(self) -> str:
        return self.base_url + "/api/v1/rpc"

    @property
    def health_url(self) -> str:
        return self.base_url + "/api/v1/health"

    def _request(self, request: urllib.request.Request) -> tuple[int, bytes]:
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as exc:
            body = exc.read()
            # A server-side RPC error is still useful to the scenario, so parse
            # it and preserve the status for diagnostics instead of losing body.
            try:
                payload = json.loads(body.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError):
                payload = None
            if isinstance(payload, dict) and ("ok" in payload or "error" in payload):
                if payload.get("ok") is False:
                    error = payload.get("error")
                    if isinstance(error, dict):
                        raise RpcError(
                            f"RPC HTTP {exc.code}: {error.get('message', 'request failed')}",
                            error=error,
                        ) from exc
            raise RpcTransportError(f"HTTP {exc.code} from {request.full_url}", status=exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise RpcTransportError(f"cannot reach {request.full_url}: {exc}") from exc

    def health(self) -> dict[str, Any]:
        request = urllib.request.Request(self.health_url, method="GET")
        _status, body = self._request(request)
        try:
            payload = json.loads(body.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise RpcError("health response is not JSON") from exc
        if not isinstance(payload, dict):
            raise RpcError("health response is not an object")
        return payload

    def poll_health(
        self,
        predicate: Callable[[dict[str, Any]], bool],
        *,
        timeout: float = 30.0,
        interval: float = 0.5,
        description: str = "health readiness",
    ) -> dict[str, Any]:
        """Poll the unauthenticated health endpoint while a service starts."""

        if timeout <= 0 or interval < 0:
            raise ValueError("timeout must be positive and interval must be non-negative")
        deadline = time.monotonic() + timeout
        last_transport_error: RpcTransportError | None = None
        while True:
            try:
                result = self.health()
                if predicate(result):
                    return result
            except RpcTransportError as exc:
                last_transport_error = exc
            if time.monotonic() >= deadline:
                if last_transport_error:
                    raise PollTimeout(f"timed out waiting for {description}: {last_transport_error}") from last_transport_error
                raise PollTimeout(f"timed out waiting for {description}")
            time.sleep(min(interval, max(0.0, deadline - time.monotonic())))

    def call(self, method: str, params: Mapping[str, Any] | None = None) -> Any:
        if not method:
            raise ValueError("RPC method is empty")
        call_params = dict(params or {})
        if method in {"chat.send", "project.create", "skill.create", "skill.update",
                      "skill.delete", "skill.set_enabled"}:
            call_params.setdefault("client_request_id", str(uuid.uuid4()))
        body = json.dumps(
            {"method": method, "params": call_params},
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        headers = {"Content-Type": "application/json", "Accept": "application/json"}
        if self.password:
            headers["Authorization"] = "Bearer " + self.password
        request = urllib.request.Request(self.rpc_url, data=body, headers=headers, method="POST")
        _status, raw = self._request(request)
        try:
            payload = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise RpcError(f"RPC {method} response is not JSON") from exc
        if not isinstance(payload, dict) or not isinstance(payload.get("ok"), bool):
            raise RpcError(f"RPC {method} response does not match the protocol envelope")
        if payload["ok"] is False:
            error = payload.get("error")
            if not isinstance(error, dict):
                error = {"message": "RPC failed"}
            raise RpcError(f"RPC {method} failed: {error.get('message', 'request failed')}", error=error)
        if "result" not in payload:
            raise RpcError(f"RPC {method} response has no result")
        return payload["result"]

    def poll(
        self,
        method: str,
        params: Mapping[str, Any] | None,
        predicate: Callable[[Any], bool],
        *,
        timeout: float = 30.0,
        interval: float = 0.5,
        description: str | None = None,
    ) -> Any:
        """Call an RPC until ``predicate`` matches, retrying transport failures.

        Authentication, validation, and other protocol errors fail immediately;
        a service that is still starting may be retried until the deadline.
        """

        if timeout <= 0 or interval < 0:
            raise ValueError("timeout must be positive and interval must be non-negative")
        deadline = time.monotonic() + timeout
        last_transport_error: RpcTransportError | None = None
        while True:
            try:
                result = self.call(method, params)
                if predicate(result):
                    return result
            except RpcTransportError as exc:
                last_transport_error = exc
            if time.monotonic() >= deadline:
                label = description or method
                if last_transport_error:
                    raise PollTimeout(f"timed out waiting for {label}: {last_transport_error}") from last_transport_error
                raise PollTimeout(f"timed out waiting for {label}")
            time.sleep(min(interval, max(0.0, deadline - time.monotonic())))
