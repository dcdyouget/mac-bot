# Mac Bot 技术方案 v1.2

> 状态：方案已定稿，进入开发。历史修订见 git log。

## 0. 文档地图：每类规则只有一个权威来源

| 文档 | 权威范围 | 不写什么 |
|------|----------|----------|
| **PLAN.md**（本文） | 技术方案：架构、服务端内部机制（存储、运行模型、调度、记忆与上下文、工具、浏览器、用量）、开发计划、开发环境 | 界面和交互细节、协议字段 |
| **[DESIGN.md](DESIGN.md)** | 产品行为与界面：概念、交互规则（群协作、插话、验收…）、每个页面的线框图、设计语言 | 服务端实现、协议字段 |
| **[PROTOCOL.md](PROTOCOL.md)** | 客户端 ↔ 服务端协议：连接、帧、对象、方法、事件的全部字段 | 服务端怎么实现 |
| **[REFERENCES.md](REFERENCES.md)** | 参考的开源项目和依赖库 | — |
| **[AGENTS.md](../AGENTS.md)** | 多 agent 协作规则：目录归属、分支、协议变更流程 | 功能需求 |
| **[AGENT_PROMPTS.md](AGENT_PROMPTS.md)** | 各开发线的启动 prompt，只引用上面的文档，不重复规则 | — |

文档之间有冲突时：协议字段以 PROTOCOL 为准；界面和交互以 DESIGN 为准；服务端实现和开发计划以 PLAN 为准。

---

## 1. 定位

**自托管版 Grok Bot。** 一支 Bot 团队运行在用户自己 24 小时常开的 Apple Silicon Mac 上。用户日常只和**主 Bot** 对话；交代一件事，主 Bot 就拉相关的 Bot 建群（群名就是这件事），各 Bot 在群里接力完成，做完后主 Bot 提醒用户验收。用户可以随时 @ 任意 Bot 进行指导，即使它正在干活。产品行为的完整描述见 DESIGN.md。

## 2. 关键决策

### 2.1 平台矩阵

| 端 | 平台 | v1 | 以后 | 技术 |
|----|------|:--:|:----:|------|
| **Server** | macOS（Apple Silicon） | ✅ | | Rust，无界面守护进程（LaunchAgent），JSON 文件存储 |
| **Client** | macOS | ✅ | | Rust + GPUI（gpui-kit） |
| | Android | ✅ | | Kotlin + Compose Multiplatform |
| | iOS | | ✅ | Kotlin + Compose Multiplatform（与 Android 共用 `commonMain`，以后只需加 iOS target 和 iosApp 外壳） |
| | Windows | | ✅ | Rust + GPUI（与 macOS 共用代码，只能在 Windows 上编译） |

服务端只支持 macOS，不做 Linux。

### 2.2 决策表

| 项 | 决策 |
|----|------|
| 部署形态 | 无界面服务端 + 客户端。所有业务操作都在客户端里通过协议完成；服务端只带极简的本机 Web 管理页和 CLI（5.0） |
| 网络 | 服务端监听一个端口（默认 7788）。客户端用任意 IP 或域名连接；公网代理由用户自己解决 |
| 安全 | v1 只做访问密码（5.0.1）。TLS、审计、设备管理都放到以后 |
| 存储 | JSON 文件：JSONL 追加日志是事实来源，JSON 快照用于快速读取；不使用数据库（5.2） |
| 主 Bot | 每台 Host 内置一个，不能删除。只协调、不干活：建群、分派、跟进、提醒用户验收（5.3） |
| 群 = 一件事 = 项目 | 有 Home 目录和系统维护的公告（成员状态、产物、项目要点） |
| Bot 执行模型 | 任务按 durable 流程执行（借鉴 pi-durable）；**群消息只来自 `send_msg` 工具**，不流式；插话走 steer；详细过程只在用户打开时推送（5.3、5.11） |
| 并行 | 同一个 Bot 可以同时在多个群里干活，有全局上限和每个 Bot 的上限；同一个群里同一个 Bot 的活按顺序做（5.3.5） |
| 记忆 | 三种（用户、Bot、项目），**完全由服务端管理**，客户端不展示也不编辑；只有公告里的「项目要点」只读展示（5.5） |
| 工具 | 照搬 pi 的文件工具和 bash，另有 agent-browser、子代理、web、`send_msg`、技能（5.10） |
| 技能 | Agent Skills 规范，所有 Bot 共用；客户端可以增删改，逻辑都在服务端 |
| 浏览器 | agent-browser sidecar；**每个 Bot 一个浏览器会话**，同一个 Bot 并行的任务各用一个标签页（5.8） |
| 连接 | 一个 TCP 端口：主连接 `/ws`（消息、事件、心跳、运行轨迹）+ 按需的画面连接 `/ws/screen` + HTTP（文件）。QUIC 以后作为画面的可选加速（5.7） |
| 模型 | 用户自定义服务商和模型；每个 Bot 一个模型；默认认为模型支持看图 |
| 通知 | Android：前台服务保持长连接，弹本地通知。iOS（以后）：后台走 APNs |
| 许可证、签名 | MIT；签名和公证由用户负责 |

## 3. 参考

见 [REFERENCES.md](REFERENCES.md)。最主要的：pi（运行时、模型接入、工具、技能）、hermes-agent 和 OpenClaw（记忆与上下文）、nightly-labs/openbot（群协作与上下文包，只借鉴设计）、vercel-labs/agent-browser（浏览器）、gpui-kit（桌面 UI）、Compose Multiplatform（移动端）。

## 4. 产品行为

见 [DESIGN.md](DESIGN.md)：核心概念（第 1 章）、完整场景（第 3 章）、群协作规则（4.5）、工作详情与运行轨迹（4.6）、工作台（4.8）、仪表盘（4.9）、技能（4.10）。本文只描述它们在服务端怎么实现。

---

## 5. 架构

### 5.0 部署形态

