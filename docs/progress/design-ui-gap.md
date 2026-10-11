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

## 本轮交付与检查

- macOS：沿用既有 GPUI 客户端，改造三栏与窄窗、连接/断线、群聊/跟进、消息块及问答/审批/接管状态、工作台、Bot 详情与设置、技能草稿、定时任务、仪表盘、搜索、Computer、远程 Home 浏览和产物版本预览；接入浅色/深色/半透明 token、设计文案、菜单栏常驻、唯一提示音与消息深链。编译命令：`cargo check --manifest-path clients/mac/Cargo.toml -p macbot-desktop --locked`，结果通过。
- Android：沿用 Compose 客户端，改造消息/群信息/跟进、Bot、定时任务、产物版本、问答/审批/接管、主题、通知、字号上限、读屏与焦点；设计 07 的 485 个 key 已入资源，适用页面用规范 key，唯一提示音使用设计原始 m4a。编译命令：在 `clients/mobile/` 运行 `./gradlew :androidApp:assembleDebug`，结果通过。
- 服务端/协议：公告版本更新、引用模式、多选/过期问题、链接预览真实生产入口、定时任务更换 Bot、待看消息插话、项目产物快照与历史、失败任务幂等重试、默认工具和审批能力标记；`docs/PROTOCOL.md`、Rust、Schema、fixtures、Kotlin 模型同步。管理页对接真实状态/日志/密码接口。`cargo check --manifest-path server/Cargo.toml -p macbot-gateway` 及 `-p macbot-orchestrator` 通过；管理页内联 JS 语法与 token CSS 源文件一致性检查通过。
- 本轮**未执行任何单元测试、集成测试、E2E、UI 验收、安装、部署或服务重启**。编译与静态检查仅排除基础编码错误，不代表产品验收通过。

### 明确限制与待决定项

- Auto Review “严格/标准/宽松”的风险阈值在新版设计中未定义，现有 `approvals.mode` 也不等同三档。已向用户请求产品决定；在明确前不启用猜测的自动放行。
- Android 同一 Activity 内未实现可等价采样背景的 backdrop blur，半透明主题按 token 规则退回对应不透明主题；CSS 多层阴影以 Compose 单层 elevation 近似。macOS GPUI 仅提供整窗系统材质，无法逐个表面精确控制 blur/saturate，也不能独立设置标题栏高度。
- 产物历史只从新登记文件开始记录：仅项目 Home 内不超过 5 MiB 的普通文件，敏感/越界路径、URL 与目录不建快照；旧产物不回填。远程 Home 通过 Host 文件 API 浏览与复制路径，无法在客户端本机 Finder 打开远程目录。
- 消息深链最多向前查找 20 页（2,000 条），超出上限会提示；跨 Host 链接必须匹配 `node_id`。管理页 HTTP Basic Auth 凭据由浏览器管理，网页无法可靠登出。

### 用户手工检查建议（本轮未执行）

1. 按三端 README 在隔离环境启动各组件，核对浅色/深色/半透明、窄窗和离线/重连状态。
2. 在真实 Host 上走群聊：引用/纯引用、多选/自由回答、公告并发冲突、待看消息插话、审批三态、失败任务重试、产物预览/历史。
3. 检查 Mac 菜单栏隐藏/重开、深链定位、通知声音；检查 Android 读屏/字号/系统减弱动效与接管画面；检查 `/admin` 初设、状态、日志、密码修改与端口提示。
