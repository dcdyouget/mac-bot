# 07 · 文案与 i18n key 对照表

- 所有界面文案都通过 key 引用（AGENTS.md §4），三端共用同一套 key；v1 只有 `zh-CN`，`en` 以后再补。
- key 命名：`模块.子区域.含义`，小写加下划线。占位符用 `{name}`；数量用 ICU 复数 `{count, plural, other {# 个}}`（中文只需 other）。
- 本表与原型（`prototype/*.html`）里的文字一一对应。新增文案先在这里加 key，再写代码。
- 文件位置建议：desktop `clients/mac/assets/i18n/zh-CN.json`；mobile `shared/src/commonMain/composeResources/values-zh/strings.xml`（key 里的 `.` 换成 `_`）。

## common · 通用

| key | zh-CN |
|-----|-------|
| common.ok | 好 |
| common.cancel | 取消 |
| common.save | 保存 |
| common.delete | 删除 |
| common.edit | 编辑 |
| common.done | 完成 |
| common.close | 关闭 |
| common.back | 返回 |
| common.more | 更多 |
| common.retry | 重试 |
| common.copy | 复制 |
| common.copy_path | 复制路径 |
| common.open | 打开 |
| common.preview | 预览 |
| common.details | 详情 |
| common.stop | 停止 |
| common.search | 搜索 |
| common.new | 新建 |
| common.add | 添加 |
| common.submit | 提交 |
| common.send | 发送 |
| common.enable | 启用 |
| common.all | 全部 |
| common.view_all | 全部 › |
| common.show_in_finder | 在 Finder 中显示 |
| common.share_open_with | 分享 / 用其他应用打开 |
| common.provided_by | {bot} 提供 |
| common.updated_at | {time} 更新 |
| common.you | 你 |
| common.today | 今天 |
| common.yesterday | 昨天 |
| common.minutes | {count} 分 |
| common.hours_minutes | {h} 时 {m} 分 |
| common.tokens | {value} tok |

## status · 状态（三端统一）

| key | zh-CN |
|-----|-------|
| status.idle | 空闲 |
| status.standby | 待命 |
| status.working | 工作中 |
| status.working_n | {count} 个活 |
| status.queued | 排队 |
| status.waiting_user | 等你 |
| status.need_approval | 需要你审批 |
| status.blocked | 卡住 |
| status.done | 完成 |
| status.cancelled | 已停止 |
| status.failed | 失败 |
| status.recovered | 已恢复 |
| status.review | 待验收 |
| status.review_you | 待你验收 |
| status.lead | 负责人 |
| project.status.active | 进行中 |
| project.status.review | 待验收 |
| project.status.done | 已完成 |
| project.status.archived | 已归档 |
| project.status.new | 新建 |

## nav · 导航

| key | zh-CN |
|-----|-------|
| nav.tab.messages | 消息 |
| nav.tab.workbench | 工作台 |
| nav.tab.me | 我的 |
| nav.workbench | 工作台 |
| nav.workbench.running | {count} 进行中 |
| nav.dashboard | 仪表盘 |
| nav.routines | 定时任务 |
| nav.skills | 技能 |
| nav.settings | 设置 |
| nav.bots_manage | Bot 管理 |
| nav.appearance_notify | 外观与通知 |

## sidebar · 侧栏 / 消息列表

| key | zh-CN |
|-----|-------|
| sidebar.search.placeholder | 搜索 |
| sidebar.section.groups | 群 · {count} |
| sidebar.section.bots | Bot · {count} |
| sidebar.main_bot.tag | 主 Bot |
| sidebar.main_bot.subtitle | 只协调，不干活 |
| sidebar.completed | 已完成 |
| sidebar.archived_hint | 已归档 {count} · 在搜索里查看 |
| sidebar.approval_line | ⚠ 需要你审批 · {summary} |
| sidebar.empty_groups | 还没有群。对总管说「帮我做…」，它会拉群去做。 |
| sidebar.hidden_bots | 隐藏的 Bot {count} › |
| sidebar.menu.pin | 置顶 |
| sidebar.menu.rename | 重命名 |
| sidebar.menu.mark_done | 标记完成 |
| sidebar.menu.mute | 关闭通知 |
| sidebar.menu.open_home | 打开 Home |
| sidebar.menu.archive | 归档 |
| sidebar.menu.hide | 隐藏 |
| sidebar.swipe.pin | 置顶 |
| sidebar.swipe.done | 完成 |
| sidebar.swipe.mute | 静音 |

