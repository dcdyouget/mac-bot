# DECISIONS.md：决定记录

> 只记仍然有效的产品和技术决定，一条一行，新的写在最上面。被推翻的决定直接改写或删除，并在提交说明里写明。2026-10-11 之前的多开发线协调记录见 `docs/archive/COORDINATION-2026-10-09_11.md`。

## 开发方式
- [2026-10-12] `--mock` 提供需认证且按请求 ID 幂等的 `mock.trigger_bot_dm` 测试方法，用于客户端真实 `message.created` 通知验收；正式服务端和公共协议不提供此方法。
- [2026-10-12] v1.0 记忆批次采用 3 个独立 worktree + 3 个 subagent 并行（server / desktop / android），主 agent 在 main 侧负责合并、设计原型和共享文档；subagent 之间的代码隔离靠不同目录保证，合并顺序为 server → desktop → android，零冲突。
- [2026-10-11] Tauri 迁移分批进行：先保留 GPUI 作为功能对照；新界面追平后才删除旧代码。三种外观在 macOS 使用 window-vibrancy，系统「降低透明度」开启时关闭材质。
- [2026-10-11] 一个主 agent 负责全部代码，按模块派 subagent；只保留 `main` 一条长期分支；不再设集成线（AGENTS.md）。
- [2026-10-11] macOS 桌面客户端界面层从 GPUI 迁移到 Tauri 2 + React；`macbot-client-core`（网络层、数据层）不变。
- [2026-10-09] 服务端使用 JSON 文件存储，不引入数据库（PLAN 5.2）。

## 记忆与上下文
- [2026-10-13] 用户记忆来源行的时间表示该条记忆最初形成的时间，优先显示 `created_at`；后续编辑不会把来源时间改成 `updated_at`。旧数据没有 `created_at` 时回退 `updated_at`。
- [2026-10-12] 用户记忆整理按会话持久游标批处理；记忆写入前保存可恢复操作收据，删除保护、事件与修改日志可幂等补齐；失败状态和退避在 `/admin/status.memory_curator` 查询。
- [2026-10-12] `data/memory/changelog.jsonl` 启动裁剪：先按 operation receipt 补齐未完成写入，再清掉无法确认提交的尾部，最后按 seen_revision + 7 天保留历史增量。`macbot-memory::MemoryStore::prune_memory_changes` 负责这一步。
- [2026-10-12] `memory.user.create/update/delete` 与 `memory.profile.update` 的错误响应统一为 `{ code, message, error.details.current? }`，`quota_full` 时 `details.current.entries` 带该范围的条目目录；`memory.user.list/create/update` 成功时返回 `{ entry, quota }`，方便客户端原地更新。
- [2026-10-12] 旧 Bot 写的 `MemoryEntry.source.by = { kind: "bot" }` 仍按「Bot 名 记下」在设置 → 记忆里显示以保留可追溯性；新数据不会再产生（所有 Bot 工具被禁止 `scope = user`）。
- [2026-10-12] 上下文组装末尾的快照缓存写失败不再提前终止 run（fix v1.0 #1）：`MemoryError` 包成 `role: tool, is_error: true` 的 tool_result 注入下一次模型请求，同时持久化 `memory.failed` 事件，调用 ID 与原工具错误一致。执行路径为 `macbot-gateway/src/execution.rs` 的 `prepare_model_context` 与 `macbot-gateway/src/memory_tools.rs` 的 IO 错误包装。
- [2026-10-12] Android 档案字段点击进入 `MemoryProfileEditScreen`（fix v1.0 #2），不是列表内原页编辑（参者 `design/04-mobile.md` 记忆段「只读行进入单项编辑页」）。
- [2026-10-11] 用户档案在「设置 → 记忆」可编辑（`memory.profile.*`）；整理线程写的记忆 `source.by = system`，界面同时显示「记忆整理」和触发它的会话。
- [2026-10-11] 用户偏好只由独立的用户记忆整理线程（curator，maintenance 模型，只看用户本人消息）和用户本人写；所有 Bot（含主 Bot）不写。curator 可更新用户手动改过的条目，但只依据更晚的用户消息（PLAN 5.5.9）。
- [2026-10-11] 新增用户档案（称呼、语言、身份可选；时区取 Host），首次连接时主 Bot 在私聊里对话式初始化，之后变化记为偏好（PLAN 5.5.8）。
- [2026-10-12] 档案语言为空时，每条 `chat.send` 可带发送时客户端系统语言 `client_locale`；该字段随消息补发，档案显式语言优先，旧客户端回退 Host 默认语言，不修改 `macbot-client-core`。
- [2026-10-11] 核心记忆 = 用户档案 + 用户偏好 + 本 Bot 经验，全文进 L2；项目记忆、工作记录只放目录。
- [2026-10-11] 记忆写入立即提交，replace / remove 带 `expected_version` 做冲突检查，不再 run 结束统一提交（PLAN 5.5.1）。
- [2026-10-11] L2 只放核心记忆（用户记忆 + Bot 经验）全文和大块记忆目录；项目记忆、工作记录按需用 `memory_search` 调取。
- [2026-10-11] 段内记忆变化通过增量消息追加到上下文，L0–L2 段内不变；80% 压缩时先提取记忆，再用最新记忆重建 L2（PLAN 5.5.3）。
- [2026-10-11] 记忆增量消息用 `<memory_update>` 文本块放在下一条 user 消息或 tool_result 前面，不新增 system 消息；每段累计上限 1,000 字。用户在客户端修改用户记忆，也走同一套版本和增量消息。
- [2026-10-11] 记忆增加条目版本号和只追加的修改日志 `data/memory/changelog.jsonl`，段记录 `seen_revision`（PLAN 5.5.7）。
- [2026-10-12] 用户记忆与档案写入将请求签名、原响应和待发布事件随记忆快照原子保存；同一 `client_request_id` 重试恢复原响应并补发事件，不重复修改。

