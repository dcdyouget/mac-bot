# Mac Bot 规划 v0.8

> 状态：规划中，尚未开始编码。
> v0.8 变更：界面和组件设计拆分到 DESIGN.md（含线框图、设计语言、群聊专章）。
> v0.7 变更：安全部分精简为「每次连接带上密码」；浏览器操控改用 vercel-labs/agent-browser（三种模式评估、实时画面代理）；记忆增加项目级共享层。
> v0.6 变更：鉴权改为「访问密码」，去掉配对；密码换会话令牌，管理页也需要密码；首次设置向导；防暴力破解。
> v0.5 变更：服务端改为无界面守护进程 macbotd（.pkg 安装、LaunchAgent），只带极简的本机 Web 管理页和 CLI；所有业务配置都通过客户端走 API；桌面 App 改为纯客户端。
> v0.4 变更：确定部署形态为中心 Host + 客户端（支持多 Host，预留节点和联邦扩展）；v1 不做 Windows，只做 macOS、Android、iOS；当前阶段只做设计和规划。
> v0.3 变更：移动端增加 iOS，改用 Kotlin Multiplatform + Compose Multiplatform；补充开发环境说明。
> v0.2 变更：去掉虚拟机和沙箱；改为多 Bot，每个 Bot 有独立工作间；新增群聊和 Bot 间协作；电脑操控改为控制系统自带的浏览器，并推迟到后续阶段；交互全面对齐 Grok Bot；Android 改用原生技术栈。

## 1. 定位

**自托管版 Grok Bot。** 一个人拥有一支 Bot 团队，这些 Bot 运行在自己 24 小时常开的 Apple Silicon Mac 上。每个 Bot 有名字和明确职责；Bot 之间可以拉群、互发消息、交接任务。用户在 macOS、Android 或 iOS 客户端（Windows 放到 v2）上填入 `host:port` 和访问密码，就可以像和同事聊天一样给 Bot 派活。

## 2. 已确认的决策

| 项 | 决策 |
|----|------|
| 部署形态 | **无界面服务端 + 客户端**（详见 5.0）：服务端 `macbotd` 是守护进程，只带一个极简的本机 Web 管理页（`/admin`）和 CLI；Bot、模型、记忆等所有配置都通过客户端走 API；客户端可以连接多台 Host |
| 部署 | 服务端运行在 M 系列 Mac 上，16 GB 内存；**不用虚拟机，不做沙箱**（沙箱作为远期可选项） |
| 网络 | 服务端监听一个固定端口，客户端填 `host:port` 直连。公网代理由用户自行解决，不在本项目范围内 |
| 安全 | v1 **只做访问密码**：客户端每次连接都带上密码，管理页用同一个密码；其他安全措施全部放到以后（详见 5.0.1） |
| Bot | 支持多个 Bot，**每个 Bot 有独立工作间**（目录、记忆、会话、定时任务）；支持**群聊和 Bot 间消息** |
| 电脑操控 | 以网页任务为主，**使用系统浏览器**（复用已有的登录凭证）；放到后续阶段，先把 Bot 形态跑通 |
| 系统权限 | 屏幕录制和辅助功能权限**不强制**；没授权时提示「部分功能不可用」 |
| 模型 | 用户自定义 provider 和模型；默认认为模型支持看图 |
| 存储 | SQLite |
| 桌面端 | Rust + GPUI（gpui-kit）。**v1 只做 macOS**；Windows 客户端和 Computer Node 放到 v2 |
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
| **vercel-labs/agent-browser**（Rust，Apache-2.0） | **浏览器操控主力**：无头/有头、复用 Chrome profile、每个 Bot 独立会话、WebSocket 推送实时画面并接收输入（见 5.8） |
| Playwright MCP 扩展模式、mcp-chrome | 远期备选：通过 Chrome 扩展控制用户正在使用的浏览器 |
| **cua-driver**（trycua/cua，MIT） | 后台控制原生 Mac App，**不抢鼠标、不抢焦点**（可选，需要系统权限） |
| **Peekaboo**（openclaw/peekaboo，MIT） | macOS 截图和 GUI 自动化 CLI/MCP（可选） |
| **gpui-kit**（[longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)，原名 gpui-component，Apache-2.0，约 1.6 万 star） | 桌面端组件库：75+ 组件，包括 Markdown/HTML 渲染、不等高虚拟列表（消息流）、Dock 和可拖拽面板、表单、浮层、菜单、主题；Longbridge Pro 在生产环境使用；自带给 AI 编程助手用的 skills |

