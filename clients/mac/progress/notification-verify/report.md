# Tauri 原生通知探针

日期：2026-10-12。打包版 `bot.mac.desktop`，隔离 mock `127.0.0.1:17791`，客户端数据根 `/tmp/macbot-notification-probe-client`。

## 结果

- seq 71 的 Bot 私聊事件进入处理链：`message.created`，`seq=71 > old_seq=70`；`chat_code`、`bot_code`、稳定消息 ID、Host ID 均有效；分类为 `DirectMessage`，当前会话是 `chat_main`，窗口失焦，候选通过并调用 UserNotifications。
- 调用前 `UNAuthorizationStatus=NotDetermined (0)`。系统设置中已有 `Mac Bot` 应用条目，但「允许通知」为关闭；不是“列表未注册”。打开该开关后，授权回调为 `granted=true, error=none`，`addNotificationRequest` completion 为 `error=none`。
- seq 72 在授权开启后再次通过同一候选链；调用前状态为 `Authorized (2)`，add completion 仍为 `error=none`。重启隔离 App 查询 delivered notifications 得到 `count=0`；当时观察到没有 Mac Bot 消息横幅。
- 专注模式当前是否启用无法从本次可访问的系统设置状态确认，因此不归因于 Focus。授权后请求被系统 API 接受，但本次没有可见横幅/通知中心条目。原始“未出现通知”现象的可证实前置阻塞是 macOS 的 Mac Bot 通知总开关关闭；本次不构成 TCC 证据。
- `design/01-foundations.md` §11 规定：App 前台但消息不在当前会话时播放应用提示音；当前会话不响。系统通知只在 App 非前台弹出。保留现有 `show_system = !window_focused`，无产品代码修改。

## 证据

- [system-settings-window.png](system-settings-window.png)：仅保留 macOS「通知」设置窗口；Mac Bot 通知已开启，桌面、通知中心、锁屏均允许，提醒样式为临时。
- 未保留横幅缺失截图：原图包含与本次验证无关的桌面内容；“未见横幅”的观察由探针日志中的授权后 add 成功和 delivered count=0 记录支持。
- [diagnostic.log](diagnostic.log)：事件序号、分类、窗口状态、授权状态、授权回调和添加结果；不含消息正文。

验证构建：Tauri `cargo check --workspace --locked` 通过；带临时诊断的隔离 `.app`/`.dmg` 打包成功。诊断改动只在 `/private/tmp/macbot-notification-probe`，未合入。
