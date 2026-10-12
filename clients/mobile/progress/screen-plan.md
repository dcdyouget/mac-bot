# Android 原型逐屏验收计划

按 `design/prototype/mobile.html` 的 `LIST` 顺序共 **42 个屏幕状态**。v1 只有 Android target；每批在 API 36 `macbot_api36` + mock（AVD 访问 Mac 用 `10.0.2.2:7789`）逐项操作，并对照原型 Android 列。原型 iOS 列只用于确认共用内容，不代表本轮有 iOS 客户端验收。

## 证据口径

- **有真实 mock 证据**：报告写明 mock / AVD 设置，且对应截图或 RPC 记录可以相互印证。仅代表该次状态与数据走通过，不等于视觉验收或三种外观通过。
- **截图待核**：progress 中有疑似对应截图，但没有足够的运行记录或截图画面无法确认 ID、mock 来源、数据和操作；计划里不计通过。
- **缺截图**：没有可对应的现存证据。
- 每个屏幕都要分别留浅色、深色、半透明截图。现有 `theme-audit/audit.md` 指出 Android `platformBackdropBlurSupported() = false`，半透明尚未实现；带 `translucent` 的旧截图不能作为毛玻璃通过证据。半透明列先留实际结果，直到背景模糊真实可见。
- `theme-audit` 记录的 artifact 404 已被后续 `artifact-reverify/report.md` 更新：server `5befd49` 加入 mock 文件后，`art_mock_brief` 与 `src/main.rs` 已真 mock 复验通过。旧 404 是历史失败，不再作为当前结果。

## 批次与覆盖

每行的“主题证据”按 `浅 / 深 / 半透明` 顺序列出。批次执行时即使截图待核也重新走实际 mock，并用 `-light.png`、`-dark.png`、`-translucent.png` 保存新证据。命名示例：`clients/mobile/progress/screen-acceptance/batch-01/list-light.png`。每屏截图需能辨认页面内容；涉及点击、手势、发送或持久化时，另附同目录 `batch-NN/notes.md` 记录动作及 mock 结果。

### 批次 1：消息与协作闭环（13 屏，最高优先）

入口从“消息”底栏开始；共享 mock 数据：主 Bot、编码 Bot、`登录功能` 群、待验收、待审批、一个运行中 assignment 与 queued 消息、三道决策题。缺对象时先确认 mock seed，不以空状态代替目标状态。

| ID / 原型状态 | 导航或触发 | 主题证据：浅 / 深 / 半透明 |
|---|---|---|
| `list` 消息列表 | 启动后“消息”底栏；核对主 Bot、群审批提醒、Bot 状态 | 有真实 mock 截图（theme-audit light/dark）；translucent 待验收 |
| `mainchat` 主 Bot 私聊 | 消息列表点“总管”；核对等你处理条、群创建和验收消息 | 有真实 mock 截图（theme-audit chat 有 mock 主 Bot 内容）；translucent 待验收 |
| `follow` 主 Bot 跟进 | 主 Bot 私聊点跟进/面板按钮 | 截图待核（integration/S6 followup-main）；translucent 待验收 |
| `botchat` Bot 私聊 | 消息列表点“编码”；核对待审批条与审批消息 | 截图待核（S7/messages、integration/S6）；translucent 待验收 |
| `botfollow` Bot 跟进 | Bot 私聊点跟进/面板按钮 | 截图待核（integration/S6 followup-bot）；translucent 待验收 |
| `group` 群聊 | 消息列表点“登录功能”；核对成员状态、公告、消息块、composer | 截图待核（有 S6/S7 group 截图，缺对应 mock 运行记录）；translucent 待验收 |
| `mention` 群聊 @ 成员 | 群聊点 composer 的 `@` | 缺截图；translucent 待验收 |
| `queue` 待看消息发送 | 运行中的群任务里不带 @ 发送消息；核对 queued 提示和 mock delivery | 有真实 mock / RPC 记录（`queue/queue-latest-rpc-evidence.txt`）；translucent 待验收 |
| `queuetrace` 待看消息工作详情 | 从 queued 任务打开工作详情；核对待看消息与“改为插话” | 有真实 mock 截图与 RPC（queue latest 系列）；translucent 待验收 |
| `reply` 引用回复 | 群聊右滑消息回复；核对引用预览与发送 | 截图待核（gestures/right-swipe 截图，缺 mock 结果记录）；translucent 待验收 |
| `decide1` 多题未答 | 群聊打开含三题的 question deck | 截图待核（integration/protocol2 有相关状态图，缺当前 mock 对应记录）；translucent 待验收 |
| `decide2` 回答一题 | 选择第一题答案，保留第二题输入和第三题未答 | 截图待核（integration/protocol2）；translucent 待验收 |
| `decide3` 全部答完 | 完成其余选择题并确认回复到达 | 截图待核（integration/protocol2）；translucent 待验收 |

