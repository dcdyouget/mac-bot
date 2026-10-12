# v1.1 聊天界面逐节审计

## §1 消息时间分组与尾角

- `app/src/App.tsx:3350` 的 `buildMessageTimeline` 使用相邻 `created_at` 计算跨天日期、≥5 分钟时间分隔；同发送者且 <5 分钟的连续消息合并，任务卡片、进度和系统通知隔断分组。
- `app/src/App.tsx:2710` 渲染日期胶囊、时间分隔；`MessageBubble` 将作者与时间放在组头，并在气泡悬停时显示精确时间。
- `app/src/styles.css:113` 依据 `group-end` 只为最后一条消息显示尾角；连续消息沿用小圆角。此次为末条气泡补充朝头像侧的尾角样式。

## §2 完成任务卡片单行

- 已实现：`app/src/components/MessageBlocks.tsx:61` 的 `TaskCard` 对 `done` 状态直接渲染单行，显示完成图标、Bot、任务名、用时与详情入口；其他状态保留完整卡片。
- `app/src/components/messageBlocksTaskCard.ts` 映射 queued、waiting、blocked、failed、cancelled、done 等状态，重启恢复通过 `recovered_at` 附加状态提示。