> cua 和 lume：lume 是在 Apple Silicon 上管理 macOS/Linux 虚拟机的工具，cua 是跑在虚拟机里的电脑操控 Agent 框架。我们不用虚拟机，所以**不采用 lume**；同一仓库里的 **cua-driver**（不用虚拟机、直接后台操控本机 App）可以作为原生桌面操控的候选。

## 4. 从 Grok Bot 提炼的交互规格

> 本节是需求层面的规格。落到界面上的具体设计见 [DESIGN.md](DESIGN.md)。

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
- **每个 Bot 有自己的「屏幕」**：在 Mac Bot 里就是一个独立的 agent-browser 会话（`--session <bot_id>`）。同一块屏幕同一时间只跑一个电脑操作任务，不同 Bot 之间可以并行。
- 在会话中打开 **Agent Computer** 可以看实时画面；关掉预览后任务继续执行。
- **接管**：遇到密码、2FA、验证码、支付时，Bot 暂停，用户接管完成后告诉 Bot 继续。
- 登录态在所有 Bot 之间共享（因为用的是同一个系统浏览器），这一点和 Grok Bot 一致。

### 4.8 移动端
- 首页：会话列表，右上角 **+** 可新建 Bot 或群聊；全局搜索，范围包括消息、Bot、群聊、文件、定时任务。
- 聊天：完整支持 @、Reply、附件、审批、草稿发送和丢弃。
- 定时任务：可以查看和暂停/恢复；编辑和 Test run 只能在桌面端做（与 Grok 一致）。
- 通知：Bot 有结果、有提问、需要审批时推送。

## 5. 架构

### 5.0 部署形态与角色

**术语**
- **Host（主机，即服务端）**：真正运行 Bot 的机器。Bot 的会话、记忆、定时任务、工作间都存放在这里。v1 只有 Mac 能当 Host。
- **Client（客户端）**：用来看和操控 Bot 的界面，可以同时连接多台 Host。
- **Computer Node（电脑节点，后期）**：把自己的浏览器、shell 等能力借给某台 Host 上的 Bot 使用，本身不运行 Bot。参考 OpenClaw 的 Gateway + Node，以及 Grok Bot 的 Local computer。

**候选方案对比**

| | A. 中心 Host + 瘦客户端 | B. 每台机器都是对等节点（各自有 Bot，跨机通信） |
|---|---|---|
| 分发 | 简单：桌面端一个安装包，手机端只有客户端 | 每个平台都要实现完整的服务端：Windows 上的 shell、密钥存储、自启动、电脑操控都要再做一遍 |
| Bot 是否一直在线 | 只要 Mac 不关机，Bot 就在 | 笔记本合盖、Windows 睡眠后，这台机器上的 Bot 就离线了，跨机群聊会断 |
| 数据一致性 | 只有一份数据，没有同步问题 | 跨机群聊的记录存在哪台机器？节点离线后消息怎么补发？用户画像要在多台机器之间同步，会出现冲突 |
| 记忆 | 集中存放，所有 Bot 共享同一份用户画像 | 分散在各台机器，需要做同步 |
| 跨机能力 | Client 可以同时连接多台 Host，所以操控另一台机器上的 Bot 天然就支持 | 原生支持 |
| 复杂度 | 低 | 高（节点之间要配对和互信、消息要路由、要做补发和去重） |

**决策：采用 A（中心 Host），服务端做成无界面的守护进程，所有管理都走 API（API-first）。从第一天起按「多 Host + 节点」预留扩展点。**

