# S5 客户端配置隔离与恢复计划

状态：执行前只读方案；本次未移动文件、未改配置、未发送 RPC。现有 backup 根为：

`~/Library/Application Support/MacBot/S5-backup-20261010-gcdb9tuk`

该目录位于 `~/Library/Application Support/MacBot` 内，因此不能把整个 `MacBot` 配置目录移动到这个 backup 根。只对已确认的文件和子目录做分项、可逆移动，保留父目录和 backup 根本身。

## macOS 桌面端

源码把未加密客户端数据根固定为 `~/Library/Application Support/MacBot`，或由绝对路径环境变量 `MACBOT_CLIENT_DATA_DIR` 覆盖（[storage_paths.rs:10-26](../../../clients/mac/crates/macbot-desktop/src/storage_paths.rs#L10)）。当前现场已确认父目录为 `0700`；已存在的普通文件为 `0600`。

Fresh 前退出桌面 App，并只移动下列已知路径到 `$BACKUP/desktop-client/`，不存在的项目跳过：

| 源路径 | 内容/恢复说明 |
| --- | --- |
| `hosts.json`、`hosts.lock` | Host 元数据、`node_id`、地址、游标；实现见 [host_storage.rs:41-58](../../../clients/mac/crates/macbot-desktop/src/host_storage.rs#L41)；密码不在 JSON 内 |
| `development-secrets.json` | 仅在显式 `MACBOT_SECRET_BACKEND=file` 时使用的开发期外部凭据映射；保持 `0600`，不读取、不复制内容 |
| `local-settings.json` | 桌面主题、通知和开机启动偏好；实现见 [local_settings.rs:29-54](../../../clients/mac/crates/macbot-desktop/src/local_settings.rs#L29) |
| `update.json`、`updates/` | 更新源配置与下载缓存；实现见 [update.rs:26,330,591-596](../../../clients/mac/crates/macbot-desktop/src/update.rs#L26) |
| `cache/` | 每个 Host 的 bootstrap UI 快照及 `.lock`；实现见 [state_cache.rs:38-54](../../../clients/mac/crates/macbot-desktop/src/state_cache.rs#L38) |
| `outbox/` | 每个 Host 的待发 `chat.send` 队列；实现见 [outbox.rs:30-50](../../../clients/mac/crates/macbot-desktop/src/outbox.rs#L30) |

不要移动 `~/Library/Application Support/MacBot` 父目录、`S5-backup-20261010-gcdb9tuk` 自身或未确认用途的 `release-candidates/`。不要导出、删除或改名 macOS Keychain 中 service `bot.mac.desktop.host-password` 的项目；Host 密码默认由 [host_storage.rs:3-7,200-204](../../../clients/mac/crates/macbot-desktop/src/host_storage.rs#L3) 存在 Keychain，只有显式 file backend 才使用上述 `development-secrets.json`。

恢复时先关闭新桌面 App，把 fresh 期间新生成的同名文件/目录移到 backup 的 `desktop-fresh/` 留证，再将 `desktop-client/` 中的原项目移回原路径并核对 `0700/0600`。这样旧 `hosts.json` 的 Host ID 能重新访问原 Keychain 项；fresh 产生的缓存和 outbox 不会混入旧 Host。整个操作应在 watcher 和相关 LaunchAgent 已停止后进行。

## Android 模拟器

当前包名为 `bot.mac.mobile`，只读 `adb` 信息显示应用数据根为 `/data/user/0/bot.mac.mobile`。不要 `pm clear`、卸载或删除应用数据；现有 `install -r` 继续保留数据。

应用把 Host 列表和游标写入 SharedPreferences `macbot_state`（[AndroidPlatform.kt:24-26,112-130](../../../clients/mobile/shared/src/androidMain/kotlin/bot/mac/mobile/core/platform/AndroidPlatform.kt#L24)），凭据写入 `macbot_credentials` 并用 Android Keystore alias `macbot.credentials.v1` 加密（[AndroidPlatform.kt:63-108](../../../clients/mobile/shared/src/androidMain/kotlin/bot/mac/mobile/core/platform/AndroidPlatform.kt#L63)）。每个 Host 的状态键为 `snapshot:<hostId>`、`last_seq:<hostId>`，另有 `selected_host` 和 `device_id`（[ClientRepository.kt:416-432](../../../clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile/core/state/ClientRepository.kt#L416)）；通知 ledger 使用 `macbot_notification_ledger`（[MacBotNotifications.kt:218-229](../../../clients/mobile/androidApp/src/main/kotlin/bot/mac/mobile/MacBotNotifications.kt#L218)）。这些文件属于应用私有数据，不能通过 adb 导出或清空。

Fresh 连接使用 Android Connect UI 新增 Host，建议名称使用新的明确名称（例如 `S5 fresh 20261010`），地址 `10.0.2.2:7788`，使用已在管理页设置的新访问密码，可按用户已有授权由 agent 在登录页输入；保留旧 Host 和旧名称，不编辑旧 Host。`saveHost(existingId = null)` 会生成新的本地 Host ID（[ClientRepository.kt:104-116](../../../clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile/core/state/ClientRepository.kt#L104)），因此新 Host 的快照、游标、凭据和通知 ledger 与旧 Host 分离。

Fresh server 的 `node_id` 改变时，新 Host 首次 hello 会记录新值；客户端对同一 Host 会拒绝不一致的 `node_id`（[ClientRepository.kt:387-394](../../../clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile/core/state/ClientRepository.kt#L387)）。因此不能把旧 Host 改指向 fresh server，也不能复用旧 Host ID；应选择新 Host 完成验收，旧 Host 留作回退入口。验收失败时只切回旧 Host，保留 fresh Host 和现场证据，不清除 Android 数据。

## 执行与恢复边界

1. 只读记录 backup 根权限、客户端父目录权限、已确认条目名称/权限和 Android `dataDir`；不读取任何 JSON、SharedPreferences、Keychain、Keystore 或密码值。
2. 停止桌面 App/watcher 后，逐项移动桌面文件；每项记录源、目标、权限和是否存在，禁止把父目录整体移动进自身。
3. 安装/启动 fresh server 后，桌面使用清空后的客户端文件集合；Android 通过新 Host 连接，旧 Host 保留。
4. 回退时按桌面 `desktop-fresh` → `desktop-client` 的顺序恢复；Android 仅选择旧 Host，不执行 `pm clear`、卸载或凭据删除。

