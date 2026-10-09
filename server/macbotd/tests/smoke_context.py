#!/usr/bin/env python3
"""Production context/permission acceptance on an isolated fake provider."""

from __future__ import annotations

import argparse
import json
import threading
import time
import uuid
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import Any

from smoke_collaboration import (
    Daemon,
    FakeProviderHandler,
    TOKEN,
    rpc,
    start_provider,
    wait_until,
)


class ContextProviderHandler(FakeProviderHandler):
    bodies: list[dict[str, Any]] = []
    body_lock = threading.Lock()
    context_marker = ""
    project_marker = ""

    def _scripted_tool(self, messages: list[dict[str, Any]]) -> tuple[str, dict[str, Any]] | None:
        prompt = json.dumps(messages, ensure_ascii=False)
        called = self._called_tools(messages)
        if self.context_marker in prompt and self.project_marker not in prompt and "skill" not in called:
            return None
        if self.project_marker in prompt:
            if "project_find" not in called:
                return "project_find", {"query": "context-project"}
            if "memory" not in called:
                return "memory", {
                    "scope": "project",
                    "project_id": self.scenario["project_id"],
                    "action": "add",
                    "kind": "project_fact",
                    "content": "ordinary bot project permission sentinel",
                }
            if "memory_search" not in called:
                return "memory_search", {"query": "ordinary bot project permission sentinel"}
        return None

    def do_POST(self):  # noqa: N802
        if self.path.rstrip("/") != "/v1/chat/completions" or not self._authorized():
            self._json(401, {"error": {"message": "fake token required"}})
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = json.loads(self.rfile.read(length))
        with self.body_lock:
            type(self).bodies.append(body)
            type(self).calls += 1
        if body.get("model") != "collaboration-fake":
            self._json(400, {"error": {"message": "unexpected model"}})
            return
        scripted = self._scripted_tool(body.get("messages", []))
        if scripted is not None:
            self._stream_tool(*scripted)
        else:
            self._stream("production context fake complete")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7795")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--daemon-command", required=True)
    parser.add_argument("--browser-bin", default=None)
    args = parser.parse_args()
    base = args.url.rstrip("/")
    suffix = uuid.uuid4().hex[:8]
    context_marker = f"/context-skill-{suffix}"
    project_marker = f"CONTEXT_PROJECT_CHECK_{suffix}"
    ContextProviderHandler.context_marker = context_marker
    ContextProviderHandler.project_marker = project_marker
    ContextProviderHandler.scenario = {}
    provider_server, provider_url = start_provider()
    # The inherited handler's server is not used; bind the context subclass.
    provider_server.shutdown()
    provider_server.server_close()
    provider_server = ThreadingHTTPServer(("127.0.0.1", 0), ContextProviderHandler)
    threading.Thread(target=provider_server.serve_forever, daemon=True).start()
    provider_url = f"http://127.0.0.1:{provider_server.server_port}/v1"
    daemon = Daemon(args)
    try:
        daemon.start()
        provider = rpc(
            base,
            args.password,
            "provider.create",
            {
                "name": f"context-fake-{suffix}",
                "api_kind": "openai-completions",
                "base_url": provider_url,
                "api_key": TOKEN,
                "client_request_id": f"context-provider-{suffix}",
            },
        )["provider"]
        rpc(base, args.password, "model.refresh", {"provider_id": provider["id"]})
        model = rpc(
            base,
            args.password,
            "model.upsert",
            {
                "provider_id": provider["id"],
                "model_id": "collaboration-fake",
                "display_name": "Context fake",
                "caps": {"vision": False, "tools": True, "reasoning": False},
                "client_request_id": f"context-model-{suffix}",
            },
        )["model"]["ref"]
        skill_name = f"context-skill-{suffix}"
        rpc(
            base,
            args.password,
            "skill.create",
            {
                "name": skill_name,
                "content": f"---\nname: {skill_name}\ndescription: context skill\n---\nCONTEXT_SKILL_BODY_{suffix}",
                "client_request_id": f"context-skill-create-{suffix}",
            },
        )
        worker = rpc(
            base,
            args.password,
            "bot.create",
            {
                "name": f"context-worker-{suffix}",
                "model": model,
                "tools": {"files": False, "bash": False, "browser": False, "subagent": False, "web": False, "mcp": False},
            },
        )["bot"]
        project = rpc(
            base,
            args.password,
            "project.create",
            {
                "name": f"context-project-{suffix}",
                "goal": "context permission acceptance",
                "member_bot_ids": [worker["id"]],
                "client_request_id": f"context-project-{suffix}",
            },
        )
        ContextProviderHandler.scenario = {"project_id": project["project"]["id"]}
        chat_id = project["chat"]["id"]

        def send(text: str, key: str) -> str:
            result = rpc(
                base,
                args.password,
                "chat.send",
                {
                    "chat_id": chat_id,
                    "text": text,
                    "mentions": [{"kind": "bot", "bot_id": worker["id"], "instruction": text}],
                    "client_request_id": key,
                },
            )
            return result["message"]["id"]

        send(f"{context_marker} use the skill", f"context-chat-{suffix}")
        assignment = {}
        wait_until(
            lambda: bool(
                [
                    assignment.update(item)
                    for item in rpc(base, args.password, "assignment.list", {})["items"]
                    if item.get("origin_chat_id") == chat_id and item.get("bot_id") == worker["id"]
                ]
            ),
            "context assignment admission",
        )
        wait_until(
            lambda: any(
                item.get("type") == "run.end"
                and item.get("data", {}).get("status") == "done"
                for item in rpc(base, args.password, "trace.history", {"assignment_id": assignment["id"], "limit": 500})["items"]
            ),
            "explicit skill run",
        )
        with ContextProviderHandler.body_lock:
            bodies = list(ContextProviderHandler.bodies)
        assert any(f"CONTEXT_SKILL_BODY_{suffix}" in json.dumps(body, ensure_ascii=False) for body in bodies), bodies

        send(project_marker, f"context-project-check-{suffix}")

        def approve_and_find_memory() -> bool:
            for approval in rpc(base, args.password, "approval.list", {}).get("approvals", []):
                if approval.get("state") == "pending":
                    rpc(
                        base,
                        args.password,
                        "approval.decide",
                        {"approval_id": approval["id"], "decision": "allow_once"},
                    )
            with ContextProviderHandler.body_lock:
                current = list(ContextProviderHandler.bodies)
            return any(
                "ordinary bot project permission sentinel" in json.dumps(body, ensure_ascii=False)
                for body in current
            )

        wait_until(
            approve_and_find_memory,
            "ordinary Bot project memory commit",
        )
        assert any(
            f"context-project-{suffix}" in json.dumps(body, ensure_ascii=False)
            for body in ContextProviderHandler.bodies
        ), "private project_find did not return a visible project"
        print(json.dumps({"ok": True, "port": 7795, "home": str(args.home), "provider_calls": len(ContextProviderHandler.bodies)}))
    finally:
        daemon.close()
        provider_server.shutdown()
        provider_server.server_close()


if __name__ == "__main__":
    main()
