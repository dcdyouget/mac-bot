# Mac Bot 规划 v0.11

> 状态：规划中，尚未开始编码。
> v0.11 变更：新增主 Bot（日常对话 + 所有群的负责人）；任务接力（收到 → 工作中 → 完成 → @下一个）；两段式输出（发言 / 干活）；插话；系统维护的公告和产物；按群并行和并发上限；工作台和统计；用量记账。
> v0.10 变更：项目 = 一件事 = 一个群（私聊里自动建项目、项目生命周期）；记忆简化为用户、Bot、项目三种；上下文全自动管理，界面不暴露任何控件。
> v0.9 变更：新增「记忆与上下文管理」（每个会话独立执行线程、四层记忆、当前项目由会话决定、分段与压缩、群上下文包）。
> v0.8 变更：界面和组件设计拆分到 DESIGN.md（含线框图、设计语言、群聊专章）。
> v0.7 变更：安全部分精简为「每次连接带上密码」；浏览器操控改用 vercel-labs/agent-browser（三种模式评估、实时画面代理）；记忆增加项目级共享层。
> v0.6 变更：鉴权改为「访问密码」，去掉配对；密码换会话令牌，管理页也需要密码；首次设置向导；防暴力破解。
> v0.5 变更：服务端改为无界面守护进程 macbotd（.pkg 安装、LaunchAgent），只带极简的本机 Web 管理页和 CLI；所有业务配置都通过客户端走 API；桌面 App 改为纯客户端。
> v0.4 变更：确定部署形态为中心 Host + 客户端（支持多 Host，预留节点和联邦扩展）；v1 不做 Windows，只做 macOS、Android、iOS；当前阶段只做设计和规划。
> v0.3 变更：移动端增加 iOS，改用 Kotlin Multiplatform + Compose Multiplatform；补充开发环境说明。
> v0.2 变更：去掉虚拟机和沙箱；改为多 Bot，每个 Bot 有独立工作间；新增群聊和 Bot 间协作；电脑操控改为控制系统自带的浏览器，并推迟到后续阶段；交互全面对齐 Grok Bot；Android 改用原生技术栈。

## 1. 定位

**自托管版 Grok Bot。** 一个人拥有一支 Bot 团队，这些 Bot 运行在自己 24 小时常开的 Apple Silicon Mac 上。你日常只和**主 Bot** 对话：交代一件事，它就拉相关的 Bot 建群（群名就是这件事），在群里分派任务；各 Bot 按「收到 → 工作中 → 完成 → @下一个」接力，做完后主 Bot 通知你。你也可以随时在群里 @ 任意 Bot 进行指导，即使它正在干活。用户在 macOS、Android 或 iOS 客户端（Windows 放到 v2）上填入 `host:port` 和访问密码，就可以像和同事聊天一样给 Bot 派活。

## 2. 已确认的决策