1. **服务端 = 无界面的守护进程 `macbotd`**，装好就能用。
   - 安装包是 `.pkg`，安装时做三件事：把程序装进系统；注册当前用户的 **LaunchAgent**，开机登录后自动启动、崩溃后自动重启；安装结束时用浏览器打开 `http://localhost:7788/admin`。
   - **为什么必须是用户级 LaunchAgent，而不是 root 级 LaunchDaemon**：只有在用户会话里运行，才能访问该用户的钥匙串、用户的 Chrome（后期电脑操控要用），以及将来可能用到的屏幕录制和辅助功能权限。
   - 打包形式：二进制放在一个**没有界面的 `.app` 包**里（`LSUIElement`）。好处是签名、公证和系统权限都有一个稳定的身份，升级后不用重新授权。
2. **所有管理操作都走同一个端口的 API。** Bot 的增删改、模型和 provider、记忆、定时任务、审批规则、设备管理，**全部在客户端里完成**；服务端本身不提供业务界面。
3. **鉴权：只用一个访问密码，不做配对**（详见 5.0.1）。客户端填 `host:port` 和密码就能连上；管理页也用这个密码。
4. **服务端只提供一个极简的 Web 管理页**（`/admin`，需要密码）。它只负责客户端连上之前的那些事：
   - 运行状态（版本、端口、node_id、运行时长、已连接的设备）
   - 修改密码
   - 修改端口和 Host 名称、查看日志、重启服务
   - 页面是嵌入二进制的单个 HTML 文件加少量原生 JS，**不引入前端构建链**
5. **命令行 `macbot`**（与 `macbotd` 是同一个二进制的子命令）：`macbot status`、`macbot passwd`（设置或重置密码，适合通过 SSH 远程配置 Mac mini）、`macbot logs`、`macbot restart`。CLI 通过本地 Unix socket 和守护进程通信，靠文件权限鉴权，不需要密码。
6. **桌面 App（GPUI）和手机 App 都只是客户端**，不再内置 Host，也不管理服务端的生命周期。
7. **Client 支持多个 Host**：侧栏顶部有一个 Host 切换器，类似 Slack 切换工作区，可以同时连接家里的 Mac mini 和办公室的另一台 Mac。「操控另一台机器上的 Bot」就是靠这个实现的。
8. **全局唯一标识**：每台 Host 有一个 `node_id`（UUID）和名字；Bot、会话、消息的 ID 都用 UUIDv7，并记录所属的 `node_id`。Bot 的完整地址写作 `bot_id@node_id`。
9. **服务端核心与平台无关**：和平台相关的能力都放在 trait 后面，包括密钥存储、自启动、浏览器桥接、桌面操控。服务端没有界面，所以以后移植到 Windows、Linux（例如 NAS），只需要实现这几个 trait。

**首次使用流程**
1. 在 Mac mini 上双击 `MacBot-Server.pkg` 安装，安装结束后浏览器自动打开 `localhost:7788/admin`。
2. 管理页显示「设置访问密码」（首次设置向导）。如果 Mac mini 没接显示器，也可以 SSH 上去执行 `macbot passwd`。
3. 在 Mac、Android 或 iOS 客户端里「添加 Host」，填入 `host:port` 和密码。
4. 之后的一切操作，包括创建 Bot、配置模型等，都在客户端里完成。

> 为什么不在 .pkg 安装界面里直接设置密码：macOS 安装器不方便加自定义输入页（需要写已经不推荐的 Installer 插件），所以改为安装完成后的首次设置向导，体验上等同于「安装时设置密码」。

**端口上的路由规划（同一个端口）**

| 路径 | 用途 | 访问范围 |
|------|------|----------|
| `/ws` | 客户端协议：请求、响应、事件推送 | 密码 |
| `/api/v1/*` | HTTP JSON，和 `/ws` 能力一致，方便脚本和第三方集成 | 密码 |
| `/admin` | 极简管理页 | 密码（HTTP Basic Auth）；**未设置密码前只允许 localhost 访问** |

#### 5.0.1 密码鉴权（v1 精简版：先跑通）

