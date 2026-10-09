# Mac Bot 参考清单（按组件）

> 列出每个组件参考了哪些开源项目、借鉴了什么、以什么方式使用。
> 「使用方式」分四种：
> - **依赖**：直接作为库或二进制使用
> - **sidecar**：作为独立进程一起分发
> - **借鉴设计**：只参考思路，用 Rust 或 Kotlin 重写，不复制代码
> - **产品参考**：闭源产品，只参考交互
>
> 许可证和 star 数核对于 2026-10-09。**PolyForm Noncommercial、LobeHub Community License 这类非 OSI 许可证的项目，一律只借鉴设计，不复用代码。**

## 总览

| 组件 | 主要参考 | 使用方式 |
|------|----------|----------|
| 产品形态与交互 | Grok Bot、nightly-labs/openbot、CopilotKit/OpenBot | 产品参考 / 借鉴设计 |
| 服务端运行时（持久化、恢复） | pi（pi-durable） | 借鉴设计 |
| 模型接入 | pi（pi-ai） | 借鉴设计 |
| 工具（文件、bash） | pi（coding-agent tools）、ripgrep 系列库 | 借鉴设计 + 依赖 |
| 浏览器 | vercel-labs/agent-browser | **sidecar** |
| 子代理 | pi（pi-durable 子任务）、OpenClaw（sessions_spawn） | 借鉴设计 |
| 技能 | Agent Skills 规范、pi 的技能实现、anthropics/skills | 遵循规范 + 借鉴设计 |
| 记忆 | hermes-agent、OpenClaw、nightly-labs/openbot | 借鉴设计 |
| 上下文管理与压缩 | hermes-agent、OpenClaw、nightly-labs/openbot、pi-durable | 借鉴设计 |
| 群协作与编排 | nightly-labs/openbot、LobeHub Agent Groups、AutoGen、CopilotKit/OpenBot | 借鉴设计 |
| 审批与安全 | Grok Bot、CopilotKit/OpenBot | 产品参考 / 借鉴设计 |
| 部署形态（Host + 客户端 + 节点） | OpenClaw | 借鉴设计 |
| 桌面客户端 | GPUI（Zed）、gpui-kit | **依赖** |
| 移动客户端 | Compose Multiplatform、Ktor | **依赖** |
| 仪表盘 | OpenAI 用量页、GitHub 贡献热力图 | 产品参考 |
| 原生桌面控制（远期可选） | trycua/cua（cua-driver）、openclaw/Peekaboo | sidecar（MCP） |

---