### 批次 2：群信息、产物与工作流（11 屏）

从 `登录功能` 群或“工作台”进入。mock 数据：公告 Markdown、成员、`art_mock_brief` 项目文件、`src/main.rs`、进行中的 trace、定时任务 3 条、审批项、三类通知。artifact 必须看到文件正文，不接受只打开预览框。

| ID / 原型状态 | 导航或触发 | 主题证据：浅 / 深 / 半透明 |
|---|---|---|
| `slash` 技能列表 | 群聊 composer 输入 `/` | 缺截图；translucent 待验收 |
| `slash_q` 技能筛选 | 输入 `/auth` | 缺截图；translucent 待验收 |
| `slash_picked` 已选技能 | 选择 `web-auth-patterns`，核对技能 chip 与 @ 人 | 缺截图；translucent 待验收 |
| `groupinfo` 群信息 | 群聊公告条/顶栏进入；核对 Markdown、成员、产物 | 有真实 mock 深色截图（artifact-reverify/report.md）；浅色仅旧截图待核；translucent 待验收 |
| `artifact` 产物预览 | 群信息打开 `art_mock_brief`，再打开 `src/` 与 `main.rs` | 有真实 mock 深色截图及 seed 复验（artifact-reverify）；浅色旧截图显示历史 404，需当前复验；translucent 待验收 |
| `trace` 工作详情 / 运行轨迹 | 群任务点“详情” | 有真实 mock light/dark 截图（theme-audit task detail）；translucent 待验收 |
| `work` 工作台 | 点底栏“工作台”；核对分段、横幅、任务和群聊跳转 | 缺可追溯 mock 证据（只有未关联截图）；translucent 待验收 |
| `routines` 定时任务列表 | “我的”→“定时任务” | 截图待核（progress/routines 只有浅色截图）；深色/半透明待验收 |
| `routine` 定时任务详情 | 列表点一条任务；检查启停、编辑、试运行、删除 | 截图待核（progress/routines 只有浅色截图）；深色/半透明待验收 |
| `approve` 审批弹层 | 从工作台或通知点审批项 | 截图待核（notifications 有路由图，缺一致的当前 mock 记录）；translucent 待验收 |
| `notify` 通知 | 触发待审批、待验收和普通消息，查看系统通知及点击路由 | 截图待核（notifications 下截图无可审计触发记录）；translucent 待验收 |

### 批次 3：创建、查找与个人管理（11 屏）

从消息页新建，从搜索入口检索；从“我的”进入仪表盘、技能、记忆、Bot 管理。mock 需要可搜索消息/项目、统计数据、技能、用户记忆、主 Bot 与多个普通 Bot。