- 首次设置密码有两种方式：本机打开 `localhost:7788/admin`，或者执行 `macbot passwd`。服务端只存密码的 argon2 哈希。
- **客户端每次连接都直接带上密码**：`/ws` 和 `/api/v1` 在 `Authorization: Bearer <密码>` 头里传。客户端把密码存进系统钥匙串（iOS Keychain、Android Keystore、macOS 钥匙串）。
- 管理页 `/admin` 用 HTTP Basic Auth 登录，密码相同。
- 唯一保留的防护：**设置密码之前，只接受 localhost 的请求**（只是一行判断，用来防止局域网里有人抢先设密码）。
- 忘记密码：在本机执行 `macbot passwd` 重置。
- 以后再做：会话令牌和设备管理、登录限速、TLS（目前走公网时可以由代理层加 TLS，客户端填 `wss://`）。

```
┌──────────── Mac（macbotd，无界面守护进程，LaunchAgent）──────────────┐
│ Gateway  axum，固定端口（默认 7788）：/ws /api/v1 /admin               │
│   密码鉴权 / 协议版本 / 事件流（按 seq 断线续传）                    │
│ Local CLI  Unix socket ← `macbot status|passwd|logs`               │
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
 │ v1 macOS（纯客户端）│          │ Android + iOS，Ktor WS    │
 └─────────────────┘            │ iOS 后台通知走 APNs        │
                                └──────────────────────────┘
        macbotd ⇄ agent-browser（sidecar，每个 Bot 一个会话；画面流只监听 localhost，由 macbotd 代理）
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
│   ├── macbot-server        # 守护进程 + CLI，二进制名为 macbotd（`macbot` 是指向它的软链接）；内嵌 /admin 页面
│   └── macbot-client        # 桌面端用的协议客户端（重连、本地缓存）
├── apps/
│   ├── desktop/             # gpui-kit（只依赖这一个 crate，它会固定匹配的 GPUI 版本）
│   └── mobile/              # Kotlin Multiplatform + Compose Multiplatform
│       ├── shared/          #   共享代码：UI、ViewModel、Ktor WebSocket、kotlinx.serialization、本地缓存
│       ├── androidApp/      #   Android 外壳（前台服务、通知）
│       └── iosApp/          #   Xcode 工程外壳（APNs 注册）
└── docs/
```

**分发产物**

| 产物 | 内容 |
|------|------|
| `MacBot-Server.pkg` | macbotd（放在无界面的 .app 包里）、LaunchAgent plist、`macbot` CLI 软链接；装在 Host 上 |
| `MacBot.dmg` | 桌面客户端（GPUI），装在任意 Mac 上 |
| `MacBot.apk` / iOS（TestFlight 或自签） | 手机客户端 |

另外提供 `install.sh`（`curl … \| sh`），给喜欢命令行安装的用户。

### 5.2 数据模型（SQLite 草案）

