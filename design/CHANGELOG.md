# design/ 更新记录

> 给后续实现的 agent 看：这里按版本记录设计稿**改了什么、改动落在哪些文件、实现时要注意什么**。具体数值以各规范文档为准，本文只做索引。新的版本写在最上面。

---

## 2026-10-16：收到确认、申请与总结分开发布

- 替代本日早先的首回复占位方案：私聊和群聊由程序先发完整收到确认，不再生成「…」。
- 问题/申请与最终总结各自发布新消息；回答只更新问题卡片状态，完成不覆盖确认或问题。
- 保留旧流式消息显示兼容，不新增视觉组件。

## 2026-10-16：统一聊天回复占位

- 私聊和群聊在 Bot 开始回复时显示同一种有真实消息 ID 的三点占位；首条回复原地填入，有文字或内容块即显示，不等待流式结束。
- 后续独立回复使用新消息 ID；消息更新按对话和消息 ID 匹配，最终全文不能重复追加。
- 规范位置：`06-message-blocks.md`；不更改气泡尺寸、颜色和既有动效参数。

## 2026-10-16：少成员群头像

- 1–3 个成员改为侧向错位堆叠；超过 3 个成员保留现有 2×2 网格及溢出计数。计数包含主 Bot，主 Bot 优先展示。
- 规范位置：`01-foundations.md`、`08-interaction.md`；桌面与 Android 共用同一人数分界。

## v1.2（2026-10-11）：更精致、更灵动

> 参考 Apple 信息（iMessage）与 Grok Bot。只改视觉和动效，不改信息结构、协议和文案。实现时以 `prototype/base.css` 末尾的「v1.2 精致与灵动」一节为准，它覆盖前面的同名规则。

### 1. 气泡
- 圆角统一 18；一组里最后一个气泡带 Apple 式小尾巴（宽气泡不带）；气泡间距 3、内边距 8 13。
- **我发的气泡改为实心渐变**（`bubble.out.top` → `bubble.out`，白字），代替原来的淡蓝底黑字；气泡里的链接、@、行内代码、引用块、技能标签都改成白色系。
- 别人的气泡：白底 + 0.5px 描边 + 极淡阴影。
- **文件**：01 §3.4、02 §5.2、`tokens.json`（`bubble.out*`、`radius.bubble`）、`base.css`。

### 2. Tapback 表情回应
- 回应从气泡底部挪到**最后一个气泡的上角**，做成浮起的小胶囊；自己点过的变成强调色；出现时弹出。
- **文件**：02 §5.5、`blocks.js`（`MB.msg` 的 `tapback` 参数）、`base.css`（`.tapback`）。原型：`mac.html#main`、`#group`。

### 3. 悬浮玻璃输入框
- 输入框变成悬浮的玻璃胶囊（模糊 + 浮起阴影），消息在下面滚动；有内容时出现光环、发送键弹出。
- **文件**：02 §5.7、`tokens.json`（`glass`、`ring.focus`、`elevation.float`、`radius.composer`）。

### 4. 玻璃质感与细节
- 工具栏、面板页头、日期胶囊（吸顶）、悬停工具条、菜单和弹层改为玻璃质感；卡片圆角 14 + 0.5px 描边；会话行、徽标、chip、分段控件的圆角和阴影统一调细；全局字距 -0.01em。
- **文件**：02 §5.8、§5.9。

### 5. 动效
- 新增带回弹的缓动 `ease.spring` / `ease.spring.soft`（360ms）。新消息进入（我的从输入框方向进入）、按压缩放到 0.97、发送键弹出、Tapback 弹出、弹层 / 对话框 / sheet 弹入、卡片悬停上移、正在输入的波浪。系统开启「减弱动态效果」时全部关闭。
- **文件**：01 §8.1、`tokens.json`（`motion`）、`base.css`（`@keyframes` 一组）。

---

## v1.1（2026-10-11）：在 v1.0 基础上的追加修改

### 1. 消息时间不再放在气泡里
- **改了什么**：去掉气泡右下角的时间和每条消息的已读双勾，改成：日期胶囊（跨天）+ 时间分隔（与上一条相隔 ≥ 5 分钟时，插一行居中时间，参考微信 / iMessage）+ 组头时间（Bot 的一组消息上方、气泡外面显示「名字 · 头衔 · 时间」，参考 Slack）+ 精确时间（桌面悬停气泡、手机向左拖动消息流）。已读只在我最后一条消息下方显示「已送达 / 已读」。
- **文件**：06 §2.1（规则）、02 §5、03 §6.6、04、07（`chat.status.*`）、08（时间格式）、`prototype/blocks.js`（`MB.msg`、`MB.timesep`、`MB.day`）、`base.css`（`.timesep` `.grp-head` `.htime`）。
- **实现注意**：时间分隔由客户端根据相邻两条消息的 `created_at` 计算，不需要服务端支持；组头里的名字从气泡内部移到了外部。分组规则：同一发送者、相隔 < 5 分钟、中间没有其他内容（时间分隔、任务卡片、系统事件）的连续消息合成一组——组头只在最上面，头像和尾角只在最后一个气泡；插了时间分隔就开始新的一组（原型里由 `MB.mergeGroups` 实现）。

