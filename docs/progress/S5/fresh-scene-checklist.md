# S5 fresh 场景最短执行清单

状态：执行前清单，不是通过记录。只有安装、初始化、两端连接和真实 UI 场景全部完成，才可把 S5 标为通过。最终 server SHA/pkg 尚未冻结；6ba7efe 仅为中间审计包，question P1 修复并通过 runtime 实测前不得开始。

## 1. 冻结与备份

1. 等待 server 修复后冻结一组完整产物：最终 server pkg、desktop `ba0e8f17b5dfd779efc5cd4b41c7168a57d70b72` DMG、Android 同签名 Release APK；核对各自 source SHA、`dirty=false`、SHA-256 和包内 daemon/sidecar。6ba7efe 审计见 [server-pkg-6ba7efe-verified.json](server-pkg-6ba7efe-verified.json)，但不作为最终包；桌面候选见 [desktop-dmg-ba0e8f1-candidate.json](desktop-dmg-ba0e8f1-candidate.json)。
2. 暂停 watcher，记录 `scripts/dev/status.sh`、7788/7789 listener、LaunchAgent PID/实际 binary、桌面和 Android 版本。停止并确认 7788 已释放；不批量取消旧 approval/job，不触碰未知进程。
3. 复用 [fresh-install-journal.json](fresh-install-journal.json) 中现有的 `backup_root`（当前 `prepared_empty_only`），确认分类目录没有业务文件后再移动 `~/MacBot`、LaunchAgent、pkg 安装 App、开发 App/CLI links 和两端连接配置。外部 file secret 目录和 `~/.macbot-dev-password` 只记录路径/权限，不读取、复制、截图或写入文档。

## 2. 安装与初始化

1. 按 [fresh-install-plan.md](fresh-install-plan.md) 卸载旧 pkg App/LaunchAgent，区分 `~/Applications/MacBotServer.app` 与 `/Applications/MacBot Server.app`；确认 7788 关闭后再安装。
2. 用户在 Installer GUI 或 `sudo installer -pkg <固定pkg> -target /` 处完成管理员认证；agent 不代输管理员密码。安装后核对 `/Applications/MacBot Server.app`、`~/Applications/MacBot Server.app`、LaunchAgent、source stamp、daemon/sidecar hash，确认 LaunchAgent PID 与 7788 listener PID 相同。
3. 使用干净 `~/MacBot` 启动服务，先只读确认 `/api/v1/health` 返回 `setup_required=true`。用户按指定测试密码在 `/admin` 页面手动输入、确认和提交；agent 不代填，具体值不进入命令行、日志、截图或提交。随后用正式客户端完成 bootstrap，保留管理页和两端连接截图。
4. 按 runbook 配置开发期 file backend 和仓库外 secret dir，恢复已授权的 MiniMax CN provider/default model；只输入已有凭据，不打印或重新索要 key。若配置结果不明，停止在只读核验，不重复创建 provider。

## 3. 两端连接门槛

- 桌面：安装固定 DMG，打开正式 `.app`，连接 `127.0.0.1:7788`；观察连接页、会话列表、主 Bot 私聊和断线重连后的消息/事件渲染，截图实际窗口。
- Android：`adb -s emulator-5554 install -r <签名APK>`，保留应用数据；连接 `10.0.2.2:7788`，观察连接页、会话列表、主 Bot 私聊、流式正文和轨迹页，截图实际模拟器画面。
- `bootstrap.py`、health 或任何 API 返回只能作为协议基线，不能替代上述两端 UI 观察。

## 4. 完整场景顺序

所有脚本使用 Python 3 标准库，正式服务显式传 `--url http://127.0.0.1:7788`，输出写入新的 `docs/progress/S5/` 证据文件；RPC 前先保存 UUID/journal，结果不明绝不换 UUID 重发。

1. **S1 单 Bot**：
   ```sh
   python3 scripts/e2e/s0/bootstrap.py --url http://127.0.0.1:7788 --json
   python3 scripts/e2e/s1/private_chat.py --url http://127.0.0.1:7788 --create-worker --json
   ```
   在桌面和 Android 同时观察私聊流式消息、`write/read/bash` 消息块、实时 trace 和历史回放；确认 marker 文件、回复、`run.end=done`。恢复场景在确认本次任务和 checkpoint 后单独执行：
   ```sh
   python3 scripts/e2e/s1/recovery.py --url http://127.0.0.1:7788 --create-worker --restart-service --json
   ```
   该参数会 kill 已核验的正式 7788 LaunchAgent PID，只能用于本次明确的恢复验收；验收点是同一 `run_id` 的 `run.resume`、文件 marker、消息和最终 done。