## host · Host 与连接

| key | zh-CN |
|-----|-------|
| host.switch.title | 切换 Host |
| host.add | 添加 Host… |
| host.manage | 管理 Host |
| host.offline_since | 离线 · {time} |
| host.field.address | 地址 |
| host.field.address_hint | host:port、ws:// 或 wss:// |
| host.field.password | 密码 |
| host.field.name | 名称 |
| host.connect | 连接 |
| host.reconnecting | 正在重连 {host}… |
| host.reconnecting_detail | 本地缓存可浏览，发送已暂停 |
| host.error.unreachable | 地址连不通 |
| host.error.password | 密码错误（401）。忘了密码？在 Bot 主机上运行 macbot passwd |
| host.error.version | 服务端版本不兼容，请升级 |
| host.error.no_password | 服务端还没设置密码：请先在 Bot 主机上打开 localhost:7788/admin |

## onboarding · 首次启动

| key | zh-CN |
|-----|-------|
| onboarding.welcome | 欢迎使用 Mac Bot |
| onboarding.connect_subtitle | 连接你的 Bot 主机 |
| onboarding.meet_main | 认识你的主 Bot：总管 |
| onboarding.meet_main_desc | 日常和它聊；有事它会拉群。选一个团队模板： |
| onboarding.template.only_main | 先只用主 Bot |
| onboarding.footer | 对总管说「帮我做…」，它会拉群去做 |
| onboarding.empty.title | 你好，我是总管 |
| onboarding.empty.desc | 把要做的事交给我：需要几个 Bot 配合的，我会拉群分工，做完提醒你验收；小事我直接转给合适的 Bot。 |
| onboarding.suggest.login | 帮我给 App 加个登录 |
| onboarding.suggest.research | 调研 3 个竞品 |
| onboarding.suggest.daily | 每天 9 点给我发晨报 |

## chat · 会话

| key | zh-CN |
|-----|-------|
| chat.main.subtitle | 主 Bot · 只协调，不干活 |
| chat.bot.subtitle | {label} · 工作中 {count} 个活 |
| chat.group.subtitle | {count} 位成员 |
| chat.composer.placeholder.dm | 给 {name} 发消息… |
| chat.composer.placeholder.group | 在 {group} 里发消息，@ 指定 Bot… |
| chat.composer.placeholder.offline | 已断开，正在重连… |
| chat.composer.attach.file | 文件… |
| chat.composer.attach.image | 图片… |
| chat.composer.attach.screenshot | 截图 |
| chat.composer.attach.from_home | 从 Home 选择… |
| chat.mention.title | 提及成员 |
| chat.mention.everyone | everyone |
| chat.mention.steer_hint | 工作中 · 插话 |
| chat.mention.steer_desc | @ 正在工作的 Bot = 插话，在它的下一步生效 |
| chat.slash.recent | 最近使用 |
| chat.slash.all | 全部技能 · {count} |
| chat.slash.matched | 匹配「{query}」· {count} |
| chat.slash.keys | ↑↓ 选择 · ↵ / Tab 插入 · Esc 关闭 |
| chat.slash.shared | ϟ 技能 · 所有 Bot 共用 |
| chat.slash.used | {count} 次 |
| chat.slash.empty_title | 没有叫「{query}」的技能 |
| chat.slash.empty_desc | 继续输入会作为普通文字发送 |
| chat.slash.create | 新建技能「{query}」 |
| chat.slash.disabled_for | 对{bot}停用 |
| chat.slash.loaded | 已加载技能 {name} |
| chat.drop.title | 松开发送到「{group}」 |
| chat.drop.desc | 文件会上传到 Bot 主机（单个最大 100 MB） |
| chat.delivery.queued | 已送达 · 等下一步 |
| chat.delivery.read | {bot} 已读取 ✓ |
| chat.jump_latest | ↓ {count} |
| chat.loading_older | 正在加载更早的消息… |
| chat.typing | 正在输入… |
| chat.status.sent | 已送达 |
| chat.status.read | 已读 |
| block.task_card.done_line | {bot} · {title} · 用时 {duration} |
| chat.menu.reply | 回复 |
| chat.menu.react | 表情回应 |
| chat.menu.copy | 复制 |
| chat.menu.quote_to_announcement | 引用到公告 |
| chat.menu.view_trace | 查看工作详情 |
| chat.menu.delete | 删除 |
| chat.pin.waiting_you | 等你处理 {count} |
| chat.pin.pending_approval | 待审批 {count} |

