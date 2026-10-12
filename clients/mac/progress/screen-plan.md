# Tauri 桌面逐屏 GUI 验收计划

范围：`design/prototype/mac.html` 的 58 个屏幕。当前记录为 6 verified、12 partial、40 not_verified；下面把 52 个未充分验收状态拆成 5 个互不重叠批次。每个屏幕先沿原生 UI 导航进入，再按其交互手动触发瞬时态；每个状态保存浅色、不透明，深色、不透明，浅色、半透明，深色、半透明截图，文件名统一为 `progress/acceptance/<id>-<theme>-<material>.png`。如窗口尺寸是状态的一部分，另存指定尺寸截图。原型以 URL hash 定位同一屏核对。

## 批次 1：首屏、会话与消息交互（22 屏）

**screen IDs：** `main`, `botchat`, `group`, `mention`, `msgmenu`, `queue`, `queuedone`, `reply`, `quote`, `decide1`, `decide2`, `decide3`, `single`, `annedit`, `changes`, `completed`, `empty`, `loading`, `ctxmenu`, `newgroup`, `attach`, `dragfile`。

**路径/触发：** 首次连接后默认进入主 Bot 私聊；左栏依次打开 Bot 私聊和「登录功能」群。输入 `@` 打开成员候选；悬停/右键消息触发工具条和菜单，再分别选择回复、引用、工作详情；用 composer 的 `+` 打开附件菜单并拖入本地文件。对 Bot 发送一条无 @ 消息触发待看队列，进度详情中检查队列，待任务结束后触发已查看态。打开群公告编辑、侧栏完成分组、右键群菜单、新建群、验收卡片提修改意见。`empty` 用 mock 专用清空主聊天控制后截图；`loading` 用可控慢响应触发，不能靠静态原型截图。

**mock 数据：** `chat_main`、`chat_login`、`chat_test`；Bot `bot_main`/`bot_product`/`bot_code`/`bot_test`；`prj_login` 公告、成员和任务卡片；问题组 `asg_mock_questions` 的 answered/expired/pending 三题；消息菜单需要一条可回复、可引用的 Bot 消息；附件需要 `prj_login` Home 有文件。待看态需要工作中的 `asg_mock_working` 及两条未 @ 的排队消息，随后需有一条已读取事件。

**待补缺口：** prototype 的 `decide1`（三题未答）与 `decide2`（一题已答）是特定瞬时序列，现有 mock 历史题覆盖已答/过期/待答，但不等于这两个精确序列；需从 UI 创建或添加可控场景。等待态、空态、慢加载、拖拽和菜单都需手动触发。只有消息块 fixture 而没有对应 chat history 的屏幕不能算通过。

## 批次 2：工作台、工作详情、轨迹与电脑（8 屏）

**screen IDs：** `detail`, `queuepanel`, `artifact`, `trace`, `routines`, `work`, `computer`, `computer2`。

**路径/触发：** 左栏「工作台」切换按 Bot/群/状态；点工作行打开群工作详情，再点「轨迹」进入全屏；从产物卡片打开单文件与目录预览。左栏「定时任务」检查任务列表、运行记录、创建/试运行/停用操作。由 Bot 详情或任务详情进入 Agent Computer，接管后检查交还状态。

**mock 数据：** `asg_mock_working`、`asg_mock_queued`、`asg_mock_waiting`、`asg_mock_done`、`asg_mock_failed`、`asg_mock_delegation`，待验收项目 `prj_mock_review` 与 `prj_login` 文件；computer 需要 `bot_test` 的 tab 列表、screen state 和连续 JPEG 帧；routine 需要三条 routine（启用、停用、运行失败）及运行历史。

**待补缺口：** `trace.history` 按 assignment ID 从 `state.traces` 读取；当前 mock 场景虽然创建多状态 assignment/message，但需要确认对应 trace 序列确实被写入。原型轨迹需要模型请求/响应、工具调用、子代理、插话及队列审查等多种 item。2026-10-12 的 routines GUI 记录显示真实 Tauri 窗口拿到空列表，而 mock 源码有 3 条 seed；验收前需确认 GUI 连接的 mock 版本、数据目录和 `routine.list` 响应一致，不能以空白页覆盖有数据原型。接管需真的发送 takeover 并切换 driver，不可只切换视觉状态。

## 批次 3：连接、通用设置、记忆与 Bot 设置（17 屏）

