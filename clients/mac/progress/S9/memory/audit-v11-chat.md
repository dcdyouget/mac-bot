# v1.1 聊天界面逐节审计

## §1 消息时间分组与尾角

- `app/src/App.tsx:3350` 的 `buildMessageTimeline` 使用相邻 `created_at` 计算跨天日期、≥5 分钟时间分隔；同发送者且 <5 分钟的连续消息合并，任务卡片、进度和系统通知隔断分组。
- `app/src/App.tsx:2710` 渲染日期胶囊、时间分隔；`MessageBubble` 将作者与时间放在组头，并在气泡悬停时显示精确时间。
- `app/src/styles.css:113` 依据 `group-end` 只为最后一条消息显示尾角；连续消息沿用小圆角。此次为末条气泡补充朝头像侧的尾角样式。

## §2 完成任务卡片单行

- 已实现：`app/src/components/MessageBlocks.tsx:61` 的 `TaskCard` 对 `done` 状态直接渲染单行，显示完成图标、Bot、任务名、用时与详情入口；其他状态保留完整卡片。
- `app/src/components/messageBlocksTaskCard.ts` 映射 queued、waiting、blocked、failed、cancelled、done 等状态，重启恢复通过 `recovered_at` 附加状态提示。

## §3 审批两档、作用范围与内置规则

- 已实现：`app/src/components/SettingsDetail.tsx:280` 只显示「自动允许 / 先问我」两档；缺省 `mode` 选中自动允许。提示文案说明仅命中「先问我」规则的操作仍需审批。
- 每条规则含 `bot_id`（空值代表所有 Bot）和 `builtin`；Bot 范围与动作可以编辑，自定义规则可删除，内置规则的输入、选择器和删除操作均禁用并显示只读标记。
- 新建规则初始为 `ask_first`、所有 Bot、自定义可编辑。

## §4 待看消息

- 已有：`app/src/App.tsx:870` 从任务详情读取 `Assignment.pending_messages`，按消息 `delivery.state = queued` 收集完整消息列表；`app/src/components/TracePage.tsx:193` 展示列表、已查看状态及「改为插话」；轨迹 `queue.review` 事件已格式化。
- 已有：工作台任务标题显示待看数量（`app/src/App.tsx:2911`），任务详情调用 `message.steer`，并保留服务端 assignment/message 更新。
- 补齐：群聊输入栏在存在工作中 Bot 且草稿没有 @ 提及时显示待看提示；每个 Bot 的快捷按钮设置立即生效提及。消息送达状态现依据各条 `delivery` 展示 queued / delivered / read；群状态 chip 展示 `pending_messages` 数量。

## §5 回复、引用与消息菜单

- 已实现：`app/src/App.tsx:2710` 为消息时间线提供上下选择和 R/Q/⌘C 操作；`messageActions.model.ts` 在输入区、组合键和 IME 场景间过滤快捷键，并构造 `quote`、`quote_mode` 与回复目标 Bot mention。
- 已实现：`app/src/components/ChatEntry.tsx:56` 的右键菜单包括表情、回复、引用、复制文字、引用到公告、查看工作详情、复制消息链接；仅用户消息追加删除。菜单支持 Escape 与方向键导航。
- 已实现：输入框上方回复/引用条可取消，发送时调用协议 `chat.send.quote` / `quote_mode`，不创建线程。

## §6 决策卡片堆叠与键盘操作

- 已实现：消息连续 `question` block 在 `app/src/App.tsx:3455` 聚合为 QuestionDeck；未答题默认激活，答题成功后切至下一道待答题，题签可切换查看已答题目，答案状态驱动原地更新。
- 补齐：聚焦卡片时按 1–9 选择对应选项，O 打开自由回答；多题堆叠卡片支持 ←/→ 切题。可编辑输入框中的按键仍留给文本编辑。

## §7 `/` 技能列表

- 已实现：`app/src/components/chatSkillPickerModel.ts` 基于 `last_invoked_at` 选近 7 天最近三项，其他技能列入全部列表；输入查询按名称筛选，空结果提供新建入口。
- `app/src/components/ChatSkillPicker.tsx` 显示列表、禁用状态与最近调用数；`App.tsx` 在 `/` 后提供方向键、Enter/Tab、Esc 选择交互，选中技能成为输入栏标签。
- `MessageBlocks.tsx` 展示发出消息里的技能标签；服务端确认加载成功后才展示 `loaded_skills` 状态。
