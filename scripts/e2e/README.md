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
# 主连接断线补发：只创建自己的 skill，断线后更新并验证同一 cursor 的 skill.updated replay
python3 scripts/e2e/s0/connection_replay.py --url http://127.0.0.1:7788
# 全新生产 Host 只有主 Bot 时，先由场景创建一个独立 worker：
python3 scripts/e2e/s1/private_chat.py --url http://127.0.0.1:7788 --create-worker
# S1 重启恢复是破坏性检查，默认拒绝；确认要 kill -9 时才加显式开关：
python3 scripts/e2e/s1/recovery.py --url http://127.0.0.1:7788 --bot-id <bot_id> --restart-service
python3 scripts/e2e/s2/login_feature.py --product-bot-id <id> --coding-bot-id <id> --test-bot-id <id>
python3 scripts/e2e/s3/skills_usage_search.py --bot-id <non_main_bot_id>
python3 scripts/e2e/s4/routines_browser.py --url http://127.0.0.1:7788 --bot-id <bot_id>
# S4 scheduler: creates a near-future cron, waits for trigger=schedule and done;
# it never calls routine.test_run and deletes only its own routine.
python3 scripts/e2e/s4/scheduled_routine.py --url http://127.0.0.1:7788 --bot-id <bot_id>
# S4 screen transport: saves two validated low-quality JPEG frames and ACKs
# each one; add --takeover only to verify start/release without sending input.
python3 scripts/e2e/s4/screen_transport.py --url http://127.0.0.1:7788 --bot-id <bot_id> \
  --output docs/progress/S4/screen-transport-run
```

画面传输脚本要求 low 画质帧宽度不超过 640，并保存帧契约失败的原始证据。静态页面可能只产生一帧，若要验收 ACK 后的续帧，应让 Bot 使用会产生动画或页面变化的浏览器会话；脚本不会把单帧静态画面当作两帧通过。

S1 可显式添加 `--approve-test-tools-once`，仅按本次 marker/run/Bot/chat 和完整参数核验后，对该次 write/bash 使用 `allow_once`；recovery 只批准自己的精确 Bash 命令。未知参数、风险不一致或其他任务一律不批准，不修改全局审批规则。旧 `--approve-test-bash-once` 保留为 alias。

S1 要求 Bot 的私聊回复包含测试 marker，并从 `trace.history` 看到成功的 `write`、`read`、`bash` 调用及 `run.end=done`，同时确认 read/bash 的返回内容包含 marker。`s1/recovery.py` 默认不连接、不创建任务、不杀进程；显式 `--restart-service` 后才会核对 `com.macbot.server` 与 7788 的同一 PID，确认 `$MACBOT_HOME/data/jobs/*.json` 中本次 run 的安全 checkpoint，再 kill -9，并等待 KeepAlive 新 PID、同一 `run_id` 的 `run.resume`、文件 marker 和 `run.end=done`。S2 创建两个带唯一 marker 的项目，等待两个项目的任务时间区间实际重叠，再等编码任务处于 `working` 后发送 steer；只有送达状态为 `read`、assignment 的 `steers[].applied_at` 非空且 trace 有 `steer`，才继续等待三个 Bot 的完成交接和项目 `review` 状态。脚本不会调用 `project.confirm_done` 伪造验收。S3 完整验证用户 skill 的 create/get/update/disable/enable/delete、usage 三种查询、带 marker 的 search，以及从非主 Bot 私聊写入偏好后在主 Bot 私聊读取偏好的跨会话行为。S4 API 脚本只创建并删除自己的 routine，验证 `routine.test_run` 产生的实际 assignment、运行完成和 `trace.history`；`/ws/screen` 画面、接管输入、Chrome 登录状态和 Android 通知仍需手动证据。

这些命令会在 Host 上创建带 `macbot-e2e-*` 前缀的测试项目和消息；S0 主连接场景和 S3 都只删除各自创建的 skill。脚本的 PASS 只表示 API/WebSocket checks 通过，不能替代桌面/Android UI、流式显示、截图、通知、浏览器接管或全新安装演练。