| ID / 原型状态 | 导航或触发 | 主题证据：浅 / 深 / 半透明 |
|---|---|---|
| `newgroup` 新建群 | 消息页点 FAB / 新建，再选建群 | 截图待核（onboarding 不是该屏；缺当前创建流程证据）；translucent 待验收 |
| `search` 搜索 | 消息页或工作台点搜索，输入有结果的关键词并切筛选 | 缺截图；translucent 待验收 |
| `computer` Agent Computer | 从 Bot/任务打开屏幕或 Computer 页面，核对横屏/接管态 | 缺截图；translucent 待验收 |
| `dash` 仪表盘 | 我的→仪表盘；核对指标、热力图横滚、曲线筛选 | 缺可追溯 mock 证据；translucent 待验收 |
| `skills` 技能 | 我的→技能，搜索并打开详情 | 缺截图；translucent 待验收 |
| `me` 我的 | 点底栏“我的”；核对 Host 和分组导航 | 截图待核（notifications/my、memory 流程截图，未证明页面状态）；translucent 待验收 |
| `memory` 用户记忆 | 我的→记忆；核对列表、搜索、来源、用量 | 有真实 mock light/dark 截图（theme-audit light、artifact-reverify dark）；translucent 待验收，现有 translucent 图不能证明模糊 |
| `memoryedit` 编辑记忆 | 点一条记忆，编辑保存；再验证删除操作 | 截图待核（memory 下有相关截图但缺本 screen 的 mock 交互记录）；translucent 待验收 |
| `bots` Bot 管理 | 我的→Bot 管理 | 截图待核（bot-detail 截图只可确认详情 UI，不能代表列表）；translucent 待验收 |
| `botedit` Bot 编辑 | Bot 管理→编码→编辑；核对工具开关与规则保存 | 缺截图；translucent 待验收 |
| `botdetail` Bot 详情 | Bot 管理点“编码” | 截图待核（progress/bot-detail 三主题文件未附 mock 来源/操作记录）；translucent 待验收 |

### 批次 4：手势、Host、设置与连接状态（7 屏）

从消息列表/群聊操作；准备一个在线 Host 和一个离线 Host。设置主题逐个切换，通知开关需保存并重启复核。首次连接与断线状态要实际连接 mock，不用手工固定页面替代。

| ID / 原型状态 | 导航或触发 | 主题证据：浅 / 深 / 半透明 |
|---|---|---|
| `longpress` 长按消息 | 群聊长按消息；核对表情、回复/引用/复制/公告/工作详情菜单 | 截图待核（gestures 有截图，缺对应 mock 记录）；translucent 待验收 |
| `swipe` 左滑会话 | 消息列表左滑群行；分别触发置顶、完成、静音并验证状态 | 截图待核（gestures 有截图，缺持久化/mock 记录）；translucent 待验收 |
| `hostswitch` 切换 Host | 顶栏 Host 名称→Host sheet；切到在线和离线 Host | 截图待核（host-list/attachment 截图，缺该 sheet 的 mock 记录）；translucent 待验收 |
| `annedit` 编辑公告 | 群信息→编辑公告，修改 Markdown 并保存 | 缺截图；translucent 待验收 |
| `settings` 外观与通知 | 我的→外观与通知；主题、半透明、四类通知开关分别操作并重启复核 | 有真实 mock light/dark 截图（theme-audit）；半透明视觉明确未实现（theme-audit/audit.md） |
| `connect` 首次连接 | 清空首次启动状态，输入 mock 地址/口令/名称，连接并选模板 | 截图待核（onboarding/notifications 有过程截图，缺可追溯当前 mock 记录）；translucent 待验收 |
| `offline` 断线重连 | 连接成功后停止 mock，再恢复 mock；核对断线条、恢复和状态补齐 | 缺截图；translucent 待验收 |

## 执行顺序与完成判定

1. **先做批次 1**：覆盖最高频消息路径及审批、队列、决策等协议状态；随后批次 2、3、4。
2. 每个 ID 三主题各截图；同时核对 Android 原型列的层级、尺寸、文案、触控目标和外壳。半透明需确认系统栏、底部导航、sheet 确有背景模糊/透出；仅设置显示“半透明”不算通过。
3. mock 交互状态需留服务端/RPC 或持久化证据，例如发送消息、审批结果、记忆修改、定时任务试运行、Host 断线恢复。截图无法说明的状态记录在对应批次 `notes.md`。
4. 本计划只盘点证据与安排验收，不改变现有截图状态。当前证据不足以宣称 42 屏逐屏或三主题已完成；mock 深色 artifact 已在后续复验修复，Android 半透明仍是明确缺口。
