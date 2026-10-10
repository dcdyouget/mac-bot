# S5 fresh S1 kill-9 恢复清单

本清单只用于 fresh 新 home 的一次可控恢复验收；不复用旧项目、旧 run 或旧审批，不批量取消 pending。当前已知现场：

- Home：`/Users/gongshaojie/MacBot`
- worker：`01a12661-e385-7743-b897-2adb25e69f7c`（`Fresh-Coding`）
- direct chat：`dm_01a12661-e385-7743-b897-2adb25e69f7c`
- 最近已完成的基础 S1 run：`run_chat_01a12662_52cf_77fa_9fcd_9825c3ff68a9`；该 run 已 done，**不得**用于 kill-9
- 待只读复核的服务 PID：`88341`；实际执行前必须重新得到 LaunchAgent、7788 listener、命令行三者一致

## 新请求与 journal

1. 只读核对 `/api/v1/health`（生产、`setup_required=false`）、`bootstrap`、worker/chat 身份和当前 pending；保存旧 pending/job 的数量或哈希，不能处理未知项。
2. 生成新 marker，例如 `s5-fresh-recovery-<唯一后缀>` 和新 `client_request_id`。先把以下脱敏字段写入本次 journal，再发唯一一次 `chat.send`：`method`、`client_request_id`、`chat_id=dm_01a12661-e385-7743-b897-2adb25e69f7c`、marker、relative path、expected command、home、worker_id、started_at。
3. 请求正文必须要求每个模型 turn 只调用一个工具：先执行精确命令
   `sleep 20; mkdir -p e2e; printf '%s\n' '<marker>' > e2e/<marker>.txt`，返回后停止；下一 turn 只对 `e2e/<marker>.txt` 执行 `read`，再回复 marker。不能混入其他工具或路径。
4. 结果不明时停止写操作，只读 `chat.history`/`trace.history`/`approval.list` 按原 UUID reconciliation；不得换 UUID 重发。

## 允许 kill 的唯一 checkpoint

只有同时满足以下条件，才允许进入受控 kill 步骤：

- `trace.history(chat_id=...)` 中本轮 marker 只对应一个 `run_id`；该 run 有 `tool.start` 的 `bash`，args 的 `command` 与上面 expected command 完全相等。
- 该 run 尚未有 `run.end`，没有提前 `read`，且该 bash call 已有成功 `tool.end`；不存在第二个 marker run 或重复 bash。
- `~/MacBot/data/jobs/*.json` 中唯一匹配本轮 `run_id` 的 job：`status=running`、`unsafe_replay=false`、`checkpoint.run_id` 相同、`checkpoint.messages` 为数组，文件 mtime 在本请求开始后；记录 job path、job_id、commit_seq、mtime 和 checkpoint 摘要。
- 若 bash 被 approval 卡住，只能从 `approval.list(state=["pending"])` 选择 exact candidate：`bot_id`、`chat_id`、assignment（本私聊应为 null）、tool=`bash`、risk=`exec`、detail JSON 的 command 完全一致；先记录 map/run/checkpoint，再以 `approval.decide {approval_id, decision:"allow_once"}` 一次处理。未知审批保持 pending。

## PID 与 kill 前后证据

执行前重新记录：`launchctl print gui/<uid>/com.macbot.server` 的 PID、`lsof -nP -iTCP:7788 -sTCP:LISTEN -t` 的唯一 PID、`ps -ww -p <pid> -o command=`。三者必须都是同一 PID `88341`（若已变化，以新只读核验结果为准），命令必须是非 mock `macbotd --port 7788`；否则停止，不 kill。

journal 至少保存：`service_before {pid, listener_pids, executable, label, port}`、`run_id`、`job_id`/checkpoint 摘要、approval review、`kill_intent.at`、marker SHA-256。只对这一个已核验 PID 发送 SIGKILL；不操作未知 PID、旧 job 或其他 LaunchAgent。

## 恢复通过条件

- KeepAlive 拉起的新服务通过同一套身份核验：旧 PID 已退出，7788 listener 只有新 PID，label 仍为 `com.macbot.server`，命令仍为生产非 mock；health 通过。
- 同一 `run_id` 的 trace 出现 `run.resume`，没有新兄弟 run；随后出现 `run.end` 且 `data.status=done`。
- `~/MacBot/bots/01a12661-e385-7743-b897-2adb25e69f7c/e2e/<marker>.txt` 是普通文件、非 symlink，内容严格等于 marker；chat history 中同一 Bot 的新回复包含 marker，且不重复生成成功消息。
- 记录 `service_after`、resume/run.end trace 摘要、marker path/hash、旧 pending/job 哈希对比；旧业务数据和未知 pending 必须保持不变。
- 双端截图只作为联合证据：桌面和 Android 各保存重连中、恢复后 trace/完成画面并实际查看；截图不能替代上述 durable/PID/API 断言。

既有参考：`scripts/e2e/s1/recovery.py:141-293,326-439`；联合恢复证据 `docs/progress/S1/production-aa2c99a-joint-recovery.json`。该历史证据中的旧 marker/run/PID 仅用于字段形状参考，不得在 fresh 重放。