- **Host**：运行 macbotd 的 Mac，存放 Bot 的会话、记忆、定时任务和工作间。**Client**：桌面和手机 App，可以同时连接多台 Host。
- 选择「中心 Host + 客户端」而不是「每台机器都有 Bot、跨机通信」，原因是：只有一份数据，没有同步问题；Mac 常开，Bot 一直在线；只需要在 macOS 上实现服务端。
- 预留的扩展点：每台 Host 有 `node_id`，所有 ID 用 UUIDv7。以后可以做 Computer Node（让 Bot 操作其他机器的浏览器）和多 Host 联邦。

**macbotd**
1. 无界面守护进程，以**用户级 LaunchAgent** 运行。不能用 root 级的 LaunchDaemon，因为只有在用户会话里才能访问用户的钥匙串和 Chrome。二进制放在一个没有界面的 `.app` 包里（`LSUIElement`），这样签名、公证和系统权限都有稳定的身份。
2. 安装包 `.pkg` 做三件事：安装程序、注册 LaunchAgent、安装结束时打开 `http://localhost:7788/admin`。
3. `/admin` 是极简管理页：首次设置密码、运行状态、改端口和名称、查看日志、重启。页面是嵌入二进制的单个 HTML 文件。
4. CLI 和守护进程是同一个二进制：`macbot status | passwd | logs | restart | update`，通过本地 Unix socket 通信。
5. 平台相关的能力（钥匙串、自启动、浏览器、桌面控制）放在 trait 后面，只实现 macOS 版本，测试时换成假实现。

**首次使用**：安装 .pkg → 浏览器打开管理页，设置密码（没接显示器时可以 SSH 上去执行 `macbot passwd`）→ 在客户端「添加 Host」，填地址和密码 → 之后的一切操作都在客户端里完成。

#### 5.0.1 鉴权

- 密码只存 argon2 哈希（`data/auth.json`，权限 0600）。
- 客户端每次连接都带 `Authorization: Bearer <密码>`；`/admin` 用 HTTP Basic Auth，密码相同。
- **设置密码之前，只接受来自 localhost 的请求。**
- 忘记密码时，在本机执行 `macbot passwd` 重置。

#### 5.0.2 结构图

```
┌──────────── Mac（macbotd，无界面守护进程，LaunchAgent）──────────────┐
│ Gateway  axum，端口 7788：/ws（主连接） /ws/screen（画面） /api/v1 /admin │
│ Local CLI  Unix socket ← macbot status|passwd|logs|restart|update   │
│ Orchestrator  主 Bot、群、任务派发与交接、插话、路由、防循环          │
│ Runtime       durable 执行、send_msg、子代理、审批、挂起与恢复       │
│ Scheduler     并发调度、排队、定时任务                              │
│ Memory        三种记忆、分段与压缩、记忆整理（只在服务端）           │
│ Providers     模型接入（OpenAI 兼容 / Responses / Anthropic / Google）│
│ Tools/Skills  文件、bash、browser、subagent、web、send_msg、技能      │
│ Usage         每次模型调用都记账、按小时汇总                         │
│ Store         JSON 文件（JSONL 日志 + JSON 快照）  Secrets：钥匙串    │
│ 文件：~/MacBot/projects/<slug>/（Home）  ~/MacBot/bots/<bot>/         │
└──────────────────────────────────▲───────────────────────────────────┘
        macbotd ⇄ agent-browser    │ ws(s)://任意 IP 或域名:端口
                     ┌─────────────┴──────────────┐
            Desktop（GPUI，macOS）      Mobile（Compose MP：v1 只有 Android）
```

### 5.1 仓库结构

```
mac-bot/
├── AGENTS.md  COORDINATION.md  LICENSE  README.md
├── docs/                    # 见第 0 章的文档地图
├── protocol/                # 【server-mac】契约的代码形式（kotlin/ 归 client-android）
│   ├── rust/                #   macbot-protocol crate：PROTOCOL.md 里全部类型的 serde 定义
│   ├── schema/              #   从 Rust 类型导出的 JSON Schema（生成物，提交到仓库）
│   ├── fixtures/            #   每种对象、事件、块、TraceItem 的示例；scenarios/*.jsonl 供 mock 回放
│   └── kotlin/              #   schema → kotlinx.serialization 的生成脚本
├── server/                  # 【server-mac】独立的 Cargo workspace
│   ├── crates/
│   │   ├── macbot-store         # JSON 文件存储
│   │   ├── macbot-durable       # Entry / Commit / Job / Inbox / Steer / Resume / Compaction
│   │   ├── macbot-providers     # 模型接入与模型目录
│   │   ├── macbot-tools         # 工具框架和内置工具
│   │   ├── macbot-skills        # 技能扫描、索引、加载、管理
│   │   ├── macbot-memory        # 三种记忆、上下文组装、压缩、整理
│   │   ├── macbot-orchestrator  # 主 Bot、群、任务、路由、调度
│   │   ├── macbot-browser       # agent-browser sidecar 管理、标签页、画面
│   │   ├── macbot-usage         # 记账和汇总
│   │   └── macbot-gateway       # axum：/ws、/ws/screen、/api/v1、/admin
│   └── macbotd/                 # 二进制：守护进程 + CLI + --mock；packaging/（LaunchAgent、pkg 脚本）
├── clients/
│   ├── mac/                 # 【client-mac】独立的 Cargo workspace：macbot-client-core + macbot-desktop
│   └── mobile/              # 【client-android】Kotlin Multiplatform（v1 只有 Android target）
│       ├── shared/          #   commonMain：core/ + feature/*（归属见 AGENTS.md）
│       └── androidApp/          #   （iOS 以后再加 iosApp/）
└── scripts/                 # 跨模块脚本：协议代码生成、端到端场景测试、打包
```

`server/` 和 `clients/mac/` 是两个独立的 Cargo workspace，都用 path 依赖引用 `protocol/rust`。

**分发产物**：`MacBot-Server.pkg`（服务端）、`MacBot.dmg`（桌面客户端）、`MacBot.apk`，另外提供 `install.sh`。

