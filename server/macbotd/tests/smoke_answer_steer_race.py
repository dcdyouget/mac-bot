#!/usr/bin/env python3
"""Production-RPC admission matrix for waiting assignment continuations.

Cases: bot_cap, global_cap, serial_project, cross_project_parallel.
Each invocation uses fresh home and ephemeral local ports; only the model
provider is a controllable local OpenAI-compatible server.
"""
from __future__ import annotations
import argparse, json, shlex, socket, threading, time, sys
from pathlib import Path
from http.server import ThreadingHTTPServer
sys.path.insert(0, str(Path(__file__).resolve().parent))
from smoke_collaboration import Daemon, FakeProviderHandler, TOKEN, rpc, wait_until

class Provider(FakeProviderHandler):
    lock = threading.Lock()
    calls = 0
    inflight = 0
    max_inflight = 0
    order: list[str] = []
    a_first = threading.Event()
    release_a_question = threading.Event()
    b_started = threading.Event()
    a_resumed = threading.Event()
    release_b = threading.Event()
    a_resume_has_answer = False
    a_resume_has_question_call = False
    scenario = "matrix"

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        messages = body.get("messages", [])
        latest_user = next((m.get("content", "") for m in reversed(messages) if m.get("role") == "user"), "")
        raw = json.dumps(body, ensure_ascii=False)
        with type(self).lock:
            type(self).calls += 1
            call_number = type(self).calls
            type(self).inflight += 1
            type(self).max_inflight = max(type(self).max_inflight, type(self).inflight)
            if call_number == 1:
                marker = "A"
            elif call_number == 2:
                marker = "B"
            elif call_number == 3:
                marker = "A"
            elif "ADMISSION-B" in latest_user:
                marker = "B"
            elif "ADMISSION-A" in latest_user:
                marker = "A"
            elif "ADMISSION-B" in raw and "ADMISSION-A" not in raw:
                marker = "B"
            elif "ADMISSION-A" in raw:
                marker = "A"
            else:
                marker = "?"
            type(self).order.append(marker)
            if call_number == 3:
                type(self).a_resume_has_answer = any(
                    message.get("role") == "tool" and "answer A" in str(message.get("content", ""))
                    for message in messages
                ) or any(
                    message.get("role") == "user" and "answer A" in str(message.get("content", ""))
                    for message in messages
                )
                type(self).a_resume_has_question_call = any(
                    tool_call.get("function", {}).get("name") == "question"
                    for message in messages
                    for tool_call in message.get("tool_calls", [])
                )
        try:
            called = self._called_tools(messages)
            if marker == "A" and "question" not in called:
                type(self).a_first.set()
                if type(self).scenario == "answer_steer_race":
                    assert type(self).release_a_question.wait(60), "A question gate timed out"
                self._stream_tool("question", {"question": "A needs an answer"})
            elif marker == "B":
                type(self).b_started.set()
                assert type(self).release_b.wait(60), "B gate timed out"
                self._stream_tool("send_msg", {"intent": "done", "text": "B done", "mentions": []})
            elif marker == "A":
                type(self).a_resumed.set()
                self._stream_tool("send_msg", {"intent": "done", "text": "A done", "mentions": []})
            else:
                self._stream_tool("send_msg", {"intent": "done", "text": "unexpected", "mentions": []})
        finally:
            with type(self).lock:
                type(self).inflight -= 1

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--case", choices=["bot_cap", "global_cap", "serial_project", "cross_project_parallel", "answer_steer_race"], required=True)
    args = parser.parse_args()
    args.home = args.home.resolve(); args.home.mkdir(parents=True, exist_ok=False)
    config = args.home / "config.toml"; config.write_text('host_name = "Admission matrix"\n')
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with socket.socket() as s: s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]
    args.url = f"http://127.0.0.1:{port}"; args.password = "isolated-admission"; args.browser_bin = None
    args.daemon_command = shlex.join([str(args.binary.resolve()), "--config", str(config), "--home", str(args.home), "--port", str(port), "--password", args.password])
    daemon = Daemon(args)
    def call(method, params=None): return rpc(args.url, args.password, method, params or {})
    limits = {
        "bot_cap": (8, 3, 1, 1, "bot_parallel_limit", "same_bot_two_projects"),
        "global_cap": (1, 2, 2, 2, "global_limit", "different_bots"),
        "serial_project": (8, 3, 3, 1, "serial_in_project", "same_bot_same_project"),
        "cross_project_parallel": (8, 3, 2, 1, None, "same_bot_two_projects"),
        "answer_steer_race": (8, 3, 1, 1, "bot_parallel_limit", "same_bot_two_projects"),
    }
    global_limit, bot_default, bot_parallel, bot_count, reason, topology = limits[args.case]
    Provider.scenario = args.case
    try:
        daemon.start()
        settings = call("settings.get")["settings"]
        concurrency = settings.get("concurrency", {})
        concurrency.update({"global": global_limit, "bot_default": bot_default})
        call("settings.update", {"patch": {"concurrency": concurrency}})
        provider_id = call("provider.create", {"name": "local gated", "api_kind": "openai-completions", "base_url": f"http://127.0.0.1:{provider.server_port}/v1", "api_key": TOKEN})["provider"]["id"]
        model = call("model.upsert", {"provider_id": provider_id, "model_id": "admission-fake", "display_name": "fake", "caps": {"tools": True, "vision": False, "reasoning": False}})["model"]["ref"]
        bots = [call("bot.create", {"name": f"worker-{i}", "model": model, "max_parallel": bot_parallel})["bot"] for i in range(bot_count)]
        # Project ids are distinct except when this case deliberately tests serial admission.
        created_a = call("project.create", {"name": "A project", "goal": "admission", "member_bot_ids": [bots[0]["id"]]})
        p_a = created_a["project"]
        chat_a = created_a["chat"]["id"]
        if topology == "same_bot_same_project":
            p_b, chat_b = p_a, chat_a
        else:
            b_bot = bots[1] if args.case == "global_cap" else bots[0]
            created_b = call("project.create", {"name": "B project", "goal": "admission", "member_bot_ids": [b_bot["id"]]})
            p_b, chat_b = created_b["project"], created_b["chat"]["id"]
        a = call("assignment.create", {"project_id": p_a["id"], "origin_chat_id": chat_a, "bot_id": bots[0]["id"], "title": "A question", "instruction": "ADMISSION-A ask a question then finish", "from": "user", "trigger_message_id": None, "parent_assignment_id": None, "priority": 1, "root_message_id": None, "loop_hops": 0})
        assert Provider.a_first.wait(20), "A did not call provider"
        queued_message = None
        if args.case == "answer_steer_race":
            queued_message = call("chat.send", {"chat_id": chat_a, "text": "STEER-RACE please include this note", "mentions": [], "client_request_id": "answer-steer-race-message"})["message"]
            assert any(item.get("assignment_id") == a["id"] and item.get("state") == "queued" for item in queued_message.get("delivery", [])), queued_message
            Provider.release_a_question.set()
        wait_until(lambda: bool(call("question.list", {"assignment_id": a["id"], "state": ["pending"]})["questions"]), "A pending question", 20)
        request_files = list((args.home / "data/run_requests").glob("*.json"))
        original_run = next(json.loads(path.read_text())["run_id"] for path in request_files if json.loads(path.read_text()).get("assignment_id") == a["id"])
        b_bot = bots[1] if args.case == "global_cap" else bots[0]
        b = call("assignment.create", {"project_id": p_b["id"], "origin_chat_id": chat_b, "bot_id": b_bot["id"], "title": "B blocker", "instruction": "ADMISSION-B finish when provider gate opens", "from": "user", "trigger_message_id": None, "parent_assignment_id": None, "priority": 1, "root_message_id": None, "loop_hops": 0})
        assert Provider.b_started.wait(20), "B did not enter provider gate"
        question = call("question.list", {"assignment_id": a["id"], "state": ["pending"]})["questions"][0]
        if args.case == "answer_steer_race":
            barrier = threading.Barrier(3)
            outcomes = {}
            def answer_worker():
                barrier.wait()
                try: outcomes["answer"] = call("question.answer", {"question_id": question["id"], "text": "answer A", "client_request_id": "answer-steer-race-answer"})
                except Exception as error: outcomes["answer_error"] = repr(error)
            def steer_worker():
                barrier.wait()
                try: outcomes["steer"] = call("message.steer", {"message_id": queued_message["id"], "bot_id": bots[0]["id"], "assignment_id": a["id"], "client_request_id": "answer-steer-race-steer"})
                except Exception as error: outcomes["steer_error"] = repr(error)
            threads = [threading.Thread(target=answer_worker), threading.Thread(target=steer_worker)]
            for thread in threads: thread.start()
            barrier.wait()
            for thread in threads: thread.join(30)
            assert all(not thread.is_alive() for thread in threads), "concurrent answer/steer RPC hung"
            assert "answer_error" not in outcomes, outcomes
            assert "steer_error" not in outcomes, outcomes
            time.sleep(0.8)
        else:
            call("question.answer", {"question_id": question["id"], "text": "answer A"})
        if reason:
            time.sleep(0.8)
            snapshot = call("assignment.get", {"assignment_id": a["id"]})["assignment"]
            assert snapshot["status"] == "queued", snapshot
            assert snapshot["queue_reason"] == reason, snapshot
            assert Provider.calls == 2, {"calls": Provider.calls, "order": Provider.order}
            assert not Provider.a_resumed.is_set()
        elif args.case == "cross_project_parallel":
            assert Provider.a_resumed.wait(10), "cross-project A did not use the second Bot slot while B remained held"
            assert Provider.calls == 3 and Provider.max_inflight >= 2, {"calls": Provider.calls, "max_inflight": Provider.max_inflight, "order": Provider.order}
        Provider.release_b.set()
        if reason and args.case != "answer_steer_race":
            assert Provider.a_resumed.wait(20), "queued A did not resume after B released capacity"
        elif args.case == "answer_steer_race":
            wait_until(lambda: call("assignment.get", {"assignment_id": a["id"]})["assignment"]["status"] in {"done", "blocked", "failed"}, "A final", 30)
        wait_until(lambda: all(call("assignment.get", {"assignment_id": x["id"]})["assignment"]["status"] == "done" for x in (a, b)), "A and B done", 30)
        if args.case != "answer_steer_race":
            assert Provider.calls == 3, {"calls": Provider.calls, "order": Provider.order}
            assert Provider.a_resume_has_answer, "third provider request must contain persisted answer A"
            assert Provider.a_resume_has_question_call, "third provider request must retain A's original question tool call"
        requests_a = [json.loads(path.read_text()) for path in (args.home / "data/run_requests").glob("*.json") if json.loads(path.read_text()).get("assignment_id") == a["id"]]
        assert len(requests_a) == 1 and requests_a[0]["run_id"] == original_run, requests_a
        jobs_a = [json.loads(path.read_text()) for path in (args.home / "data/jobs").glob("job_*.json") if json.loads(path.read_text()).get("checkpoint", {}).get("run_id") == original_run]
        assert len(jobs_a) == 1 and jobs_a[0]["status"] == "done", jobs_a
        answer_events = [json.loads(line) for line in (args.home / "data/traces" / f"{original_run.removeprefix('run_')}.jsonl").read_text().splitlines() if json.loads(line).get("type") == "tool.end" and json.loads(line).get("data", {}).get("preview") == "answer A"]
        if args.case == "answer_steer_race":
            assert call("assignment.get", {"assignment_id": a["id"]})["assignment"]["status"] == "done", call("assignment.get", {"assignment_id": a["id"]})
            records = [json.loads(path.read_text()) for path in (args.home / "data/continuations").glob("*.json") if json.loads(path.read_text()).get("assignment_id") == a["id"]]
            assert records and all(record.get("state") in {"done", "uncertain"} for record in records), records
            assert not any(record.get("state") in {"queued", "claimed", "input_committed", "effect_unknown"} for record in records), records
            assert Provider.calls == 3, {"calls": Provider.calls, "order": Provider.order}
            assert Provider.a_resume_has_answer, "the canonical concurrent answer must reach the original run"
            assert Provider.a_resume_has_question_call, "resumption must retain the original question tool call"
            delivery = call("chat.history", {"chat_id": chat_a, "after_seq": 0, "limit": 1000})["messages"]
            steer = next(item for item in delivery if item.get("id") == queued_message["id"])
            assert len(answer_events) == 1, answer_events
            trace_events = [json.loads(line) for line in (args.home / "data/traces" / f"{original_run.removeprefix('run_')}.jsonl").read_text().splitlines()]
            consumed_steers = [event for event in trace_events if event.get("type") == "steer" and event.get("data", {}).get("message_id") == queued_message["id"]]
            assert all(item.get("state") in {"read", "delivered"} for item in steer.get("delivery", [])), steer
            assert len(consumed_steers) == 1 and consumed_steers[0]["data"].get("text") == "STEER-RACE please include this note", consumed_steers
            assert len(call("assignment.get", {"assignment_id": a["id"]})["assignment"]["steers"]) <= 1
        else:
            assert len(answer_events) == 1, answer_events
        print(json.dumps({"result": "passed", "case": args.case, "after_answer_calls": 3 if not reason else 2, "max_inflight": Provider.max_inflight, "order": Provider.order, "queue_reason": reason, "steer_rpc_conflict": "steer_error" in outcomes if args.case == "answer_steer_race" else None, "steer_trace_count": len(consumed_steers) if args.case == "answer_steer_race" else None}, ensure_ascii=False))
    finally:
        Provider.release_b.set(); daemon.close(); provider.shutdown()
if __name__ == "__main__": main()