| 项 | 决策 |
|----|------|
| 部署形态 | **无界面服务端 + 客户端**（详见 5.0）：服务端 `macbotd` 是守护进程，只带一个极简的本机 Web 管理页（`/admin`）和 CLI；Bot、模型、记忆等所有配置都通过客户端走 API；客户端可以连接多台 Host |
| 部署 | 服务端运行在 M 系列 Mac 上，16 GB 内存；**不用虚拟机，不做沙箱**（沙箱作为远期可选项） |
| 网络 | 服务端监听一个固定端口，客户端填 `host:port` 直连。公网代理由用户自行解决，不在本项目范围内 |
| 安全 | v1 **只做访问密码**：客户端每次连接都带上密码，管理页用同一个密码；其他安全措施全部放到以后（详见 5.0.1） |
| Bot | 支持多个 Bot，**每个 Bot 有独立工作间**（目录、记忆、会话、定时任务）；支持**群聊和 Bot 间消息** |
| **主 Bot** | 每台 Host 内置一个、不能删除（可以改名，默认叫「总管」）。它是日常对话对象，也是**所有群的负责人**：负责建群、拉人、分派任务、跟进、验收，以及完成后通知你 |
| **群 = 一件事 = 项目** | 群名就是这件事的名字；有 Home 目录、系统自动维护的公告（成员状态、产物、项目记忆） |
| **两段式输出** | 每个 Bot 的输出分两路：**群里发言**（短、快，用发言模型）和**干活**（长、带工具、过程不进群）。详见 5.3 |
| **并行** | 同一个 Bot 可以同时在多个群里干活（每个群是独立的会话，只是加载的项目记忆不同）。有全局上限（默认 8）和每个 Bot 的上限（默认 3，主 Bot 默认 4）；同一个群里同一个 Bot 的活按顺序做 |
| **可观测** | 工作台（每个 Bot 正在干什么）+ 统计（每个 Bot 的 token、模型、按群分布、费用）；每次模型调用都记账 |
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
| **nightly-labs/openbot**（[GitHub](https://github.com/nightly-labs/openbot)，PolyForm 非商用许可，**只参考设计，不复用代码**） | 与本项目形态最接近的开源实现：本地优先的 AI 队友桌面应用；每个 agent 有独立工作区；**频道（群聊）里每个 agent × 每个频道有一条独立执行线程**；按包组装频道上下文；先做确定性路由；单个负责人加显式委派；每个根请求最多 8 次自动委派；按 80% 阈值压缩；频道记忆；还能从 Grok Bot 导入 agent |
| **CopilotKit/OpenBot**（[GitHub](https://github.com/CopilotKit/OpenBot)，MIT） | 企业版的「每个 Bot 一台电脑」：常驻角色在每个频道都生效；接管流程有审计记录；用 CEL 写策略；集中的 Memory 页 |
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

### 4.3 主 Bot 与群（= 一件事 = 项目）

- **主 Bot**是日常对话对象，也是所有群的负责人。你对它说「帮我做 xxx」，它判断需要多人配合时就**建群**：群名就是这件事，创建 Home 目录，拉相关的 Bot 进群，在群里发开场（事项、目标、流程），并 @ 第一个 Bot。
- 群成员是主 Bot 加上 1–6 个 Bot。也可以手动建群（⌘N），主 Bot 自动加入。
- 群里的协作是**任务接力**：被 @ 的 Bot 先回复「收到」（状态变为 ⟳ 工作中），在后台干活；完成后在群里报告产物和位置，并 @ 下一个 Bot；最后一个 Bot 完成后 @主 Bot。主 Bot 验收、写总结、标记完成，然后在私聊里通知你。
- **公告**由系统自动维护：Home 路径、目标、流程、成员分工和状态、产物清单、项目记忆。
- 你可以随时在群里 **@ 任意 Bot**。它正在干活时，这条消息就是**插话**：它立即回复，指令插进当前工作，下一步生效。
- 群里不 @ 任何人时，消息交给主 Bot 处理。
- **群里不出现**审批请求、密码或登录请求、外发草稿，这些都回到对应 Bot 的私聊里。
- 群的状态：进行中 → 已完成 → 已归档。

### 4.4 Bot 之间的协作
- **交接**：完成报告里的 @ 就是交接，附带产物清单和说明。
- **私信**：Bot 可以异步给另一个 Bot 发消息，对方被唤醒处理，之后再回复，过程用户可见。
- **防循环**：一条用户消息引发的 Bot 间往返默认最多 8 次。

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
- 首页：会话列表，右上角 **+** 可新建项目或 Bot；全局搜索，范围包括消息、Bot、项目、文件、定时任务。
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
│ Orchestrator（主 Bot 建群、群路由、任务流转、交接、插话、防循环）     │
│   │                                                               │
│ Bot Runtime：两段式输出（发言 / 干活）· Scheduler 并发调度            │
│   ├ Durable Engine（Rust 版 pi-durable）                           │
│   ├ Providers（Rust 版 pi-ai）                                     │
│   ├ Memory（hermes 风格）                                          │
│   └ Tools：workspace 文件 / shell（需审批）/ web_fetch /            │
│           send_message / handoff / memory / session_search /       │
│           routine / ask_user / [后期] browser / desktop / MCP       │
│ Scheduler（并发、排队、Routines）  Usage（每次模型调用都记账）        │
│ Store：SQLite（rusqlite + FTS5）  Secrets：macOS 钥匙串             │
│ 文件：~/MacBot/projects/<群>/（Home）  ~/MacBot/bots/<bot>/          │
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
bots(id, node_id, name, label, description, avatar, is_main, work_model_ref, voice_model_ref, max_parallel, pinned, hidden, created_at)
                                                            -- is_main：主 Bot，全局唯一
chats(id, node_id, kind[direct|project|bot_dm], title, project_id, created_at)
                                                            -- kind=project 的会话就是项目群，与 projects 一对一
threads(id, bot_id, chat_id, loaded_project_id, segment_no, summary_entry_id, token_estimate, updated_at)
                                                            -- 执行线程：每个 (Bot, 会话) 一条，见 5.5
thread_segments(id, thread_id, no, reason[start|compact|project_loaded|snapshot_stale], summary, memory_snapshot_json, created_at)
chat_members(chat_id, member_kind[user|bot], member_id)
messages(id, chat_id, seq, sender_kind, sender_id, reply_to, mentions, content_json, created_at)   -- FTS5
runs(id, bot_id, chat_id, thread_id, assignment_id, phase[voice|work|memory|compact], trigger_message_id, status, started_at, ended_at)
entries(id, thread_id, segment_no, run_id, seq, kind, json) -- 不可变，参照 pi-durable 的 Entry
jobs(id, owner, kind, status, checkpoint_json, updated_at)  -- durable 引擎内部可恢复的状态机（原 pi-durable Task）
assignments(id, project_id, bot_id, title, instruction, from_kind[user|bot], from_id, parent_id,
            status[queued|acked|working|blocked|waiting_user|done|failed|cancelled], queue_reason,
            started_at, finished_at, tokens_in, tokens_out, cost)          -- 群里的「任务」，见 5.3
assignment_steers(id, assignment_id, message_id, text, applied_at)  -- 插话
artifacts(id, project_id, bot_id, assignment_id, title, path_or_url, kind, created_at, updated_at)
submissions(id, bot_id, request_id UNIQUE, status)          -- 幂等
approvals(id, bot_id, run_id, tool, args_json, status, decision, rule_id)
projects(id, name, slug, goal, flow, deadline, home_path, status[active|done|archived], lead_bot_id, chat_id, created_by[user|bot_id], created_at, done_at)
                                                            -- 一个项目 = 一个群；lead 默认为主 Bot
project_members(project_id, bot_id, role_note)            -- 主 Bot + 1–6 个 Bot；role_note=分工
memories(id, scope[user|bot|project], kind[fact|self|worklog|summary], bot_id, project_id, content, source_bot_id, source_chat_id, updated_at)   -- FTS5
skills(id, name, description, body_md, updated_at)
routines(id, bot_id, project_id, name, schedule, tz, instructions, enabled, next_run_at)
routine_runs(id, routine_id, run_id, status, started_at)   -- 每个 Routine 只保留 20 条
files(id, bot_id, chat_id, path, mime, size, created_at)
providers(id, name, api_kind, base_url, secret_ref)
models(id, provider_id, model_id, caps_json, context_window, cost_json)
auth(id=1, password_hash)                                   -- 单行
node(node_id, name, created_at)                             -- 本机 Host 的身份（单行）
usage(id, ts, bot_id, project_id, chat_id, assignment_id, run_id, phase[voice|work|memory|compact],
      provider_id, model_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost)
usage_daily(day, bot_id, project_id, model_id, phase, input_tokens, output_tokens, cache_read_tokens, cost)   -- 汇总表，供统计页查询
settings(key, value_json)                                   -- 并发上限、默认模型等
events(seq, chat_id, type, payload_json)                    -- 推送和断线补发
```

三层分开存储：
- **消息层**（messages）：群里发言、私聊，用户可见。
- **任务层**（assignments、artifacts）：状态、产物、公告的数据来源。
- **运行层**（threads、runs、entries、jobs）：干活的完整过程，在「工作详情」里展示。

### 5.3 Bot 运行模型：两段式输出 + 并发调度

#### 5.3.1 两段式输出（发言 / 干活）

每个 Bot 是同一个「人格」，但输出分两路，分别由两类模型调用承担：

| | **发言（voice）** | **干活（work）** |
|---|---|---|
| 作用 | 群里和私聊里的简短回复：收到、插话确认、回答问题、完成报告的措辞 | 真正执行任务：调用工具、读写文件、浏览网页、运行命令 |
| 模型 | `voice_model_ref`，默认是便宜、快的模型（可以和干活模型相同） | `work_model_ref` |
| 上下文 | Bot 身份 + 公告 + 最近的群消息 + 这个 Bot 在本群的任务状态 | Bot 身份 + 三种记忆 + 公告 + 任务简报 + 干活线程（见 5.5） |
| 工具 | 不带工具，只输出结构化 JSON | 完整工具集 + 协作工具（见下） |
| 输出去向 | 群消息 / 私聊消息 | 干活线程（`entries`），**不进群**；工作详情里可以看 |
| 时延 | 秒级 | 分钟到小时级，可以恢复 |
| 是否占并发名额 | 否（有单独的速率限制） | 是 |

**一个任务的生命周期**（对应 `assignments.status`）：
1. **派发**：被 @、被交接，或者主 Bot 分派 → 创建 assignment，状态为 `queued`。
2. **收到**：调用一次发言模型，输出 `{reply, accept, task_title}`，把 reply 发到群里，例如「收到，开始写 PRD 和原型」；状态变为 `acked`。如果 `accept=false`（例如不是自己的职责），回复里说明原因并 @主 Bot。
3. **排队或开工**：Scheduler 判断是否有并发名额。有名额就进入 `working`，开始干活 run；没有就保持 `acked` 并记录 `queue_reason`，界面显示「… 排队」。
4. **干活**：干活 run 在 (Bot, 群) 的执行线程里运行。可以使用的协作工具：
   - `group_update(text)`：发一条进展小字到群里，每个任务最多 3 条。
   - `complete_task(summary, artifacts[], next?: {bot, instruction})`：结束任务。
   - `report_blocked(reason, need: user|bot, who?)`：报告卡住。
   - `ask_user(question, options?)`：向用户提问，在私聊里显示提问卡片。
   - `send_message(bot, text)`：私信其他 Bot。
   - `memory(...)`、`project_note(text)`：写项目记忆。
5. **完成**：`complete_task` 被调用后，系统**确定性地**生成完成报告，格式为「✓ 已完成 {summary}」+ 产物卡片 + `@next 指令`，不再调用一次模型；把 artifacts 登记到公告；状态变为 `done`；如果有 `next`，就为下一个 Bot 创建新的 assignment（回到第 1 步）。
6. **卡住**：`report_blocked` → 状态变为 `blocked` 或 `waiting_user`，群里显示卡住报告，并通知主 Bot（需要用户时同时通知用户）。
7. **结束**：流程中最后一个 Bot 完成后 @主 Bot。主 Bot 用干活 run 验收（检查产物是否齐全，必要时打回），然后写总结、标记群完成，并在主 Bot 私聊里发完成通知和推送。

**插话（用户 @ 一个正在干活的 Bot）**：
1. 立即调用一次发言模型生成确认回复，例如「收到，改为只做邮箱登录」，发到群里。
2. 写入 `assignment_steers`，并作为高优先级的用户指令注入干活线程。在下一个工具调用的边界生效（pi-durable 的 steer）。
3. 如果插话是「停 / 取消」，就中止干活 run，状态变为 `cancelled`，Bot 回到待命。
4. 如果是决定性内容，例如「只做邮箱」，自动写入项目记忆。

**主 Bot 的额外职责**（L0 规则 + 专用工具）：
- `create_project(name, goal, flow, members[], deadline?)`：建群、建 Home 目录、生成公告、发开场。
- `assign(bot, instruction)`：在群里 @ 某个 Bot 并创建 assignment。
- `list_bots()`：按头衔和描述挑选成员；找不到合适的 Bot 时提议新建，需要用户同意。
- `finish_project(summary)`：验收、写总结、标记完成、通知用户。
- 监控：某个成员 `blocked` 超过阈值或者失败时，主 Bot 被唤醒去跟进（重试、换人或问用户）。

#### 5.3.2 并发调度

- **调度单位**：(Bot, 群) 的干活 run。私聊和定时任务也各算一个调度单位。
- **三层限制**：
  1. 全局上限：默认 8，受 API 速率限制和内存（浏览器会话）约束。
  2. 每个 Bot 的上限 `max_parallel`：默认 3，主 Bot 默认 4。
  3. 同一个群内，同一个 Bot 的 assignment **串行**执行，避免同一个人自己跟自己冲突。
- **排队顺序**：先按优先级（用户直接 @ > 交接 > 定时任务），再按到达时间。名额释放时唤醒排队的任务。
- **资源**：
  - 每个干活 run 有自己的执行线程。
  - 浏览器会话按 (Bot, 群) 隔离，`--session <bot>-<project>`。
  - Home 目录由群成员共享。约定每个 Bot 写自己的子目录（例如 `product/`、`code/`、`test/`），共享文件由负责人维护。
- **记忆并发**：三种记忆的写入都是追加或按条目替换，在 run 结束时提交。冲突时以后写入的为准，后台整理时再合并。
- 发言调用不占并发名额，单独按每分钟次数限速。

#### 5.3.3 持久化与恢复

- 每一步都先提交到 SQLite 再推送。进程重启后，处于 `working` 的 assignment 自动 resume。
- 有副作用的工具调用标记为「不可安全重放」，resume 到这类调用时，先问用户或主 Bot。
- 上下文怎么组装、何时压缩：见 5.5。

### 5.4 群消息路由（确定性优先，不靠模型猜）

1. 用户消息 **@ 了某些 Bot** → 投递给被 @ 的 Bot。对方在本群有进行中的任务，就当作**插话**；没有就**创建新任务**。
2. 用户**回复了**某条 Bot 消息 → 投递给那个 Bot，规则同上。
3. **没有 @ 也没有回复** → 交给**主 Bot**，由它回答，或者用 `assign` 转派（转派时在群里 @ 对方，让用户看到）。不再设单独的路由模型。
4. Bot 在完成报告或发言里 @ 了其他 Bot → 为被 @ 的 Bot 创建任务，也就是交接。
5. **防循环**：一条用户消息引发的 Bot 之间自动派发链，默认最多 8 次，超过后暂停，等用户在横幅上选择；同一个 Bot 对同一个触发只响应一次。
6. Bot 私信（`send_message`）走 `bot_dm` 会话，用户可以查看，在群里以「✉」卡片的形式出现。

### 5.5 记忆、项目与上下文管理

> 参考：hermes-agent（冻结快照、压缩前先存记忆）、OpenClaw（压缩和裁剪）、**nightly-labs/openbot**（每个 agent × 每个频道一条独立执行线程；频道上下文按包组装；按 80% 阈值压缩；记忆写入在一轮成功结束后才提交）、**CopilotKit/OpenBot**（常驻角色在每个频道都生效）。

#### 5.5.1 原则：上下文由系统全自动管理，用户不用操心

- 界面上**没有**上下文用量、压缩、开始新话题、切换项目这类控件。分段、压缩、加载哪份记忆，全部由服务端自动完成。
- 用户能感知到的只有两件事：
  - Bot 记住了什么。消息流里有一行轻量提示，记忆页可以查看和修改，但不是必须操作。
  - 当前在哪个项目里。看群名就知道。

#### 5.5.2 三种记忆

| 记忆 | 共享范围 | 内容 | 谁来写 | 自动加载的字数上限 |
|------|----------|------|--------|-------------|
| **用户记忆** `user` | 所有 Bot 共享 | 用户的习惯、偏好、常用信息，例如「回复用中文」「周报周五交」 | 任何 Bot 在对话中发现后写入；用户也可以直接编辑 | 1,500 字 |
| **Bot 记忆** `bot` | 每个 Bot 独立 | ① **自我定义**：在资料（名字、头衔、长期规则）之外，自己总结出的做事方式和经验。② **工作记录**：自己做过的事，例如「10/08 在『v0.1 发布』里写了公告初稿」 | Bot 自己写；项目完成和定时任务运行后，系统自动追加工作记录 | 经验 2,000 字 + 最近的工作记录 1,500 字 |
| **项目记忆** `project` | 该项目的成员 Bot 共享 | 这件事的目标、决议、分工、进展、关键数据和链接 | 成员 Bot 在群里形成结论时写入；负责人维护 | 3,000 字 |

- **字数上限是刻意设的**：写满之后，Bot 必须合并或替换旧条目，记忆因此会越来越精炼。
- **工作记录会自动滚动**：超出上限时，旧的记录由系统合并成按月的摘要，例如「9 月：参与 4 个项目，主要负责内容写作」。原始记录保留，可以检索。
- 每条记忆都记录来源（哪个 Bot、哪个项目或私聊、什么时间写的）。
- 写入时机：一轮 run 中先暂存，**这一轮成功结束后才提交**（nightly-labs 的做法）。

#### 5.5.3 项目怎么驱动上下文切换

**一个会话对应一个固定的上下文，切换项目 = 进入另一个群。** 因此不需要「切换」按钮：

| 会话 | 加载的记忆 |
|------|-----------|
| 项目群 | 用户记忆 + 自己的 Bot 记忆 + **这个项目的项目记忆** |
| 私聊 | 用户记忆 + 自己的 Bot 记忆 +（自动判断的）相关项目记忆 |
| 定时任务 | 用户记忆 + Bot 记忆 + 任务所属项目的记忆（如果有） |

**私聊里提到某个项目时**（例如「v0.1 发布进展怎么样了」），系统自动处理：
1. Bot 调用 `project_find` 找到相关项目（按项目名和项目记忆检索），然后在这条私聊线程里**自动加载该项目的记忆**；底层是开一个新段，见 5.5.6。
2. 如果用户要推进这件事（不只是问问），Bot 会把消息转到项目群里继续，并在私聊里留一句「已转到『v0.1 发布』群里处理」和一个跳转链接。这样**项目里的事留在项目里**，私聊保持轻量。

同一个 Bot 可以同时在多个项目里：
- 它在每个项目群里各有一条**独立的执行线程**，上下文互不干扰，只通过三种记忆互通。
- 它在不同群里的活可以**并行**（受并发上限约束，见 5.3.2）；同一个群里的活按顺序做。

#### 5.5.4 主 Bot 什么时候建群

用户对主 Bot 说「帮我干个什么」时，由主 Bot 判断（用户直接对普通 Bot 说时，普通 Bot 自己做完；如果需要别人配合，就转给主 Bot 建群）。满足**任一**条件就建群，否则直接在私聊里做完：
- 需要**其他 Bot** 参与（超出自己的职责）
- 预计**跨多次对话**或耗时较长（多步骤、需要等待、有截止日期）
- 有**持续的产出物**（文档、代码、方案）需要沉淀
- 用户明确说「建个群」「拉个群」

不建群的例子：「查下明天天气」「把这段话翻译一下」「总结这个链接」。

建群的过程**不打断用户**（主 Bot 设置里可以改成「建群前先问我」）：
1. 主 Bot 起群名（简短的事项名，例如「登录功能」），写好目标、流程和截止时间，创建 Home 目录。
2. 用 `list_bots` 按头衔和描述挑选相关 Bot 拉进群（nightly-labs 的做法），自己担任负责人。
3. 在私聊里发一张**新群卡片**：群名、目标、成员、负责人，以及 [进入群] 按钮。
4. 在群里发开场消息：事项、目标、流程，并 @ 第一个 Bot。
5. 如果找不到合适的 Bot，就提议「要不要新建一个『测试』Bot？」。**新建 Bot 必须经过用户同意**。

如果判断错了，用户说一句「不用建群」，系统就把群解散，事情回到私聊。

#### 5.5.5 项目生命周期

- **进行中**：显示在侧栏「群」列表。
- **完成**：主 Bot 验收通过后标记完成，或者用户直接点。完成时会做四件事：
  1. 主 Bot 在群里发总结，并写进项目记忆，包括结果、产出物和遗留问题。
  2. 每个成员 Bot 的工作记录追加一条。
  3. 如果过程中发现了用户偏好，提炼进用户记忆。
  4. 主 Bot 在私聊里发完成通知，同时推送到手机。
- **已完成**的项目折叠到侧栏的「已完成」里。可以继续发消息，发了就自动重新打开。
- **归档**：从列表中隐藏，但记忆和记录保留，可以搜索。

#### 5.5.6 自动上下文管理（内部机制，用户不可见）

**执行线程和分段**
- 每个（Bot，会话）组合有一条执行线程（`threads` 表）。
- 线程按**段（segment）**推进，以下情况开新段：上下文达到模型窗口的 80%；私聊里自动加载了一个项目的记忆；记忆快照过期（用户修改了记忆，或者距上次生成快照超过 24 小时）。
- 开新段的步骤：
  1. **记忆提取**：先跑一轮只开放 `memory` 工具的 run，把要点写进三种记忆（hermes / OpenClaw 的 memory flush）。
  2. **裁剪**：旧的工具输出只保留「调用了什么、结果摘要」。
  3. **摘要**：用便宜的辅助模型为较早的对话生成摘要，最近几轮原样保留。
  4. 重新生成记忆快照。
- 完整消息永远保留，压缩只影响下一轮送给模型的内容。

**每一轮的上下文组装**（越稳定越靠前，便于 prompt cache）

| 层 | 内容 | 变化时机 |
|----|------|----------|
| L0 平台规则 | 工具使用、审批、群聊礼仪、交接格式、「何时建项目」规则 | 发版时 |
| L1 Bot 身份 | 资料（名字、头衔、长期规则）+ 技能清单 + 工具清单 | 编辑 Bot 时 |
| L2 记忆快照 | 用户记忆 + Bot 记忆 + 当前项目记忆 | **只在开新段时生成**；中途写入的记忆在下一段生效 |
| L3 会话上下文 | 私聊：段摘要 + 最近的消息。项目群：**群上下文包**（见下） | 每轮 |
| L4 当前 run | 工具调用和结果 | 每步 |
| 按需检索 | `session_search`、`memory_search`、`project_find`、`chat_history`、读文件 | Bot 自己调用，不自动注入 |

**群上下文包**（每轮给项目群里的成员 Bot，而不是把整个聊天记录都塞进去）：
- 项目目标、负责人、每个成员的分工
- 本次请求，以及它引用或回复的原消息
- 最近 30 条群消息
- 更早消息的版本化摘要，完整记录可以用 `chat_history` 取回
- 附件只给引用

**Bot 间消息必须自包含**：交接或私信时，写清楚请求、背景和期望的产出。

**发言调用**只读公告、最近的群消息和自己的任务状态，不读完整记录，也不带工具。

#### 5.5.7 记忆整理

- run 结束时，L0 里有一条规则提醒 Bot：如果用户表达了偏好、纠正了它、或者形成了结论，就写进相应的记忆。
- 每天空闲时跑一次后台整理：合并重复条目，清理过期内容，把工作记录合并成月度摘要。整理全程自动，不需要用户确认；改动可以在记忆页的「最近变化」里看到。

#### 5.5.8 公告的生成

公告不单独存储，而是由以下数据**实时拼出来**：projects（名称、目标、流程、截止日期、Home）、project_members（分工）、assignments（每个成员的当前状态）、artifacts（产物清单）、memories（项目记忆）。
- 任何相关事件都会触发 `announcement.updated` 推送。
- 干活 run 的上下文里放的是公告的文本版，这就是项目的共享上下文。

#### 5.5.9 用量记账与统计

- **每次模型调用都记一行 `usage`**，包括 Bot、群、任务、阶段（发言 / 干活 / 记忆 / 压缩）、服务商、模型、输入和输出 token、缓存读写 token、费用。费用 = token × `models.cost_json` 里配置的单价；没有配置单价时费用为空。
- 写入时同时更新 `usage_daily` 汇总表，统计页只查汇总表。
- 任务卡片上的实时 token 数由 `usage.tick` 事件推送，每个任务每 2 秒最多推一次。
- API：`GET /api/v1/usage?from&to&group_by=bot|project|model|phase&bot_id&project_id`。

### 5.6 Provider
- API 类型：`openai-completions`、`openai-responses`、`anthropic-messages`、`google-generative`。
- 用户新增 provider 时填 base_url、key（存入钥匙串）和 API 类型，然后添加模型 ID 或从 `/models` 拉取列表。
- 统一的流式事件：TextDelta、ThinkingDelta、ToolCallDelta、Usage、Stop。
- 每个 Bot 单独选**干活模型**和**发言模型**；发言模型默认用全局设置里便宜、快的模型。记忆整理和压缩也有单独的默认模型。

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
| **P1 骨架** | Cargo workspace、protocol、store、providers（OpenAI 兼容 + Anthropic）、**和主 Bot 的私聊**流式对话、durable resume、执行线程和分段、自动压缩（80% 阈值，先做摘要；记忆提取放到 P3）、密码鉴权、`/admin` 管理页（含首次设置密码）、`macbot` CLI、LaunchAgent 安装脚本、GPUI 侧栏和聊天 | 桌面端填 host:port 和密码后能和 Bot 对话；服务端重启后正在进行的 run 能自动恢复 |
| **P2 主 Bot 与群协作** | 主 Bot；Bot 增删改；**群 = 项目**（主 Bot 建群、Home 目录、公告、产物登记、生命周期）；**assignments 任务流转**（收到 → 工作中 → 完成 → @下一个、卡住）；**两段式输出**；**插话**；确定性路由（没 @ 时交给主 Bot）；**并发调度**（全局、每个 Bot、群内串行）；审批卡片；防循环；**工作台**；用量记账 | 场景测试：对主 Bot 说「给 App 加邮箱登录」→ 自动建群、产品 → 编码 → 测试依次接力 → 中途 @编码 插话生效 → 主 Bot 验收并在私聊里通知；同时另一个群里的编码任务并行；工作台能看到两件活 |
| **P3 记忆、技能与统计** | 三种记忆（用户、Bot、项目）；工作记录自动追加和滚动；私聊里自动识别并加载项目记忆；开新段前的记忆提取；项目完成总结；后台记忆整理；session_search / memory_search；记忆页；Skills 和 `/` 引用；**统计页**（按 Bot / 群 / 模型 / 阶段，含趋势） | 跨会话记住用户偏好，能回答「我上周说过什么」 |
| **P4 移动端** | Compose Multiplatform 客户端：消息（主 Bot / 群 / Bot）、群（状态条、公告、任务卡片、插话）、工作详情、工作台、统计、审批、通知、搜索；Android 前台服务；iOS 接入 APNs | 在小米 17 和 iPhone 上都能完成 P2 的场景 |
| **P5 Routines** | 调度器、通过对话创建、Test run、运行历史 | 「每天 9 点总结 xxx」按时执行并推送结果 |
| **P6 电脑操控** | 以 sidecar 方式集成 agent-browser（每个 Bot 一个会话，复用 Chrome profile）；macbotd 代理实时画面和输入；Agent Computer 面板和接管 | 用系统 Chrome 已有的 X 登录态刷帖并总结 |
| **P7 打磨** | 语音、文件页、全局搜索；客户端和服务端的自动更新（服务端通过 `/admin` 或 `macbot update` 更新）；签名的 .pkg 和 .dmg | 双击 .pkg 安装后，到客户端登录、开始对话，全程不需要碰终端 |
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
