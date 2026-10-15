#!/usr/bin/env python3
"""Crash an approved unsafe tool, then prove the identical decision cannot replay it."""
import sys,json,threading,socket,argparse,os,time,signal
from pathlib import Path
from smoke_collaboration import Daemon,FakeProviderHandler,rpc,wait_until,TOKEN
from http.server import ThreadingHTTPServer
parser=argparse.ArgumentParser(); parser.add_argument('--home',type=Path,required=True); parser.add_argument('--binary',type=Path,required=True); cli=parser.parse_args()
release=threading.Event(); started=threading.Event()
class Handler(FakeProviderHandler):
 def do_POST(self):
  size=int(self.headers['Content-Length']); body=json.loads(self.rfile.read(size));
  type(self).calls+=1
  if type(self).calls==1:
   self._stream_tool('bash',{'command':f'printf x >> {home}/effect.txt; sleep 4; echo done','background':False})
  else:
   self._stream_tool('bash',{'command':f'printf x >> {home}/effect.txt; sleep 4; echo done','background':False})

provider=ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=provider.serve_forever,daemon=True).start()
s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close()
home=cli.home.resolve();home.mkdir(parents=True,exist_ok=False)
config=home/'config.toml';config.write_text('host_name = "Fault Audit"\n')
a=argparse.Namespace(url=f'http://127.0.0.1:{port}',password='audit-only',home=home,browser_bin=None,daemon_command=f'{cli.binary.resolve()} --config {config} --home {home} --port {port} --password audit-only')
d=Daemon(a)
def call(m,p={}):return rpc(a.url,a.password,m,p)
try:
 d.start()
 p=call('provider.create',{'name':'fault fake','api_kind':'openai-completions','base_url':f'http://127.0.0.1:{provider.server_port}/v1','api_key':TOKEN})['provider']
 m=call('model.upsert',{'provider_id':p['id'],'model_id':'collaboration-fake','display_name':'fake','caps':{'tools':True,'vision':False,'reasoning':False}})['model']['ref']
 b=call('bot.create',{'name':'queued audit','model':m,'max_parallel':1,'tools':{'subagent':False,'files':True,'bash':True,'browser':False,'web':False,'mcp':False}})['bot']
 r=call('routine.create',{'bot_id':b['id'],'name':'queued audit','instructions':'audit routine','schedules':[{'cron':'*/5 * * * *','label':'five'}]})['routine']
 one=call('routine.test_run',{'routine_id':r['id']})['run']
 def approvals(): return call('approval.list',{'state':['pending']})
 print('approval list=',json.dumps(approvals()))
 aps=wait_until(lambda: approvals().get('approvals') or approvals().get('items'), 'subagent approval',20)
 ap=approvals()['approvals'][0]
 # Simulate the allowed WAL/in-memory checkpoint being newer than the per-job
 # JSON snapshot after a snapshot write failure. The decision must inspect the
 # authoritative live job, not this stale file copy.
 stale_snapshot=None
 for job_path in (home/'data/jobs').glob('*.json'):
  job=json.loads(job_path.read_text())
  pending=job.get('checkpoint',{}).get('pending_tool')
  if isinstance(pending,dict) and pending.get('name')=='bash':
   job['checkpoint']['pending_tool']={**pending,'id':'stale-snapshot-call','name':'ask_user'}
   job_path.write_text(json.dumps(job))
   stale_snapshot=job_path
   break
 assert stale_snapshot is not None, 'could not locate persisted approval job snapshot'
 decision={'approval_id':ap['id'],'decision':'allow_once','client_request_id':'audit-approval-identical-retry'}
 call('approval.decide',decision)
 wait_until(lambda:(home/'effect.txt').exists(), 'external effect',20)
 print('effect before kill=',(home/'effect.txt').read_text())
 before=[json.loads(p.read_text()) for p in (home/'data/jobs').glob('*.json')]
 print('jobs before kill=',json.dumps([{'run_id':j['checkpoint'].get('run_id'),'status':j['status'],'unsafe':j['unsafe_replay']} for j in before]))
 os.killpg(d.process.pid,signal.SIGKILL);d.process.wait(timeout=5)
 time.sleep(5)
 d.start();time.sleep(2)
 after=[json.loads(p.read_text()) for p in (home/'data/jobs').glob('*.json')]
 print('jobs after restart=',json.dumps([{'run_id':j['checkpoint'].get('run_id'),'status':j['status'],'unsafe':j['unsafe_replay']} for j in after]))
 print('effect after restart=',(home/'effect.txt').read_text(),'provider calls=',Handler.calls)
 assert (home/'effect.txt').read_text()=='x'
 assert any(j['status']=='suspended' and j['unsafe_replay'] for j in after)
 print('replaying same approval.decide request=',json.dumps(decision))
 print('retry response=',json.dumps(call('approval.decide',decision)))
 time.sleep(2)
 print('effect after identical request retry=',(home/'effect.txt').read_text(),'provider calls=',Handler.calls)
 assert (home/'effect.txt').read_text()=='x', 'retry duplicated the unsafe external effect'
 assert Handler.calls == 1, 'retry restarted provider/tool execution'
 print('PASSED: identical approval decision retry did not replay the unsafe effect')

finally:
 release.set();d.close();provider.shutdown()