### 5.2 数据存储：JSON 文件

```
~/MacBot/
├── data/
│   ├── node.json  auth.json  settings.json
│   ├── providers.json  models.json       # API Key 存在钥匙串，这里只存引用
│   ├── devices.json
│   ├── bots/<bot_id>.json
│   ├── projects/<project_id>.json
│   ├── chats/<chat_id>/chat.json + messages.jsonl
│   ├── assignments/<yyyy-mm>/<assignment_id>.json
│   ├── artifacts/<project_id>.jsonl
│   ├── threads/<thread_id>/thread.json + segments/<n>.json + entries.jsonl
│   ├── jobs/<job_id>.json                # durable 检查点
│   ├── memories/user.json  bots/<bot_id>.json  projects/<project_id>.json
│   ├── approvals.jsonl  questions.jsonl  steers.jsonl
│   ├── routines/<routine_id>.json + runs.jsonl
│   ├── skills-index.json  skill-invocations.jsonl
│   ├── uploads/<upload_id>
│   ├── usage/raw/<yyyy-mm-dd>.jsonl  usage/hourly/<yyyy-mm>.json
│   └── events/<segment>.jsonl            # 持久事件（带全局 seq），保留 7 天或 10 万条
├── projects/<slug>/                       # 项目 Home
├── bots/<bot_id>/                         # Bot 自己的目录（私聊和转交小事的工作目录）
├── skills/<name>/SKILL.md
└── runs/<run_id>/                         # 被截断的工具输出全文、完整请求（调试开关打开时）
```

**写入规则**
- **JSONL 日志**（消息、轨迹、事件、用量、审批…）：只追加，每行一个完整的 JSON，写完 `fsync`。启动时截掉不完整的最后一行。**日志是事实来源。**
- **JSON 快照**（Bot、群、任务、记忆…）：先写临时文件，`fsync`，再 `rename`（原子替换）。快照可以由日志重建。
- **跨文件的提交**：先把提交记录追加到 `events` 日志，再更新各个快照；重启时回放还没有体现到快照里的事件。
- **并发**：只有 macbotd 一个进程写数据，启动时对 `data/.lock` 加文件锁；进程内的写入经过单一写队列串行化。
- **内存索引**：小对象（Bot、群、任务、记忆、技能、未完成的 job）启动时全部读入内存；消息和轨迹按需从 JSONL 分页读取，每个 JSONL 维护一个 `seq → 字节偏移` 的稀疏索引。
- **检索**：用 ripgrep 的库（`grep-searcher`）直接扫描 JSONL。

**实体与 PROTOCOL 的对应**：Bot、Chat、Message、Project、Assignment、Artifact、Approval、Question、Skill、Routine、Provider、Model、Settings、Device 的字段与 PROTOCOL 第 4 章一致（存储时另外带 `node_id` 和内部字段）。下面是只在服务端存在的实体：

```
Thread   { id, kind: bot|subagent, bot_id, chat_id, assignment_id?, parent_run_id?, segment_no, token_estimate, updated_at }
            -- 执行线程：每个（Bot，会话）一条；子代理每次调用一条
Segment  { thread_id, no, reason: start|compact|project_loaded|snapshot_stale, summary, memory_snapshot, created_at }
Run      { id, thread_id, assignment_id?, phase: chat|work|coordinate|subagent|memory|compact, status, started_at, ended_at }
Entry    { thread_id, aseq, run_id, kind, json }     -- 不可变；TraceItem 和 durable 内部记录共用这个文件
Job      { id, owner, kind, status, checkpoint, updated_at }
Memory   { id, scope: user|bot|project, kind: fact|self|worklog|summary, bot_id?, project_id?, content,
           source: { bot_id?, chat_id?, at }, updated_at }
Usage    { ts, bot_id, project_id?, chat_id, assignment_id?, run_id, phase, provider_id, model_id,
           input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost }
Event    { seq, event, data }                         -- 即 PROTOCOL 6.1 的持久事件
```

### 5.3 Bot 运行模型

#### 5.3.1 两种模式

| | **私聊（对话模式）** | **群里和转交的任务（工作模式）** |
|---|---|---|
| 触发 | 用户在和这个 Bot 的私聊里发消息 | 被 @、被交接、被主 Bot 分派或转交 |
| 执行 | 一次 durable run（可以调用工具），phase=`chat` | 一个任务（assignment）对应一次 durable run，phase=`work` |
| 会话里看到什么 | 模型的文本回复，**流式显示** | **只看到 `send_msg` 发出的完整消息，不流式**；模型的文本、思考和工具调用都不进群 |
| 详细过程 | 运行轨迹（5.11），用户打开时才推送 | 同左 |

#### 5.3.2 `send_msg`：Bot 往会话里说话的唯一途径（工作模式）

```text
send_msg(
  text: string,                                   // Markdown
  intent: "ack" | "progress" | "decision" | "done" | "blocked",
  to?: "task_chat" | "user" | {bot: id},          // 默认是任务所在的会话；"user" = 和用户的私聊；{bot} = Bot 间私信
  mentions?: [ bot_id | {bot, instruction?} | "main" | "user" ],   // @ 谁，谁就行动
  artifacts?: [{title, path_or_url}],             // done 时登记产物
  options?: [string]                              // decision 时的选项
) -> { message_id, then: "continue" | "wait" | "end" }
```

| intent | 用途 | 系统做什么 | 之后 run 怎么走 |
|--------|------|-----------|----------------|
| `ack` | 开工后的第一步：一句话说明收到、打算怎么做（L0 规则要求） | 发消息 | 继续 |
| `progress` | 阶段进展，每个任务最多 3 条 | 发消息 | 继续 |
| `decision` | 需要别人决策 | 发消息；有选项时生成 Question；@用户 → `waiting_user` 并推送；@Bot → 那个 Bot 收到一个回答问题的任务，本任务 `waiting_bot` | 挂起，回复到达后恢复 |
| `done` | 完成 | 发完成消息；产物登记到公告；状态 `done`；@ 到的 Bot 收到新任务（交接）；@主 Bot 时唤醒主 Bot 去汇总并提醒用户验收 | 结束 |
| `blocked` | 卡住 | 发卡住消息；状态 `blocked`；总是唤醒主 Bot 跟进；@用户时推送；@Bot 时那个 Bot 收到一个帮忙的任务 | 挂起 |

