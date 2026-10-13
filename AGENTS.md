# AGENTS.md：开发约定（单 agent + subagent）

> 本仓库由**一个主 agent** 负责全部代码，按需派 **subagent** 写具体模块。动手之前先读完本文，再读文档。**每类规则只有一个权威来源**（见 PLAN 第 0 章）：协议字段看 PROTOCOL.md，产品行为看 DESIGN.md，界面视觉、像素、文案 key 看 `design/`，服务端实现和开发计划看 PLAN.md。

## 1. 模块划分（给 subagent 分配任务的单位）

| 模块 | 目录 | 产出 |
|------|------|------|
| **server** | `server/` | `macbotd`（守护进程 + CLI + `--mock` + `/admin`） |
| **protocol** | `protocol/rust/`、`protocol/schema/`、`protocol/fixtures/`、`protocol/kotlin/` | 协议的 Rust 类型、JSON Schema、fixtures、Kotlin 模型生成 |
| **desktop** | `clients/mac/` | macOS 桌面客户端（Tauri 2：Rust 网络层与数据层 `macbot-client-core` + React/TypeScript 界面） |
| **mobile** | `clients/mobile/` | Android 客户端（KMP 工程，v1 只有 Android target） |
| **design** | `design/` | 视觉规范、原型、token、文案 key、提示音 |

**`clients/mobile` 的代码组织**（包名 `bot.mac.mobile`）：`shared/src/commonMain` 下分为 `core/`（网络、重连和补发、协议模型、状态、设计系统、导航、fixtures 加载）和 `feature/*`（connect、chat、trace、group、mainbot、approval、workbench、bots、settings、dashboard、skills、search、routines、computer）。**commonMain 里不要直接使用 Android API**，平台能力用 expect/actual，这样以后加 iOS 只需要增加 target 和 iosApp 外壳。

> v1 不做 iOS。记忆由服务端管理；**客户端只有「用户记忆」一个页面**（设置 → 记忆），可以查看和编辑；Bot 记忆、项目记忆不展示。

**主 agent 与 subagent 的分工**
- 主 agent：拆任务、派 subagent、审查结果、做决定、改共享文件、提交与推送。**代码开发、编译、运行测试、启动应用、截图、调研等能交给 subagent 的工作，主 agent 一律不亲自做。**
- subagent：**一次只负责一个模块**，只改该模块目录。任务说明里写清楚目标、要读的文档、验收标准。
- **共享文件只由主 agent 改**：`docs/`、`design/`、`AGENTS.md`、`DECISIONS.md`、`protocol/` 的契约部分（`docs/PROTOCOL.md` 与 Rust 类型）。
- 并行的 subagent 如果可能改到同一批文件，给它们各开一个临时 worktree（做完由主 agent 合并后删除）；只动各自目录时可以直接在主目录工作。
- 主 agent 不能只看写代码的 subagent 自己的汇报：每批改动合入后，另派一个验证 subagent 跑相关测试、对着 mock 实际打开应用并截图，主 agent 根据验证结果决定是否通过。

## 2. 分支与提交

- 只有一条长期分支 `main`。小改动直接提交；大改动开**临时短分支**，完成后合并到 main 并删除。不保留按模块划分的长期分支。
- 合入前确认能编译、测试通过，然后推送 origin。
- 提交说明的前缀按模块：`server:`、`protocol:`、`desktop:`、`android:`、`design:`、`docs:`。结尾按仓库约定附上 Co-Authored-By。
- 每个模块在自己目录的 README 里写清楚**怎么编译、运行、打包**（命令、环境变量、产物路径）。
- 不要提交构建产物、密钥或 `local.properties`（见 `.gitignore`）。

## 3. 协议变更（最重要）

协议是服务端和两个客户端之间唯一的契约。**一次协议变更在同一个提交（或同一个短分支）里改完所有地方**，不留两端对不上的中间状态：
1. `docs/PROTOCOL.md`
2. `protocol/rust` 的类型 → 重新导出 `protocol/schema` → 补充或更新 `protocol/fixtures`
3. `protocol/kotlin` 运行生成脚本，更新 Kotlin 模型
4. 服务端实现、mock、两个客户端的使用处
5. 提交前缀 `protocol:`

规则：
- 只新增字段、事件或消息块类型，属于兼容变更，不升协议版本；删除或修改已有字段才需要升版本。不再使用的字段保留、写空值。
- 客户端必须**忽略不认识的字段和事件**，不认识的消息块用 `fallback_text` 显示。
- 重要的产品或技术决定（包括协议取舍）记进根目录的 `DECISIONS.md`，一条一行。

## 4. 共同的技术约定

- **语言和文案**：界面文案用中文，但都要通过 i18n 的 key 引用，不要把文案硬编码在组件里。代码、注释、标识符用英文。
- **设计**：界面、颜色、字号、圆角、间距、文案 key 以 `design/` 为准（入口 `design/README.md`）。桌面端直接复用 `design/prototype/tokens.css` 与 `base.css`；Android 按同名 token 实现（例如 `bg.sidebar` → `bgSidebar`）。
- **连接**：客户端连接的地址是任意 `host:port` 或 `ws(s)://域名`，不要假设是 localhost。
- **服务端数据**：服务端使用 JSON 文件存储，不引入数据库（见 PLAN 5.2）。
- **开发环境**（这台 Mac 上已经装好）：
  - Rust stable，crates.io 走清华 tuna 镜像；`cargo search` 需要加 `--registry crates-io`。
  - Xcode 26.6（Tauri 打包 macOS App 需要）；Node.js（Tauri 前端构建）。
  - JDK 21、Android SDK 36、Gradle 9.8；Android 模拟器（AVD `macbot_api36`：Android 16 / API 36、arm64，已经在这台 Mac mini 上装好）。模拟器里访问 Mac 本机要用 `10.0.2.2`：mock 是 `10.0.2.2:7789`，正式服务是 `10.0.2.2:28789`（模拟器里没有 curl，测试连通性用 `adb shell toybox nc`）。
- **在 Mac mini 上能看到效果**：这台 Mac mini（M4，192.168.31.162）就是目标 Host。每个模块的成果都要能在这台机器上直接运行和查看（README 写清楚命令）。
- **mock 优先**：客户端先基于 `protocol/fixtures` 开发；服务端提供 `macbotd --mock` 之后，切换到 mock；最后再连接真实服务端。
- **测试**：
  - server：单元测试，加一个基于 mock provider 的场景测试。
  - 客户端：核心逻辑（重连、补发、状态合并）要有单元测试；界面至少能对着 mock 跑通。
- **不确定时**：按文档地图找权威文档；文档没有覆盖到的，按最简单可行的做法实现，并把决定记进 `DECISIONS.md`。

## 5. 阶段

阶段划分见 PLAN 第 6 章：S0 契约与骨架 → S1 单 Bot 闭环 → S2 主 Bot 与群协作 → S3 技能、仪表盘、记忆 → S4 浏览器、定时任务、推送 → S5 打磨与分发。各阶段的功能已经实现过一轮；当前重点是**按 `design/` 改造三端界面**，其中桌面端界面层从 GPUI 迁移到 Tauri。