## block · 消息块（与 PROTOCOL 4.6 同名）

| key | zh-CN |
|-----|-------|
| block.project_card.created | 新建了群聊 |
| block.project_card.subtitle | 群聊 · {members} |
| block.project_card.enter | 进入群聊 |
| block.question.single | 单选 |
| block.question.multi | 多选 |
| block.question.from | 来自 {bot} |
| block.question.free_text_hint | 也可以直接回复文字 |
| block.question.selected_n | 已选 {count} 项 |
| block.question.answered | 你选择了 {letters} · {options} |
| block.question.expired | 已过期 |
| block.question.title_deck | {bot} 需要你决定 · 第 {n} / {total} 题 |
| block.question.answered_count | 已答 {count} |
| block.question.all_answered | 已回答 {count} 个问题 |
| block.question.tab | 问题 {n} |
| block.question.other | 其他，我来说… |
| block.question.you_picked | 你选择了 {letters} |
| block.question.you_replied | 你回复了 |
| block.question.your_reply | 你的回复 |
| block.question.tap_to_submit | 点一项就提交 |
| chat.queue.hint | {bots} 正在干活，这条消息不会打断{count, plural, one {它} other {它们}}，会在交付前看到 |
| chat.queue.at_now | @{bot} 立即生效 |
| chat.delivery.main_received | 总管 已收到 |
| chat.delivery.queued_for | {bot} 交付前会看到 |
| chat.delivery.queue_seen | {bot} 交付前已查看 ✓ |
| trace.queue.title | 待看消息 |
| trace.queue.hint | 没有 @ {bot} 的消息不会打断它，交付前会一起查看。点「改为插话」可以让某一条在下一步就生效。 |
| trace.queue.to_steer | 改为插话 |
| trace.queue.seen | 已查看 |
| trace.item.queue_review | 交付前查看新消息 {count} 条 |
| workbench.queued_msgs | 待看 {count} |
| chat.reply_bar.title | 回复 {name} |
| chat.quote_bar.title | 引用 {name} |
| chat.quote_bar.hint | 不指定谁回应 |
| chat.menu.quote | 引用 |
| chat.menu.copy_text | 复制文字 |
| chat.menu.copy_link | 复制消息链接 |
| block.completion.done | 已完成 |
| block.completion.notify_main | 全部完成，请汇总验收 |
| block.task_card.title | 任务 · {title} |
| block.task_card.elapsed | 已 {duration} |
| block.task_card.took | 用时 {duration} |
| block.task_card.queued_reason | 排在「{task}」之后 |
| block.task_card.recovered | 服务重启后继续 |
| block.blocked.title | 卡住了 |
| block.blocked.need | 需要：{who} |
| block.blocked.switch_model | 换模型 |
| block.review_card.title | 待验收 · {project} |
| block.review_card.confirm | 确认完成 |
| block.review_card.request_changes | 提修改意见 |
| block.review_card.confirmed | 你已确认完成 · {time} |
| block.review_card.changes_requested | 你提了修改意见 · 已派给对应的 Bot |
| block.review_card.changes_dialog_title | 提修改意见 · {project} |
| block.review_card.changes_dialog_hint | 意见会发到群里并 @总管，由总管派给对应的 Bot；群回到「进行中」。 |
| block.delegation.text | 交给 {bot} 处理（不建群） |
| block.approval.title | 需要审批 · {tool} |
| block.approval.meta | 来自 {project} · 命中规则「{rule}」 |
| block.approval.matched_rule | 命中规则「{rule}」 |
| block.approval.allow_once | 允许一次 |
| block.approval.always_allow | 总是允许 |
| block.approval.deny | 拒绝 |
| block.approval.allowed_once | 已允许一次 · 你 {time} |
| block.approval.always_allowed | 已设为总是允许 · 你 {time} |
| block.approval.denied | 已拒绝 · 你 {time} |
| block.approval.expired | 已过期（任务已结束） |
| block.approval_ref.text | {bot} 在私聊里等你审批 |
| block.approval_ref.go | 前往 |
| block.takeover.title | 需要你接管 |
| block.takeover.start | 接管并处理 |
| block.takeover.active | 你正在操作 · Bot 已暂停 |
| block.takeover.back_to_screen | 回到画面 |
| block.takeover.release | 交还给 Bot |
| block.takeover.done | 已交还给 Bot |
| block.bot_dm_ref.text | {a} 私信了 {b}（{count} 条） |
| block.bot_dm_ref.expand | 展开 |
| block.loop_paused.title | 已暂停 |
| block.loop_paused.desc | Bot 之间连续往返 {count} 次 |
| block.loop_paused.continue | 再允许 8 次 |
| block.loop_paused.end | 结束这一轮 |
| block.loop_paused.continued | 你允许继续 8 次 |
| block.loop_paused.ended | 你结束了这一轮 |
| block.system.announcement_updated | {who} 更新了公告 |
| block.system.member_added | {bot} 加入了群 |
| block.system.created | 总管 创建了群，拉入 {members} |
| block.fallback.hint | 这类消息需要更新客户端才能完整显示 |

