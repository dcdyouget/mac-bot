# v1.0 更新说明

> 记录 v1.0 相对之前设计的改动。每条写清楚改了什么、为什么、影响哪些文档和模块。实现状态：**已实现**（服务端 5 commit、桌面端 18 commit、Android 19 commit；339 个 server 测试 + 140 个共享 / 37 个 Android 测试全过，clippy `-D warnings` 通过，桌面端 tsc/vite build 通过；截图待 Mac mini 解锁后补）。

## 2026-10-11 记忆与上下文改造

**目标**：用户只管一直和 Bot 对话，不用考虑开新会话；上下文管理在后台完成；记忆变化能实时生效。

### 改了什么

| # | 之前 | 现在 | 权威位置 | 状态 |
|---|------|------|---------|------|
| 1 | 记忆在 run 中暂存，run 成功结束才提交；提交失败只写服务端日志 | `memory` 工具调用时立即提交，返回条目 id、新版本号、剩余配额；失败直接返回给模型并发事件 | PLAN 5.5.1、5.5.5 | ✅ `server: persist versioned memory with recoverable changelog` |
| 2 | 并发修改后写的直接覆盖 | replace / remove 带 `expected_version`，版本不符返回冲突和当前内容 | PLAN 5.5.1 | ✅ 同上 + `enforce memory ownership` |
| 3 | 选中范围的记忆全部放进 L2 | L2 只放核心记忆（用户记忆 + Bot 经验）全文，项目记忆和工作记录只放目录，正文按需 `memory_search` | PLAN 5.5.1、5.5.4 | ✅ `server: deliver memory deltas through cached model context` |
| 4 | run 进行中看不到记忆变化；记忆被改会让快照过期并开新段 | 每次调用模型前检查全局版本，把可见范围内的变化作为增量消息追加到上下文；L0–L2 段内不变 | PLAN 5.5.3 | ✅ 同上 + 删 `memory_snapshot_stale` 触发器、修 `previous_snapshot=None` bug |
| 5 | 开新段时的「记忆提取」只写在文档里，代码未接入 | 80% 压缩时先跑记忆提取，再摘要，再用最新记忆重建 L2，清掉旧增量消息 | PLAN 5.5.3 | ✅ `server: cover memory deltas onboarding and segment transitions` |
| 6 | 无条目版本号、无修改日志 | 条目带 `version`；新增只追加的 `data/memory/changelog.jsonl`；每段记录 `seen_revision`；按最小已读位置 + 7 天保留清理 | PLAN 5.5.7 | ✅ `server: persist versioned memory with recoverable changelog` |
| 7 | 整个上下文是一个 system 字符串，没有缓存断点 | L0–L2 与 L3–L4 拆成两个 system block，Anthropic 在 L2 末尾加 `cache_control` | PLAN 5.5.4 | ✅ `server: deliver memory deltas through cached model context`（`macbot-providers` 加 cache_control） |
| 8 | 工作记录原样存整轮回复 | 存提炼后的「做了什么、结果如何」 | PLAN 5.5.5 | ✅ `server: enforce memory ownership and profile onboarding`（Bot 改走摘要路径） |

| 9 | 任何 Bot 都能写用户记忆 | 用户偏好只由独立的整理线程 curator（只看用户本人消息，批量处理）和用户本人写；Bot 不写 | PLAN 5.5.9 | ✅ `server: enforce memory ownership and profile onboarding`（Bot 写 user scope 被工具拒绝）+ `server: persist versioned memory with recoverable changelog`（curator tombstone 阻止按删除前消息重建） |
| 10 | 没有用户档案 | 新增用户档案（称呼、语言、身份可选；时区取 Host），首次连接时主 Bot 对话式初始化 | PLAN 5.5.8 | ✅ `server: enforce memory ownership and profile onboarding`（`set_user_profile` 临时工具 + onboard 标记 + 一次性提醒） |
| 11 | 「核心记忆」没有明确定义 | 核心记忆 = 用户档案 + 用户偏好 + 本 Bot 经验，约 4,000 字以内，全文进 L2 | PLAN 5.5.1 | ✅ `server: deliver memory deltas through cached model context`（`assemble_context` 按 5.5.2 输出 L2） |