```
bots(id, node_id, name, label, description, avatar, model_ref, pinned, hidden, created_at)
chats(id, node_id, kind[direct|group|bot_dm], title, project_id, created_at)   -- 群聊可绑定项目
chat_members(chat_id, member_kind[user|bot], member_id)
messages(id, chat_id, seq, sender_kind, sender_id, reply_to, mentions, content_json, created_at)   -- FTS5
runs(id, bot_id, chat_id, trigger_message_id, status, owner_task_id, started_at, ended_at)
entries(id, run_id|conversation_id, seq, kind, json)       -- 不可变，参照 pi-durable 的 Entry
tasks(id, owner, kind, status, checkpoint_json, updated_at) -- 可恢复的状态机
submissions(id, bot_id, request_id UNIQUE, status)          -- 幂等
approvals(id, bot_id, run_id, tool, args_json, status, decision, rule_id)
projects(id, name, description, is_global, created_at)
bot_projects(bot_id, project_id)
memories(id, scope[user|project|bot], project_id, bot_id, content, updated_at)
skills(id, name, description, body_md, updated_at)
routines(id, bot_id, name, schedule, tz, instructions, enabled, next_run_at)
routine_runs(id, routine_id, run_id, status, started_at)   -- 每个 Routine 只保留 20 条
files(id, bot_id, chat_id, path, mime, size, created_at)
providers(id, name, api_kind, base_url, secret_ref)
models(id, provider_id, model_id, caps_json, context_window, cost_json)
auth(id=1, password_hash)                                   -- 单行
node(node_id, name, created_at)                             -- 本机 Host 的身份（单行）
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

### 5.5 记忆（hermes 风格，三层作用域）

| 作用域 | 谁能看到 | 内容 | 例子 |
|--------|----------|------|------|
| **用户画像**（`scope=user`） | 所有 Bot | 用户是谁、有什么偏好 | 「我在上海，回复用中文，周报周五交」 |
| **项目记忆**（`scope=project`） | 关联了该项目的 Bot | 某个项目或主题的共享事实、约定、进展 | 「mac-bot 项目用 Rust + gpui-kit，仓库在 …；当前在做 P2」 |
| **Bot 笔记**（`scope=bot`） | 只有这个 Bot | 它在自己职责范围内学到的东西 | 「X 时间线总结时跳过广告和转推」 |

- **项目（Project）** 是一个共享记忆空间，由 `projects` 表加 `bot_projects` 关联表组成。
  - 内置一个「**全局**」项目，所有 Bot 默认关联，用来放团队级的共享事实。
  - 其他项目由用户或 Bot 创建，然后把相关 Bot 拉进来。
  - **群聊可以绑定一个项目**：群里的 Bot 在这个群里工作时，自动读写这个项目的记忆，即使它平时没有关联该项目。群聊协作的上下文就是这样共享的。
- **注入方式**：会话开始时以快照形式注入，各层有自己的字数上限。顺序是用户画像、关联项目（全局项目在前，当前群绑定的项目优先）、Bot 笔记。运行中通过 `memory(scope, project?, action, content)` 工具增删改，下次会话生效。
- **写入权限**：Bot 可以写自己的笔记，也可以写它关联的项目记忆；用户画像也允许 Bot 写，但客户端会给出「Bot 更新了你的画像」提示，方便用户审查。
- `session_search`：对 messages 做 FTS5 检索，用于回答「我之前说过什么」。默认搜索本 Bot 参与过的会话，加上它关联项目所绑定的群聊。
- 客户端入口：
  - Bot 详情 → Memory 页：编辑这个 Bot 的笔记，查看它关联了哪些项目
  - 侧栏 → Projects 页：管理项目、项目记忆和成员
  - Settings → Profile 页：编辑用户画像

### 5.6 Provider
- API 类型：`openai-completions`、`openai-responses`、`anthropic-messages`、`google-generative`。
- 用户新增 provider 时填 base_url、key（存入钥匙串）和 API 类型，然后添加模型 ID 或从 `/models` 拉取列表。
- 统一的流式事件：TextDelta、ThinkingDelta、ToolCallDelta、Usage、Stop。
- 每个 Bot 单独选模型；群聊路由器可以单独配一个便宜的模型。

### 5.7 协议
- WebSocket，JSON 帧 `{v, id, type, payload}`，请求/响应和服务端推送并存。
- 重连时带上 `last_seq`，服务端从 `events` 表补发。
- 鉴权：每次连接都在 `Authorization: Bearer <密码>` 里带上密码，服务端校验通过后返回 `node_id`。密码错误时，客户端提示重新输入。
- 多 Host：客户端为每台 Host 分别保存 `{node_id, name, host:port, device_token}`。握手时服务端返回自己的 `node_id`，客户端据此识别同一台 Host 地址变化的情况（例如局域网 IP 和公网域名是同一台）。

### 5.8 电脑操控：浏览器使用 vercel-labs/agent-browser

**选型：[vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser)**（Apache-2.0，约 4.4 万 star，纯 Rust 编写，直接用 CDP 控制浏览器，不依赖 Node）。
- 它只发布可执行程序，不提供 Rust 库，所以以 **sidecar 进程**的方式集成：放进 .pkg 一起分发，由 macbotd 调用它的 CLI（`--json` 输出），或者使用它的 MCP 模式（`agent-browser mcp`）。
- 原先自研 Chrome 扩展的方案降级为远期备选。

**三种模式评估**

| 模式 | agent-browser 用法 | 能拿到登录态吗 | 锁屏和无人值守 | 结论 |
|------|---------------------|----------------|----------------|------|
| ① 无头、不带 profile | 默认 | ❌ 干净的浏览器 | ✅ | 适合查公开网页、做调研，**作为不需要登录的任务的默认模式** |
| ② 无头、带 profile | `--profile Default`：把本机 Chrome 的 profile 复制一份只读快照来启动；或 `--profile <目录>`：使用一个持久的独立 profile | ✅ 能拿到本机 Chrome 的 cookie 和登录状态 | ✅ 不依赖屏幕，锁屏也能跑 | **主力模式**：例如「登录 X 刷帖总结」 |
| ③ 有头、连接本机正在运行的 Chrome | `--auto-connect`（Chrome 144 以上，需要在 `chrome://inspect` 里开启远程调试） | ✅ 直接用你正在使用的那个 Chrome | ❌ 会占用你自己的浏览器和标签页；连接时 Chrome 可能弹出授权确认，需要人点击 | 只用于「需要人在旁边」的场景，例如导入登录状态；**不作为 Bot 日常模式** |