## group · 群、公告、产物

| key | zh-CN |
|-----|-------|
| group.statusbar.lead | ★ 负责人 |
| group.announcement | 公告 |
| group.announcement.meta | {who} · {time} 更新 |
| group.announcement.edit | 编辑 |
| group.announcement.edit_title | 编辑公告 |
| group.announcement.edit_hint | 一段自由文本，不规定格式，按 Markdown 显示。群里的 Bot 也会读写这段文字。 |
| group.announcement.version | 版本 {n} · {len} / 8000 字 |
| group.announcement.save_shortcut | ⌘↵ 保存 |
| group.announcement.conflict | {who} 刚刚也改了公告（版本 {n}）。查看差异后再保存 |
| group.announcement.empty | 还没有公告。点「编辑」写下这件事要做什么。 |
| group.open_home | 打开 Home |
| group.members | 成员与状态 |
| group.artifacts | 产物 |
| group.info | 群信息 |
| artifact.preview.tabs.preview | 预览 |
| artifact.preview.tabs.source | 源文件 |
| artifact.preview.tabs.history | 历史 |
| artifact.preview.list_title | 本群产物 · {count} |
| artifact.preview.version | 第 {n} 版 |

## follow · 跟进（右面板 / 跟进页）

| key | zh-CN |
|-----|-------|
| follow.title | 跟进 |
| follow.waiting_in_chat | 本对话中等你处理 |
| follow.pending_approval_in_chat | 本对话中待审批 |
| follow.groups | 正在跟进的群 |
| follow.bot_tasks | 正在干的活 |
| follow.queued | … 排队 {count} · {task} |
| follow.go_review | 去验收 |
| follow.handle | 处理 |
| follow.jump_group | 群聊 |
| follow.jump_dm | 私聊 |

## new · 新建（⌘N）

| key | zh-CN |
|-----|-------|
| new.title | 新建 |
| new.input.placeholder | 输入一件事（建群），或一个名字（新建 Bot） |
| new.group | 新建群「{name}」 |
| new.group.hint | 总管会自动加入并担任负责人 |
| new.bot | 新建 Bot「{name}」 |
| new.members | 选择成员（1–6 个，已选 {count}，另加总管） |
| new.note | 说明（会写进公告） |
| new.create_group | 建群 |
| new.footer | 建群后，总管会写好公告，在群里发开场并 @ 第一个 Bot |

## search · 搜索（⌘K）