### 2. 完成的任务卡片收起成一行
- **改了什么**：`task_card` 在 done 状态不再显示完整卡片，改为一行灰字「✓ Bot 名 · 任务名 · 用时 · 详情」，避免和 Bot 自己发的完成报告重复。其他状态（排队、工作中、等你、失败、已恢复）仍是完整卡片。
- **文件**：06 #7、DESIGN.md 4.3、`blocks.js`（`MB.task`）、`base.css`（`.mb-task-line`）、07（`block.task_card.done_line`）。
- **背景**：任务卡片由服务端按 Assignment 状态生成，不是任何 Bot 发的；「完成」由干活的 Bot 调用 `send_msg(done)` 决定，不是主 Bot。

### 3. 审批模式改为两档，默认自动允许
- **改了什么**：去掉 Auto Review 的「严格 / 标准 / 宽松」三档，改为「自动允许（默认）/ 先问我」。规则表增加作用范围（所有 Bot / 某个 Bot）和来源（内置只读 / 你添加的）。
- **文件**：03（设置 · 工具与审批）、07（`settings.tools.approval_mode.*`、`settings.tools.rule.*`）、`prototype/mac.html`（`#set_tools`）；协议见 PROTOCOL `Settings.approvals`、`ApprovalRule.bot_id/builtin`。

### 4. 待看消息（不打断正在干活的 Bot）
- **改了什么**：只有 @ 某个 Bot 或回复它的消息才打断它；其余群消息进入它的「待看消息」，交付前一起查看。新增：输入时的提示条和 [@X 立即生效]、送达状态「X 交付前会看到 → 交付前已查看」、状态条和工作台的「待看 N」、工作详情的「待看消息」列表和 [改为插话]、轨迹的「交付前查看新消息」。
- **文件**：03 §6.4、04、07（`chat.queue.*`、`trace.queue.*`）、`blocks.js`（`MB.queueHint`、`MB.queueItem`）；原型 `mac.html#queue/#queuepanel/#queuedone`、`mobile.html?only=queue,queuetrace`。协议见 `Assignment.pending_messages`、轨迹 `queue.review`、PLAN 5.3.4。

### 5. 回复与引用、右键菜单
- **改了什么**：右键消息（手机长按）弹出菜单：表情行 + 回复（R）· 引用（Q）· 复制文字 · 引用到公告 · 查看工作详情 · 复制消息链接（自己的消息多「删除」）。**回复**交给原发送者；**引用**只附原文、不指定谁回应。输入框上方显示「回复 X」或「引用 X · 不指定谁回应」条。不做线程。
- **文件**：06 #3、03 §6.3、04、07、08（R / Q 快捷键）、`blocks.js`（`MB.replyBar`、`MB.quote`）；原型 `mac.html#msgmenu/#reply/#quote`。协议：`chat.send.quote` + `quote_mode`。

### 6. 决策卡片：选项在消息里，答完原地变样，多题堆叠
- **改了什么**：`question` 块回答后原地变成已回答样式（选中项高亮、其余变淡；自由回答显示「你的回复」）；一条消息里连续多个 `question` 合成一叠，没答的露出 1–2 层，答完自动翻到下一题，全部答完后用「✓ 问题 N」标签切换查看结果；最后一项「其他，我来说…」可以原地输入。
- **文件**：06 #5 #5a、03 §6.3、04、07（`block.question.*`）、`blocks.js`（`MB.questionOne`、`MB.questionDeck`）；原型 `mac.html#decide1-3/#single`。

### 7. `/` 技能列表
- **改了什么**：输入 `/` 弹出技能列表（最近使用 + 全部，继续输入筛选，无匹配时可新建）；选中后变成输入框里的技能标签，发出的气泡开头带同样的标签。手机上列表贴在输入栏上方。
- **文件**：03 §6.2、04、07（`chat.slash.*`）、`blocks.js`（`MB.skillPicker`、`MB.skillChip`）、`base.css`（`.sk-*`、`.skill-chip`）。

### 8. 消息块按协议命名，并补齐缺的块
- **改了什么**：块名统一用协议名（`project_card`、`question`、`completion`、`delegation`、`review_card`、`task_card`、`approval_ref`、`loop_paused`、`takeover_request`、`bot_dm_ref`、`link_preview`……）；新增完成报告、群里的审批提示、防循环暂停、接管的三个状态、链接预览、未知类型兜底。`project_card` 支持 `context = created | mentioned`。
- **文件**：06 §3（对照表）、`blocks.js`、`blocks.html`。

