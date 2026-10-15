#!/usr/bin/env python3
"""Verify an accepted question answer recovers after its inbox append fails.

The daemon, WAL, question flow, restart, and provider calls use production code.
Only the OpenAI-compatible provider is local and scripted.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import threading
from http.server import ThreadingHTTPServer

from smoke_collaboration import Daemon, FakeProviderHandler, TOKEN, http_json, rpc, wait_until


ANSWER = "S12 answer survives WAL/inbox append failure"


class Provider(FakeProviderHandler):
    calls = 0
    initial_wait = threading.Event()
    release_initial = threading.Event()
    resume_started = threading.Event()
    release_resume = threading.Event()
    third_prompt = threading.Event()
    third_messages: list[dict] = []

    def do_POST(self) -> None:
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        messages = body.get("messages", [])
        cls = type(self)
        cls.calls += 1
        if cls.calls == 1:
            cls.initial_wait.set()
            assert cls.release_initial.wait(60), "initial provider gate timed out"
            self._stream_tool(
                "send_msg",
                {
                    "intent": "decision",
                    "text": "Please answer",
                    "options": ["Continue"],
                    "mentions": ["user"],
                },
            )
            return
        if cls.calls == 2:
            cls.resume_started.set()
            assert cls.release_resume.wait(60), "resume provider gate timed out"
            try:
                self._stream_tool(
                    "send_msg", {"intent": "done", "text": "Late done", "mentions": []}
                )
            except (BrokenPipeError, ConnectionResetError):
                # Expected after the harness SIGKILLs the daemon with this
                # provider request still in flight.
                pass
            return
        cls.third_messages = messages
        cls.third_prompt.set()
        self._stream_tool(
            "send_msg",
            {"intent": "done", "text": "Recovered and finished", "mentions": []},
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--home", type=Path, required=True)
    args = parser.parse_args()
    args.home = args.home.resolve()
    args.home.mkdir(parents=True, exist_ok=False)
    config = args.home / "config.toml"
    config.write_text('host_name = "Answer inbox recovery"\n', encoding="utf-8")
    Provider.calls = 0
    Provider.initial_wait = threading.Event()
    Provider.release_initial = threading.Event()
    Provider.resume_started = threading.Event()
    Provider.release_resume = threading.Event()
    Provider.third_prompt = threading.Event()
    Provider.third_messages = []
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    args.url = f"http://127.0.0.1:{port}"
    args.password = "isolated-answer-inbox-recovery"
    args.browser_bin = None
    args.daemon_command = shlex.join(
        [
            str(args.binary.resolve()),
            "--config",
            str(config),
            "--home",
            str(args.home),
            "--port",
            str(port),
            "--password",
            args.password,
        ]
    )
    daemon = Daemon(args)

    def call(method: str, params: dict | None = None) -> dict:
        return rpc(args.url, args.password, method, params or {})

    def raw_call(method: str, params: dict | None = None) -> dict:
        return http_json(
            f"{args.url}/api/v1/rpc",
            {"method": method, "params": params or {}},
            args.password,
        )

    try:
        daemon.start()
        provider_id = call(
            "provider.create",
            {
                "name": "local gated",
                "api_kind": "openai-completions",
                "base_url": f"http://127.0.0.1:{provider.server_port}/v1",
                "api_key": TOKEN,
            },
        )["provider"]["id"]
        model = call(
            "model.upsert",
            {
                "provider_id": provider_id,
                "model_id": "answer-inbox-recovery",
                "display_name": "answer-inbox-recovery",
                "caps": {"tools": True, "vision": False, "reasoning": False},
            },
        )["model"]["ref"]
        bot = call("bot.create", {"name": "answer-inbox-recovery", "model": model})["bot"]
        created = call(
            "project.create",
            {
                "name": "answer-inbox-recovery",
                "goal": "recover accepted question answers",
                "member_bot_ids": [bot["id"]],
            },
        )
        project = created["project"]
        chat_id = created["chat"]["id"]
        assignment = call(
            "assignment.create",
            {
                "project_id": project["id"],
                "origin_chat_id": chat_id,
                "bot_id": bot["id"],
                "title": "Recover answer",
                "instruction": "ask then finish",
                "from": "user",
                "trigger_message_id": None,
                "parent_assignment_id": None,
                "priority": 1,
                "root_message_id": None,
                "loop_hops": 0,
            },
        )
        assert Provider.initial_wait.wait(20), "initial provider call did not arrive"
        queued = call(
            "chat.send",
            {
                "chat_id": chat_id,
                "text": "ordinary message queued during work",
                "mentions": [],
                "client_request_id": "answer-inbox-ordinary",
            },
        )["message"]
        assert any(
            delivery.get("assignment_id") == assignment["id"]
            and delivery.get("state") == "queued"
            for delivery in queued.get("delivery", [])
        ), queued
        Provider.release_initial.set()
        wait_until(
            lambda: bool(
                call("question.list", {"assignment_id": assignment["id"], "state": ["pending"]})[
                    "questions"
                ]
            ),
            "pending question",
            20,
        )
        question = call(
            "question.list", {"assignment_id": assignment["id"], "state": ["pending"]}
        )["questions"][0]
        call(
            "message.steer",
            {
                "message_id": queued["id"],
                "bot_id": bot["id"],
                "assignment_id": assignment["id"],
                "client_request_id": "answer-inbox-steer",
            },
        )
        assert Provider.resume_started.wait(20), "steer continuation did not reach provider"

        inbox_wal = args.home / "data" / "inbox.jsonl"
        assert inbox_wal.is_file(), "the initial steer must create the inbox WAL"
        backup = inbox_wal.with_suffix(".jsonl.smoke-backup")
        os.replace(inbox_wal, backup)
        inbox_wal.mkdir()
        failed = raw_call(
            "question.answer",
            {
                "question_id": question["id"],
                "text": ANSWER,
                "client_request_id": "answer-inbox-append-failure",
            },
        )
        assert failed.get("ok") is False and failed.get("error", {}).get("code") == "internal", failed
        answered = call(
            "question.list", {"assignment_id": assignment["id"], "state": ["answered"]}
        )["questions"]
        assert len(answered) == 1 and answered[0]["answer"]["text"] == ANSWER, answered
        inbox_wal.rmdir()
        os.replace(backup, inbox_wal)

        process = daemon.process
        assert process is not None
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=10)
        Provider.release_resume.set()
        daemon = Daemon(args)
        daemon.start()
        assert Provider.third_prompt.wait(45), "restart did not request the answer recovery prompt"
        user_answers = [
            message
            for message in Provider.third_messages
            if message.get("role") == "user" and message.get("content") == ANSWER
        ]
        assert len(user_answers) == 1, {
            "error": "expected one distinct user answer input",
            "messages": Provider.third_messages,
        }
        wait_until(
            lambda: call("assignment.get", {"assignment_id": assignment["id"]})["assignment"][
                "status"
            ]
            == "done",
            "recovered assignment done",
            45,
        )
        trace_path = args.home / "data" / "traces" / f"{assignment['id']}.jsonl"
        trace_rows = [json.loads(line) for line in trace_path.read_text().splitlines() if line]
        run_ids = sorted({row["run_id"] for row in trace_rows if row.get("run_id")})
        assert len(run_ids) == 1, run_ids
        jobs = [
            json.loads(path.read_text())
            for path in (args.home / "data" / "jobs").glob("job_*.json")
            if json.loads(path.read_text()).get("checkpoint", {}).get("run_id") == run_ids[0]
        ]
        assert len(jobs) == 1 and jobs[0]["status"] == "done", jobs
        inbox_items = json.loads((args.home / "data" / "inbox.json").read_text())
        answer_items = [
            item
            for item in inbox_items
            if item.get("message_id") == f"question-answer:{question['id']}"
        ]
        assert len(answer_items) == 1 and answer_items[0]["delivery"] == "read", answer_items
        assert Provider.calls == 3, Provider.calls
        consumed = jobs[0]["checkpoint"].get("runtime_inbox_consumed", [])
        assert consumed.count(answer_items[0]["id"]) == 1, consumed
        assert ANSWER in str(jobs[0]["checkpoint"]["messages"]), jobs[0]["checkpoint"]
        print(
            json.dumps(
                {
                    "result": "passed",
                    "assignment_id": assignment["id"],
                    "question_id": question["id"],
                    "run_id": run_ids[0],
                    "answer_input_id": answer_items[0]["id"],
                    "consumed_count": consumed.count(answer_items[0]["id"]),
                    "provider_calls": Provider.calls,
                    "user_answer_message_count": len(user_answers),
                    "assignment_status": "done",
                    "home": str(args.home),
                },
                ensure_ascii=False,
            )
        )
    finally:
        Provider.release_initial.set()
        Provider.release_resume.set()
        daemon.close()
        provider.shutdown()
        provider.server_close()


if __name__ == "__main__":
    main()
