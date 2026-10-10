# S5 fresh RPC journal 审计

状态：执行前只读审计，未发送真实写 RPC，不代表 S5 通过。

## 结论

`fresh-scene-checklist.md` 要求“写 RPC 先保存 UUID/journal，结果不明不换 UUID”，但现有 S0–S4 脚本普遍不满足。`scripts/e2e/rpc.py` 的 `RpcClient.call()` 只为少数方法临时生成 `client_request_id`，不会落盘；`poll()` 遇到传输错误会重新调用并生成新 UUID，不能用于写 RPC。

Fresh 阶段不直接运行带未记录写调用的场景脚本。由标准库临时 runner 逐项执行：先把 `method`、UUID、脱敏参数和精确作用域写入外部 journal，再发送一次；结果不明时只读 reconciliation，保持原 UUID，不重发。journal 不记录 provider key、密码或模型密钥，只记录已授权的 key 来源路径。

## 脚本入口审计

| 阶段 | 可直接使用 | 必须改由外部 journal runner 执行 |
| --- | --- | --- |
| S0 | `bootstrap.py`、heartbeat/连接只读检查 | `s0/connection_replay.py:405-440,492` 的 skill create/update/delete 无预写 journal |
| S1 | health、bootstrap、只读 trace/history | `s1/private_chat.py:306,343`；`recovery.py:301,356`；`background.py:84-97,384,432` 均没有完整的写前 journal；approval.decide 还需精确人工核对 |
| S2 | `project_followup.py:718-755` 的 chat.send；已有 partial journal 的 resume | `login_feature.py:781-783` question.answer、`821-823` approval.decide、`1308` project.confirm_done 无 UUID/journal；不要盲用 `--answer-question-option`、`--approve-test-tools-once`、`--confirm-projects` |
| S3 | `usage_readonly.py` | `skills_usage_search.py:72-93,147-195` 和 `skill_update_scope.py` 的 skill CRUD/记忆 chat.send 无逐调用 journal/resume |
| S4 | `android_notification_probe.py`、已登录 Chrome 的只读观察 | `routines_browser.py:93-143`、`scheduled_routine.py:210-237`、`screen_transport.py` 的 takeover.start/release 无完整 journal；未知结果不能执行 finally 清理重试 |

`project_followup.py` 对 approval 有预观察、异常后 `approval.list` reconciliation 并拒绝重复批准，但 `approval.decide` 本身没有 `client_request_id`，只能依赖 approval ID 幂等。当前服务仍有错误 `approval_ref` 残留问题，question、approval、confirm_done 必须先核对 question/approval ID、Bot、chat/project、assignment、run、checkpoint 和 effective target；任何不确定状态保持 pending。

## provider.py 边界

`scripts/dev/provider.py:78-96` 为 provider.create/update、model.upsert、settings.update 只在内存生成 UUID，无预写 journal 或未知结果 reconciliation；已有 MiniMax provider 时仍会执行 provider.update。Fresh 仅可做 `--check-upstream` 或只读 provider.list。需要恢复配置时逐项人工执行并记录 key 来源路径，绝不把 key、密码、请求体或 key 哈希写入 journal、日志、截图或提交。

