# Mac Bot 规划 v0.3

> 状态：规划中，尚未开始编码。
> v0.3 变更：移动端增加 iOS，改用 Kotlin Multiplatform + Compose Multiplatform；补充开发环境说明。
> v0.2 变更：去掉虚拟机和沙箱；改为多 Bot，每个 Bot 有独立工作间；新增群聊和 Bot 间协作；电脑操控改为控制系统自带的浏览器，并推迟到后续阶段；交互全面对齐 Grok Bot；Android 改用原生技术栈。

## 1. 定位

**自托管版 Grok Bot。** 一个人拥有一支 Bot 团队，这些 Bot 运行在自己 24 小时常开的 Apple Silicon Mac 上。每个 Bot 有名字和明确职责；Bot 之间可以拉群、互发消息、交接任务。用户在 macOS、Windows、Android 或 iOS 客户端上填入 `host:port` 并完成配对后，就可以像和同事聊天一样给 Bot 派活。

## 2. 已确认的决策

| 项 | 决策 |
|----|------|
| 部署 | 服务端运行在 M 系列 Mac 上，16 GB 内存；**不用虚拟机，不做沙箱**（沙箱作为远期可选项） |
| 网络 | 服务端监听一个固定端口，客户端填 `host:port` 直连。公网代理由用户自行解决，不在本项目范围内 |
| 安全 | v1 只做**设备配对和令牌鉴权**；TLS、审计等以后再做 |
| Bot | 支持多个 Bot，**每个 Bot 有独立工作间**（目录、记忆、会话、定时任务）；支持**群聊和 Bot 间消息** |
| 电脑操控 | 以网页任务为主，**使用系统浏览器**（复用已有的登录凭证）；放到后续阶段，先把 Bot 形态跑通 |
| 系统权限 | 屏幕录制和辅助功能权限**不强制**；没授权时提示「部分功能不可用」 |
| 模型 | 用户自定义 provider 和模型；默认认为模型支持看图 |
| 存储 | SQLite |
| 桌面端 | Rust + GPUI（macOS / Windows） |
| 移动端 | **Android（小米 17）+ iOS**：**Kotlin Multiplatform + Compose Multiplatform**，一套代码同时出 Android 和 iOS 两端，UI、网络、状态全部共享。Android 端本身就是原生 Compose，APK 小；Compose 的 iOS 支持从 1.8.0 起已经稳定 |
| 通知 | Android：前台服务保持长连接，收到事件后弹系统通知。iOS：App 进后台后无法保持长连接，必须走 **APNs**，由 macbot-server 直接调用 APNs HTTP/2 接口，使用用户自己的 .p8 密钥（需要 Apple 开发者账号）；没有配置时，只在 App 前台运行期间通知 |
| 签名和公证 | 由用户负责 |
| 交互和页面 | **全面参考 Grok Bot** |
| 断电恢复 | 暂不考虑 |

## 3. 参考项目与用途

