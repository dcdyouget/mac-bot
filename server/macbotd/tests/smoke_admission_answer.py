#!/usr/bin/env python3
"""Production RPC admission regression: answering A while same Bot slot is held by B."""
from __future__ import annotations
import argparse, json, shlex, socket, threading, time
from pathlib import Path
import sys
from http.server import ThreadingHTTPServer
from smoke_collaboration import Daemon, FakeProviderHandler, REPO, TOKEN, rpc, wait_until

class Provider(FakeProviderHandler):
    lock = threading.Lock()
    calls = 0
    a_first = threading.Event()
    b_started = threading.Event()
    a_resumed = threading.Event()
    release_first_resume = threading.Event()
    release_b = threading.Event()
    body_order: list[str] = []
    resume_answers: list[int] = []
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        with type(self).lock:
            type(self).calls += 1
            idx=type(self).calls
            raw=json.dumps(body,ensure_ascii=False)
            marker='A' if 'ADMISSION-A' in raw else ('B' if 'ADMISSION-B' in raw else '?')
            type(self).body_order.append(marker)
        if marker == 'A' and 'question' not in self._called_tools(body.get('messages', [])):
            type(self).a_first.set()
            self._stream_tool('question', {'question':'A needs an answer'})
        elif marker == 'B':
            type(self).b_started.set()
            assert type(self).release_b.wait(40), 'B gate timeout'
            self._stream_tool('send_msg', {'intent':'done','text':'B done','mentions':[]})
        elif marker == 'A':
            type(self).a_resumed.set()
            with type(self).lock:
                type(self).resume_answers.append(sum(
                    1 for message in body.get('messages', [])
                    if message.get('role') == 'tool' and 'answer A' in message.get('content', '')
                ))
                resume_number = len(type(self).resume_answers)
            if resume_number == 1:
                assert type(self).release_first_resume.wait(30), 'first A resume gate timeout'
            try:
                self._stream_tool('send_msg', {'intent':'done','text':'A done','mentions':[]})
            except BrokenPipeError:
                pass
        else:
            self._stream_tool('send_msg', {'intent':'done','text':'unexpected','mentions':[]})

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--home',type=Path,required=True); ap.add_argument('--binary',type=Path,required=True); args=ap.parse_args()
    args.home=args.home.resolve(); args.home.mkdir(parents=True,exist_ok=False)
    config=args.home/'config.toml'; config.write_text('host_name = "Admission regression"\n')
    provider=ThreadingHTTPServer(('127.0.0.1',0),Provider); threading.Thread(target=provider.serve_forever,daemon=True).start()
    with socket.socket() as s: s.bind(('127.0.0.1',0)); port=s.getsockname()[1]
    args.url=f'http://127.0.0.1:{port}'; args.password='isolated-admission'; args.browser_bin=None
    args.daemon_command=shlex.join([str(args.binary.resolve()),'--config',str(config),'--home',str(args.home),'--port',str(port),'--password',args.password])
    daemon=Daemon(args)
    def call(m,p=None): return rpc(args.url,args.password,m,p or {})
    try:
        daemon.start()
        pid=call('provider.create',{'name':'local gated','api_kind':'openai-completions','base_url':f'http://127.0.0.1:{provider.server_port}/v1','api_key':TOKEN})['provider']['id']
        model=call('model.upsert',{'provider_id':pid,'model_id':'admission-fake','display_name':'fake','caps':{'tools':True,'vision':False,'reasoning':False}})['model']['ref']
        bot=call('bot.create',{'name':'single slot','model':model,'max_parallel':1})['bot']
        a=call('assignment.create',{'project_id':None,'origin_chat_id':bot['dm_chat_id'],'bot_id':bot['id'],'title':'A question','instruction':'ADMISSION-A ask a question then finish','from':'user','trigger_message_id':None,'parent_assignment_id':None,'priority':1,'root_message_id':None,'loop_hops':0})
        assert Provider.a_first.wait(20),'A did not call provider'
        wait_until(lambda: bool(call('question.list',{'assignment_id':a['id'],'state':['pending']})['questions']),'A pending question',20)
        request_files = list((args.home/'data/run_requests').glob('*.json'))
        original_request = next(json.loads(path.read_text()) for path in request_files if json.loads(path.read_text()).get('assignment_id') == a['id'])
        original_run = original_request['run_id']
        job_path = next(path for path in (args.home/'data/jobs').glob('*.json') if json.loads(path.read_text()).get('checkpoint',{}).get('run_id') == original_run)
        legacy_job = json.loads(job_path.read_text()); legacy_job['unsafe_replay'] = True; job_path.write_text(json.dumps(legacy_job))
        commits = args.home/'data/jobs/commits.jsonl'
        records = [json.loads(line) for line in commits.read_text().splitlines()]
        for record in records:
            if record.get('job',{}).get('checkpoint',{}).get('run_id') == original_run:
                record['job']['unsafe_replay'] = True
        commits.write_text(''.join(json.dumps(record,separators=(',',':'))+'\n' for record in records))
        daemon.close(); daemon.start()
        assert Provider.calls == 1, 'safe legacy question must not resume without an answer'
        b=call('assignment.create',{'project_id':None,'origin_chat_id':bot['dm_chat_id'],'bot_id':bot['id'],'title':'B blocker','instruction':'ADMISSION-B finish when provider gate opens','from':'user','trigger_message_id':None,'parent_assignment_id':None,'priority':1,'root_message_id':None,'loop_hops':0})
        assert Provider.b_started.wait(20),'B did not consume Bot slot'
        question=call('question.list',{'assignment_id':a['id'],'state':['pending']})['questions'][0]
        call('question.answer',{'question_id':question['id'],'text':'answer A'})
        time.sleep(1)
        current=call('assignment.get',{'assignment_id':a['id']})['assignment']['status']
        print(json.dumps({'after_answer_status':current,'provider_calls_while_B_held':Provider.calls,'provider_order':Provider.body_order,'a':a['id'],'b':b['id']}))
        assert Provider.calls == 2, f'A bypassed admission while B held: calls={Provider.calls}, order={Provider.body_order}'
        assert current in ('queued','waiting_user'), f'A should remain queued/waiting until slot: {current}'
        Provider.release_b.set()
        assert Provider.a_resumed.wait(20),'A did not resume after B released slot'
        job = json.loads(job_path.read_text())
        assert job['checkpoint'].get('continuation',{}).get('phase') == 'input_committed', 'fault window must occur after durable answer injection'
        daemon.close()
        Provider.release_first_resume.set()
        daemon.start()
        wait_until(lambda: all(call('assignment.get',{'assignment_id':x['id']})['assignment']['status']=='done' for x in (a,b)),'A and B done',30)
        assert original_run in [json.loads(path.read_text())['checkpoint'].get('run_id') for path in (args.home/'data/jobs').glob('*.json')], 'answer must continue the original durable run'
        assert Provider.body_order == ['A','B','A','A'], f'restart must resume the same checkpoint: {Provider.body_order}'
        assert Provider.resume_answers[-1] == 1, f'answer must be injected exactly once after restart: {Provider.resume_answers}'
        events=[json.loads(line) for line in (args.home/'data/events/events.jsonl').read_text().splitlines()]
        a_statuses=[event.get('data',{}).get('assignment',{}).get('status') for event in events if event.get('event')=='assignment.updated' and event.get('data',{}).get('assignment',{}).get('id')==a['id']]
        assert 'queued' in a_statuses and 'working' in a_statuses, f'clients must observe A queued then admitted working: {a_statuses}'
        print(json.dumps({'result':'passed','provider_calls':Provider.calls,'order':Provider.body_order,'resume_answer_counts':Provider.resume_answers}))
    finally:
        Provider.release_b.set(); daemon.close(); provider.shutdown()
if __name__=='__main__': main()
