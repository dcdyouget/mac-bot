# Mac Bot 端到端场景

这些脚本通过协议规定的 HTTP RPC；主连接补发场景额外使用 Python 3 标准库实现的 `/ws`。所有脚本均不依赖第三方包。默认连接本机 mock：

```sh
python3 scripts/e2e/s0/bootstrap.py
```

正式服务可显式指定地址和密码；密码也可以放在 `MACBOT_PASSWORD`，脚本默认读取 `~/.macbot-dev-password`，不会打印密码：

```sh
python3 scripts/e2e/s0/bootstrap.py --url http://127.0.0.1:7788
```

S0 脚本默认连接 `127.0.0.1:7789`，固定使用 mock 约定的密码 `dev`（不会被 `~/.macbot-dev-password` 覆盖）；正式服务请显式指定 `--url ...:7788`，此时才读取 `MACBOT_PASSWORD` 或 `~/.macbot-dev-password`。脚本先轮询无需鉴权的 `/api/v1/health`，再调用协议规定的 `bootstrap({})`，确认响应是 `{ok, result}`、`hello.protocol == 1`，并确认会话列表非空、恰好一个 `is_main` Bot，且主 Bot 的 `dm_chat_id` 出现在会话列表中。服务刚启动时会轮询传输层错误；鉴权、协议和字段错误会立即失败。S0 通过只代表服务端能被协议客户端读取，不能代表后续阶段已经完成。

后续阶段的验收剧本来自 `docs/PLAN.md` 第 6 章和 `docs/DESIGN.md` 第 3 章。执行脚本时必须连接真实服务端，并把客户端画面、RPC 响应和服务端日志作为证据；mock 只能用于检查协议外形和客户端状态机。

| 阶段 | 端到端验收边界（脚本应覆盖） | 通过证据 |
| --- | --- | --- |
| S1 | 桌面和 Android 与真实服务连接；与一个 Bot 私聊；Bot 读写文件、运行命令；轨迹实时查看和回放；服务重启后任务恢复 | 两端截图、`chat.history`、`trace.history`、重启前后任务/消息与日志 |
| S2 | 执行「给 App 加邮箱登录」完整场景：建群、产品→编码→测试交接；两个群并行；运行中插话到达并被读取；待验收确认或提修改意见 | 两端截图、`project.get`、`chat.history`、`assignment.list`、插话 `delivery` 状态 |
| S3 | 两端仪表盘数据一致；技能新增/修改/启停；Bot 跨会话记住用户偏好 | 两端截图、技能和仪表盘 RPC 响应、跨会话消息证据 |
| S4 | 使用 Chrome 登录状态刷 X 并总结；手机接管登录；定时任务按时运行并通知 | 桌面/手机截图、`/ws/screen` 画面、routine/run、通知和日志 |
| S5 | 全新安装：卸载 → 安装 pkg → 管理页设置密码 → 两端连接 → 完整场景 | 安装日志、管理页/两端截图、完整场景 RPC 与日志 |

场景脚本应在响应中验证协议字段和状态迁移，并在断言失败时报告复现方法与证据路径；不要因为 mock 返回了 fixture 就把对应阶段标记为通过。

真实服务上的 API checks：

```sh
python3 scripts/e2e/s1/private_chat.py --url http://127.0.0.1:7788 --bot-id <bot_id>
# 协议fallback心跳：HTTP ping及同一主WS每20秒ping，持续至少60秒，不重连/不调用模型
python3 scripts/e2e/s0/heartbeat.py --url http://127.0.0.1:7788 --json
# 主连接断线补发：只创建自己的 skill，断线后更新并验证同一 cursor 的 skill.updated replay
python3 scripts/e2e/s0/connection_replay.py --url http://127.0.0.1:7788
# 全新生产 Host 只有主 Bot 时，先由场景创建一个独立 worker：
python3 scripts/e2e/s1/private_chat.py --url http://127.0.0.1:7788 --create-worker
# S1 重启恢复是破坏性检查，默认拒绝；确认要 kill -9 时才加显式开关：
python3 scripts/e2e/s1/recovery.py --url http://127.0.0.1:7788 --bot-id <bot_id> --restart-service
python3 scripts/e2e/s2/login_feature.py --product-bot-id <id> --coding-bot-id <id> --test-bot-id <id>
# 在两个已有项目群继续执行（不会 project.create、回答旧问题或 confirm_done）；先只做静态检查，真实运行需单独授权
python3 scripts/e2e/s2/project_followup.py --project-id <project1> --project-id <project2> \
  --product-bot-id <product_id> --coding-bot-id <coding_id> --test-bot-id <test_id> \
  --journal docs/progress/S2/<unique-followup>.json --json
# 未完成的 follow-up 只能用同一 journal 恢复；未知 chat.send 结果会停止且绝不重发
python3 scripts/e2e/s2/project_followup.py --project-id <project1> --project-id <project2> \
  --product-bot-id <product_id> --coding-bot-id <coding_id> --test-bot-id <test_id> \
  --main-run-id <main_run_for_project1> --main-run-id <main_run_for_project2> \
  --resume-journal docs/progress/S2/<unique-followup>.json --json

# 本地严格回归（不连接 Host、不读密码、不写业务状态）
python3 scripts/e2e/s2/project_followup.py --self-test

```

