# Server chat routing 独立验收

目标 binary：`/tmp/macbot-audit-20261015-iEFvBx/main-verify-target/debug/macbotd`
SHA-256：`f3eea3ffb57e6dbaa3ef5f8aa13cbb81904f23a58cf951eb23c1f046ed7d7eb0`

六个场景均使用独立 `/tmp/crq.*` home、显式配置、随机 loopback 端口及本地 gated fake provider；每个场景以如下命令运行（`CASE`、`HOME` 对应表格值）：

```sh
python3 /tmp/macbot-audit-20261015-iEFvBx/smoke_chat_route_identity.py \
  --binary /tmp/macbot-audit-20261015-iEFvBx/main-verify-target/debug/macbotd \
  --home HOME --case CASE
```

| 场景 | HOME / CASE | 结果与关键断言 | 日志 |
|---|---|---|---|
| 混合成员 `@everyone` | `/tmp/crq.checkmix.2Yb9K` / `mixed_everyone` | 3 个目标 assignment、3 个目标 durable job 均 `done`；provider 9 次；活跃 assignment 的单条 inbox 被消费；两名 idle 收件 assignment 的初始 instruction 各只含一次触发消息标记，无重复 steer。Main 自动属于项目成员。 | `/tmp/macbot-audit-20261015-iEFvBx/chat-route-mixed-everyone.log` |
| 显式 `@main` | `/tmp/crq.strictmain` / `main` | assignment 通过一条 `steers` 收到消息；delivery/inbox 为 0；provider 实际输入含标记（system 2 / user 2）；仅一条 run request；assignment/job `done`。 | `/tmp/macbot-audit-20261015-iEFvBx/chat-route-main.log` |
| 回复 Bot 消息 | `/tmp/crq.strictreply` / `reply` | quote 指向对应 Bot 消息且保留；仅一条 inbox，checkpoint 确认消费该 inbox；provider 含标记（system 2 / user 2）；assignment/job `done`。 | `/tmp/macbot-audit-20261015-iEFvBx/chat-route-reply.log` |
| 引用 Bot 消息 | `/tmp/crq.strictref` / `reference` | quote 保留；没有 runtime inbox，仍是普通 queued delivery；provider 输入含引用文本（system 2 / tool 1 / user 1）；assignment/job `done`。 | `/tmp/macbot-audit-20261015-iEFvBx/chat-route-reference.log` |
| 普通消息后显式 steer | `/tmp/crq.strictord` / `ordinary_steer` | steer 前仍 queued 且没有 inbox；显式 steer 后只有一条 inbox并被 checkpoint 消费；provider 含标记（system 2 / user 2）；assignment/job `done`。 | `/tmp/macbot-audit-20261015-iEFvBx/chat-route-ordinary-steer.log` |
| 重放相同 `client_request_id` | `/tmp/crq.strictrtry` / `request_retry` | 两次 RPC 返回同一 canonical message ID；仅一条触发 assignment、一条 run request、无重复 inbox；初始 instruction 仅含一次标记；provider 2 次；assignment/job `done`。 | `/tmp/macbot-audit-20261015-iEFvBx/chat-route-request-retry.log` |

六个日志均为 `ok: true`。脚本 `py_compile` 通过；结束后未发现残留的测试 `macbotd` 进程。过程中仅修正了 harness 对 `project.create`（Main 自动加入 roster）、`assignment.create` 返回形状，以及 Main 通过 `steers` 投递的预期；这些是测试脚本适配，不是产品缺陷。本轮未接触生产 Host 服务或主工作区数据。

计数说明：`provider_marker_roles` 汇总整个场景的多次 provider 请求，不是单次请求内的重复输入数；消费去重由唯一 inbox/消费 checkpoint/单 run 断言证明。reference 的 queued 断言位于交付前，最终可经 queue.review 合法读取。
