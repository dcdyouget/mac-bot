# Batch 04 屏幕验收记录

验收环境：正式 APK `androidApp-debug.apk`，SHA-256 `247cb7e1279a1f09a921f1f751c2bdd713a093300ea7d8995ad2523022778b6c`；AVD `macbot_api36` / `emulator-5562`；隔离 mock `10.0.2.2:17933`，名称 `Batch04`，密码 `dev`，数据目录 `/tmp/macbot-mobile-batch04-20261012`。未使用 `emulator-5558`。浅色/深色通过系统夜间模式切换。截图仅保留在本目录。

| ID | 结果 | 实测记录 |
|---|---|---|
| `connect` 首次连接 | **pass** | 清空 App 数据后连接独立 mock，选中模板进入群聊；浅、深模式均留有首次连接/已连接截图。连接地址、模板选择和 mock 身份可从截图追溯。 |
| `offline` 断线重连 | **pass** | 连接后停止 mock，界面出现“正在重连…”状态；重启同一 mock 后约 18 秒恢复群聊和列表数据。浅、深断线与恢复截图均已留存。 |
| `settings` 外观与通知 | **partial** | 外观与通知页浅、深截图；四类通知开关操作后强制停止并重启，状态保持，随后恢复初始值。半透明开关打开后页面仍是不透明材质，提示 Android 当前只支持跨窗口模糊，不能模糊本应用窗口；因此真实半透明验收 **fail**，不计为通过。 |
| `hostswitch` 切换 Host | **pass** | 从顶栏打开 Host sheet，显示 `Batch04`（`10.0.2.2:17933`，已连接）和 `Offline`（`10.0.2.2:17934`）。切到 Offline 后顶栏变成 Offline 并进入重连；再选 Batch04 后回到已连接群聊。留有浅、深 sheet 与离线/在线状态截图。 |
| `annedit` 编辑公告 | **pass** | 群聊右上菜单 → 编辑公告，输入 Markdown 文本并保存；公告编辑器版本由 0 增至 1，内容保留。另有浅、深编辑器截图。 |
| `swipe` 左滑会话 | **partial** | 消息列表的群行左滑可显示「取消置顶 / 标记完成 / 取消静音」动作；点「标记完成」后列表出现「已完成」分区。未把置顶、完成、静音三项逐一做可逆操作并验证重启后的持久化，故不判全通过。浅、深动作菜单及操作后截图已留存。 |
| `longpress` 长按消息 | **fail** | 在 `官网改版` 群的消息卡片上长按约 1.5–2 秒，浅、深模式均未出现表情、回复/引用、复制、公告或工作详情菜单。消息本身可见，预期交互未触发。 |

## 截图索引

- 首次连接与恢复：`connect-template-{light,dark}.png`、`connect-home-{light,dark}.png`、`connect-template-selected-light.png`、`offline-{light,dark}.png`、`reconnected-{light,dark}.png`
- 设置与半透明：`settings-{light,dark}.png`、`settings-translucent-dark.png`、`settings-notif-*.png`、`settings-persisted-dark.png`、`settings-restored-dark.png`
- Host 切换：`hostswitch-{light,dark}.png`、`hostswitch-offline-dark.png`、`hostswitch-online-dark.png`、`host-offline-connect-attempt-dark.png`
- 公告编辑：`annedit-{light,dark}.png`、`annedit-save-light.png`
- 左滑与长按：`swipe-{light,dark}.png`、`swipe-completed-dark.png`、`swipe-muted-dark.png`、`longpress-{light,dark}.png`

说明：本批是实际 mock 交互验收；Android 同应用窗口的真实背景模糊仍未实现。半透明状态的说明和截图不能作为 blur 通过证据。