`--main-run-id` 是可选的严格绑定增强；提供时必须是两个项目各自已验证的精确本机 `run_requests/run_*.json` ID。未提供时保留 canonical request 的严格 trigger/parent 图，后续可在 resume 时补入 run ID。脚本只接受
`bot_id=main`、`assignment_id=null` 且 request instruction 与唯一 canonical user fallback/knownText 完全相等的 run。未提供时，脚本从该项目 chat 的完整 trace 中筛选安全格式的 `run.start`，读取精确对应的本机 `run_requests/run_*.json`，匹配必须唯一，再由同 chat trace 的 `assign` tool.end 精确绑定新 root assignment；缺少候选会继续等待，候选歧义、身份冲突、trace 截断或 assign 配对不完整会 STOP/PARTIAL。它不按时间、sender 邻接、marker 或 assignment 邻近关系猜测归属；已成功绑定的项目会先持久化，另一项目未就绪时不会丢失部分证据。显式 `--main-run-id` 始终优先且 resume 时不可被自动发现覆盖。

```sh
python3 scripts/e2e/s3/skills_usage_search.py --bot-id <non_main_bot_id>
python3 scripts/e2e/s4/routines_browser.py --url http://127.0.0.1:7788 --bot-id <bot_id>
# S4 scheduler: creates a near-future cron, waits for trigger=schedule and done;
# it never calls routine.test_run and deletes only its own routine.
python3 scripts/e2e/s4/scheduled_routine.py --url http://127.0.0.1:7788 --bot-id <bot_id>
# S4 screen transport: saves two validated low-quality JPEG frames and ACKs
# each one; add --takeover only to verify start/release without sending input.
python3 scripts/e2e/s4/screen_transport.py --url http://127.0.0.1:7788 --bot-id <bot_id> \
  --output docs/progress/S4/screen-transport-run
# Android 系统通知只读探针；OBSERVED 仅证明系统展示，不计 full S4，默认不清通知/重启 App
python3 scripts/e2e/s4/android_notification_probe.py --marker <新macbot-e2e-marker> \
  --timeout 90 --output docs/progress/S4/<unique-notification-probe>.json
# S3 usage dashboard read-only consistency check; --from/--to must be a closed
# historical RFC 3339 range with an explicit timezone.
python3 scripts/e2e/s3/usage_readonly.py --url http://127.0.0.1:7788 \
  --from 2026-10-09T00:00:00Z --to 2026-10-09T18:00:00Z --json
```

画面传输脚本要求 low 画质帧宽度不超过 640，并保存帧契约失败的原始证据。静态页面可能只产生一帧，若要验收 ACK 后的续帧，应让 Bot 使用会产生动画或页面变化的浏览器会话；脚本不会把单帧静态画面当作两帧通过。

S1 可显式添加 `--approve-test-tools-once`，仅按本次 marker/run/Bot/chat 和完整参数核验后，对该次 write/bash 使用 `allow_once`；recovery 只批准自己的精确 Bash 命令。未知参数、风险不一致或其他任务一律不批准，不修改全局审批规则。旧 `--approve-test-bash-once` 保留为 alias。

S1 要求 Bot 的私聊回复包含测试 marker，并从 `trace.history` 看到成功的 `write`、`read`、`bash` 调用及 `run.end=done`，同时确认 read/bash 的返回内容包含 marker。`s1/recovery.py` 默认不连接、不创建任务、不杀进程；显式 `--restart-service` 后才会核对 `com.macbot.server` 与 7788 的同一 PID，确认 `$MACBOT_HOME/data/jobs/*.json` 中本次 run 的安全 checkpoint，再 kill -9，并等待 KeepAlive 新 PID、同一 `run_id` 的 `run.resume`、文件 marker 和 `run.end=done`。S2 创建两个带唯一 marker 的项目，等待两个项目的任务时间区间实际重叠，再等编码任务处于 `working` 后发送 steer；只有送达状态为 `read`、assignment 的 `steers[].applied_at` 非空且 trace 有 `steer`，才继续等待三个 Bot 的完成交接和项目 `review` 状态。默认只到 review；只有显式传入 `--confirm-projects` 才调用 `project.confirm_done`，应在真实产物验收后启用。S3 完整验证用户 skill 的 create/get/update/disable/enable/delete、usage 三种查询、带 marker 的 search，以及从非主 Bot 私聊写入偏好后在主 Bot 私聊读取偏好的跨会话行为。S4 API 脚本只创建并删除自己的 routine，验证 `routine.test_run` 产生的实际 assignment、运行完成和 `trace.history`；`/ws/screen` 画面、接管输入、Chrome 登录状态和 Android 通知仍需手动证据。

这些命令会在 Host 上创建带 `macbot-e2e-*` 前缀的测试项目和消息；S0 主连接场景和 S3 都只删除各自创建的 skill。脚本的 PASS 只表示 API/WebSocket checks 通过，不能替代桌面/Android UI、流式显示、截图、通知、浏览器接管或全新安装演练。

`s3/usage_readonly.py` 只调用 `usage.summary`、`usage.heatmap`、`usage.timeseries` 和 `usage.breakdown`，不会创建任务或技能。它要求范围已结束且 `summary.current.requests > 0`；无请求时输出 `EMPTY`，不算通过。结果中的 `status=PASS` 只代表 API 局部检查，`full_s3_pass` 始终为 `false`；`cost` 遵循 `Money` 的未知值语义，允许明细中已知和未知成本混合，并使用容差比较已知汇总。`timeseries top=100` 达到上限时输出 `PARTIAL`、`api_local_pass=false`，不强判完整汇总。

后台回归 `s1/background.py` 默认写唯一文件，`--evidence` 可指定新路径；存在时拒绝覆盖。S2 新测用 `--partial-output <新文件>`，续接用 `--resume-partial <原文件>`；主请求 UUID 在 RPC 前保存，未知结果不重发。审批前核唯一 map/run/checkpoint 与已验证项目 Home；`--confirm-projects` 默认关闭，待真实产物验收后再启用。