| key | zh-CN |
|-----|-------|
| search.placeholder | 搜索消息、Bot、群、产物、定时任务 |
| search.filter.messages | 消息 |
| search.filter.groups | 群 |
| search.filter.bots | Bot |
| search.filter.artifacts | 产物 |
| search.filter.routines | 定时任务 |
| search.keys | ↑↓ 选择 · ↵ 打开 · ⌘↵ 在右侧打开 |

## trace · 工作详情 / 运行轨迹

| key | zh-CN |
|-----|-------|
| trace.instruction | 任务指令 |
| trace.process | 工作过程 |
| trace.follow_latest | 跟随最新 |
| trace.back_to_latest | ↓ 回到最新 |
| trace.thinking_summary | 思考摘要 |
| trace.full_log | 完整记录 |
| trace.at_it | @ 它 |
| trace.screen | 屏幕 |
| trace.filter.model | 模型 |
| trace.filter.tools | 工具 |
| trace.filter.subagents | 子代理 |
| trace.filter.steer | 插话 |
| trace.search | 搜索轨迹 |
| trace.item.start | 开始干活 |
| trace.item.model_request | 模型请求 #{n} |
| trace.item.subagents | 子代理 ×{count} 并行 |
| trace.item.steer | 插话（{who}） |
| trace.item.steer_applied | 已在下一步生效 |
| trace.summary | 模型请求 {req} · 工具调用 {tools} · 子代理 {sub} · 插话 {steer} · 输入 {in} / 输出 {out} · {cost} |
| trace.running | 运行中 |
| trace.queue.review_message | 原文 |
| trace.queue.review_result | 处理结果 |
| trace.queue.review_details | 查看处理结果 |
| trace.state.loading | 正在加载轨迹… |
| trace.state.empty | 暂无运行轨迹 |
| trace.state.no_matches | 没有匹配的轨迹 |
| trace.state.error | 加载运行轨迹失败 |
| trace.state.load_more | 加载更早记录 |
| trace.state.unknown | 未知记录 |
| trace.state.instruction_empty | 暂无任务指令 |
| trace.state.request_context | 上下文 |
| trace.state.thinking | 思考 |
| trace.state.tool_failed | 工具执行失败 |
| trace.state.run_failed | 运行失败 |
| trace.state.returned | 任务结束 |
| trace.state.waiting | 等待中 |
| trace.state.resumed | 继续运行 |
| trace.state.compaction | 上下文压缩 |
| trace.state.subagent_task | 子代理任务 |
| trace.state.from | 来自 |

## workbench · 工作台

| key | zh-CN |
|-----|-------|
| workbench.title | 工作台 |
| workbench.view.by_bot | 按 Bot |
| workbench.view.by_group | 按群 |
| workbench.view.by_status | 按状态 |
| workbench.running | 运行中 {running} / 上限 {limit} |
| workbench.waiting_review | 等你验收 |
| workbench.waiting_approval | 等你审批 |
| workbench.concurrency | {running}/{max} 并发 |
| workbench.main_not_counted | 总管只协调，不占并发名额 |
| workbench.done_today | 今天完成 {count} |

## dashboard · 仪表盘

| key | zh-CN |
|-----|-------|
| dashboard.title | 仪表盘 |
| dashboard.range.today | 今天 |
| dashboard.range.7d | 7 天 |
| dashboard.range.30d | 30 天 |
| dashboard.range.90d | 90 天 |
| dashboard.export_csv | 导出 CSV |
| dashboard.kpi.tokens | 总 token |
| dashboard.kpi.cost | 费用 |
| dashboard.kpi.requests | 模型请求 |
| dashboard.kpi.cache_hit | 缓存命中 |
| dashboard.kpi.tasks_done | 完成任务 |
| dashboard.kpi.vs_prev | 较上期 |
| dashboard.heatmap | 活动热力图 |
| dashboard.heatmap.calendar | 日历 |
| dashboard.heatmap.week_hour | 星期 × 小时 |
| dashboard.heatmap.less | 少 |
| dashboard.heatmap.more | 多 |
| dashboard.daily | 每日 token 消耗 |
| dashboard.dimension | 维度 |
| dashboard.dimension.model | 模型 |
| dashboard.dimension.bot | Bot |
| dashboard.dimension.project | 项目 |
| dashboard.metric | 指标 |
| dashboard.drill.phases | 按项目和阶段拆分 |
| dashboard.phase.chat | 私聊 |
| dashboard.phase.work | 干活 |
| dashboard.phase.subagent | 子代理 |
| dashboard.phase.coordinate | 协调 |
| dashboard.phase.memory | 记忆整理 |
| dashboard.phase.compact | 压缩 |

