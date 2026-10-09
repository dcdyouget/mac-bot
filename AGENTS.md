# AGENTS.md：多 agent 并行开发约定

> 本仓库由多个 AI agent 同时开发。动手之前先读完本文，再读 `docs/` 下的文档。**每类规则只有一个权威来源**（见 PLAN 第 0 章）：协议字段看 PROTOCOL.md，界面和交互看 DESIGN.md，服务端实现和开发计划看 PLAN.md。各开发线的启动 prompt 在 `docs/AGENT_PROMPTS.md`。

## 1. 开发线和目录归属（四条开发线 + 一条集成线）

| 开发线 | 负责的目录（只改这些） | 产出 |
|--------|------------------------|------|
| **server-mac** | `server/`、`protocol/rust/`、`protocol/schema/`、`protocol/fixtures/` | `macbotd`（守护进程 + CLI + `--mock`）、协议的 Rust 类型、JSON Schema、fixtures |
| **client-mac** | `clients/mac/` | macOS 桌面客户端（Rust + GPUI + gpui-kit） |
| **client-android** | `clients/mobile/`（除下面归 client-ios 的部分）、`protocol/kotlin/` | KMP 的 core、Android 外壳，以及归属自己的 feature |
| **client-ios** | `clients/mobile/iosApp/`、`clients/mobile/shared/src/iosMain/`，以及 `shared` 里归属自己的 feature | iOS 外壳，以及归属自己的 feature |
| **integrator**（集成线） | `scripts/`、`docs/progress/`；负责整理 `COORDINATION.md` | 在这台 Mac mini 上持续部署 main 分支、编写和运行端到端场景、截图存档、分派问题；**不修改其他开发线的代码** |

**`clients/mobile/shared/src/commonMain/` 的归属**（包名 `bot.mac.mobile`）：

| 目录 | 归属 |
|------|------|
| `core/`（网络、重连和补发、协议模型、状态存储、设计系统、导航框架、fixtures 加载） | client-android |
| `feature/connect`（Host 管理和连接）、`feature/chat`（私聊、消息块渲染、送达状态）、`feature/group`（群、公告、状态条、任务卡片、插话）、`feature/mainbot`（新群卡片、待验收卡片、转交）、`feature/approval`（审批、提问）、`feature/search`、`feature/routines`（查看、暂停/恢复） | client-android |
| `feature/trace`（工作详情、全屏运行轨迹、历史任务）、`feature/workbench`、`feature/dashboard`、`feature/skills`（增删改、导入、启用/停用）、`feature/bots`（Bot 列表和资料编辑）、`feature/settings`、`feature/computer`（Agent Computer） | client-ios |

> 记忆完全由服务端管理，**任何客户端都没有记忆相关的页面**。

- `docs/`、`AGENTS.md`、根目录文件：**不要随意修改**。确实需要修改时，单独提一个 commit，并在提交说明里写清楚理由。
- 需要改别人目录里的东西时，不要直接改：在仓库根目录的 `COORDINATION.md` 里追加一条请求（谁、要什么、为什么），由负责的开发线处理。
- 两条开发线都需要的代码（例如 mobile 的 core）由归属方先提供**接口**，另一方基于接口加 fixtures 开发，不必等实现完成。

## 2. 分支与提交

- 每条开发线使用自己的 git worktree 和分支，互不干扰：
  ```bash
  git worktree add ../mac-bot-server        -b dev/server-mac
  git worktree add ../mac-bot-client-mac    -b dev/client-mac
  git worktree add ../mac-bot-client-android -b dev/client-android
  git worktree add ../mac-bot-client-ios    -b dev/client-ios
  git worktree add ../mac-bot-integrator    -b dev/integrator
  ```
- **小步提交，频繁合入 main**（至少每完成一个可以运行的小功能合一次）：先 `git fetch && git rebase origin/main`，确认能编译、测试通过，再合入 main 并推送。
- 提交说明的前缀：`server:`、`protocol:`、`client-mac:`、`mobile-core:`、`android:`、`ios:`、`mobile-<feature>:`、`integrator:`。
- 每条开发线在自己目录的 README 里写清楚**怎么编译和运行**（命令、环境变量、产物路径），集成线据此编写部署脚本。结尾按仓库约定附上 Co-Authored-By。
- 不要提交构建产物、密钥或 `local.properties`（见 `.gitignore`）。

## 3. 协议变更流程（最重要）

协议是四条开发线之间唯一的契约。

1. 任何一方需要改协议时，先在 `COORDINATION.md` 里写明需求。
2. **server-mac** 负责落地，顺序是：
   1. 修改 `docs/PROTOCOL.md`。
   2. 修改 `protocol/rust` 中的类型。
   3. 重新导出 `protocol/schema`。
   4. 补充或更新 `protocol/fixtures`。
   5. 用一个 `protocol:` 前缀的 commit 合入 main。
3. 各客户端随后跟进：client-android 运行 `protocol/kotlin` 下的生成脚本，更新 Kotlin 模型；client-mac 直接依赖 `protocol/rust`，无需额外操作。
4. 只新增字段、事件或消息块类型，属于兼容变更，不升协议版本；删除或修改已有字段，才需要升版本，并且要提前在 `COORDINATION.md` 里通知。
5. 客户端必须**忽略不认识的字段和事件**，不认识的消息块用 `fallback_text` 显示。

## 4. 共同的技术约定

- **语言和文案**：界面文案用中文，但都要通过 i18n 的 key 引用，不要把文案硬编码在组件里。代码、注释、标识符用英文。
- **设计**：颜色、字号、圆角、间距以 DESIGN.md 第 2 章为准。三端各自实现一套设计 token，名称保持一致（例如 `bg.sidebar`、`bubble.user`）。
- **连接**：客户端连接的地址是任意 `host:port` 或 `ws(s)://域名`，不要假设是 localhost。
- **服务端数据**：服务端使用 JSON 文件存储，不引入数据库（见 PLAN 5.2）。
- **开发环境**（这台 Mac 上已经装好）：
  - Rust stable，crates.io 走清华 tuna 镜像；`cargo search` 需要加 `--registry crates-io`。
  - Xcode 26.6 + Metal 工具链 + iOS 26.5 模拟器。
  - JDK 21、Android SDK 36、Gradle 9.8。
- **在 Mac mini 上能看到效果**：这台 Mac mini（M4，192.168.31.162）就是目标 Host。每个阶段的成果都要能部署到这台机器上运行和查看（集成线负责部署）。
- **mock 优先**：客户端先基于 `protocol/fixtures` 开发；server-mac 提供 `macbotd --mock` 之后，切换到 mock；最后再连接真实服务端。
- **测试**：
  - server：单元测试，加一个基于 mock provider 的场景测试。
  - 客户端：核心逻辑（重连、补发、状态合并）要有单元测试；界面至少能对着 mock 跑通。
- **不确定时**：按文档地图找权威文档；文档没有覆盖到的，按最简单可行的做法实现，并在 `COORDINATION.md` 里记录你的决定。

## 5. 阶段与汇合点

阶段划分见 PLAN 第 6 章（唯一的开发计划）：S0 契约与骨架 → S1 单 Bot 闭环 → S2 主 Bot 与群协作 → S3 技能、仪表盘、记忆 → S4 浏览器、定时任务、推送 → S5 打磨与分发。**目标是一次做完全部功能**：每完成一个阶段，在 COORDINATION.md 打卡后直接进入下一阶段；四条开发线都打卡后，由集成线在 Mac mini 上运行该阶段的联调场景。只有被阻塞时才停下来。
