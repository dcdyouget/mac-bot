#!/usr/bin/env python3
"""Exercise the public message.steer RPC against a waiting decision run."""
from __future__ import annotations
import argparse, json, shlex, socket, threading, time
from pathlib import Path
from http.server import ThreadingHTTPServer
from smoke_collaboration import Daemon, FakeProviderHandler, TOKEN, rpc, wait_until

class Provider(FakeProviderHandler):
    calls = 0
    first = threading.Event()
    release_decision = threading.Event()
    resumed = threading.Event()
    blocked_started = threading.Event()
    release_blocked = threading.Event()
    blocked_resumed = threading.Event()
    lock = threading.Lock()
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        raw = json.dumps(body, ensure_ascii=False)
        with type(self).lock:
            type(self).calls += 1
            call = type(self).calls
        messages = body.get("messages", [])
        if call == 1:
            type(self).first.set()
            assert type(self).release_decision.wait(30), "initial decision gate timed out"
            self._stream_tool("send_msg", {"intent":"decision", "text":"Choose a direction", "mentions":[], "options":["continue","change"]})
        elif call == 2:
            assert "please use the new direction" in raw, raw
            assert "send_msg" in self._called_tools(messages), "resume lost the original decision call"
            type(self).resumed.set()
            self._stream_tool("send_msg", {"intent":"done", "text":"Applied the steer", "mentions":[]})
        elif call == 3:
            type(self).blocked_started.set()
            assert type(self).release_blocked.wait(30), "blocked wait gate timed out"
            self._stream_tool("send_msg", {"intent":"blocked", "text":"Waiting on a safe handoff", "mentions":[]})
        elif call == 4:
            assert "please continue from safe block" in raw, raw
            assert "send_msg" in self._called_tools(messages), "blocked resume lost original send_msg call"
            type(self).blocked_resumed.set()
            self._stream_tool("send_msg", {"intent":"done", "text":"Completed after safe block", "mentions":[]})
        elif "UNSAFE-STEER" in raw:
            self._stream_tool("bash", {"command": f"printf x >> {HOME}/unsafe-effect.txt", "background": False})
        else:
            raise AssertionError(f"unexpected provider call {call}: {raw}")