## routines · 定时任务

| key | zh-CN |
|-----|-------|
| routines.title | 定时任务 |
| routines.filter.running | 运行中 |
| routines.filter.paused | 已暂停 |
| routines.paused | 已暂停 |
| routines.next_run | 下次运行 {time} · 由 {bot} 执行 |
| routines.test_run | 试运行 |
| routines.field.name | 名称 |
| routines.field.bot | 执行的 Bot |
| routines.field.instruction | 指令 |
| routines.field.when | 何时运行 |
| routines.field.add_time | 添加时间 |
| routines.field.timezone | 时区 {tz} |
| routines.field.deliver_to | 结果发到 |
| routines.deliver.dm | 和 {bot} 的私聊 |
| routines.deliver.group | 某个群 |
| routines.history | 运行记录（最近 20 次） |
| routines.run.ok | ✓ 成功 |
| routines.run.trace | 轨迹 |

## skills · 技能

| key | zh-CN |
|-----|-------|
| skills.title | 技能 |
| skills.search | 搜索技能 |
| skills.source.builtin | 内置 |
| skills.source.mine | 我的 |
| skills.source.imported | 导入 |
| skills.draft | 草稿 |
| skills.usage | 所有 Bot 可用 · 最近 7 天调用 {count} 次 |
| skills.draft_from | 由 {bot} 在「{project}」里生成 · 还没对任何 Bot 生效 |
| skills.publish | 发布 |
| skills.discard | 丢弃 |
| skills.attached_files | 附带文件 |
| skills.disable_for | 对个别 Bot 停用 |
| skills.new.title | 新建技能 |
| skills.new.mode.write | 手写 |
| skills.new.mode.folder | 从文件夹 |
| skills.new.mode.git | 从 Git |
| skills.new.mode.zip | 从 zip |
| skills.new.name | 名称（小写字母、数字、-） |
| skills.new.desc | 描述（Bot 根据它决定什么时候用） |
| skills.new.draft_hint | 草稿：Bot 生成的技能先放在「草稿」里，你点「发布」后才对所有 Bot 生效 |
| skills.new.create | 创建并编辑 SKILL.md |

## bot · Bot 详情与设置

| key | zh-CN |
|-----|-------|
| bot.groups | 所在的群 |
| bot.history | 历史任务 |
| bot.week_usage | 本周消耗 |
| bot.field.name | 名称 |
| bot.field.label | 头衔 |
| bot.field.rules | 长期规则 |
| bot.field.model | 模型（私聊和干活共用） |
| bot.field.max_parallel | 最多同时干几件活 |
| bot.field.tools | 工具 |
| bot.field.skills | 技能 |
| bot.field.skills_all | 全部可用（{count}） |
| bot.field.browser_mode | 浏览器模式 |
| bot.field.notifications | 通知 |
| bot.avatar.other | 用 emoji 或图片 |
| bot.tool.file | 文件 |
| bot.tool.bash | bash |
| bot.tool.browser | 浏览器 |
| bot.tool.subagent | 子代理 |
| bot.tool.web | 网页 |
| bot.tool.mcp | MCP |
| bot.action.pin | 置顶 |
| bot.action.hide | 隐藏 |
| bot.action.duplicate | 复制 |
| bot.action.delete | 删除 Bot |
| bot.main.rule | 建群规则 |
| bot.main.rule.auto | 自动建群 |
| bot.main.rule.ask | 建群前先问我 |
| bot.main.model_hint | 主 Bot 只协调、不干活，建议用便宜、快的模型 |
| bot.main.no_tools | 主 Bot 没有工具开关，不能删除。 |

## computer · Agent Computer