### 9. 公告改为自由文本
- **改了什么**：公告是一段 Markdown 文本，Bot 和用户都能改（带版本冲突提示）；去掉「目标 / 截止 / 流程 / 项目要点」等固定字段，也没有「流程」概念（一个群会处理多件事）。成员与状态、产物不属于公告正文。
- **文件**：03（群信息、编辑公告）、04（群信息、编辑公告）、07（`group.announcement.*`）；原型 `mac.html#group/#annedit`、`mobile.html?only=groupinfo,annedit`。

### 10. 页面与布局调整
- 栏宽统一：侧栏 260 · 列表栏 260 · 会话区 ≥ 480 · 右面板 280。
- 右面板只放「正在跟进的事」和「当前对话里等你处理的事」，不放今日统计和定时任务；手机上对应聊天页顶部的「等你处理」条和「跟进」页。
- 定时任务有独立页面（桌面侧栏底部、手机「我的」第一项），手机也能完整编辑。
- 审批提醒挂在侧栏对应的群行 / Bot 行上（橙色第二行、徽标、左侧竖条）。
- 产物标出作者，可以预览（产物预览页：预览 / 源文件 / 历史）；工作台每个任务有 [群聊] 跳转。
- 新增 Bot 私聊、群聊卡片、Bot 详情和设置、主 Bot 设置、Host 切换和添加、右键菜单、+ 附件、拖入文件、提修改意见、已完成的群、空状态、加载骨架、「你正在操作」、仪表盘星期×小时和下钻、新建技能、技能草稿、设置六个分页、窄窗口；手机补齐 Bot 管理 / 编辑 / 详情、长按、左滑、Host 切换等。
- **文件**：03 §6–6.6、04 §4.1；原型 `mac.html`（55 屏）、`mobile.html`（40 组）。

### 11. 用户记忆可以编辑
- **改了什么**：推翻「客户端没有记忆页面」：新增「设置 → 记忆」（手机：我的 → 记忆），只开放用户记忆。列表显示每条记忆的来源（哪个 Bot、哪个会话、什么时间，或你手动添加）；可以搜索、原地编辑、删除（可撤销）、添加；显示 1,500 字配额用量；保存时版本冲突给出对比。Bot 记忆和项目记忆仍不展示。
- **文件**：03 §6.7、04、07（`memory.*`、`settings.section.memory`）、`base.css`（`.mem-row`）；原型 `mac.html#set_memory/#set_memory_edit/#set_memory_conflict`、`mobile.html?only=memory,memoryedit`；协议 PROTOCOL 5.10a（`memory.user.*`）；DESIGN.md 1、4.12、4.13、功能矩阵；PLAN 5.5；AGENTS.md。
- **实现注意**：设置导航里「记忆」排在「通用」之后，其余分页序号顺延。用户的修改和 Bot 的 `memory` 工具走同一套条目、版本、配额和增量消息。

### 12. 用户档案与记忆整理
- **改了什么**：「设置 → 记忆」顶部新增「关于你」档案卡片（称呼、语言、身份可编辑；时区跟随 Host 只读）。用户记忆改由后台「记忆整理」写入，来源行显示「记忆整理 根据 · 会话名 · 时间」；旧数据仍显示 Bot。
- **文件**：03 §6.7、04、07（`memory.profile.*`、`memory.source.curator`、`memory.desc`）；协议 PROTOCOL 5.10a（`UserProfile`、`memory.profile.*`）；DESIGN.md 4.12；PLAN 5.5.8、5.5.9；原型 `mac.html#set_memory` / `#set_memory_onboarded` / `#set_memory_profile_conflict`、`mobile.html?only=memory,memoryonboarded`。
- **实现注意**：档案编辑走 `memory.profile.update` 走版本冲突；`onboarded = false` 时首次主 Bot 私聊用 `set_user_profile` 走一轮 init 流程；用户记忆改由 curator（PLAN 5.5.9）批量写，Bot 写 user scope 会被工具拒绝。
- **2026-10-13 补充**：来源行显示 `created_at`（旧数据回退 `updated_at`），列表排序仍使用 `updated_at`。

### 13. 其他
- 主 Bot 头像从皇冠改为黑框眼镜（01 §10，token `avatar.glasses`）。
- 三端唯一提示音「啵啵 · 气泡」`sounds/04-bubble-pop`（01 §11）；设置里只有「提示音」开关。
- 文案 key 对照表 `07-copy.md`；交互细则 `08-interaction.md`（焦点顺序、快捷键、无障碍、头像 / 截断 / 时间格式）。
- 桌面端界面层改为 Tauri 2 + React，直接复用 `tokens.css` 和 `base.css`（README「交付形式」）。

---

## v1.0（2026-10-10）：首版

- 设计系统：`tokens.json`（浅色 / 深色 / 半透明三套）、`base.css` 组件、01–05 规范文档。
- 可交互原型：`index.html`（总览）、`mac.html`、`mobile.html`、`web.html`、`blocks.html`。
- 参考：Grok Bot、Telegram Desktop、Zeron、MonoCode、Apple HIG、Material 3（见 README）。
