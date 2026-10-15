# Server 未覆盖链路只读审查

审查范围：仅服务端代码静态链路与现有测试证据；未改源码/共享文档，未运行测试。权威约束已对照 `AGENTS.md`、`docs/PLAN.md`、`docs/DESIGN.md`、`docs/PROTOCOL.md`。本次未确认新缺陷；“未覆盖”仅表示本次没有穷尽检查，不等于缺陷。

## 1. 用户记忆 expected_version 与持久化

**契约**：PLAN §5.5.1（记忆立即提交、replace/remove 按版本冲突）；PROTOCOL §5.10a（更新/删除及 profile update 携带 expected_version，冲突带当前值）。代码路径：客户端 RPC → feature service CAS → memory store 持久事务/WAL → 增量事件 → gateway publish。

- 入口/参数：`FeatureService::user_memory_rpc` 固定 owner 为 `local-user`，update/delete/profile update 解析协议参数；不采信请求中的 owner 字段：[features.rs](../../server/crates/macbot-gateway/src/features.rs)、[features.rs](../../server/crates/macbot-gateway/src/features.rs)。
- CAS/落盘：`transact_user_memory` 调用 `transact_user_persisted_with_actor`，将请求 id、签名和操作交给 memory store；冲突映射为包含 current 的 feature conflict：[features.rs](../../server/crates/macbot-gateway/src/features.rs)、[features.rs](../../server/crates/macbot-gateway/src/features.rs)。存储层先校验 receipt/idempotency 和 expected version，再构造候选、持久化变更并发布内存状态：[memory/lib.rs](../../server/crates/macbot-memory/src/lib.rs)、[memory/lib.rs](../../server/crates/macbot-memory/src/lib.rs)、[memory/lib.rs](../../server/crates/macbot-memory/src/lib.rs)、[memory/lib.rs](../../server/crates/macbot-memory/src/lib.rs)。
- 事件：成功事务通过 `append_event_once(memory-user-rpc:{request_id}, ...)` 写增量事件；audit 失败另写 memory.failed：[features.rs](../../server/crates/macbot-gateway/src/features.rs)。
- 现有证据：`user_memory_rpc_persists_cas_events_and_profile_without_owner_input` 覆盖恶意 owner 被忽略、stale version 冲突、版本递增、profile timezone 不可写以及重开后恢复：[features.rs](../../server/crates/macbot-gateway/src/features.rs)。存储层还有 update/delete/profile CAS 与事务恢复用例（memory/lib.rs 测试区）。
- 未覆盖：本次没有逐项审计所有崩溃点/多进程并发及客户端消费增量后的生效时序。

## 2. 技能安装/删除与远程路径边界

**契约**：DESIGN §4.10、PROTOCOL §5.8；RPC 需要返回 skill.updated / skill.deleted，import 支持 path/git/upload。

- RPC 入口：`skill_rpc` dispatch list/get/create/update/delete/set_enabled/publish/import；各 mutation 调用 `finish_skill_mutation`，生成持久化与对应事件：[features.rs](../../server/crates/macbot-gateway/src/features.rs)、[features.rs](../../server/crates/macbot-gateway/src/features.rs)、[features.rs](../../server/crates/macbot-gateway/src/features.rs)、[features.rs](../../server/crates/macbot-gateway/src/features.rs)。
- 文件状态：delete 禁止 builtin，且要求路径位于 install_root 下，之后删除目录并移除 registry entry：[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)。import path 先 canonicalize，校验 SKILL.md/目录并复制到 install_root；git 通过带 `--` 的 clone 参数，subdir 交给 resolve_git_subdir；zip 检查 enclosed_name 和 symlink：[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)、[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)、[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)。Upload ID 拒绝分隔符和 `..`，路径拼在 home/uploads 下：[features.rs](../../server/crates/macbot-gateway/src/features.rs)。
- 事件/持久化：mutation 的 finish helper 会保存 registry snapshot 并返回变更事件；事件契约见 PROTOCOL §8 的 skill.updated/deleted：[features.rs](../../server/crates/macbot-gateway/src/features.rs)、[PROTOCOL.md](../../docs/PROTOCOL.md)。
- 现有证据：skills crate 测试覆盖 zip traversal、filesystem symlink、git subdir escape、builtin 写保护：[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)、[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)、[skills/lib.rs](../../server/crates/macbot-skills/src/lib.rs)。
- 未覆盖：本次未验证远程 Git 服务端的网络/认证/资源限制、upload 文件创建端到端及路径竞争窗口；未发现能据静态证据确认的越界写入。

## 3. 消息 @ / reply / quote / 待看语义

**契约**：PLAN §5.4、§5.3.6；PROTOCOL §4.5、§5.2。用户 @ 指定 Bot / main，everyone 展开群成员；引用 reply 默认触发原 Bot，reference 仅保存引用；无 @ 的群消息进入当前 working Bot 的 queued delivery；queue.review 消费后变 read；chat.last_read_seq 是用户阅读进度，不代表 Bot 已读。