**@ 的规则**：@ 某个 Bot 就是让它行动。系统为它在这个会话里创建任务，指令是 `instruction`，没有就用整条消息。如果它在这个群里已经有进行中或挂起的任务，这条消息就作为插话或回复送进去。可以同时 @ 多个 Bot。@主 Bot 会唤醒它的协调流程。@用户只是推送提醒。

**状态由系统维护**：派发时状态变为 `queued` 或 `working`，之后跟随 `send_msg` 的 intent 变化，不依赖模型怎么措辞。

**兜底**：run 结束时既没有 `done` 也没有 `blocked`，系统追加一轮提示，要求 Bot 汇报；仍然没有汇报，就发一条系统消息（`task_no_report`）并唤醒主 Bot。

#### 5.3.3 任务的生命周期

1. **派发**：用户 @、其他 Bot 在 `send_msg` 里 @，或者主 Bot `assign` / `delegate` → 创建 assignment。
2. **排队或开工**：Scheduler 判断是否有并发名额，有就 `working` 并开始 durable run，没有就 `queued` 并记录原因。
3. **干活**：在（Bot，群）的执行线程里运行（上下文见 5.5）。先 `send_msg(ack)`，然后按需调用工具和子代理，阶段节点 `progress`，需要决策时 `decision` 并挂起。
4. **完成**：`send_msg(done, artifacts, mentions: [下一个 Bot])` → 交接（回到第 1 步）。
5. **卡住**：`send_msg(blocked)` → 主 Bot 跟进：重试、换人或问用户。
6. **待验收**：流程中最后一个 Bot 在 `done` 里 @主 Bot。主 Bot **不检查产物内容**，只按公告里的产物清单汇总，把群改为 `review`，在主 Bot 私聊里发待验收卡片并推送。
7. **验收**：用户确认 → 群 `done`，主 Bot 发总结、写项目记忆。用户提修改意见 → 主 Bot 拆分后派给对应的 Bot（回到第 1 步）。

#### 5.3.4 插话（steer）

插话**不会额外调用模型来立即回复**：
1. 用户消息进入任务 run 的 inbox，消息的 delivery 为 `queued`。
2. 在下一个步骤边界（当前的模型调用或工具调用结束后）注入，delivery 变为 `delivered`。下一次模型调用包含它之后，delivery 变为 `read`。轨迹里记一条 `steer`。
3. Bot 读到后，通常用 `send_msg` 回应（L0 规则建议回应）。
4. 任务正挂起在 decision 上时，这条消息就是回复，run 立即恢复。
5. Bot 在这个群里没有任务时，创建一个新任务。
6. 停止：用户点 [停止]（`assignment.stop`），或者发「@编码 停一下」→ 中止 run，状态 `cancelled`，发一条系统消息。

#### 5.3.5 主 Bot

- **只协调，不干活**：没有文件、bash、浏览器、子代理工具。在群里同样只通过 `send_msg` 说话（开场、转派、待验收、总结）；和用户的私聊是对话模式。phase=`coordinate`，**不占任务并发名额**。
- **工具**：
  - `list_bots`、`create_project(name, goal, flow, members, deadline?)`
  - `assign(project, bot, instruction)`、`delegate(bot, instruction)`（小事不建群，结果回到主 Bot 私聊）
  - `project_status(project)`
  - `request_review(project, summary)`、`finish_project(project, summary)`（只能在用户确认之后调用）
  - `propose_bot(name, label, description)`（必须经过用户同意）
  - `notify_user(text)`、`send_msg`
  - 记忆和检索工具
- **被唤醒的时机**：
  - 用户私聊。
  - 群里没有 @ 任何人的消息。
  - 被 @。
  - 有成员 `blocked` 或 `failed`。
  - 某个任务 2 小时没有进展。
- **普通 Bot 转交给主 Bot**：用户在和普通 Bot 的私聊里提出一件需要别人配合的事时，这个 Bot 用 `send_msg(to: {bot: 主 Bot}, mentions: ["main"])` 把事情转给主 Bot，并在私聊里告诉用户「已交给总管拉群处理」；之后由主 Bot 建群。
- **提议新建 Bot**：`propose_bot` 会在主 Bot 私聊里生成一个提问卡片（「要不要新建『测试』Bot？」）。用户选「新建」后，服务端创建这个 Bot，主 Bot 继续。
- **建群规则**：满足任一条件就建群：需要其他 Bot、要跨多次对话、有需要沉淀的产出、用户要求建群。小事用 `delegate`；闲聊和知识问答直接回答。设置 `main_bot.auto_create_project=false` 时，建群前先问用户。用户说「不用建群」时，解散群，事情回到私聊。

#### 5.3.6 并发调度

- **调度单位**：（Bot，群）的工作 run；私聊和定时任务也各算一个。
- **限制**：
  - 全局上限 `concurrency.global`，默认 8。
  - 每个 Bot 的上限 `max_parallel`，默认 3。
  - 同一个群里，同一个 Bot 的任务串行执行。
  - 挂起中的任务不占名额。
  - 主 Bot 的协调调用和私聊对话不占名额，单独按每分钟次数限速。
- **排队顺序**：用户直接 @ > 交接 > 定时任务；同级按到达时间。
- **子代理**：每个 run 最多同时 4 个，全局最多 12 个，不占 Bot 的名额。
- **共享资源**：
  - 浏览器按 Bot 共享（5.8）。
  - Home 目录由群成员共享，约定每个 Bot 写自己的子目录（例如 `product/`、`code/`、`test/`）。
  - 记忆的写入在 run 结束时提交，冲突时以后写入的为准。

