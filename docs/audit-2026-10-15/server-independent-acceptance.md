# Server 独立验收记录

验收对象：`/tmp/macbot-audit-20261015-iEFvBx/main-verify-target/debug/macbotd`，SHA-256 `4e144697de35a911c78ca58fedce376ebac88062010804fbf56a8c1a008bfa62`（2026-10-16 00:31:42 CST，82,559,224 bytes）。所有场景使用隔离临时 home/config、显式端口及本地 fake provider；未连接用户实际 Host 服务。命令中的口令统一记为 `<redacted>`。

## 主场景

- Collaboration：`python3 server/macbotd/tests/smoke_collaboration.py --daemon-command '<binary> --config <isolated-config> --home <isolated-home> --port 55540 --password <redacted>' --home <isolated-home> --url http://127.0.0.1:55540 --password <redacted>`。通过。30 assignments、35 durable jobs 均为 done；核心主任务、父子恢复、审查修订、steer、提问/接管/决策/阻塞断言通过。日志：`/tmp/macbot-audit-20261015-iEFvBx/server-smokes.zP4Llz/smoke_collaboration.log`。
- Dispatch：`python3 server/macbotd/tests/smoke_dispatch.py --daemon-command '<binary> --config <isolated-config> --home <isolated-home> --port 55857 --password <redacted>' --home <isolated-home> --url http://127.0.0.1:55857 --password <redacted>`。通过；输出 `ok=true`、handoff 成功、队列原因 `bot_parallel_limit`，并验证并发/串行重叠集合。日志：`/tmp/macbot-audit-20261015-iEFvBx/server-smokes.zP4Llz/smoke_dispatch.log`。
- Product closure：`python3 server/macbotd/tests/smoke_product_closure.py --daemon-binary <binary> --temp-root /tmp/pcl.2hPcdR --port 28431`。通过，16 次 provider 请求；项目 done、5 assignments 全 done；产出原始与修订两个文件，修订内容含用户要求的 marker；确认项目完成的用户确认及 review-card 断言通过。持久化 state/job 文件在 `/private/tmp/pcl.2hPcdR/user-home/MacBot/data/`：6 jobs 全 done。相比旧报告的 5 个 job，多出的一个是私有 chat/coordinator run，并非失败。日志：`/tmp/macbot-audit-20261015-iEFvBx/server-smokes.zP4Llz/smoke_product_closure_shortroot.log`。

## 相关补充场景

- Routine queue：`python3 server/macbotd/tests/smoke_routine_queue.py --home /tmp/rq.zOm1DR/home --binary <binary>`。通过；两次 routine 按序执行，队列、重启不重复 provider 请求、durable request 关联及两条 usage 记录断言通过。日志：`/tmp/rq.zOm1DR/routine-queue.log`。
- Routine recovery：`python3 server/macbotd/tests/smoke_routine_recovery.py --home /tmp/rr.ykva9d/home --binary <binary>`。通过；杀进程并重启后 job 保持 suspended、未替换 run、未重复 provider 副作用，原 request id 保留。日志：`/tmp/rr.ykva9d/routine-recovery.log`。
- Cancelled approvals：`MACBOT_CONFIG=/tmp/ca.Rdd95Y/config.toml python3 server/macbotd/tests/smoke_cancelled_approvals.py --url http://127.0.0.1:28460 --password <redacted> --daemon-command '<binary> --port 28460 --password <redacted>' --home /tmp/ca.Rdd95Y/home`。通过；取消后 provider call 数未增长、job cancelled；正常拒绝审批事件/停止消息断言通过。日志：`/tmp/ca.Rdd95Y/smoke_cancelled_approvals.log`。

## 启动限制与覆盖边界

Product closure 使用较长 `/tmp/macbot-audit-.../server-smokes.../productN` 根目录时，脚本等待健康检查超时；按相同配置直接启动并捕获 stderr，进程退出码为 1，原始错误为 `path must be shorter than SUN_LEN`。这是 macOS Unix domain socket 路径长度限制：daemon socket 位于 `<home>/data/macbotd.sock`。缩短临时根到 `/tmp/pcl.2hPcdR` 后即成功；证据不是 timeout、管道阻塞或 IPv6 问题，因此未延长 timeout。受此限制，较长 socket 路径组合未覆盖。

本轮使用的旧 binary 在 runtime usage `assignment.updated` 上曾有内部字符串类型无法由客户端 typed decoder 解码的问题；该协议事件修复由另一独立验收负责，本报告的业务 smoke 不作为该问题的验证证据。