| 项目 | 借鉴点 |
|------|--------|
| **Grok Bot**（[docs.x.ai/grok-bot](https://docs.x.ai/grok-bot/overview)） | 产品形态、页面结构、交互细节（见第 4 节） |
| **hermes-agent**（[GitHub](https://github.com/NousResearch/hermes-agent)） | 记忆系统：两份有字数上限的 curated 笔记 + 快照注入 + 会话全文检索 |
| **pi / pi-ai**（[GitHub](https://github.com/earendil-works/pi/tree/main/packages/ai)） | Provider 层：按 API 类型抽象、模型目录（能力和价格）、任意 OpenAI 兼容端点 |
| **pi / pi-durable**（[GitHub](https://github.com/earendil-works/pi/tree/main/packages/durable)） | 持久化运行：Entry、Commit、Task checkpoint、inbox 排队、`requestId` 幂等、resume、compaction |
| **OpenClaw**（[多 Agent 文档](https://docs.openclaw.ai/multi-agent)） | 每个 agent 一套独立的 workspace 和会话存储；默认隔离，显式开启跨 agent 通信；设备配对 |
| **LobeHub Agent Groups**（[RFC 130](https://lobehub.com/blog/rfc-130)） | 群聊编排：supervisor 决定下一个发言者，以及公开发言还是私信 |
| **AutoGen GroupChat** | Selector 模式：由 LLM 选下一个发言者，并设置终止条件 |
| **Playwright MCP 扩展模式**、**mcp-chrome**、**Browser MCP**、**real-browser-mcp** | 通过 **Chrome 扩展**控制用户日常使用的浏览器，直接复用登录态（Chrome 136 起，默认 profile 不再允许 `--remote-debugging-port`，扩展是正路） |
| **cua-driver**（trycua/cua，MIT） | 后台控制原生 Mac App，**不抢鼠标、不抢焦点**（可选，需要系统权限） |
| **Peekaboo**（openclaw/peekaboo，MIT） | macOS 截图和 GUI 自动化 CLI/MCP（可选） |
| **gpui-kit**（[longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)，原名 gpui-component，Apache-2.0，约 1.6 万 star） | 桌面端组件库：75+ 组件，包括 Markdown/HTML 渲染、不等高虚拟列表（消息流）、Dock 和可拖拽面板、表单、浮层、菜单、主题；Longbridge Pro 在生产环境使用；自带给 AI 编程助手用的 skills |

> cua 和 lume：lume 是在 Apple Silicon 上管理 macOS/Linux 虚拟机的工具，cua 是跑在虚拟机里的电脑操控 Agent 框架。我们不用虚拟机，所以**不采用 lume**；同一仓库里的 **cua-driver**（不用虚拟机、直接后台操控本机 App）可以作为原生桌面操控的候选。

## 4. 从 Grok Bot 提炼的交互规格

### 4.1 Bot
- 创建入口：侧栏点 **New** 或按 `Cmd/Ctrl+N`，打开 **New chat** 面板，选 **Create new Bot**；也可以直接输入名字后选 **Create "xxx" Bot**。
- 资料字段：**Name**、**Label**（头衔，例如「Daily X tech scout」）、**Description**（长期规则，例如「未经批准不得对外发消息」）、**Avatar**，以及 Mac Bot 新增的 **Model**。
- 菜单操作：Edit Profile、**Pin**（置顶）、**Hide from sidebar**（隐藏后仍在运行，可在 Hidden Bots 中恢复）、**Duplicate**（复制资料、技能和定时任务，不复制历史和记忆）、Delete。
- 每个 Bot 有自己的会话、记忆和定时任务。提倡「一个 Bot 专做一件事」。

### 4.2 对话
- 输入框：文本、粘贴图片和链接、附件；`@` 可提及 Bot、群、定时任务；`/` 可引用技能；语音输入放到后期。
- 消息支持 **Reply**（回复某条消息，形成线程）和表情回应。
- **插话**：Bot 执行任务时，用户的新消息优先级更高，可以改变当前这一轮的方向。
- **Stop**：立即停止，但不撤销已经完成的动作。
- 会话流里除了普通消息，还会展示：工具活动（可折叠）、电脑操作、生成的文件、提问、**审批请求**、草稿卡片（发送 / 丢弃）。

### 4.3 群聊
- 在 New chat 里选 **2 到 6 个 Bot**，自动生成群名（可改），之后可以增减成员。
- 不 @ 任何人：由参与的 Bot 自己决定谁回应（实现见 5.4）。
- `@Bot`：指定负责人；`@everyone` 慎用。
- 对某个 Bot 的消息点 **Reply**，就只发给这个 Bot。
- **群里不出现**审批请求、密码或登录请求、外发草稿，这些一律回到对应 Bot 的私聊里。
- Bot 在群里的交接消息只有文字；需要给对方看图时，Bot 直接私发。

### 4.4 Bot 之间的协作
- Bot 可以**异步**给另一个 Bot 发消息：对方被唤醒、处理请求，之后再回复；整个交接过程用户可见。
- **任务归属**：每个阶段只有一个负责人；用 `handoff` 把归属交给另一个 Bot。
- 典型场景：某个系统归谁管、专家评审、被另一个角色卡住、长时间运行的任务。

### 4.5 审批
- 卡片上的选项：**Allow once**、**Deny**、**Always allow**。
- 全局 **Auto Review** 有两档：Require Approval / Always Allow。另外可以用自然语言写允许和拒绝规则（在 Settings → Agent 里配置）。
- 审批只在私聊里出现；手机上会推送通知。

### 4.6 技能与定时任务（Skills & Routines）
- **Skill**：可复用的做事说明，包括何时使用、需要什么输入和权限、步骤、校验方式、产出格式、哪些步骤需要审批。所有 Bot 共享，在输入框里用 `/` 引用。「示范一次生成技能」放到后期。
- **Routine**：某个 Bot 按计划执行一件事。通过和 Bot 对话来创建；可以查看下次运行时间；每个 Bot 最多 50 个 Routine，每个 Routine 保留最近 20 条运行记录，最短间隔 5 分钟。
- 管理入口：Bot → **View conversation details** → **Routines**，可以启用/暂停、**Test run**、编辑、查看历史、删除。

### 4.7 电脑（后期阶段）
- **每个 Bot 有自己的「屏幕」**：在 Mac Bot 里就是一个独立的 Chrome 窗口或标签组。同一块屏幕同一时间只跑一个电脑操作任务，不同 Bot 之间可以并行。
- 在会话中打开 **Agent Computer** 可以看实时画面；关掉预览后任务继续执行。
- **接管**：遇到密码、2FA、验证码、支付时，Bot 暂停，用户接管完成后告诉 Bot 继续。
- 登录态在所有 Bot 之间共享（因为用的是同一个系统浏览器），这一点和 Grok Bot 一致。

### 4.8 移动端
- 首页：会话列表，右上角 **+** 可新建 Bot 或群聊；全局搜索，范围包括消息、Bot、群聊、文件、定时任务。
- 聊天：完整支持 @、Reply、附件、审批、草稿发送和丢弃。
- 定时任务：可以查看和暂停/恢复；编辑和 Test run 只能在桌面端做（与 Grok 一致）。
- 通知：Bot 有结果、有提问、需要审批时推送。

## 5. 架构

```
┌──────────────── Mac（macbot-server，单进程）─────────────────────┐
│ Gateway  axum + WebSocket，固定端口（默认 7788）                    │
│   配对 / 令牌鉴权 / 协议版本 / 事件流（按 seq 断线续传）               │
│                                                                   │
│ Orchestrator（群聊路由、Bot 间消息、handoff、防循环）                 │
│   │                                                               │
│ Bot Runtime × N（每个 Bot 一个 actor，各有一个 inbox，同时只跑一个 run）│
│   ├ Durable Engine（Rust 版 pi-durable）                           │
│   ├ Providers（Rust 版 pi-ai）                                     │
│   ├ Memory（hermes 风格）                                          │
│   └ Tools：workspace 文件 / shell（需审批）/ web_fetch /            │
│           send_message / handoff / memory / session_search /       │
│           routine / ask_user / [后期] browser / desktop / MCP       │
│ Scheduler（Routines）                                              │
│ Store：SQLite（rusqlite + FTS5）  Secrets：macOS 钥匙串             │
│ 文件：~/MacBot/bots/<bot>/workspace   ~/MacBot/shared               │
└───────────────────────────────────────────────────────────────────┘
        ▲ ws://host:port                     ▲
 ┌──────┴──────────┐            ┌────────────┴─────────────┐
 │ Desktop（GPUI）  │            │ Mobile（Compose MP）      │
 │ macOS / Windows  │            │ Android + iOS，Ktor WS    │
 └─────────────────┘            │ iOS 后台通知走 APNs        │
                                └──────────────────────────┘
        ▲ 后期：Chrome 扩展「Mac Bot Connector」通过 localhost 连接服务端
```

### 5.1 仓库结构

```
mac-bot/
├── crates/
│   ├── macbot-protocol      # 消息与事件定义（serde），协议版本
│   ├── macbot-store         # SQLite 表结构、迁移、FTS5
│   ├── macbot-durable       # Entry / Commit / Task / Inbox / Resume / Compaction
│   ├── macbot-providers     # LLM provider 与模型目录
│   ├── macbot-memory        # curated memory + session_search
│   ├── macbot-orchestrator  # 群聊路由、Bot 间消息、handoff
│   ├── macbot-tools         # 内置工具；后期加入 browser、desktop、mcp
│   ├── macbot-server        # 守护进程
│   └── macbot-client        # 桌面端共用的协议客户端（重连、本地缓存）
├── apps/
│   ├── desktop/             # gpui-kit（只依赖这一个 crate，它会固定匹配的 GPUI 版本）
│   ├── mobile/              # Kotlin Multiplatform + Compose Multiplatform
│   │   ├── shared/          #   共享代码：UI、ViewModel、Ktor WebSocket、kotlinx.serialization、本地缓存
│   │   ├── androidApp/      #   Android 外壳（前台服务、通知）
│   │   └── iosApp/          #   Xcode 工程外壳（APNs 注册）
│   └── chrome-extension/    # 后期
└── docs/
```

在 macOS 上分发时，桌面 App 内置服务端（作为 LoginItem 或 LaunchAgent 运行）。首次启动时选择「把这台 Mac 作为 Bot 主机」，界面会显示配对码；如果只想当客户端，就去连接别的主机。

### 5.2 数据模型（SQLite 草案）

```
bots(id, name, label, description, avatar, model_ref, pinned, hidden, created_at)
chats(id, kind[direct|group|bot_dm], title, created_at)
chat_members(chat_id, member_kind[user|bot], member_id)
messages(id, chat_id, seq, sender_kind, sender_id, reply_to, mentions, content_json, created_at)   -- FTS5
runs(id, bot_id, chat_id, trigger_message_id, status, owner_task_id, started_at, ended_at)
entries(id, run_id|conversation_id, seq, kind, json)       -- 不可变，参照 pi-durable 的 Entry
tasks(id, owner, kind, status, checkpoint_json, updated_at) -- 可恢复的状态机
submissions(id, bot_id, request_id UNIQUE, status)          -- 幂等
approvals(id, bot_id, run_id, tool, args_json, status, decision, rule_id)
memories(id, scope[user|bot], bot_id, kind[profile|notes], content, updated_at)
skills(id, name, description, body_md, updated_at)
routines(id, bot_id, name, schedule, tz, instructions, enabled, next_run_at)
routine_runs(id, routine_id, run_id, status, started_at)   -- 每个 Routine 只保留 20 条
files(id, bot_id, chat_id, path, mime, size, created_at)
providers(id, name, api_kind, base_url, secret_ref)
models(id, provider_id, model_id, caps_json, context_window, cost_json)
devices(id, name, platform, token_hash, last_seen_at, revoked)
usage(id, bot_id, run_id, model_id, input_tokens, output_tokens, cost)
events(seq, chat_id, type, payload_json)                    -- 推送和断线补发
```

用户可见的**消息层**（messages）与 Bot 内部的**运行层**（runs、entries、tasks）分开存储。工具活动这类运行层事件以「活动卡片」的形式挂在对应 Bot 的消息下面。

### 5.3 Bot 运行模型
- 每个 Bot 是一个 actor，有一个持久化的 **inbox**。消息来源包括：用户私聊、群聊里被路由到它、其他 Bot 的消息、Routine 触发。
- 同一时间只执行一个 run。用户私聊的新消息作为 **steer** 插入当前 run，在下一轮生效；其他来源的消息排队。
- 每一步都先提交到 SQLite 再推送给客户端。进程重启后自动 resume。有副作用的工具调用标记为「不可安全重放」，resume 到这类调用时先询问用户。
- 上下文组装顺序：system 提示（Bot 资料 + 用户画像快照 + Bot 笔记快照 + 可用技能清单）→ 当前会话最近的消息（群聊时是群的最近 N 条）→ 本次 run 的内部 entries。

### 5.4 群聊路由（核心算法）
1. 用户消息 @ 了某些 Bot：只投递给被 @ 的 Bot。
2. 用户 Reply 了某个 Bot 的消息：只投递给这个 Bot。
3. 两者都没有：**路由器**做一次轻量 LLM 调用，输入是成员资料、群目标、当前负责人和最近消息，输出 0 到 n 个应回应的 Bot，以及各自的理由。默认倾向当前负责人。参考 AutoGen selector 和 LobeHub supervisor。
4. Bot 在群里发言时 @ 了其他 Bot：触发被 @ 的 Bot。
5. **防循环**：一条用户消息引发的 Bot 间往返设硬上限（默认 8 跳），超过就暂停并请用户介入；同一个 Bot 对同一个触发只回应一次；Bot 可以选择「沉默」。
6. **归属**：群上记录 `current_owner`，`handoff(to, summary)` 更新归属，并在群里显示交接卡片。

Bot 之间私聊（`send_message(to_bot, text)`）走 `bot_dm` 类型的会话。用户可以查看，但默认不显示在主列表，而是在双方会话里以「交接」卡片的形式出现。

### 5.5 记忆（hermes 风格）
- **用户画像**（`scope=user`，所有 Bot 共享，有字数上限）：用户是谁、有什么偏好。
- **Bot 笔记**（`scope=bot`，每个 Bot 独立，有字数上限）：这个 Bot 在自己职责范围内学到的东西。
- 会话开始时把两者以快照形式注入（保证 prompt cache 稳定）；运行中通过 `memory` 工具增删改，下次会话生效。
- `session_search`：对 messages 做 FTS5 检索，用于回答「我之前说过什么」，默认只搜本 Bot 参与过的会话。
- 客户端在 Bot 详情 → Memory 页里可以查看、编辑、删除。

### 5.6 Provider
- API 类型：`openai-completions`、`openai-responses`、`anthropic-messages`、`google-generative`。
- 用户新增 provider 时填 base_url、key（存入钥匙串）和 API 类型，然后添加模型 ID 或从 `/models` 拉取列表。
- 统一的流式事件：TextDelta、ThinkingDelta、ToolCallDelta、Usage、Stop。
- 每个 Bot 单独选模型；群聊路由器可以单独配一个便宜的模型。

### 5.7 协议
- WebSocket，JSON 帧 `{v, id, type, payload}`，请求/响应和服务端推送并存。
- 重连时带上 `last_seq`，服务端从 `events` 表补发。
- 配对流程：服务端界面显示 6 位配对码（或者二维码，内含 host:port 和配对码）→ 客户端提交配对码和设备名 → 服务端返回设备令牌 → 之后每次连接带令牌。设备可在 Settings → Devices 里吊销。

### 5.8 电脑操控（后期阶段，先定方向）
- **浏览器（主线）**：自研 Chrome 扩展 **Mac Bot Connector**，装进用户日常使用的 Chrome（或其他 Chromium 系浏览器），通过 localhost WebSocket 连接 macbot-server。实现参考 Playwright MCP 扩展模式和 mcp-chrome。
  - 每个 Bot 一个独立窗口或标签组，作为它的「屏幕」。
  - 观察网页：精简后的 DOM / 无障碍树快照（参考 browser-use 的元素编号），截图作为补充。
  - 动作：navigate、click、type、scroll、extract。
  - 实时画面和接管：用 `chrome.debugger` 的 `Page.startScreencast` 推帧给客户端；客户端的点击和输入通过 `Input.dispatch*` 回放到页面。**不需要屏幕录制权限。**
  - 待验证：锁屏后被遮挡的 Chrome 窗口会被节流，影响截图和 screencast（DOM 操作一般不受影响）。
  - 过渡方案：先通过 MCP 客户端接入现成的 Playwright MCP（`--extension`），然后再换成自研扩展。
- **原生桌面（可选）**：接入 cua-driver 或 Peekaboo（MCP 子进程）。没有辅助功能或屏幕录制权限时自动禁用，并在 Settings → Computer 里说明原因。

## 6. 页面清单（桌面端）

| 区域 | 内容 |
|------|------|
| 侧栏 | New（`Cmd+N`）、Search、置顶的 Bot、Bot 列表、群聊列表、Hidden Bots、Skills、Settings；每项显示状态点（空闲 / 运行中 / 等待你） |
| 会话主区 | 头部（头像、名字、Label，以及后期的 Agent Computer 按钮、详情按钮）；消息流；输入框 |
| 会话详情抽屉 | Profile、Routines、Files、Memory、Members（群聊） |
| New chat 面板 | Create new Bot / 选 2 到 6 个 Bot 建群 |
| Skills 页 | 列表、编辑、新建 |
| Settings | Models & Providers、Agent（Auto Review 档位与允许/拒绝规则）、Devices、Computer（后期）、Appearance、Language、Usage、Host（端口、配对码） |
| Agent Computer | 后期：右侧面板，显示实时画面，提供接管按钮 |

## 7. 里程碑（按顺序推进，不先做技术验证）

| 阶段 | 内容 | 验收 |
|------|------|------|
| **P1 骨架** | Cargo workspace、protocol、store、providers（OpenAI 兼容 + Anthropic）、单个 Bot 私聊流式对话、durable resume、配对、GPUI 侧栏和聊天 | 桌面端填 host:port 并配对后能和 Bot 对话；服务端重启后正在进行的 run 能自动恢复 |
| **P2 多 Bot 与群聊** | Bot 增删改、Pin/Hide/Duplicate、独立 workspace 和基础工具、审批卡片、群聊路由、@ 和 Reply、Bot 间消息与 handoff、防循环 | 3 个 Bot 在群里分工完成一个任务，中间有交接；审批只出现在私聊里 |
| **P3 记忆与技能** | 用户画像和 Bot 笔记、session_search、Memory 页、Skills 和 `/` 引用 | 跨会话记住用户偏好，能回答「我上周说过什么」 |
| **P4 移动端** | Compose Multiplatform 客户端：会话列表、聊天、群聊、审批、通知、搜索；Android 前台服务；iOS 接入 APNs | 在小米 17 和 iPhone 上都能完成 P2 的场景 |
| **P5 Routines** | 调度器、通过对话创建、Test run、运行历史 | 「每天 9 点总结 xxx」按时执行并推送结果 |
| **P6 电脑操控** | 先用 Playwright MCP 扩展模式过渡，再上 Mac Bot Connector 扩展；Agent Computer 实时画面和接管；可选接入 cua-driver | 用系统 Chrome 已有的 X 登录态刷帖并总结 |
| **P7 打磨** | Windows 打包、语音、文件页、全局搜索、用量统计、自动更新 | |

## 8. 开发环境

| 用途 | 工具 | 备注 |
|------|------|------|
| 服务端和桌面端 | Rust stable（rustfmt、clippy），Xcode（提供 Metal 编译器，GPUI 需要） | crates.io 走清华 tuna 镜像 |
| GPUI | 只依赖 `gpui-kit` 0.7.x（它会 re-export GPUI、gpui-base、gpui-component 和 Lucide 图标），不单独依赖 `gpui` | Xcode 26 需要额外下载 Metal 工具链：`xcodebuild -downloadComponent MetalToolchain` |
| 移动端 | JDK 21、Gradle（项目内使用 wrapper）、Android SDK 36 + build-tools 36.1、platform-tools（adb） | 真机调试用 USB 或无线 adb 连接小米 17 |
| iOS | Xcode 26 + iOS Simulator 运行时；真机需要签名 | |
| Windows 客户端 | **只能在 Windows 上构建**（GPUI 的 Windows 后端需要在 Windows 上编译 DirectX 着色器），用 GitHub Actions 的 windows runner 构建 | |
| 浏览器扩展 | Chrome | P6 阶段 |

## 9. 暂不考虑（记录在案）
断电重启后自动恢复、TLS 和审计、公网代理、虚拟机或沙箱、示范一次生成技能、小米厂商推送、多用户。

## 10. 待确认
1. 日常使用的浏览器是 **Chrome**（或其他 Chromium 系）吗？如果是 Safari，第 5.8 节的方案需要重新设计。
2. 用户画像默认所有 Bot 共享、Bot 笔记各自独立，这样可以吗？