| key | zh-CN |
|-----|-------|
| computer.title | {bot} 的浏览器 |
| computer.state.bot | ● Bot 操作中 |
| computer.state.you | ● 你正在操作（Bot 已暂停） |
| computer.state.idle | ○ 空闲 |
| computer.take_over | 接管 |
| computer.release | 交还给 Bot |
| computer.request | {bot} 请求你接管：{reason}。完成后点「交还给 Bot」。 |
| computer.you_control_hint | 鼠标、键盘、滚轮会实时发给浏览器 · 完成后点「交还给 Bot」，可以附一句说明 |
| computer.quality | 画质 |
| computer.quality.auto | 自动 |
| computer.quality.high | 高清 |
| computer.quality.low | 流畅 |
| computer.connection.connecting | 正在连接浏览器画面… |
| computer.connection.disconnected | 浏览器画面已断开 |
| computer.connection.error | 浏览器画面连接失败 |
| computer.empty.tabs | 暂无浏览器标签页 |
| computer.empty.frame | 暂无浏览器画面 |
| computer.tab.untitled | 未命名标签页 |
| computer.tab.switch_hint | 接管后可切换标签页 |
| computer.release.note | 给 Bot 留一句说明（可选） |
| computer.fullscreen.enter | 全屏查看 |
| computer.fullscreen.exit | 退出全屏 |
| computer.tabs.label | 浏览器标签页 |

## settings · 设置

| key | zh-CN |
|-----|-------|
| settings.section.general | 通用 |
| settings.section.memory | 记忆 |
| memory.desc | 这里是用户记忆：所有 Bot 共享的、关于你的习惯和偏好。「记忆整理」会根据你说的话自动记下，你也可以直接修改、删除或添加。修改后，所有 Bot 在下一次思考前就会知道。 |
| memory.profile.title | 关于你 |
| memory.profile.name | 称呼 |
| memory.profile.name_hint | Bot 怎么叫你 |
| memory.profile.language | 语言 |
| memory.profile.language_system | 跟随系统 |
| memory.profile.identity | 身份 |
| memory.profile.identity_hint | 一句话介绍你，例如「独立开发者，主要做 Rust 和 Android」 |
| memory.profile.timezone | 时区 |
| memory.profile.timezone_value | {tz} · 跟随 Host |
| memory.profile.saved | 已保存 |
| memory.source.curator | 记忆整理 根据 |
| memory.usage | 用量 |
| memory.usage_value | {used} / {limit} 字 |
| memory.usage_hint | 接近上限时，Bot 会自己合并相近的条目；你也可以删掉不需要的。 |
| memory.search | 搜索记忆 |
| memory.add | 添加 |
| memory.source.bot | {bot} 记下 |
| memory.source.user | 你手动添加 |
| memory.edit.count | {count} / 500 字 · ⌘↵ 保存 |
| memory.deleted | 已删除一条记忆 |
| memory.undo | 撤销 |
| memory.conflict | {who} 刚刚也修改了这条记忆（版本 {n}）。查看对方的版本后再决定保存哪一份 |
| memory.conflict.mine | 用我的 |
| memory.conflict.theirs | 用对方的 |
| memory.quota_full | 记忆已满，请先删除或合并几条 |
| memory.delete_entry | 删除这条记忆 |
| memory.mobile_hint | 左滑条目可以删除，点击条目编辑 |
| settings.section.models | 模型与服务商 |
| settings.section.runtime | 运行与并发 |
| settings.section.tools | 工具与审批 |
| settings.section.skills | 技能 |
| settings.section.computer | 电脑与浏览器 |
| settings.section.hosts | Host 连接 |
| settings.section.about | 关于 |
| settings.appearance | 外观 |
| settings.theme | 主题 |
| settings.theme.light | 浅色 |
| settings.theme.dark | 深色 |
| settings.theme.system | 跟随系统 |
| settings.translucent | 半透明材质 |
| settings.translucent_hint | 系统开启「降低透明度」时自动关闭 |
| settings.font_size | 消息字号 |
| settings.language | 语言 |
| settings.notify.need_you | 需要你（审批、提问、接管） |
| settings.notify.review | 待验收 |
| settings.notify.dm_mention | 私聊和 @ 你的消息 |
| settings.notify.all_group | 群里所有消息 |
| settings.notify.sound | 提示音 |
| settings.launch_at_login | 登录时打开 Mac Bot |
| settings.keep_in_menubar | 关闭窗口后留在菜单栏 |
| settings.models.keychain_hint | API Key 保存在 Bot 主机的钥匙串里 |
| settings.models.add_provider | 添加服务商 |
| settings.models.fetch | 拉取模型 |
| settings.models.test | 测试连接 |
| settings.models.connected | ● 已连接 |
| settings.models.key_invalid | ! Key 无效 |
| settings.runtime.global | 全局最多同时运行的任务 |
| settings.runtime.per_bot | 每个 Bot 默认并发 |
| settings.runtime.sub_per_task | 每个任务最多同时跑子代理 |
| settings.runtime.sub_global | 全局最多同时跑子代理 |
| settings.runtime.loop_limit | Bot 之间连续往返上限 |
| settings.tools.defaults | 新 Bot 默认开启的工具 |
| settings.tools.approval_mode | 审批模式 |
| settings.tools.approval_mode.auto | 自动允许 |
| settings.tools.approval_mode.ask | 先问我 |
| settings.tools.approval_mode.auto_hint | 自动允许：只有命中「先问我」规则的操作才需要你审批（默认） |
| settings.tools.rule.scope_all | 所有 Bot |
| settings.tools.rule.builtin | 内置 |
| settings.computer.perm.granted | 已授权 |
| settings.computer.perm.denied | 未授权 |
| settings.computer.perm.unknown | 未知 |
| settings.tools.rules | 规则（冲突时「先问我」优先） |
| settings.tools.rule.ask | 先问我 |
| settings.tools.rule.allow | 自动允许 |
| settings.tools.add_rule | 添加规则 |
| settings.computer.permissions | Bot 主机的系统权限 |
| settings.computer.permissions_hint | 权限状态来自运行 macbotd 的 Mac。 |
| settings.computer.permission.screen_recording | 屏幕录制 |
| settings.computer.permission.accessibility | 辅助功能 |
| settings.computer.permission.full_disk_access | 完全磁盘访问 |
| settings.computer.permission.chrome | Chrome 自动化 |
| settings.computer.open_system_settings | 去系统设置 |
| settings.about.version | 客户端 {client} · 协议 {protocol} · Host macbotd {server} |
| settings.about.check_update | 检查更新 |
| settings.about.export_diag | 导出诊断信息 |