#### 5.3.7 持久化与恢复

- 每一步都先写入 JSONL 并 `fsync`，然后才推送。重启后，`working` 状态的任务自动 resume。
- 有副作用的工具调用标记为「不可安全重放」，resume 到这一步时先问用户或主 Bot。
- `send_msg` 用 `run_id + call_id` 做幂等键，恢复时不会重复发消息。
- 挂起中的 run 重启后继续等待，被唤醒时重新排队。

### 5.4 群消息路由（确定性规则，不用模型猜）

1. 用户 @ 了某些 Bot → 投递给它们：有进行中或挂起的任务就作为插话或回复，没有就创建新任务。
2. 用户回复了某条 Bot 消息 → 投递给那个 Bot，规则同上。
3. 没有 @ 也没有回复 → 交给主 Bot。
4. Bot 在 `send_msg` 里 @ 了其他 Bot → 为它们创建任务（交接或请求帮忙）；@主 Bot → 唤醒主 Bot。
5. **防循环**：一条用户消息引发的 Bot 之间自动派发链，最多 `concurrency.loop_hops`（默认 8）次，超过后暂停，发 `loop_paused` 块，等用户调用 `loop.resolve`。同一个 Bot 对同一个触发只响应一次。
6. Bot 私信（`send_msg(to: {bot})`）进入 `bot_dm` 会话，并在群里显示 `bot_dm_ref`。
7. 审批、提问、接管请求只出现在对应 Bot 的私聊里；群里显示 `approval_ref`。

### 5.5 记忆与上下文（完全由服务端管理）

> 客户端不展示、也不编辑记忆。唯一的例外是公告里的「项目要点」（`Announcement.highlights`），它是项目记忆的只读摘录。
> 参考：hermes-agent（冻结快照、压缩前先存记忆）、OpenClaw（压缩和裁剪）、nightly-labs/openbot（每个 agent × 每个频道一条独立线程、频道上下文包、80% 阈值、记忆在一轮成功结束后才提交）。

#### 5.5.1 三种记忆

| 记忆 | 共享范围 | 内容 | 谁来写 | 自动加载的上限 |
|------|----------|------|--------|---------------|
| 用户记忆 | 所有 Bot | 用户的习惯和偏好 | 任何 Bot 发现后写入 | 1,500 字 |
| Bot 记忆 | 只有这个 Bot | 自我定义（做事方式、经验）+ 工作记录（做过的事） | Bot 自己写；任务完成和定时任务运行后，系统自动追加工作记录 | 经验 2,000 字 + 工作记录 1,500 字 |
| 项目记忆 | 群成员 | 目标、决议、分工、进展、关键数据 | 成员 Bot 形成结论时写入；主 Bot 在完成时写总结 | 3,000 字 |

- 上限是刻意设的：写满之后，`memory` 工具返回「已满」，Bot 必须合并或替换旧条目。
- 工作记录会自动滚动：超出上限的部分合并成按月的摘要。
- 写入时机：一轮 run 中先暂存，这一轮成功结束后才提交；被中断的 run 不留下记忆。
- 每条记忆记录来源（Bot、会话、时间）。

#### 5.5.2 加载哪份记忆

| 会话 | 加载 |
|------|------|
| 群 | 用户记忆 + 自己的 Bot 记忆 + 这个项目的记忆 |
| 私聊 | 用户记忆 + 自己的 Bot 记忆 + 自动判断的相关项目记忆：Bot 调用 `project_find`，找到后开新段加载；如果用户要推进那件事，Bot 把对话转到群里 |
| 定时任务 | 用户记忆 + Bot 记忆 + 所属项目的记忆（如果有） |

#### 5.5.3 执行线程、分段和压缩

- 每个（Bot，会话）组合有一条执行线程；同一个 Bot 在不同群里的上下文互不干扰。
- 线程按段推进，以下情况开新段：上下文达到模型窗口的 80%；私聊里加载了一个项目的记忆；记忆快照过期（有记忆被修改，或者距上次超过 24 小时）。
- 开新段的步骤：
  1. **记忆提取**：先跑一轮只开放 `memory` 工具的 run。
  2. **裁剪**旧的工具输出。
  3. 用 maintenance 模型给较早的对话做**摘要**，最近几轮原样保留。
  4. 重新生成记忆快照。
- 完整记录永远保留，压缩只影响下一轮送给模型的内容。

#### 5.5.4 每一轮的上下文组装（越稳定越靠前，便于 prompt cache）

| 层 | 内容 | 什么时候变 |
|----|------|-----------|
| L0 平台规则 | 工具用法、`send_msg` 规范、审批、交接格式、建群规则 | 发版时 |
| L1 Bot 身份 | 资料（名字、头衔、长期规则）+ 技能清单 + 工具清单 | 编辑 Bot 时 |
| L2 记忆快照 | 按 5.5.2 加载的记忆 | 只在开新段时 |
| L3 会话上下文 | 私聊：段摘要 + 最近的消息。群：群上下文包（见下） | 每轮 |
| L4 当前 run | 工具调用和结果 | 每步 |
| 按需检索 | `session_search`、`memory_search`、`project_find`、`chat_history`、读文件 | Bot 自己调用 |

**群上下文包**：
- 公告的文本版：目标、流程、分工、成员状态、产物、项目要点。
- 本次任务的指令，以及触发它的消息和它引用的消息。
- 最近 30 条群消息。
- 更早消息的版本化摘要。
- 附件只给引用。

群里其他成员的 `send_msg` 消息会进入上下文包，Bot 自己的模型文本不会，所以 Bot 之间只通过 `send_msg` 交流。Bot 间的消息必须自包含。

#### 5.5.5 记忆整理

- 平台规则（L0）提醒 Bot：用户表达了偏好、纠正了它，或者形成了结论时，写进相应的记忆。
- 每天空闲时，用 maintenance 模型做一次后台整理：合并重复条目、清理过期内容、把工作记录合并成月度摘要。