- 入口：`chat_send` 检查只读 bot_dm、解析 mentions；quote/reply_to 在同一 chat 查源消息，默认 reply 时把原 Bot 加到 mentions，持久消息的 reply_to 置 null、quote 保存 message_id/sender/excerpt：[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)。
- @ 路由和待看状态：Orchestrator 将未 @ 的用户消息附加到同群 working assignments 的 delivery，并递增 pending_messages；send_msg(done) 分支先经 gateway 的 queued review 流程（代码/测试名见后）再终结 assignment。done 会设 status=done 与 finished_at；decision/blocked 转为相应等待状态：[state.rs](../../server/crates/macbot-orchestrator/src/state.rs)、[state.rs](../../server/crates/macbot-orchestrator/src/state.rs)、[state.rs](../../server/crates/macbot-orchestrator/src/state.rs)。mention 驱动 handoff/main 唤醒位于后续 send_msg 分支（state.rs 3712+）。
- 用户已读：`chat.mark_read` 将 chat overlay 的 last_read_seq 单调更新为 max(current, seq)，不触碰 delivery：[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)。
- 事件/落盘：消息经 orchestrator send_msg 写入状态，由 adapter 的消息持久化/事件同步机制对外发布；mark_read 单独写 overlay 并产生 read.updated。协议明确两种状态互不等价：[PROTOCOL.md](../../docs/PROTOCOL.md)、[PROTOCOL.md](../../docs/PROTOCOL.md)。
- 现有证据：`send_msg_done_reviews_queued_messages_before_creating_delivery` 覆盖交付前 review；adapter 有 `user_chat_steer_preserves_canonical_message_and_independent_delivery`、quote 跨重启恢复、quote/reply 与 chat.history 的生产测试；`production_chat_mark_read_persists_overlay_and_event` 覆盖 overlay/event 及较小 seq 不回退：[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)。
- 未覆盖：没有重跑测试；本次未穷尽查看每种 Mention 变体的权限/成员校验、消息事件广播每一种重启交错。

## 4. 通知与工作台状态来源

**契约**：DESIGN §4.8 与通知说明；PROTOCOL §5.7。工作台包括 running/global_limit/subagents_running、pending approval/question/review/takeover、各 Bot 活跃 assignment 和 done_today。通知是消息/审批/问题/接管等事件驱动，不应把展示状态当另一个独立事实源。

- `workbench.get` 从 orchestrator snapshot 建立状态：approvals/questions 仅 pending；projects 仅 review；takeover 从 `data/takeovers/*.json` 读 pending/active，pending 且已由 Question 表示时去重：[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)。
- 汇总字段：每 Bot active/max_parallel/工作中及等待 assignment；done_today 由 done + finished_at 本地日期计算；running 排除 main，global_limit 来自 snapshot settings，subagents_running 汇总 assignment subagents_active 与 private 子代理：[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)、[adapter.rs](../../server/crates/macbot-gateway/src/adapter.rs)。
- 通知入口证据：blocked 与 @main 会唤醒 main；execution 生成 approval.requested/question.asked 等事件；协作工具 notify_user/remind 统一走 send_msg：[state.rs](../../server/crates/macbot-orchestrator/src/state.rs)、[execution.rs](../../server/crates/macbot-gateway/src/execution.rs)、[collaboration_tools.rs](../../server/crates/macbot-gateway/src/collaboration_tools.rs)。
- 现有证据：adapter 有 workbench pending approval/question、active takeover、bot counts 与 assignment 统计的测试；backend 有 question.asked 和 approval event 测试；具体测试位置可按 `workbench`、`question.asked`、`approval.requested` 搜索。协议事件表含 approval.requested/resolved、question.asked/answered：[PROTOCOL.md](../../docs/PROTOCOL.md)。
- 未覆盖：本次未端到端验证 OS push/通知权限和客户端订阅；workbench takeover 目录读取错误有显式失败分支，但没有验证全部文件系统故障注入场景。

## 结论

四条链路均找到与契约对应的入口、状态/持久化和事件实现及针对性测试证据。静态审查没有确认新增断链或状态倒退缺陷；上述未覆盖范围没有作为缺陷上报。

## 子代理与父任务身份补充核查（2026-10-16）

`RuntimeSubagentDispatch` 的child与父run共享assignment_id，但当前显式工具许可只有read、ls、find、grep、skill。ask_user/question在进入Waiting前会被工具许可拒绝，当前没有可让child自身等待审批的已授权风险工具；真实协作smoke的subagent审批属于父run的subagent.start。因此没有将“child continuation完成父assignment”列为当前可达缺陷。若将来扩展child工具许可并允许独立等待，必须重新设计/验证continuation完成归属与按assignment_id预留的active_runs，不能直接沿用父任务完成路径。本轮未扩展子代理权限。