## 产品与界面
- [2026-10-14] 主 Bot 默认名为「秘书长」；仅将仍使用旧默认名「总管」的存量主 Bot 安全迁移，自定义名称不覆盖。主 Bot 只保留消息、澄清、成员发现、委派、项目编排和状态跟进工具；私聊可启动隔离的只读 subagent，具体执行必须交给普通 Bot。
- [2026-10-14] 群聊消息区顶部不再显示公告条；公告仍保留在群信息右面板中，协议和历史数据不删除。
- [2026-10-12] “已加载技能”只在服务端成功将技能正文加入 Bot 模型上下文后显示；原用户消息用可选 `loaded_skills` 记录结果并通过 `message.updated` 持久更新，`delivery.read` 不代表技能加载。
- [2026-10-12] macOS 打包版无开发者签名身份时仍以固定 Bundle ID `bot.mac.desktop` 做 ad hoc 代码签名；通知只在 App 非前台显示系统横幅，前台其他会话仅播放提示音，当前会话不提示。
- [2026-10-12] Android 半透明实验暂不合入：API 36 模拟器上的同窗模糊在实际聊天底栏与 sheet 未达到设计稿可辨识效果，性能对照也未通过；完成可见效果和实体设备帧时/内存验收后再启用该模式。
- [2026-10-12] 服务端实际恢复持久任务时更新 `Assignment.recovered_at` 并发布 `assignment.updated`；任务 `status` 仍表示执行状态，客户端用恢复时间展示「已恢复」，正常新任务为 null。
- [2026-10-12] 桌面斜杠技能选择的「最近使用」只按服务端记录的 `Skill.last_invoked_at` 取近 7 天并排序；技能编辑时间与调用次数不代替实际调用时间。
- [2026-10-11] **用户记忆可以在客户端查看和编辑**（设置 → 记忆；手机：我的 → 记忆），推翻之前「客户端不展示记忆」的决定；Bot 记忆和项目记忆仍不展示。协议见 PROTOCOL 5.10a。
- [2026-10-11] 历史提问卡片通过 `question.list` 批量恢复已答与过期状态；审批创建时保存 `matched_rule_text` 快照，旧记录缺失时显示空值。
- [2026-10-11] `host.permissions` 对屏幕录制和辅助功能做系统预检；完全磁盘访问和 Chrome 自动化没有无副作用的可靠预检时返回 `unknown`，客户端如实显示。
- [2026-10-11] 审批模式只有两档，默认「自动允许」（只有命中「先问我」规则才审批）；另一档「先问我」。审批规则带作用范围（某个 Bot 或所有 Bot），内置规则只读。
- [2026-10-11] 待看消息：Bot 交付前由服务端一次性交给它查看（queue.review），任务带 `pending_messages` 计数。
- [2026-10-12] 用户的聊天已读游标与 Bot 的待看送达状态独立；只要目标任务的 delivery 仍为 queued，打开群聊后仍可「改为插话」。
- [2026-10-11] Bot 在消息里提到某个群时可以附群聊卡片：`send_msg.projects` → `project_card(context = mentioned)`。
- [2026-10-11] 「设置 → 电脑与浏览器」显示 Bot 主机的系统权限：`host.permissions`。
- [2026-10-11] 只有 @ 某个 Bot 或回复它的消息才打断它（下一步生效）；其余群消息进入它的「待看消息」，交付前一起查看（delivery.state = queued）。
- [2026-10-11] 右键消息可以「回复」（交给原发送者）或「引用」（不指定谁回应）：`chat.send.quote_mode = reply | reference`。
- [2026-10-11] 三端唯一提示音：`design/sounds/04-bubble-pop`（啵啵 · 气泡）。
- [2026-10-10] 不做线程，所有沟通都在群里；`reply_to`、`thread_count`、`chat.thread` 不再使用。
- [2026-10-10] 公告是一段自由文本（Markdown 显示），Bot 用 `update_announcement` 工具、用户用 `announcement.update` 修改；没有「流程」等固定字段。
- [2026-10-10] 一条消息里的多个 `question` 块合成堆叠卡片；选项在 Bot 的消息里，回答后原地变成已回答的样子。
- [2026-10-10] 主 Bot 戴黑框眼镜（秘书形象）；Android 与 iOS 外观完全一致；三端图标统一用 Lucide。
- [2026-10-10] 界面视觉、像素、文案 key 以 `design/` 为准；DESIGN.md 只管产品行为。
- [2026-10-10] 手机端功能与桌面一致（包括完整编辑定时任务），只是三栏拆成页面栈。

## 待处理
- 客户端逐屏迁移与验收仍在进行。
- 等 Mac mini 解锁后补 `clients/mac/progress/S9/memory/0{1..5}-*.png` 与 Android 模拟器上的 5 张 `clients/mobile/progress/memory/0{1..5}-*.png`（default / profile-edit / conflict / quota-full / onboarded），以及群聊中收到 `<memory_update>` 的运行轨迹截图。
- 评估 3B 及以下本地模型（Ollama / LM Studio / MLX 的 OpenAI 兼容接口）作为 curator「是否含偏好」预筛与本地 embedding 做记忆去重/语义检索（PLAN 5.5.9 后续优化）。