### 为什么
- 原来的暂存提交会静默覆盖别人的修改，提交失败时整批丢失且无人知道。
- 全部按需调取会让模型忘记偏好，且新旧查询结果并存；全部放 system 又无法实时更新。分层 + 增量消息兼顾两者，并保持 prompt cache 稳定。

### 影响的模块
- **server**：5 个 commit —— `persist versioned memory with recoverable changelog`、`deliver memory deltas through cached model context`、`enforce memory ownership and profile onboarding`、`add memory conflict quota and profile mock scenarios`、`cover memory deltas onboarding and segment transitions`。`cargo test` 339 通过、`cargo clippy -p macbot-memory -p macbot-gateway -p macbot-providers -- -D warnings` 无警告。
- **protocol**：无需改动 —— `MemoryEntry` / `UserProfile` / `memory.user.*` / `memory.profile.*` / `memory.user.updated` / `memory.user.deleted` / `memory.profile.updated` 已在 PROTOCOL 5.10a 与 schema/fixtures/Kotlin 模型中。`python3 protocol/kotlin/generate.py --check` 报告 233 fixture 值 / 144 文件 / 253 schema 对象全部一致。
- **客户端**：
  - 桌面端 18 个 commit（v1.1 §1–§13 + v1.2 §1–§5），包含「关于你」档案卡片、来源行（system 走「记忆整理 根据 · 会话名 · 时间」、user「你手动添加」、bot 保留「Bot 名 记下」旧数据）。`pnpm tsc -p tsconfig.app.json --noEmit` exit 0，37/37 Node tests 通过，tauri debug build 成功。验收报告 `clients/mac/progress/S9/memory/verification.md`。
  - Android 19 个 commit（v1.1 §1–§13 + v1.2 §1–§5 + 1 笔验证记录），同样包含档案卡片、来源行。i18n key 补齐。`./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest` 全过（140 + 37 项），debug APK 生成。验收报告 `clients/mobile/progress/memory/VERIFICATION.md`。
  - 5 张模拟器/桌面截图：桌面端因 Mac mini 锁屏未取，Android 端因模拟器画面不稳定未取；按你之前的指示，等 Mac mini 解锁后派独立验证 subagent 补 5 张 `clients/{mac,mobile}/progress/{S9/memory,memory}/0{1..5}-*.png`，不伪造、不阻塞代码合入。

### 已定细节
- `memory` 工具返回值：成功返回 `entry_id`、`version`、`revision`、`remaining_chars`；失败返回 `quota_full` / `conflict` / `not_found` 及当前内容（PLAN 5.5.1）。
- 增量消息：`<memory_update revision="…">` 文本块，放在下一条 user 消息或 tool_result 最前面；同一条目只保留最终状态；不通知 Bot 自己写的修改；大块记忆只给 id 和主题（PLAN 5.5.3）。
- 每段增量消息累计上限 1,000 字，超出即开新段重建 L2。

### 实施期间追加的决定（同步到 DECISIONS.md）
- **changelog 启动裁剪**与 PLAN 5.5.7 字面略不同：实施发现按「`revision < state.revision` 一律裁」会丢失 `state.json` 写完但 `revision` 还没回写时的恢复机会，改为「先按 operation receipt 补齐未完成写入，再清掉无法确认提交的尾部，最后按 seen_revision + 7 天保留历史增量」。
- **`memory.user.create/update/delete` 协议字段**：mock / 客户端都按 `{ entry, quota }` 形式返回，错误时携带 `code: quota_full | conflict | not_found` 和 `error.details.current`（更新 / 删除冲突时是当前条目，quota_full 时是该范围的目录）。
- **旧 Bot 写的数据保留**：`MemoryEntry.source.by = { kind: "bot" }` 视为 v1.0 之前的旧数据，**界面仍然显示「Bot 名 记下」**以保留可追溯性，但 Bot 工具现已禁止 `scope = user`，新数据不会再产生这种来源。
