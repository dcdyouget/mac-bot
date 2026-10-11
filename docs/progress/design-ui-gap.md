# 新版设计改造清单（开发中）

设计源为主仓库未跟踪的 `design/`，只读；本分支基于 `a83892e`，不包含主仓库未提交的文档改动。界面以 `design/` 为准，协议以主仓库当前 `docs/PROTOCOL.md` 为准。

| 设计页面或状态 | 现有实现 | 本轮缺口 | 负责人 |
| --- | --- | --- | --- |
| macOS 三栏、会话、群、跟进、工作台、Bot、技能、定时、仪表盘、搜索、设置、Computer、连接 | `clients/mac/crates/macbot-desktop/src/` 现有页面及状态 | 对齐 55 个原型屏幕的布局、三主题、文案、焦点/快捷键、空载错断线与操作 | macOS agent |
| Android 消息、聊天、群、跟进、工作台、Bot、技能、定时、仪表盘、搜索、设置、Computer、连接 | `clients/mobile/shared/src/commonMain/.../feature/` 现有页面 | 对齐 40 组原型屏幕和移动手势、三主题、状态/平台反馈 | Android agent |
| 消息块目录及状态 | 两端已有基础渲染 | 按 `06-message-blocks.md` 补引用、多题/多选、审批、验收、接管、未知块等状态与真实操作 | macOS、Android agent |
| Web 管理页 | `server/crates/macbot-gateway` 内置 `/admin` | 对齐 `05-web-admin.md` 的初设、状态、日志、设置及响应式布局 | Web admin agent |
| 公告自由文本及编辑、引用、问题多选、群卡片、链接预览 | 旧协议及服务端部分能力 | 同步 `PROTOCOL.md`、Rust/schema/fixtures/Kotlin、RPC、存储、事件和双端模型 | server/protocol agent；Kotlin 由 Android agent |
| 真实管理页指标与日志 | `/admin/status`、`/admin/logs` 基础输出 | 增补可核实的状态字段和日志查询 | server/protocol agent |

本轮只编译、类型检查、格式和静态检查；测试、UI 验收、安装、部署、服务重启均由用户负责。

## 设计差异与补充文案

- 旧 `docs/DESIGN.md` 第 2 章视觉数值改用 `design/tokens.json` 和 `design/prototype/base.css`；旧线程式回复改为主消息流内引用。`design/` 保持只读。
- 原协议草案只有 `chat.send.quote`，无法区分「回复原 Bot」与「仅引用」。新增兼容 `quote_mode: reply | reference`，缺省为 `reply`；纯引用不自动路由给原 Bot。
- 定时任务详情要求修改执行 Bot，兼容扩展 `routine.update.patch.bot_id`。
- Web 管理页必要文案补充 key：`admin.metrics.aria`、`admin.duration.{days_hours,hours_minutes,minutes}`、`admin.error.{host_name,port,not_ready,request,unauthorized}`、`admin.copy.success`、`admin.setup.{password,password_suffix}`、`admin.setup.error.{mismatch,required,save}`、`admin.theme.{dark,light,toggle_aria}`、`admin.kpi.{clients_note,restarting,unavailable,bots_note,browser_sessions,browser_version}`、`admin.field.{host_name,port,node_id,password}`、`admin.copy_node_id`、`admin.saved`、`admin.restart.{requested,failed,done}`、`admin.port_dialog.{title,copy,error}`、`admin.password.{current,new,mismatch,changed,error}`、`admin.logs.{filter_aria,empty,load_error}`。
- 管理页使用 HTTP Basic Auth，浏览器缓存认证凭据，页面无法可靠执行“退出”。右上角改为 `admin.auth.help`“认证说明”，解释浏览器凭据管理及修改密码入口；不显示虚假的登出成功状态。