2. **S2 主 Bot/双群（fresh clean home 重新执行；历史旧 pending 不续办）**：先在客户端原生 UI 创建并核对 product/coding/test Bot，再把三个精确 Bot ID 传给：
   ```sh
   python3 scripts/e2e/s2/login_feature.py --url http://127.0.0.1:7788 \
     --product-bot-id <product> --coding-bot-id <coding> --test-bot-id <test> \
     --partial-output docs/progress/S5/s2-login-partial.json --json
   ```
   两个群的时间区间必须实际重叠；桌面和 Android 观察公告、任务卡片、运行中插话、送达 `read`、assignment steer、交接和待验收。只有真实产物检查完成后才允许追加 `--confirm-projects`；不要用 `project.create` 或 API 状态代替主 Bot 理解和 UI 证据。中断后只用同一 partial journal `--resume-partial` 恢复。

3. **S3 技能/仪表盘/记忆**：
   ```sh
   python3 scripts/e2e/s3/skills_usage_search.py --url http://127.0.0.1:7788 --bot-id <non-main> --json
   python3 scripts/e2e/s3/usage_readonly.py --url http://127.0.0.1:7788 \
     --from <closed-rfc3339-start> --to <closed-rfc3339-end> --json
   ```
   在两端分别打开仪表盘、技能页、搜索页，核对同一固定 UTC 范围、技能新增/编辑/停用/启用/删除和搜索 marker；从非主 Bot 写入偏好，再在主 Bot 私聊读取，两端都看见真实消息和结果。usage 脚本只算 API 局部检查。

4. **S4 浏览器/定时/通知**：API 部分仅用于自己的 routine 和 scheduler：
   ```sh
   python3 scripts/e2e/s4/routines_browser.py --url http://127.0.0.1:7788 --bot-id <bot> --json
   python3 scripts/e2e/s4/scheduled_routine.py --url http://127.0.0.1:7788 --bot-id <bot> --json
   python3 scripts/e2e/s4/screen_transport.py --url http://127.0.0.1:7788 --bot-id <bot> \
     --output docs/progress/S5/screen-transport --takeover --json
   python3 scripts/e2e/s4/android_notification_probe.py --serial emulator-5554 \
     --marker <unique-marker> --output docs/progress/S5/notification.json
   ```
   另需人工在已登录 Chrome 中只读打开 X、观察真实画面；Android 观察接管、只读滑动、交还后按钮消失、同一 run/message 的状态更新和通知容量。不得发帖、点赞、关注或改账号；不得把 transport/API probe 当成 Chrome 登录、画面绘制或 Android UI 通过。question 或 takeover pending block 未清除时停止，不进入 S5 通过结论。

5. **S5 现场收尾**：保留安装日志、管理页首次设置截图、桌面/Android 连接与完整场景截图、RPC/journal、最终 PID/source stamp 和备份路径。任何一端只显示健康/API 成功而没有实际 UI/截图，均不计完整场景通过。

## 5. Approval 边界与失败处理

- 只批准当前 Bot、当前 chat/project、当前 run、精确 args/effective target/map/checkpoint 的一次性 approval。`--approve-test-tools-once` 仅在已核对脚本 marker 和参数后使用；不批准全局 Bash、subagent、memory 或宽泛 browser 权限。
- 不确定 approval 的 args、target、run/job/checkpoint 或 map 时保持 pending，先读 trace/history/journal；不要套用旧授权。`chat.send`、approval 或安装结果不明时只读核验，禁止换 UUID 重发。
- 任一安装、初始化或场景失败先保存现场和原始证据，停止新 LaunchAgent，按 [fresh-install-plan.md](fresh-install-plan.md) 从仓库外备份恢复；不清空 Android 数据、不删除密钥、不覆盖原始 journal。S5 只在所有门槛满足后记录通过。