## 1. 产品形态与交互

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **Grok Bot**（xAI，[文档](https://docs.x.ai/grok-bot/overview)） | 闭源 | 整体形态：有名字的 Bot、群聊（2–6 个 Bot）、Bot 间消息、定时任务、技能；三栏布局、黑白气泡、A/B/C 提问卡片；Agent Computer 实时画面和接管；Allow once / Always allow / Deny 审批 | 产品参考 |
| **[nightly-labs/openbot](https://github.com/nightly-labs/openbot)** | PolyForm Noncommercial 1.0.0 | **形态最接近的开源实现**：本地优先的 AI 队友桌面应用；频道里每个 agent × 每个频道一条独立执行线程；单个负责人 + 显式委派；每个根请求最多 8 次自动委派；频道记忆；从 Grok Bot 导入 agent | 借鉴设计（**非商用许可，不复用代码**） |
| **[CopilotKit/OpenBot](https://github.com/CopilotKit/OpenBot)** | MIT | 「每个 Bot 一台电脑」；常驻角色在每个频道都生效；接管流程留下事件记录（help_requested / control_taken / control_released）；/bots 状态页、/memory 记忆页 | 借鉴设计 |

## 2. 服务端运行时：持久化、恢复、执行线程

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **[earendil-works/pi](https://github.com/earendil-works/pi)** `packages/durable` | MIT | 不可变的 Entry、原子 Commit、先落盘再展示；Task 按步保存检查点、崩溃后 `resume()`；**JSONL 存储后端**（我们的 JSON 文件存储沿用「追加日志是事实来源」的思路）；Submission 按 `requestId` 幂等；inbox 排队和 steer（插话）；compaction；每个会话一个持久的 sessionId（用于 prompt cache 亲和） | 借鉴设计（Rust 重写 `macbot-durable`） |

## 3. 模型接入

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **pi** `packages/ai`（pi-ai） | MIT | 按 API 类型抽象（OpenAI Completions / Responses、Anthropic Messages、Google）；模型目录（能力、上下文窗口、单价）；统一的流式事件（text、thinking、toolcall、usage、stop）；任意 OpenAI 兼容端点（Ollama、vLLM、LM Studio、DeepSeek、Qwen、MiMo…） | 借鉴设计（Rust 重写 `macbot-providers`） |

## 4. 工具（文件、bash）

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **pi** `packages/coding-agent/src/core/tools` | MIT | `read / write / edit / ls / find / grep / bash` 的参数定义；输出截断（2,000 行或 50 KB，超出部分落盘）；edit 要求替换原文唯一且互不重叠；file-mutation-queue 串行化同一文件的写入；取消时杀掉整个进程树 | 借鉴设计 |
| **[BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep)** 的库：`grep-searcher`、`grep-regex`、`ignore` | MIT / Unlicense | grep 的搜索实现，以及遵守 .gitignore 的文件遍历（find、grep、ls） | 依赖 |
| nightly-labs/openbot | PolyForm NC | 工具出错时返回失败结果让模型修正，而不是抛异常 | 借鉴设计 |

## 5. 浏览器

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **[vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser)** | Apache-2.0 | 纯 Rust 实现、直接走 CDP；三种模式（无头 / 无头 + 复用 Chrome profile / `--auto-connect` 连接本机 Chrome）；`--session` 隔离；`--restore` 保存登录状态；无障碍树快照和 `@e1` 引用；**WebSocket 推送画面并接收输入**（鼠标、键盘、触摸），支持 ack 节流；自带的 skills 文档 | **sidecar**（随 .pkg 分发，macbotd 调它的 CLI，并代理画面流） |
| [browser-use/browser-use](https://github.com/browser-use/browser-use) | MIT | 把 DOM 精简成可交互元素列表、给元素编号的思路 | 借鉴设计 |
| [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp)（扩展模式）、[hangwin/mcp-chrome](https://github.com/hangwin/mcp-chrome) | Apache-2.0 / MIT | 用 Chrome 扩展控制用户正在使用的浏览器 | 远期备选 |

## 6. 子代理

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **pi** `packages/durable`（Abort and Subagents、Child Tasks） | MIT | 子任务归属于父任务、可以恢复、取消时连带取消 | 借鉴设计 |
| **[openclaw/openclaw](https://github.com/openclaw/openclaw)**（`sessions_spawn`） | MIT | 子 agent 默认使用隔离的上下文，只把结果交回 | 借鉴设计 |

## 7. 技能

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **[agentskills/agentskills](https://github.com/agentskills/agentskills)**（Agent Skills 规范） | Apache-2.0 | `SKILL.md` 格式：frontmatter 里的 name 和 description；`scripts/`、`references/`、`assets/` 目录；按需加载 | **遵循规范**（与 pi、Claude Code 兼容） |
| **pi** `docs/skills.md`、`src/core/skills.ts` | MIT | 启动时只把名字、描述和路径放进系统提示；`/skill:name` 强制加载；`disable-model-invocation`；递归扫描目录；同名冲突保留先发现的那个 | 借鉴设计 |
| [anthropics/skills](https://github.com/anthropics/skills)、[badlogic/pi-skills](https://github.com/badlogic/pi-skills) | 见各技能 / MIT | 技能的写法示例；可以直接导入使用 | 示例和导入来源 |

## 8. 记忆

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **[NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent)** | MIT | 两份有字数上限、由 agent 自己维护的笔记（MEMORY.md / USER.md）→ 对应我们的 Bot 记忆和用户记忆；**冻结快照**注入（保证 prompt cache 稳定）；`session_search`（SQLite FTS5）；写满后迫使 agent 合并整理 | 借鉴设计 |
| **OpenClaw** | MIT | 压缩前的 **memory flush**（先把要点写进记忆）；只自动加载最近的笔记，其余靠检索 | 借鉴设计 |
| **nightly-labs/openbot** | PolyForm NC | 记忆写入**先暂存，这一轮成功结束后才提交**；频道级记忆（对应我们的项目记忆） | 借鉴设计 |
| [mem0ai/mem0](https://github.com/mem0ai/mem0) | Apache-2.0 | 可插拔的外部记忆服务（hermes 也支持） | 远期可选 |

## 9. 上下文管理与压缩

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **hermes-agent** | MIT | 压缩时关闭当前会话、生成带摘要的子会话（会话链）；保护头部和尾部；摘要长度约为被压缩内容的 20% | 借鉴设计（对应我们的「分段」） |
| **OpenClaw** | MIT | 压缩（摘要并持久化）和裁剪（只删旧的工具输出，不持久化）分开处理 | 借鉴设计 |
| **nightly-labs/openbot** | PolyForm NC | 用到模型窗口的 **80%** 时压缩；频道上下文按包组装（目的、分工、当前请求、引用的消息、决议、最近消息、带版本的摘要），完整历史靠检索取回 | 借鉴设计（对应我们的「群上下文包」） |
| **pi-durable** | MIT | compaction 和 reset entry | 借鉴设计 |

## 10. 群协作与编排

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **nightly-labs/openbot**（channels-and-messaging） | PolyForm NC | 每个频道一个负责人；**确定性路由优先**（指定的收件人、回复对应的任务、唯一进行中的任务），判断不了时才调用模型；任务的指派、转交和结果上报；每个根请求最多 8 次自动指派；一个 agent 在所有聊天中只跑一个工作轮次（**我们改成了按群并行**）；委派消息必须自包含 | 借鉴设计 |
| [lobehub/lobehub](https://github.com/lobehub/lobehub)（Agent Groups，RFC 130） | LobeHub Community License | 由 supervisor 决定下一个发言者，以及公开发言还是私信 | 借鉴设计 |
| [microsoft/autogen](https://github.com/microsoft/autogen)（GroupChat） | 代码 MIT，文档 CC-BY-4.0 | selector 模式（由模型挑选下一个发言者）和终止条件 | 借鉴设计 |
| **CopilotKit/OpenBot** | MIT | 「常驻角色在每个频道都生效，频道消息只是角色内的具体任务」，对应我们的 L1 Bot 身份 | 借鉴设计 |
| **OpenClaw**（multi-agent） | MIT | 每个 agent 有独立的 workspace 和会话存储，默认隔离，需要显式开启跨 agent 通信 | 借鉴设计 |

## 11. 审批与安全

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| Grok Bot | 闭源 | 允许一次 / 总是允许 / 拒绝；规则分「先问我」和「自动允许」两类，冲突时「先问我」优先；审批、密码请求不在群里出现 | 产品参考 |
| **CopilotKit/OpenBot** | MIT | 所有动作都经过同一个网关，先判断规则、写审计记录，然后才执行；**fail closed**（规则出错时拒绝）；有人接管时 Bot 的动作被拒绝而不是排队 | 借鉴设计（v1 只做审批，审计放到以后） |

## 12. 部署形态

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **OpenClaw** | MIT | 自托管 Gateway + 手机或电脑以「节点」身份接入 → 对应我们的 Host + 客户端，以及 v2 的 Computer Node | 借鉴设计 |

## 13. 桌面客户端

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **GPUI**（[zed-industries/zed](https://github.com/zed-industries/zed) `crates/gpui`） | gpui crate 为 Apache-2.0（Zed 应用本身是 GPL/AGPL） | GPU 加速的 Rust UI 框架 | **依赖**（通过 gpui-kit 间接引入） |
| **[longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)**（原名 gpui-component） | 代码 Apache-2.0 | 75+ 组件：Markdown、不等高虚拟列表、Dock 和可拖动面板、表格、图表、表单、浮层；Lucide 图标；给 AI 编程助手用的 skills | **依赖** |

## 14. 移动客户端

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **[JetBrains/compose-multiplatform](https://github.com/JetBrains/compose-multiplatform)** | Apache-2.0 | 一套 Kotlin UI 代码同时出 Android 和 iOS 两端 | **依赖** |
| **[ktorio/ktor](https://github.com/ktorio/ktor)** | Apache-2.0 | 跨平台 WebSocket 和 HTTP 客户端 | **依赖** |

## 15. 仪表盘

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| OpenAI 平台用量页 | 闭源 | 指标卡、每日用量曲线、按模型分组、导出 | 产品参考 |
| GitHub 贡献热力图 | 闭源 | 53 周 × 7 天的日历热力图，分 4 档颜色 | 产品参考 |

## 16. 原生桌面控制（远期可选）

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| **[trycua/cua](https://github.com/trycua/cua)**（cua-driver） | MIT | 后台控制原生 Mac App，不抢鼠标、不抢焦点（基于 SkyLight 私有 API 和 AX）；MCP over stdio | sidecar（MCP） |
| **[openclaw/Peekaboo](https://github.com/openclaw/Peekaboo)** | MIT | macOS 截图和 GUI 自动化 CLI/MCP | sidecar（MCP），备选 |

> trycua/cua 里的 lume（虚拟机）不采用，因为已经决定不用虚拟机。

## 17. 协议（可选参考）

| 项目 | 许可证 | 借鉴什么 | 使用方式 |
|------|--------|----------|----------|
| [ag-ui-protocol/ag-ui](https://github.com/ag-ui-protocol/ag-ui) | MIT | agent 与界面之间的事件类型命名（消息流、工具调用、状态变化），供设计 `/ws` 事件时参考 | 借鉴设计 |

---

## 18. Rust 依赖库（计划使用）

| 用途 | crate | 版本（crates.io） |
|------|-------|-------------------|
| 桌面 UI | `gpui-kit` | 0.7 |
| 异步运行时 | `tokio` | 1.x |
| HTTP / WebSocket 服务 | `axum`、`tokio-tungstenite` | 0.8 / 0.30 |
| HTTP 客户端（调用模型 API） | `reqwest`、`eventsource-stream`（解析 SSE） | 0.13 / 0.2 |
| JSON 存储 | `serde_json`；`fs4`（`data/.lock` 文件锁）；`tempfile`（先写临时文件再 rename 原子替换） | 1.x / 1.1 / 3.x |
| 检索（扫描 JSONL） | `grep-searcher`、`grep-regex`（与 grep 工具共用） | 0.1 |
| JSON Schema（工具参数） | `schemars`、`serde`、`serde_json` | 1.2 |
| grep / find / ls | `grep-searcher`、`grep-regex`、`ignore` | 0.1 / 0.4 |
| 编辑 diff 渲染 | `similar` | 3.x |
| 网页转 Markdown | `dom_smoothie`（正文提取）、`htmd`（HTML 转 Markdown） | 0.18 / 0.5 |
| 定时任务 | `croner`、`chrono-tz` | 4.x / 0.10 |
| 技能目录监听 | `notify` | 8.x 稳定版 |
| 密码哈希 | `argon2` | 0.6 |
| 钥匙串（API Key） | `security-framework` | 3.x |
| 开机自启（LaunchAgent / LoginItem） | `objc2-service-management`（SMAppService） | 0.3 |
| iOS 推送（APNs） | `a2` | 0.10 |
| 嵌入 `/admin` 页面 | `rust-embed` | 8.x |
| ID | `uuid`（v7） | 1.x |
| 日志 | `tracing` | 0.1 |
