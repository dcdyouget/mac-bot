# Android 群头像独立验收

日期：2026-10-16。验证范围仅限 `clients/mobile` 的群头像组件和入口；未修改源码。

## 构建与测试

- 构建命令：`./gradlew -PmacbotNotificationCapacityIsolated=true :androidApp:assembleDebug`，`BUILD SUCCESSFUL`（59 tasks，1 executed、58 up-to-date）。
- APK：`clients/mobile/androidApp/build/outputs/apk/debug/androidApp-debug.apk`，应用包为隔离包 `bot.mac.mobile.capacitytest`；SHA-256 见 `apk.sha256`。
- 定向测试：`GroupMemberLogicTest` 5 项通过，0 failures / 0 errors；JUnit XML 为 `GroupMemberLogicTest.xml`。

## 实机界面证据

所有图片及对应 UI dump 位于本目录。

- `01-list.png`：群消息列表中的双人、三人、四人、五人群均显示合成头像，Main 眼镜处于前层。该图使用创建的隔离 QA 群数据。
- `02-five-chat.png`、`04-five-info.png`：五人群聊天顶栏和群信息显示一致的三头像加 `+2` 布局；Main 眼镜可见。使用较早隔离 mock binary SHA `fdc5f12a…` 创建的 QA 群，仅用于验证五人布局和入口渲染。
- `09-list-current.png`、`10-website-chat-header.png`、`12-website-info.png`：切换到最新隔离 mock binary SHA `e1296de8…` 并重新启动 app 后重新验证。连接日志依次出现 `websocket_open`、`hello`、`session_resume`、`sync_done`、`ready`。列表和聊天顶栏、群信息均显示“官网改版”双人头像（Main + code），与 `protocol/fixtures/scenarios/login-feature.jsonl` 中该 chat 的 `member_bot_ids=[bot_main,bot_code]` 一致。
- `05`–`10` 中名称含 `pre-m08` 的截图属于前一包，只作布局历史记录，不作为最终入口验收证据。

## 限制

Followup 页面本轮未实际进入，因此未覆盖 Followup 入口。五人 QA 群截图来自较早的隔离 mock binary；最新 mock 连接后的官方 fixture 页面另有端到端截图。模拟环境验证的是客户端展示和成员来源，不证明真实服务端调度行为。

构建与测试 XML、APK、截图和 UI dump 均留在 `/tmp/macbot-audit-20261016/final-m08/`。本轮启动的隔离 AVD 和 mock 进程已停止；正式 `bot.mac.mobile` 数据与默认模拟器未操作。