#### 5.5.6 公告

公告不单独存储，由 Project、成员分工、各成员当前任务的状态、Artifact、项目记忆（取前 20 条作为 highlights）实时拼出来。任何相关变化都推送 `announcement.updated`。

### 5.6 模型接入

- API 类型：`openai-completions`、`openai-responses`、`anthropic-messages`、`google-generative`。任意 OpenAI 兼容端点都可以接入（Ollama、vLLM、DeepSeek、Qwen 等）。
- 统一的流式事件：TextDelta、ThinkingDelta、ToolCallDelta、Usage、Stop。
- API Key 存在钥匙串里，`provider.list` 只返回 `has_key`。
- 每个 Bot 一个模型（私聊和干活共用）；主 Bot、子代理和 maintenance（记忆整理、压缩）各有默认模型。

### 5.7 连接

协议见 PROTOCOL.md。一个 TCP 端口上有三类通道：
- **主连接 `/ws`**：请求和响应、持久事件（全局 `seq`，断线补发）、临时事件、运行轨迹（订阅制）。客户端在前台时常驻。
- **画面连接 `/ws/screen`**：Agent Computer 的 JPEG 帧和输入，只在打开画面时建立。和主连接分开，是为了避免 TCP 丢包重传时画面数据阻塞消息和心跳（队头阻塞）。服务端采用 ack 节流，只发最新的一帧。
- **HTTP**：文件、上传、被截断的工具输出全文、CSV 导出、`/api/v1/rpc`。
- **以后可选**：如果实测公网或手机网络上画面卡顿，再给画面加一个 QUIC 通道（同端口的 UDP，Rust 用 quinn）作为加速，失败时自动退回 `/ws/screen`。v1 不做，原因有四个：
  1. 不少网络会限制 UDP，必须保留 TCP 作为备用，等于两套传输都要实现。
  2. QUIC 强制 TLS，需要证书管理。
  3. Kotlin Multiplatform 里没有成熟的 QUIC 客户端。
  4. 两个通道之间的鉴权和消息顺序需要额外协调。
- 客户端为每台 Host 保存 `{node_id, name, addresses[], password, last_seq}`，按顺序尝试各个地址。
- **移动端后台**：Android 用前台服务保持主连接，收到事件后弹本地通知。（iOS 以后再做：进入后台时断开，后台期间的提醒走 APNs。）

### 5.8 浏览器：vercel-labs/agent-browser

- **集成方式**：sidecar 进程，随 .pkg 一起分发。macbotd 调用它的 CLI（`--json` 输出），并代理它在 localhost 上的画面流。
- **每个 Bot 一个浏览器会话**：`--session <bot_id>`。登录状态在这个 Bot 的所有任务之间共享。
- **并行任务用标签页隔离**：同一个 Bot 在不同群里并行的任务，各自有一个标签页（tab ↔ assignment 的对应关系由 macbotd 维护）。这个 Bot 的浏览器动作在一个队列里**串行执行**；执行前先切换到该任务的标签页，避免并行任务互相抢焦点。
- **模式**（Bot 设置 `browser_mode`）：

  | 模式 | 用法 | 说明 |
  |------|------|------|
  | `headless` | 无头、不带 profile | 不需要登录的任务 |
  | `headless_profile`（默认） | `--profile Default --restore` | 复用本机 Chrome 的登录状态（复制一份只读快照），Bot 自己更新的 cookie 由 `--restore` 保存 |
  | `attach` | `--auto-connect` | 连接用户正在使用的 Chrome；只用于需要人在旁边的场景 |

- **内存**：每个会话大约 300–500 MB。会话空闲 15 分钟后关闭，下次使用时恢复。
- **画面**：agent-browser 的 screencast → macbotd → `/ws/screen` 二进制帧（PROTOCOL 第 8 节）。没有画面连接时，停止 screencast。无头模式的画面来自 CDP screencast，和 Mac 屏幕是否锁定无关。
- **接管**：`request_takeover` 工具让当前任务挂起（`wait.reason=takeover`），并发出 `takeover_request` 块；用户 `takeover.start` 之后，输入事件转发给浏览器；`takeover.release` 之后任务恢复。
- **开发时需要先确认**：`--profile Default` 读取 Chrome cookie 时会不会触发钥匙串弹窗；`--profile` 和 `--restore` 组合使用的行为。
- 原生桌面控制（cua-driver）留到以后。

### 5.9 用量记账

- **每次模型调用都记一行 Usage**：Bot、群、任务、阶段（chat / work / subagent / coordinate / memory / compact）、模型、各类 token、费用。费用 = token × 模型单价；没有配置单价时为 null。子代理的消耗记在调用它的 Bot 名下。
- 写入 raw 日志的同时更新内存里的小时汇总，每分钟落盘；重启时用 raw 日志补齐。
- 仪表盘的方法（`usage.*`）只读小时汇总，再聚合成天和周；参数和返回值见 PROTOCOL 5.11。
- `usage.tick` 每个任务每 10 秒最多推一次。

### 5.10 工具与技能

> 参考 pi 的 `packages/coding-agent/src/core/tools`。

