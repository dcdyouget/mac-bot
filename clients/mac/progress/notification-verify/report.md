# macOS 后台通知送达验证

日期：2026-10-12。正式打包脚本生成的 `MacBot.app`，bundle ID 与代码签名 identifier 均为 `bot.mac.desktop`；隔离 mock 监听 `127.0.0.1:17791`，客户端数据使用 `/tmp/macbot-notification-fix-client`。

## 结果

- 根因是 Tauri 默认 ad-hoc executable identifier 会随构建位置改变，例如 `macbot_desktop-2a61eb57deb9c557`。系统设置中同名 Mac Bot 已允许通知，但这个新身份的 `UNNotificationSettings` 为 `NotDetermined (0)`、alert/sound/Notification Center 均为 `NotSupported (0)`；`requestAuthorization` 返回 `UNErrorDomain 1`（`notificationsNotAllowed`）。
- `clients/mac/packaging/package-tauri.sh` 现在从 `Info.plist` 读取 bundle identifier，并在未配置开发者签名时显式以该 identifier ad-hoc 签名。重新打包后 executable identifier 稳定为 `bot.mac.desktop`。
- 新打包 `.app` 连到隔离 mock；客户端切到 Finder 后触发一条 Bot 私聊 DM。诊断确认 `application_active=false`、`show_system=true`；`UNAuthorizationStatus=Authorized (2)`、alert/Notification Center 为 Enabled (2)，`addNotificationRequest` completion 无错误，随后 delivered 查询 `count=2 matching=true`。测试声效被 macOS「播放通知声音」系统开关关闭；视觉送达不受影响。
- App 设置中的「开启系统通知」为开启；macOS 通知设置允许 Mac Bot 显示桌面、通知中心和锁定屏幕通知，提醒样式为临时。

## 证据

- [diagnostic.log](diagnostic.log)：完整隔离测试回调，仅包含事件标识和状态，不含消息正文。
- [system-settings-window.png](system-settings-window.png)：macOS 通知设置中 Mac Bot 权限状态截图。
- 打包产物位于本 worktree 的 `clients/mac/dist/MacBot.app` 和 `MacBot.dmg`；`codesign --verify --deep --strict` 通过。

验证：`cargo test --workspace --locked --offline`（Tauri，56 项通过）；`cargo clippy --workspace --locked --offline -- -D warnings` 通过；`npm run build`（含 `tsc -b`）通过；`npm run lint` 已在前一轮验证通过；完整 `clients/mac/packaging/package-tauri.sh` 成功。
