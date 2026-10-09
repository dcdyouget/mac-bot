#!/usr/bin/env python3
"""Production loop-hop and canonical send_msg acceptance.

The daemon and its scheduler are real.  Only the OpenAI-compatible provider is
local and scripted.  Two worker Bots hand a task back and forth; a low
``loop_hops`` setting must produce the protocol ``loop_paused`` block.  The
user then resolves the same root message and the deferred handoff must resume.
Every completion is also checked against the canonical persisted Message ID.

Example::

  python server/macbotd/tests/smoke_loop.py \
    --daemon-command 'server/target/release/macbotd --port 7796 --password dev' \
    --home /tmp/macbot-loop-smoke
"""

from __future__ import annotations

import argparse
import json
from http.server import ThreadingHTTPServer
from pathlib import Path
import sys
import threading
import time
import uuid
from typing import Any


TESTS = Path(__file__).resolve().parent
if str(TESTS) not in sys.path:
    sys.path.insert(0, str(TESTS))

from smoke_collaboration import (  # noqa: E402
    Daemon,
    FakeProviderHandler,
    TOKEN,
    rpc,
    wait_until,
)


class LoopProviderHandler(FakeProviderHandler):
    """Make each worker emit exactly one durable handoff."""

    def _scripted_tool(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        scenario = type(self).scenario
        prompt = json.dumps(messages, ensure_ascii=False)
        # The execution context retains the Bot identity in L0, while the
        # assignment instruction may be summarized away.
        if "Loop A " not in prompt and "Loop B " not in prompt:
            return None
        # A second model turn after the tool call must finish as ordinary text;
        # the first turn is the only one allowed to create the handoff.
        if any(message.get("role") == "assistant" and message.get("tool_calls") for message in messages):
            return None
        if "Loop A " in prompt:
            target, instruction = scenario["bot_b"], "loop-hop-B"
        elif "Loop B " in prompt:
            target, instruction = scenario["bot_a"], "loop-hop-A"
        else:
            return None
        return "send_msg", {
            "intent": "done",
            "text": f"loop handoff {instruction}",
            "mentions": [{"kind": "bot", "bot_id": target, "instruction": instruction}],
        }


def start_loop_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), LoopProviderHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7796")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--browser-bin", default=None)
    return parser


def canonical_messages(base: str, password: str, assignments: list[dict[str, Any]], chat_id: str) -> None:
    messages = rpc(base, password, "chat.history", {"chat_id": chat_id, "limit": 200})["messages"]
    by_id = {message["id"]: message for message in messages}
    for assignment in assignments:
        result_id = assignment.get("result_message_id")
        if not result_id:
            continue
        assert result_id in by_id, (assignment, messages)
        matches = [message for message in messages if message["id"] == result_id]
        assert len(matches) == 1, (assignment, matches)
        assert matches[0].get("assignment_id") == assignment["id"], (assignment, matches[0])


def acceptance(args: argparse.Namespace, provider_url: str) -> dict[str, Any]:
    base = args.url.rstrip("/")
    suffix = uuid.uuid4().hex[:8]
    LoopProviderHandler.scenario = {"marker": f"loop-smoke-{suffix}"}
    daemon = Daemon(args)
    daemon.start()
    try:
        boot = rpc(base, args.password, "bootstrap")
        assert any(bot.get("is_main") for bot in boot["bots"]), boot
        provider = rpc(
            base,
            args.password,
            "provider.create",
            {
                "name": f"loop-fake-{suffix}",
                "api_kind": "openai-completions",
                "base_url": provider_url,
                "api_key": TOKEN,
                "client_request_id": f"loop-provider-{suffix}",
            },
        )["provider"]
        refreshed = rpc(base, args.password, "model.refresh", {"provider_id": provider["id"]})
        assert any(model["model_id"] == "collaboration-fake" for model in refreshed["models"]), refreshed
        model_ref = rpc(
            base,
            args.password,
            "model.upsert",
            {
                "provider_id": provider["id"],
                "model_id": "collaboration-fake",
                "display_name": "Loop fake",
                "caps": {"vision": False, "tools": True, "reasoning": False},
                "client_request_id": f"loop-model-{suffix}",
            },
        )["model"]["ref"]
        rpc(
            base,
            args.password,
            "settings.update",
            {"patch": {"concurrency": {"loop_hops": 1}, "trace": {"save_full_requests": True}}, "client_request_id": f"loop-settings-{suffix}"},
        )
        bot_a = rpc(
            base,
            args.password,
            "bot.create",
            {"name": f"Loop A {suffix}", "model": model_ref, "max_parallel": 1},
        )["bot"]
        bot_b = rpc(
            base,
            args.password,
            "bot.create",
            {"name": f"Loop B {suffix}", "model": model_ref, "max_parallel": 1},
        )["bot"]
        LoopProviderHandler.scenario.update(bot_a=bot_a["id"], bot_b=bot_b["id"])
        project = rpc(
            base,
            args.password,
            "project.create",
            {
                "name": f"Loop project {suffix}",
                "goal": "loop-hop acceptance",
                "member_bot_ids": [bot_a["id"], bot_b["id"]],
                "client_request_id": f"loop-project-{suffix}",
            },
        )
        chat_id = project["chat"]["id"]
        started = rpc(
            base,
            args.password,
            "chat.send",
            {
                "chat_id": chat_id,
                "text": LoopProviderHandler.scenario["marker"],
                "mentions": [{"kind": "bot", "bot_id": bot_a["id"], "instruction": "loop-hop-A"}],
                "client_request_id": f"loop-start-{suffix}",
            },
        )
        root_message_id = started["message"]["id"]

        def loop_assignments() -> list[dict[str, Any]]:
            return [
                item
                for item in rpc(base, args.password, "assignment.list", {"limit": 200})["items"]
                if item.get("project_id") == project["project"]["id"]
            ]

        def paused() -> bool:
            history = rpc(base, args.password, "chat.history", {"chat_id": chat_id, "limit": 200})["messages"]
            return any(
                block.get("type") == "loop_paused"
                and block.get("root_message_id") == root_message_id
                and block.get("state") == "paused"
                for message in history
                for block in message.get("blocks", [])
            )

        wait_until(paused, "loop_paused block", 45)
        assignments = loop_assignments()
        assert len(assignments) >= 1, assignments
        assert all(item.get("status") == "done" for item in assignments), assignments
        assert any(item.get("loop_hops") == 1 for item in assignments), assignments
        canonical_messages(base, args.password, assignments, chat_id)

        assert rpc(
            base,
            args.password,
            "loop.resolve",
            {"root_message_id": root_message_id, "action": "continue", "client_request_id": f"loop-resolve-{suffix}"},
        ) == {}

        before_ids = {item["id"] for item in assignments}

        def resumed() -> bool:
            items = loop_assignments()
            return len(items) > len(before_ids) and any(item["id"] not in before_ids for item in items)

        wait_until(resumed, "loop continuation after resolve", 30)
        final_assignments = loop_assignments()
        canonical_messages(base, args.password, final_assignments, chat_id)
        return {
            "ok": True,
            "root_message_id": root_message_id,
            "assignment_ids": [item["id"] for item in final_assignments],
            "provider_calls": LoopProviderHandler.calls,
        }
    finally:
        daemon.close()


def main() -> None:
    args = parser().parse_args()
    provider, provider_url = start_loop_provider()
    try:
        print(json.dumps(acceptance(args, provider_url), ensure_ascii=False))
    finally:
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