#### 5.10.1 工具框架

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;      // 参数的 JSON Schema（schemars 生成）
    fn risk(&self, args: &Value) -> Risk;       // Read | Write | Exec | External
    async fn call(&self, ctx: &ToolCtx, args: Value) -> ToolResult;
}
struct ToolResult { content: Vec<Part /* Text | Image */>, details: Value, is_error: bool }
```

- **截断**：文本输出最多 2,000 行或 50 KB（bash 保留尾部，其他工具保留头部）；完整输出写入 `runs/<run_id>/`，通过 `GET /api/v1/trace/output` 读取。
- **出错**时返回 `is_error` 结果，让模型自己修正，不抛异常。
- **审批**：调用前按 `risk` 和规则判断（自动允许 / 先问我 / 拒绝）。需要问的，挂起 run（`wait.reason=approval`），在私聊里发审批卡片。内置的「先问我」规则包括：`sudo`、在 Home 以外执行 `rm -rf`、`git push`、浏览器里付款、`browser_eval`。
- **并行**：同一条 assistant 消息里的只读调用（read、grep、find、ls、web_fetch、subagent）并行执行；写类调用按顺序执行；同一个文件的写入串行化。

#### 5.10.2 工具清单

| 工具 | 参数 | 说明 |
|------|------|------|
| `read` | `path, offset?, limit?` | 文本或图片（jpg、png、gif、webp） |
| `write` | `path, content` | 自动创建父目录 |
| `edit` | `path, edits: [{oldText, newText}]` | 每段 `oldText` 必须唯一且互不重叠 |
| `ls` | `path?, limit?=500` | 目录名带 `/`，包含隐藏文件 |
| `find` | `pattern, path?, limit?=1000` | glob，遵守 .gitignore |
| `grep` | `pattern, path?, glob?, ignoreCase?, literal?, context?, limit?=100` | 遵守 .gitignore |
| `bash` | `command, timeout?, background?, cwd?` | `/bin/zsh -lc`；返回 `{output, truncated, exit_code, wall_time_seconds}`；取消时杀掉整个进程组；`background=true` 时返回 job_id |
| `bash_job` | `job_id, action: status\|output\|kill` | 管理后台进程；任务结束时一并结束（登记为产物的服务除外） |
| `browser_open` `browser_snapshot` `browser_act` `browser_get` `browser_wait` `browser_screenshot` `browser_tabs` `browser_nav` `browser_eval` | 见 agent-browser 的命令 | 会话和标签页由系统注入，模型不能访问其他 Bot 的会话或其他任务的标签页 |
| `request_takeover` | `reason` | 请求用户接管 |
| `subagent` | `task, context?, tools?, model?, max_turns?=30` | 见 5.10.3 |
| `skill` | `name, args?` | 加载 SKILL.md 全文，返回技能目录路径 |
| `web_fetch` | `url, prompt?` | 网页转 Markdown |
| `web_search` | `query, limit?` | 只有在设置里配置了搜索服务才会出现 |
| `send_msg` | 见 5.3.2 | |
| `memory` | `scope, action: add\|replace\|remove, content, id?` | |
| `memory_search` `session_search` `project_find` `chat_history` | | 检索 |
| `routine` | `action: list\|create\|update\|delete, routine?` | 通过对话为自己创建和管理定时任务（每个 Bot 最多 50 个，间隔至少 5 分钟）；主 Bot 也可以为其他 Bot 创建 |
| 主 Bot 专用 | 见 5.3.5 | |

- 相对路径以项目 Home 为准（私聊和转交的小事以 `~/MacBot/bots/<bot>/` 为准）。
- bash 不继承 macbotd 自身的密钥。

**各角色能用的工具**

| 工具 | 主 Bot | Bot | 子代理 |
|------|:---:|:---:|:---:|
| 文件、bash | — | ✅ | 默认只读，可以授权写入 |
| 浏览器 | — | ✅ | 默认只读类 |
| subagent | — | ✅ | — |
| skill、web | ✅ | ✅ | ✅ |
| send_msg | ✅ | ✅ | — |
| 记忆和检索 | ✅ | ✅ | — |

每个 Bot 可以在设置里关掉某类工具（`Bot.tools`）。

#### 5.10.3 子代理

- 一次独立的模型运行：全新的上下文，只有简短的系统提示、技能清单和 `task`。没有 Bot 身份、记忆、群消息和公告。
- 不能调用 `send_msg`、记忆、`subagent`（最多一层）、`request_takeover`。
- 同一条 assistant 消息里的多个 `subagent` 调用并行执行。只把结论交回给 Bot，保持 Bot 的上下文干净。
- 有自己的线程（`kind=subagent`，`parent_run_id`），可以恢复；轨迹里显示为嵌套的 run。
- 消耗记在所属的 Bot 名下，phase=`subagent`。

#### 5.10.4 技能

- 格式遵循 Agent Skills 规范（与 pi、Claude Code 兼容）：每个技能一个目录，SKILL.md 的 frontmatter 里有 `name` 和 `description`，可选 `disable-model-invocation`；还可以带 `scripts/`、`references/`、`assets/`。
- 位置：`~/MacBot/skills/`，另外扫描 `settings.skills.extra_dirs`（例如 `~/.agents/skills`）。监听目录变化，自动重新扫描。
- 所有 Bot 共用：系统提示里只放技能的名字、描述和路径，需要时由模型调用 `skill` 加载全文；用户可以用 `/技能名` 强制加载。可以对个别 Bot 停用。
- 客户端通过 `skill.*` 方法增删改、启用/停用、导入；逻辑全部在服务端。
- Bot 生成的技能草稿写在 `skills/.drafts/`，用户在技能页确认后才会启用。
- 内置技能：`agent-browser`、`macbot-collab`（send_msg 和交接的写法）、`project-home`（Home 目录的约定）。

### 5.11 运行轨迹

- 每个 run 的每一步都以 TraceItem（PROTOCOL 4.12）的形式追加到 `entries.jsonl`，与 durable 的内部记录在同一个文件。恢复运行和回放用的是同一份数据。
- `aseq` 在每个任务（或私聊线程）内单调递增；子代理的条目也编入所属任务的 `aseq`。
- 模型的流式片段不落盘，只推送给订阅者（`trace.delta`）；每次模型调用结束时写一条包含全文的 `llm.response`。
- 打开「保存完整请求」后，完整请求写入 `runs/<run_id>/requests/`。
- **查看流程：先拉历史，再带游标订阅**（PROTOCOL 7.1）。服务端在订阅时，先补发游标之后的条目，再推送实时条目，并返回进行中的模型调用已经输出的部分，所以不会漏数据。
- **只在有订阅时推送**；没有人看的时候只落盘。

---

## 6. 开发计划（唯一的计划）

**目标是一次做完全部功能**：阶段只决定先后顺序和联调时间点，不削减范围。**三条开发线加一条集成线**同时推进，归属见 AGENTS.md，启动 prompt 见 AGENT_PROMPTS.md。

**开发机就是目标机**：这台 Mac mini（Apple M4，16 GB，局域网 IP 192.168.31.162）既是开发机，也是最终运行 macbotd 的 Host。**每个阶段的成果都要能在这台机器上直接运行和查看**：服务端以 LaunchAgent 方式常驻，桌面客户端打包成 .app 打开使用，Android 客户端跑在本机的 Android 模拟器里（连接 `10.0.2.2:7788`）。以后换成真机时，手机连接 `192.168.31.162:7788`。

| 阶段 | server-mac | client-mac | client-android（Android 全部功能） | 联调验收 |
|------|-----------|-----------|----------------|---------|
| **S0 契约与骨架** | protocol crate（PROTOCOL 全部类型）+ schema + fixtures + 场景；server workspace；`--mock` 实现全部方法 | 工程、client-core（连接、补发、状态）、设计 token、三栏布局、连接页；可以双击打开的 .app | KMP 工程（只有 Android target）、core（连接、补发、状态、画面连接）、设计系统、导航、androidApp 外壳、feature/connect | 桌面和 Android 模拟器都连上 mock，看到会话列表 |
| **S1 单 Bot 闭环** | 存储、durable、模型接入、工具（文件、bash）、技能加载、私聊对话、基础压缩、运行轨迹、用量记账、鉴权、/admin、CLI、LaunchAgent | 私聊（流式）、消息块、运行轨迹（实时 + 回放）、模型与服务商设置 | feature/chat（私聊、消息块、送达状态）、feature/trace（运行轨迹、历史任务）、feature/settings | 真实服务端：桌面和手机都能和一个 Bot 私聊，Bot 能读写文件、跑命令；轨迹实时可看、可回放；kill -9 重启后能恢复 |
| **S2 主 Bot 与群协作** | 主 Bot、群和公告、任务派发与交接、`send_msg`、插话、子代理、并发调度、审批、提问、防循环、工作台、Bot 增删改、团队模板 | 群（状态条、公告、任务卡片、送达状态、待验收）、新建群和 Bot、Bot 设置、工作台、审批 | feature/group、feature/mainbot、feature/approval、feature/workbench、feature/bots | 「登录功能」完整场景两端都能操作；两个群并行；运行中插话生效 |
| **S3 技能、仪表盘、记忆** | 三种记忆、记忆提取与整理、`project_find`、技能管理接口（含草稿发布）和导入、仪表盘接口、搜索 | 仪表盘、技能页、搜索 | feature/dashboard、feature/skills、feature/search | 仪表盘两端数据一致；技能增删改；Bot 跨会话记住用户偏好 |
| **S4 浏览器、定时任务** | agent-browser（每个 Bot 一个会话、按任务分标签页）、`/ws/screen`、接管、定时任务和 `routine` 工具 | Agent Computer、定时任务编辑 | feature/computer（触摸接管）、feature/routines、通知完善 | 用 Chrome 的登录状态刷 X 并总结；手机上接管登录；定时任务按时运行并通知 |
| **S5 打磨与分发** | .pkg、install.sh、`macbot update` | .dmg、自动更新、快捷键、性能 | 发布版 APK、性能 | 全新安装：pkg → 设置密码 → 两端连接 → 完整场景 |

**执行规则**
- 每条线完成一个阶段后，在 `COORDINATION.md` 里打卡「Sx 完成」，然后**直接进入下一阶段**，不停下来等待。
- **集成线（integrator）** 负责：
  - 在这台 Mac mini 上持续部署 main 分支的最新版本：`scripts/dev/deploy.sh` 编译并安装 macbotd 的 LaunchAgent、编译桌面客户端 `.app`、启动 Android 模拟器并安装 APK。
  - 编写并运行每个阶段的端到端场景（`scripts/e2e/<阶段>/`）。
  - 截图存档到 `docs/progress/<阶段>/`。
  - 把问题记录到 COORDINATION.md，交给对应的开发线。
- 三条开发线都打卡某个阶段后，集成线在 Mac mini 上跑完该阶段的「联调验收」，并把结果写进 COORDINATION.md。
- 只有被其他开发线阻塞时才停下来，并在 COORDINATION.md 里写明原因。

## 7. 开发环境（这台 Mac 上已经装好）

| 用途 | 工具 |
|------|------|
| Rust | stable（rustfmt、clippy），crates.io 走清华 tuna 镜像，`cargo search` 需要加 `--registry crates-io` |
| GPUI | 只依赖 `gpui-kit` 0.7；需要 Xcode 26 和 Metal 工具链（已经安装） |
| Android | JDK 21、Gradle 9.8（项目内使用 wrapper）、Android SDK 36、build-tools 36.1、adb、Android 模拟器（AVD `macbot_api36`：Android 16 / API 36、arm64，已经在这台 Mac mini 上装好）。模拟器里访问 Mac 本机要用 `10.0.2.2`：mock 是 `10.0.2.2:7789`，正式服务是 `10.0.2.2:7788`。启动命令：`$ANDROID_HOME/emulator/emulator -avd macbot_api36`（需要无窗口运行时加 `-no-window -no-audio`） |
| 浏览器 | Chrome（S4 使用） |
| 机器 | 这台 Mac mini（M4，16 GB，192.168.31.162）就是开发机和目标 Host；数据目录开发时可以用 `MACBOT_HOME` 指到别处，部署时用 `~/MacBot` |

## 8. 不在 v1 范围内

- **以后做**：iOS 客户端（含 APNs 推送）；Windows 客户端；Computer Node；多 Host 联邦；原生桌面控制（cua-driver）；TLS、审计、设备管理；示范一次生成技能；语音输入和语音对话；小米厂商推送。
- **不做**：Linux 服务端；虚拟机或沙箱；多用户。
- 断电重启后的自动恢复需要关闭 FileVault 并开启自动登录，由用户自行决定。
