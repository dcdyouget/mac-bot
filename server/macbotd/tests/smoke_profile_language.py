#!/usr/bin/env python3
"""Isolated profile-language regression through the production runtime.

By default a local fake provider emits the original buggy display-name tool
argument. Pass --minimax-key-file only for an explicitly authorized real-model
check. Credentials are read in-process and never printed or passed on argv.
Every invocation creates a new home and stops only its own daemon.
"""

from __future__ import annotations

import argparse
from http.server import ThreadingHTTPServer
import json
from pathlib import Path
import shlex
import socket
import tempfile
import threading

from smoke_collaboration import Daemon, http_json, rpc, wait_until
from smoke_user_steer_memory import FakeProvider, FakeState


class ProfileProvider(FakeProvider):
    state = FakeState()

    def do_POST(self) -> None:  # noqa: N802
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        self.state.record(body)
        if any(message.get("role") == "tool" for message in body.get("messages", [])):
            self._text("已记住你的称呼、语言和身份。")
        else:
            self._tool("profile-display-name", "set_user_profile", {
                "expected_version": 1,
                "name": "回归测试员",
                "language": "简体中文",
                "identity": "软件工程师",
                "complete": True,
            })


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[2] / "target/debug/macbotd")
    parser.add_argument("--minimax-key-file", type=Path)
    parser.add_argument("--minimax-url", default="https://api.minimax.cn/anthropic")
    parser.add_argument("--minimax-model", default="MiniMax-M3.1-Flash-Preview")
    args = parser.parse_args()
    assert args.binary.is_file(), "build the daemon before running this smoke"
    home = Path(tempfile.mkdtemp(prefix="macbot-profile-language-"))
    password = "isolated-profile-smoke"
    base = f"http://127.0.0.1:{free_port()}"
    daemon = Daemon(argparse.Namespace(
        home=home,
        url=base,
        password=password,
        browser_bin=None,
        daemon_command=shlex.join([str(args.binary.resolve()), "--home", str(home), "--config", str(home / "config.toml"), "--port", base.rsplit(":", 1)[1], "--password", password]),
    ))
    provider_server = None
    credential_copy = None
    key = "fake-profile-language-token"
    if args.minimax_key_file:
        key = args.minimax_key_file.read_text().strip()
        assert key, "the credential file is empty"
        provider_url = args.minimax_url
        api_kind = "anthropic-messages"
        model_id = args.minimax_model
    else:
        provider_server = ThreadingHTTPServer(("127.0.0.1", 0), ProfileProvider)
        threading.Thread(target=provider_server.serve_forever, daemon=True).start()
        provider_url = f"http://127.0.0.1:{provider_server.server_port}/v1"
        api_kind = "openai-completions"
        model_id = "profile-language-fake"
    try:
        daemon.start()
        initial = rpc(base, password, "memory.profile.get")["profile"]
        assert initial["language"] is None and initial["version"] == 1
        provider = rpc(base, password, "provider.create", {
            "name": "profile-language-regression",
            "api_kind": api_kind,
            "base_url": provider_url,
            "api_key": key,
        })["provider"]
        credential_copy = home / "secrets" / f"{provider['id'].encode().hex()}.key"
        assert key not in json.dumps(provider)
        model = rpc(base, password, "model.upsert", {
            "provider_id": provider["id"],
            "model_id": model_id,
            "display_name": model_id,
            "caps": {"vision": False, "tools": True, "reasoning": False},
        })["model"]["ref"]
        rpc(base, password, "bot.update", {"bot_id": "main", "patch": {"model": model}})
        chat_id = rpc(base, password, "bot.get", {"bot_id": "main"})["bot"]["dm_chat_id"]
        rpc(base, password, "chat.send", {
            "chat_id": chat_id,
            "text": "请完成首次档案初始化：称呼我为回归测试员，使用简体中文，我的身份是软件工程师。我已确认这些信息，请直接调用 set_user_profile 保存档案，完成后简短确认即可。",
            "mentions": [],
            "client_locale": "zh-CN",
            "client_request_id": "profile-language-first-answer",
        })
        def onboarding_completed() -> bool:
            for approval in rpc(base, password, "approval.list", {"state": ["pending"]})["approvals"]:
                # Approve only the write requested by this regression. Any
                # unrelated model action fails the smoke before execution.
                assert approval["tool"] == "set_user_profile", approval["tool"]
                rpc(base, password, "approval.decide", {"approval_id": approval["id"], "decision": "allow_once"})
            return rpc(base, password, "memory.profile.get")["profile"]["onboarded"]

        wait_until(onboarding_completed, "onboarding tool completion", 150)
        profile = rpc(base, password, "memory.profile.get")["profile"]
        assert profile["language"] == "zh-CN", profile["language"]
        assert profile["updated_by"]["kind"] == "bot"
        assert profile["timezone"] == initial["timezone"]
        invalid = http_json(f"{base}/api/v1/rpc", {
            "method": "memory.profile.update",
            "params": {"name": "must not persist", "language": "zh_CN", "expected_version": profile["version"]},
        }, password)
        assert invalid["ok"] is False and invalid["error"]["code"] == "invalid_params"
        assert "BCP 47" in invalid["error"]["message"]
        assert rpc(base, password, "memory.profile.get")["profile"] == profile
        cleared = rpc(base, password, "memory.profile.update", {
            "language": "", "expected_version": profile["version"],
        })["profile"]
        assert cleared["language"] is None
        daemon.close()
        daemon.start()
        assert rpc(base, password, "memory.profile.get")["profile"] == cleared
        print(json.dumps({
            "ok": True,
            "provider": "minimax" if args.minimax_key_file else "local-fake",
            "language_after_onboarding": profile["language"],
            "invalid_patch_atomic": True,
            "empty_language": None,
            "restart_preserved_profile": True,
            "home": str(home),
        }, ensure_ascii=False))
    except Exception as error:
        # A provider error must never echo the credential, even if an upstream
        # service includes request metadata in its response.
        raise RuntimeError(str(error).replace(key, "[redacted]")) from None
    finally:
        daemon.close()
        if credential_copy:
            credential_copy.unlink(missing_ok=True)
        if provider_server:
            provider_server.shutdown()
            provider_server.server_close()


if __name__ == "__main__":
    main()