**落地方案**
- **每个 Bot 一个独立的浏览器会话**（`--session <bot_id>`），对应 Grok Bot 里「每个 Bot 有自己的屏幕」，Bot 之间可以并行。
- **登录态**：默认用模式②的 `--profile Default`，即复用本机 Chrome 的登录状态，零配置。再加上 `--session <bot_id> --restore`，让 Bot 自己在使用过程中刷新的 cookie 和 localStorage 能保存下来。
  - 注意：快照是只读的，Bot 里的改动不会写回你的 Chrome。
  - 如果某个网站在 Bot 里掉了登录，有两种处理：用户在实时画面里接管、手动登录一次（之后由 `--restore` 保存）；或者在本机 Chrome 里重新登录，下次启动时会重新拿快照。
- **内存**：每个会话是一个独立的 Chrome 进程，大约占 300–500 MB。16 GB 内存同时跑 3–5 个没问题。agent-browser 默认空闲 1 小时自动关闭浏览器，我们可以设得更短。
- **观察和动作**：使用 agent-browser 的无障碍树快照（每个元素有 `@e1` 这样的编号），按编号 click、fill、scroll；截图作为补充。它自带的 skills 文档可以改写成 Bot 的浏览器工具说明。

**实时画面和接管（agent-browser 原生支持）**
- 每个会话自带一个 **WebSocket 流服务**，同时提供两个方向：
  - 下行：JPEG 帧（附带视口尺寸、滚动位置、时间戳）和 URL 变化
  - 上行：鼠标、键盘、**触摸**事件（手机上可以直接点和滑）
  - 可调参数：画质、最大宽高、每个连接的 `maxFps`、**ack 节流**（慢网络下不会堆积旧帧）
  - 体积参考：1280×720、画质 80 约 54 KB/帧；640×360、画质 20 约 9 KB/帧
- **macbotd 做代理**：agent-browser 的流端口只监听 localhost，不对外暴露。客户端在 `/ws` 上订阅「某个 Bot 的屏幕」，macbotd 把帧转发过去，并把客户端的输入回放到浏览器。这样只需要一个端口、一个密码。
- 档位：手机默认 720 宽、画质 50、10 fps；桌面默认 1280 宽、画质 70、15 fps；网络慢时自动切到 ack 节流。
- 无头模式下的画面来自 CDP screencast，**和屏幕是否锁定无关**。

**原生桌面（可选，远期）**：接入 cua-driver 或 Peekaboo（MCP 子进程）。没有辅助功能或屏幕录制权限时自动禁用，并在 Settings → Computer 里说明原因。

**P6 开工时需要先确认**：`--profile Default` 读取 Chrome cookie 时是否会触发钥匙串弹窗（Chrome 的 cookie 由钥匙串里的 "Chrome Safe Storage" 加密）；`--profile` 与 `--restore` 组合使用时的行为。