**screen IDs：** `settings`, `set_general`, `set_memory`, `set_memory_edit`, `set_memory_conflict`, `set_models`, `set_tools`, `set_computer`, `set_hosts`, `set_about`, `onboard`, `offline`, `hosts`, `addhost`, `botdetail`, `botsettings`, `mainsettings`。

**路径/触发：** 新安装无 Host 时记录 `onboard`；连接 mock 后从「我的/设置」逐页打开通用、记忆、模型、运行与并发、工具与审批、电脑、Host、关于。搜索或新增、修改、删除记忆；另开客户端/请求更改版本后保存以触发冲突。点击侧栏 Host 菜单进入 Host 切换/管理并打开添加对话框；Bot 列表打开 Bot 详情、Bot 设置及主 Bot 设置。断开 mock 网络后截取 `offline`，恢复网络确认重连。

**mock 数据：** `bootstrap` 的 bots/chats/settings/providers/models；用户记忆列表、profile 和 quota；需第二客户端并发更新版本才能真实触发 `memory.conflict`；Host 需当前在线和另一 Host 离线记录；权限列表需授权/未授权两种系统状态。

**待补缺口：** `set_memory_conflict` 是服务端冲突响应，不会仅靠已有 fixture 自动出现；需并发修改。Host 添加错误态需真实 401/不可达响应。首次连接和断线是应用生命周期/网络手动触发。模型页需要一个有效 provider 和一个无效凭据状态，纯 mock provider 当前只有单模型，原型里的双 provider/401 对照数据要单独提供。权限状态取决于 Mac 系统设置，需记录机器状态。

## 批次 4：技能列表、编辑与输入选择器（7 屏）

**screen IDs：** `skills`, `slash`, `slash_q`, `slash_none`, `slash_picked`, `skillnew`, `skilldraft`。

**路径/触发：** 左栏「技能」打开列表；创建手写技能进入编辑器，另进入 Bot 生成草稿并发布/丢弃。回到群 composer 输入 `/`，分别键入匹配关键词与无匹配关键词，选择技能后继续输入并发送。

**mock 数据：** 至少 3 个可用技能，含 `web-auth-patterns`（近期调用数、Bot 分布）和 draft `prd-template`（来源/发布状态）；技能详情正文及可编辑保存响应。

**待补缺口：** mock 默认只 seed 3 个 builtin skills，prototype 的普通技能、调用统计和草稿需要 UI 创建或补可复位 seed。slash picker 要有启用且非 draft 的技能；`slash_none` 需要无匹配查询。创建、编辑、发布与丢弃均要实际执行并检查刷新后的服务端状态。

## 批次 5：仪表盘、搜索与窄窗口（4 屏）

**screen IDs：** `dash`, `dash2`, `search`, `narrow`。

**路径/触发：** 从「我的」打开仪表盘，切换日历/星期×小时、模型/Bot/项目维度并点图表下钻；`⌘K` 打开搜索，分别检索消息、群和 Bot；将窗口缩到 1024px 后打开右侧上下文面板，确认覆盖层及 Esc 收起行为。

**mock 数据：** 有多个日期、Bot、项目、模型、阶段的非零 usage 汇总和明细；搜索需要跨消息/项目/Bot 的命中与空结果。

**待补缺口：** mock 必须包含可下钻的小时热图和 usage 明细；只有默认用量汇总不能覆盖 `dash2`。搜索需预置可区分的消息/项目/Bot 命中与空查询。`narrow` 必须将实际 Tauri 窗口设为 1024px；原有 narrow 截图记录来自仪表盘，未验证此布局。

## 批次依赖和通过记录

- 批次 1、3、4 的页面基本可由侧栏到达；对话框、菜单、问题状态、队列状态、连接生命周期需要手动操作或隔离 mock 控制。
- 批次 2 与 5 依赖稳定且非空的任务/轨迹/画面/定时任务/用量/搜索数据。发现空数据时先保存 RPC 响应摘要、mock 版本和数据目录，再判断是 mock 缺数还是客户端未读到数据。
- 每张图须对应真实 Tauri 窗口 PID 和 mock endpoint；不能用 prototype 静态屏或旧 GPUI 截图代替。每状态对照同 ID 原型；三种外观组合均覆盖。截图验收前记录实际主题、材质、窗口尺寸、触发动作和 mock seed。