## notify · 系统通知

| key | zh-CN |
|-----|-------|
| notify.need_you.title | 需要你 · {bot} |
| notify.approval.body | 审批：{summary} |
| notify.review.title | 待验收 · 总管 |
| notify.review.body | 「{project}」做完了，请你验收 |
| notify.action.allow_once | 允许一次 |
| notify.action.deny | 拒绝 |
| notify.action.confirm | 确认完成 |
| notify.action.open | 打开 |

## admin · Web 管理页

| key | zh-CN |
|-----|-------|
| admin.setup.title | 设置访问密码 |
| admin.setup.desc | 这个页面只有本机能打开。设置后客户端用它连接。 |
| admin.setup.confirm | 确认密码 |
| admin.setup.strength | 强度 {level} |
| admin.setup.hint | 完成后，在客户端「添加 Host」里填写 {address} + 这个密码 |
| admin.title | Mac Bot 管理 |
| admin.logout | 退出 |
| admin.kpi.status | 状态 |
| admin.kpi.running | 运行中 |
| admin.kpi.uptime | 已运行 {duration} |
| admin.kpi.clients | 在线客户端 |
| admin.kpi.bots | Bot |
| admin.kpi.browser | 浏览器 |
| admin.service | 服务 |
| admin.port_restart_hint | 修改后需要重启 |
| admin.change_password | 修改密码 |
| admin.restart | 重启服务 |
| admin.save_restart | 保存并重启 |
| admin.logs | 日志 |
| admin.logs.warn | 警告 |
| admin.logs.error | 错误 |
| admin.logs.live | 实时 |
