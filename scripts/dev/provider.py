#!/usr/bin/env python3
"""Configure MiniMax CN through local RPC without persisting credentials in files."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "e2e"))
from rpc import RpcClient

KEYCHAIN_SERVICE = "bot.mac.integrator.minimax-cn"
KEYCHAIN_ACCOUNT = "macbot-integrator"
BASE_URL = "https://api.minimax.cn/anthropic"


def credential() -> str:
    value = os.environ.get("MINIMAX_API_KEY", "").strip()
    if value:
        return value
    result = subprocess.run(
        ["/usr/bin/security", "find-generic-password", "-a", KEYCHAIN_ACCOUNT,
         "-s", KEYCHAIN_SERVICE, "-w"], capture_output=True, text=True,
    )
    if result.returncode or not result.stdout.strip():
        raise RuntimeError("MiniMax credential missing from environment or login Keychain")
    return result.stdout.strip()


def upstream(key: str, model: str) -> dict:
    request = urllib.request.Request(
        "https://api.minimax.cn/anthropic/v1/messages",
        data=json.dumps({"model": model, "max_tokens": 512,
                         "messages": [{"role": "user", "content": "只回复 OK"}]}).encode(),
        headers={"Authorization": "Bearer " + key,
                 "anthropic-version": "2023-06-01", "Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as exc:
        # Never print provider bodies: they can echo credentials or request text.
        raise RuntimeError(f"MiniMax returned HTTP {exc.code}") from None
    texts = [item.get("text", "") for item in payload.get("content", [])
             if item.get("type") == "text"]
    if payload.get("type") != "message" or not any(texts):
        raise RuntimeError("MiniMax returned no final text; model access remains unverified")
    return {"model": payload.get("model"), "stop_reason": payload.get("stop_reason"),
            "usage": payload.get("usage"), "text_present": True}


def configure(args: argparse.Namespace, key: str) -> dict:
    if not args.model.startswith("MiniMax-M2"):
        raise RuntimeError("Specify model capabilities before registering a non-M2 model")
    client = RpcClient(args.url, Path(args.password_file).expanduser().read_text().strip(), timeout=60)
    parsed = urllib.parse.urlsplit(client.base_url)
    if parsed.hostname not in ("127.0.0.1", "localhost", "::1") or parsed.port == 7789:
        raise RuntimeError("Configure the local production Host on port 7788, not mock")
    health = client.health()
    if health.get("ok") is not True or health.get("setup_required") is True or health.get("mock") is True:
        raise RuntimeError("Production Host is not ready")
    # Refuse mutation if bootstrap/credentials are not valid on the target Host.
    client.call("bootstrap")
    providers = client.call("provider.list").get("providers", [])
    existing = next((p for p in providers if p.get("name") == "MiniMax CN"
                     and p.get("base_url", "").rstrip("/") == BASE_URL), None)
    if existing and existing.get("api_kind") != "anthropic-messages":
        raise RuntimeError("Existing MiniMax CN provider has a different API kind")
    def write(method: str, params: dict) -> dict:
        return client.call(method, dict(params, client_request_id=str(uuid.uuid4())))
    if existing:
        provider = write("provider.update", {"provider_id": existing["id"], "patch": {"api_key": key}})["provider"]
    else:
        provider = write("provider.create", {"name": "MiniMax CN", "api_kind": "anthropic-messages",
                                            "base_url": BASE_URL, "api_key": key})["provider"]
    if provider.get("has_key") is not True:
        raise RuntimeError("Host did not confirm credential storage")
    tested = client.call("provider.test", {"provider_id": provider["id"]})
    if tested.get("ok") is not True:
        raise RuntimeError("Host provider.test failed; defaults were not changed")
    # M2.x models accept tools and reasoning, but do not accept image input.
    model = write("model.upsert", {"provider_id": provider["id"], "model_id": args.model,
                                   "display_name": args.model, "context_window": 204800,
                                   "max_output": 8192, "caps": {"vision": False, "tools": True, "reasoning": True},
                                   "enabled": True})["model"]
    if args.set_defaults:
        write("settings.update", {"patch": {"models": {"bot_default": model["ref"],
              "main": model["ref"], "subagent": "inherit", "maintenance": model["ref"]}}})
    return {"provider_id": provider["id"], "base_url": BASE_URL, "model_ref": model["ref"],
            "provider_test": True, "defaults_updated": args.set_defaults}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-upstream", action="store_true", help="Only send a minimal real model request")
    parser.add_argument("--url", default="http://127.0.0.1:7788")
    parser.add_argument("--password-file", default="~/.macbot-dev-password")
    parser.add_argument("--model", default="MiniMax-M2.5")
    parser.add_argument("--set-defaults", action="store_true")
    args = parser.parse_args()
    key = ""
    try:
        key = credential()
        result = upstream(key, args.model)
        if not args.check_upstream:
            result["host"] = configure(args, key)
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except Exception as exc:
        message = str(exc).replace(key, "[REDACTED]") if key else str(exc)
        print(f"MiniMax configuration failed: {message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
