# Server 独立验收摘要

本轮只读 QA 未改仓库源码。最终 server diff SHA-256：`3fac8b0c159412d32a19530be93e6bb6a75b2cdaec4fd8afb539db28c6d29533`。独立 binary `/tmp/macbot-audit-20261015-iEFvBx/main-verify-target/debug/macbotd`，SHA-256 `f3eea3ffb57e6dbaa3ef5f8aa13cbb81904f23a58cf951eb23c1f046ed7d7eb0`。

Workspace test、`clippy --workspace --all-targets -- -D warnings`、`macbotd` build 全通过。Workspace 日志 `final2-workspace-test.log`（gateway 382，durable 18）、`final2-clippy.log`、`final2-build.log`。

最新 binary 的真实生产场景全部通过：claimed answer 普通文本、multi `[0,2]` 无自由文本与附自由文本；答案 WAL 已写但 inbox append EISDIR 的显式错误与 restart 修复；done-first 时晚到答案冲突；答案接纳后 SIGKILL/restart；普通 steer 的重启恢复（快照是否在 kill 前存在未可靠记录，不作为 S15 旧快照恢复证据）；answer + queued steer race；S13 普通消息经单次 `queue.review` 后读取；Main-Bot完整产品闭环。关键日志分别为 `final2-qa-claimed-*.log`、`final2-qa-answer-inbox-recovery.log`、`final2-answer-inbox-eisdir-role-audit.log`、`final2-terminal-restart-role.log`、`final2-steer-input-role-audit.log`、`final2-answer-steer-race.log`、`final2-qa-s13-live-ws.log`、`final2-product-closure.log`。

S13 live WebSocket 实际帧 82 条，旧 raw assignment event 的 session.resume replay 57 条，均直接以 Rust `macbot_protocol::EventFrame` 反序列化；总计 139 帧通过，无手工 normalize。旧 home 中 3 条 raw assignment sender 字符串在新二进制 replay 中投影为 typed sender。证据见 `final2-historical-replay.log` 与 `final2-eventframe-decode.log`。

admission 四矩阵、approval uncertain replay 和 admission answer restart 在前一独立 binary `1550387e...` 上通过且本轮投影/答案修复未触及这些行为，保留其原日志，不声称用最新 binary 重跑。`resume_answer_counts=[1,1]` 是重启前后两个 provider 请求上下文各含答案一次。真实路由 6-case 由 root 指派的另一 QA agent 执行。所有本轮 QA daemon/provider 均已退出。

逐项命令、断言、日志与 hash 见同目录 `server-independent-acceptance-final.json`。