## 6. 界面与组件设计

已迁移到 **[DESIGN.md](DESIGN.md)**，包括组件划分、每个组件的界面和功能、线框图、设计语言、群聊交互、移动端和管理页。

## 7. 里程碑（按顺序推进，不先做技术验证）

| 阶段 | 内容 | 验收 |
|------|------|------|
| **P1 骨架** | Cargo workspace、protocol、store、providers（OpenAI 兼容 + Anthropic）、单个 Bot 私聊流式对话、durable resume、密码鉴权、`/admin` 管理页（含首次设置密码）、`macbot` CLI、LaunchAgent 安装脚本、GPUI 侧栏和聊天 | 桌面端填 host:port 和密码后能和 Bot 对话；服务端重启后正在进行的 run 能自动恢复 |
| **P2 多 Bot 与群聊** | Bot 增删改、Pin/Hide/Duplicate、独立 workspace 和基础工具、审批卡片、群聊路由、@ 和 Reply、Bot 间消息与 handoff、防循环 | 3 个 Bot 在群里分工完成一个任务，中间有交接；审批只出现在私聊里 |
| **P3 记忆与技能** | 用户画像、项目记忆（含全局项目、群聊绑定项目）、Bot 笔记、session_search、Memory 页、Skills 和 `/` 引用 | 跨会话记住用户偏好，能回答「我上周说过什么」 |
| **P4 移动端** | Compose Multiplatform 客户端：会话列表、聊天、群聊、审批、通知、搜索；Android 前台服务；iOS 接入 APNs | 在小米 17 和 iPhone 上都能完成 P2 的场景 |
| **P5 Routines** | 调度器、通过对话创建、Test run、运行历史 | 「每天 9 点总结 xxx」按时执行并推送结果 |
| **P6 电脑操控** | 以 sidecar 方式集成 agent-browser（每个 Bot 一个会话，复用 Chrome profile）；macbotd 代理实时画面和输入；Agent Computer 面板和接管 | 用系统 Chrome 已有的 X 登录态刷帖并总结 |
| **P7 打磨** | 语音、文件页、全局搜索、用量统计；客户端和服务端的自动更新（服务端通过 `/admin` 或 `macbot update` 更新）；签名的 .pkg 和 .dmg | 双击 .pkg 安装后，到客户端登录、开始对话，全程不需要碰终端 |
| **v2** | Windows 客户端；Computer Node（在 Windows 上以 `macbotd node` 运行）| Mac 上的 Bot 能操作 Windows 上的浏览器 |
| **v3（可选）** | Host 联邦：跨 Host 的 Bot 消息和群聊 | |

## 8. 开发环境

| 用途 | 工具 | 备注 |
|------|------|------|
| 服务端和桌面端 | Rust stable（rustfmt、clippy），Xcode（提供 Metal 编译器，GPUI 需要） | crates.io 走清华 tuna 镜像 |
| GPUI | 只依赖 `gpui-kit` 0.7.x（它会 re-export GPUI、gpui-base、gpui-component 和 Lucide 图标），不单独依赖 `gpui` | Xcode 26 需要额外下载 Metal 工具链：`xcodebuild -downloadComponent MetalToolchain` |
| 移动端 | JDK 21、Gradle（项目内使用 wrapper）、Android SDK 36 + build-tools 36.1、platform-tools（adb） | 真机调试用 USB 或无线 adb 连接小米 17 |
| iOS | Xcode 26 + iOS Simulator 运行时；真机需要签名 | |
| Windows 客户端（v2） | **只能在 Windows 上构建**（GPUI 的 Windows 后端需要在 Windows 上编译 DirectX 着色器），用 GitHub Actions 的 windows runner 构建 | |
| 浏览器扩展 | Chrome | P6 阶段 |

## 9. 暂不考虑（记录在案）
断电重启后自动恢复、TLS 和审计、公网代理、虚拟机或沙箱、示范一次生成技能、小米厂商推送、多用户。

## 10. 待确认
（暂无。已确认：浏览器是 Chrome；记忆分为用户画像、项目记忆、Bot 笔记三层。）
