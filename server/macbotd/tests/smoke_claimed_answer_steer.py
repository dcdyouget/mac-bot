#!/usr/bin/env python3
"""Exercise question.answer arriving after a message continuation is claimed."""
from __future__ import annotations

import argparse
import json
import shlex
import socket
import sys
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from smoke_collaboration import Daemon, FakeProviderHandler, TOKEN, rpc, wait_until


class Provider(FakeProviderHandler):
    calls = 0
    initial_wait = threading.Event()
    release_initial = threading.Event()
    resume_started = threading.Event()
    release_resume = threading.Event()
    third_prompt = threading.Event()
    resume_has_answer = False
    next_prompt_answer_count = 0
    next_prompt_answer_sources = []
    expected_answer = "canonical answer"
    multi_select = False
    next_prompt_messages = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        messages = body.get("messages", [])
        type(self).calls += 1
        if type(self).calls == 1:
            type(self).initial_wait.set()
            assert type(self).release_initial.wait(60), "initial provider gate timed out"
            decision = {
                "intent": "decision", "text": "Need the answer",
                "options": ["Continue", "Pause", "Review"], "mentions": ["user"],
            }
            if type(self).multi_select:
                decision["multi"] = True
            self._stream_tool("send_msg", decision)
            return
        if type(self).calls == 2:
            type(self).resume_has_answer = "canonical answer" in json.dumps(messages)
            type(self).resume_started.set()
            assert type(self).release_resume.wait(60), "resume provider gate timed out"
            self._stream_tool("send_msg", {"intent": "done", "text": "stale done", "mentions": []})
            return
        type(self).next_prompt_messages = messages
        type(self).next_prompt_answer_count = sum(
            1
            for message in messages
            if message.get("role") == "user"
            and message.get("content") == type(self).expected_answer
        )
        type(self).next_prompt_answer_sources = [
            (message.get("role"), message.get("content"))
            for message in messages if type(self).expected_answer in str(message)
        ]
        type(self).third_prompt.set()
        self._stream_tool("send_msg", {"intent": "done", "text": "finished", "mentions": []})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    multi_answer = parser.add_mutually_exclusive_group()
    multi_answer.add_argument(
        "--multi-select", action="store_true",
        help="answer with two selected options and no free text",
    )
    multi_answer.add_argument(
        "--multi-select-with-text", action="store_true",
        help="answer with two selected options plus free text",
    )
    args = parser.parse_args()
    if args.multi_select:
        Provider.expected_answer = "Continue、Review"
        Provider.multi_select = True
    elif args.multi_select_with_text:
        Provider.expected_answer = "Continue、Review（补充：custom detail）"
        Provider.multi_select = True
    args.home = args.home.resolve()
    args.home.mkdir(parents=True, exist_ok=False)
    config = args.home / "config.toml"
    config.write_text('host_name = "Claimed answer race"\n')
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    args.url = f"http://127.0.0.1:{port}"
    args.password = "isolated-claimed-answer"
    args.browser_bin = None
    args.daemon_command = shlex.join([
        str(args.binary.resolve()), "--config", str(config), "--home", str(args.home),
        "--port", str(port), "--password", args.password,
    ])
    daemon = Daemon(args)

    def call(method, params=None):
        return rpc(args.url, args.password, method, params or {})

    try:
        daemon.start()
        provider_id = call("provider.create", {
            "name": "local gated", "api_kind": "openai-completions",
            "base_url": f"http://127.0.0.1:{provider.server_port}/v1", "api_key": TOKEN,
        })["provider"]["id"]
        model = call("model.upsert", {
            "provider_id": provider_id, "model_id": "claimed-race",
            "display_name": "claimed-race", "caps": {"tools": True, "vision": False, "reasoning": False},
        })["model"]["ref"]
        bot = call("bot.create", {"name": "claimed-race-worker", "model": model})["bot"]
        created_project = call("project.create", {
            "name": "claimed-race-project", "goal": "exercise continuation race",
            "member_bot_ids": [bot["id"]],
        })
        project = created_project["project"]
        chat_id = created_project["chat"]["id"]
        assignment = call("assignment.create", {
            "project_id": project["id"], "origin_chat_id": chat_id,
            "bot_id": bot["id"], "title": "decision wait", "instruction": "ask then resume",
            "from": "user", "trigger_message_id": None, "parent_assignment_id": None,
            "priority": 1, "root_message_id": None, "loop_hops": 0,
        })
        assert Provider.initial_wait.wait(20), "initial provider call did not arrive"
        queued = call("chat.send", {
            "chat_id": chat_id, "text": "steer while the question is pending",
            "mentions": [], "client_request_id": "claimed-answer-steer-message",
        })["message"]
        assert any(d.get("assignment_id") == assignment["id"] and d.get("state") == "queued" for d in queued.get("delivery", [])), queued
        Provider.release_initial.set()
        wait_until(
            lambda: bool(call("question.list", {"assignment_id": assignment["id"], "state": ["pending"]})["questions"]),
            "pending question", 20,
        )
        question = call("question.list", {"assignment_id": assignment["id"], "state": ["pending"]})["questions"][0]
        call("message.steer", {
            "message_id": queued["id"], "bot_id": bot["id"], "assignment_id": assignment["id"],
            "client_request_id": "claimed-answer-steer-steer",
        })
        assert Provider.resume_started.wait(20), "message continuation did not reach the gated provider call"
        answer_params = {
            "question_id": question["id"],
            "client_request_id": "claimed-answer-steer-answer",
        }
        if args.multi_select or args.multi_select_with_text:
            answer_params["option_indexes"] = [0, 2]
        if args.multi_select_with_text:
            answer_params["text"] = "custom detail"
        else:
            if not args.multi_select:
                answer_params["text"] = "canonical answer"
        answer_result = call("question.answer", answer_params)
        assert answer_result["question"]["state"] == "answered", answer_result
        if args.multi_select or args.multi_select_with_text:
            assert answer_result["question"]["answer"]["option_indexes"] == [0, 2], answer_result
            if args.multi_select_with_text:
                assert answer_result["question"]["answer"]["text"] == "custom detail", answer_result
            else:
                assert answer_result["question"]["answer"]["text"] is None, answer_result
        Provider.release_resume.set()
        assert Provider.third_prompt.wait(20), "continuation did not request a follow-up prompt"
        assert Provider.next_prompt_answer_count == 1, (
            "the accepted canonical answer must appear exactly once in the next prompt; "
            f"count={Provider.next_prompt_answer_count}, sources={Provider.next_prompt_answer_sources}, "
            f"messages={json.dumps(Provider.next_prompt_messages, ensure_ascii=False)}"
        )
        answer_user_messages = [
            message for message in Provider.next_prompt_messages
            if message.get("role") == "user"
            and message.get("content") == Provider.expected_answer
        ]
        assert len(answer_user_messages) == 1, (
            "the accepted answer must remain a distinct runtime user input even when "
            "the system context repeats the same text; "
            f"sources={Provider.next_prompt_answer_sources}, "
            f"messages={json.dumps(Provider.next_prompt_messages, ensure_ascii=False)}"
        )
        wait_until(
            lambda: call("assignment.get", {"assignment_id": assignment["id"]})["assignment"]["status"] == "done",
            "assignment leaves continuation", 30,
        )
        snapshot = call("assignment.get", {"assignment_id": assignment["id"]})["assignment"]
        questions = call("question.list", {"assignment_id": assignment["id"], "state": ["answered"]})["questions"]
        records = [json.loads(p.read_text()) for p in (args.home / "data/continuations").glob("*.json")]
        print(json.dumps({
            "assignment_status": snapshot["status"], "provider_calls": Provider.calls,
            "resume_saw_answer": Provider.resume_has_answer,
            "next_prompt_answer_count": Provider.next_prompt_answer_count,
            "next_prompt_answer_sources": Provider.next_prompt_answer_sources,
            "question_state": questions[0]["state"] if questions else None,
            "continuations": [{"state": r.get("state"), "action": r.get("action")} for r in records],
            "home": str(args.home),
        }, ensure_ascii=False))
    finally:
        Provider.release_initial.set()
        Provider.release_resume.set()
        daemon.close()
        provider.shutdown()


if __name__ == "__main__":
    main()