def main():
    parser=argparse.ArgumentParser(); parser.add_argument("--home",type=Path,required=True); parser.add_argument("--binary",type=Path,required=True); args=parser.parse_args()
    global HOME
    args.home=args.home.resolve(); args.home.mkdir(parents=True,exist_ok=False); HOME=args.home
    config=args.home/"config.toml"; config.write_text('host_name = "Waiting steer regression"\n')
    provider=ThreadingHTTPServer(("127.0.0.1",0),Provider); threading.Thread(target=provider.serve_forever,daemon=True).start()
    with socket.socket() as s: s.bind(("127.0.0.1",0)); port=s.getsockname()[1]
    args.url=f"http://127.0.0.1:{port}"; args.password="isolated-steer"; args.browser_bin=None
    args.daemon_command=shlex.join([str(args.binary.resolve()),"--config",str(config),"--home",str(args.home),"--port",str(port),"--password",args.password])
    daemon=Daemon(args)
    def call(method,params=None): return rpc(args.url,args.password,method,params or {})
    try:
        daemon.start()
        provider_id=call("provider.create",{"name":"local steer","api_kind":"openai-completions","base_url":f"http://127.0.0.1:{provider.server_port}/v1","api_key":TOKEN})["provider"]["id"]
        model=call("model.upsert",{"provider_id":provider_id,"model_id":"steer-fake","display_name":"fake","caps":{"tools":True,"vision":False,"reasoning":False}})["model"]["ref"]
        bot=call("bot.create",{"name":"steer worker","model":model,"max_parallel":1})["bot"]
        project_result=call("project.create",{"name":"steer project","goal":"wait safely","member_bot_ids":[bot["id"]]})
        project=project_result["project"]; chat_id=project_result["chat"]["id"]
        assignment=call("assignment.create",{"project_id":project["id"],"origin_chat_id":chat_id,"bot_id":bot["id"],"title":"Wait for steer","instruction":"STEER-DECISION-REPLACEMENT wait for a question then act","from":"user","trigger_message_id":None,"parent_assignment_id":None,"priority":1,"root_message_id":None,"loop_hops":0})
        assert Provider.first.wait(20), "assignment did not start"
        message=call("chat.send",{"chat_id":chat_id,"text":"please use the new direction","mentions":[],"client_request_id":"waiting-steer-message"})["message"]
        assert any(delivery.get("assignment_id")==assignment["id"] and delivery.get("state")=="queued" for delivery in message.get("delivery",[])), message
        Provider.release_decision.set()
        wait_until(lambda: call("assignment.get",{"assignment_id":assignment["id"]})["assignment"]["status"]=="waiting_user","decision wait",20)
        result=call("message.steer",{"message_id":message["id"],"bot_id":bot["id"],"assignment_id":assignment["id"],"client_request_id":"waiting-steer-promote"})
        message_id=message["id"]
        assert result["message"]["id"]==message_id
        assert Provider.resumed.wait(20),"message.steer did not resume the safe decision wait"
        wait_until(lambda: call("assignment.get",{"assignment_id":assignment["id"]})["assignment"]["status"]=="done","assignment completion",20)
        final=call("assignment.get",{"assignment_id":assignment["id"]})["assignment"]
        assert len([s for s in final.get("steers",[]) if s.get("message_id")==message_id])==1, final
        requests=[json.loads(p.read_text()) for p in (args.home/"data/run_requests").glob("*.json") if json.loads(p.read_text()).get("assignment_id")==assignment["id"]]
        assert len(requests)==1,"steer must continue original run, not create a new one"
        events=[json.loads(line) for line in (args.home/"data/events/events.jsonl").read_text().splitlines()]
        message_creates=[e for e in events if e.get("event")=="message.created" and e.get("data",{}).get("message",{}).get("id")==message_id]
        assert len(message_creates)==1,"steer must preserve one canonical message"
        stored_state=json.loads((args.home/"data/orchestrator/state.json").read_text())
        final_message=stored_state["messages"][message_id]
        assert len(final_message.get("delivery",[]))==1 and final_message["delivery"][0]["state"]=="read", final_message
        assert final.get("steers",[])[0].get("applied_at"), final
        steer_text=[m.get("content","") for m in json.loads((args.home/"data/jobs/commits.jsonl").read_text().splitlines()[-1]).get("job",{}).get("checkpoint",{}).get("messages",[]) if m.get("role")=="user"]
        assert sum("please use the new direction" in text for text in steer_text)==1, steer_text

        blocked=call("assignment.create",{"project_id":project["id"],"origin_chat_id":chat_id,"bot_id":bot["id"],"title":"Safe blocked wait","instruction":"SAFE-BLOCKED send_msg then wait for a handoff","from":"user","trigger_message_id":None,"parent_assignment_id":None,"priority":1,"root_message_id":None,"loop_hops":0})
        assert Provider.blocked_started.wait(20), "safe blocked assignment did not start"
        blocked_message=call("chat.send",{"chat_id":chat_id,"text":"please continue from safe block","mentions":[],"client_request_id":"safe-blocked-steer-message"})["message"]
        assert any(d.get("assignment_id")==blocked["id"] and d.get("state")=="queued" for d in blocked_message.get("delivery",[])), blocked_message
        Provider.release_blocked.set()
        wait_until(lambda: call("assignment.get",{"assignment_id":blocked["id"]})["assignment"]["status"]=="blocked","safe blocked wait",20)
        blocked_state=call("assignment.get",{"assignment_id":blocked["id"]})["assignment"]
        assert blocked_state.get("wait",{}).get("reason")=="blocked" and blocked_state["wait"].get("message_id"), blocked_state
        call("message.steer",{"message_id":blocked_message["id"],"bot_id":bot["id"],"assignment_id":blocked["id"],"client_request_id":"safe-blocked-steer-promote"})
        assert Provider.blocked_resumed.wait(20), "message.steer did not resume a safe blocked send_msg"
        wait_until(lambda: call("assignment.get",{"assignment_id":blocked["id"]})["assignment"]["status"]=="done","safe blocked completion",20)
        blocked_requests=[json.loads(p.read_text()) for p in (args.home/"data/run_requests").glob("*.json") if json.loads(p.read_text()).get("assignment_id")==blocked["id"]]
        assert len(blocked_requests)==1, "blocked steer must continue original run"
        blocked_state=call("assignment.get",{"assignment_id":blocked["id"]})["assignment"]
        assert any(s.get("message_id")==blocked_message["id"] and s.get("applied_at") for s in blocked_state.get("steers",[])), blocked_state

        unsafe=call("assignment.create",{"project_id":project["id"],"origin_chat_id":chat_id,"bot_id":bot["id"],"title":"Unsafe wait","instruction":"UNSAFE-STEER require approval","from":"user","trigger_message_id":None,"parent_assignment_id":None,"priority":1,"root_message_id":None,"loop_hops":0})
        wait_until(lambda: bool(call("approval.list",{"state":["pending"]}).get("approvals")),"pending unsafe approval",20)
        before_calls=Provider.calls
        call("assignment.steer",{"bot_id":bot["id"],"project_id":project["id"],"chat_id":chat_id,"text":"unsafe approval must still be reviewed","message_id":"unsafe-approval-steer","client_request_id":"unsafe-approval-steer"})
        after_unsafe=call("assignment.get",{"assignment_id":unsafe["id"]})["assignment"]
        assert after_unsafe["status"]=="waiting_user" and after_unsafe.get("wait",{}).get("reason")=="approval", after_unsafe
        assert call("approval.list",{"state":["pending"]}).get("approvals"), "steer must not consume the pending approval"
        assert Provider.calls==before_calls and not (args.home/"unsafe-effect.txt").exists(), "unsafe steer must not resume provider or execute tool"
        print(json.dumps({"result":"passed","provider_calls":Provider.calls,"assignment_id":assignment["id"],"run_id":requests[0]["run_id"],"steer_message_id":message_id,"blocked_run_id":blocked_requests[0]["run_id"],"blocked_message_id":blocked_message["id"],"steer_deliveries":1}))
    finally:
        Provider.release_decision.set(); daemon.close(); provider.shutdown()
if __name__=="__main__": main()
