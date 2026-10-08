# Mac Bot 规划草案 v0.1

> 状态：规划中，尚未开始编码。本文档用于讨论和定稿需求、架构与里程碑。

## 1. 一句话定位

**自托管版 Grok Bot**：Bot 运行在你自己 24 小时常开的 Apple Silicon Mac（如 Mac mini）上，而不是厂商的云电脑；通过 macOS / Windows 桌面客户端和 Android 客户端，用 `IP(域名):端口` 直连，下达任务、看它操作、接管或审批。

## 2. 需求清单（来自原始描述）

| # | 需求 | 备注 |
|---|------|------|
| R1 | 服务端部署在 M 系列 Mac 上 | 常驻、开机自启 |
| R2 | 服务端开固定端口，客户端填 `host:port` 连接 | 局域网直连；公网走 frp / Tailscale 等 |
| R3 | 桌面控制能力，**锁屏状态下也能完成任务** | 例：登录 X → 刷帖 → 总结 |
| R4 | 支持自定义模型 | 参考 pi-ai（provider 层） |
| R5 | 模型请求可恢复、可持久 | 参考 pi-durable |
| R6 | 记忆系统，记住用户说过的话 | 参考 hermes-agent |
| R7 | 存储用 SQLite | |
| R8 | Rust + GPUI | 追求性能 |
| R9 | 客户端：macOS、Windows、Android（小米 17） | |
| R10 | 一键安装 | |
| R11 | 交互参考 Grok Bot | |

## 3. 参考对象

### 3.1 Grok Bot 的产品形态（借鉴交互）
- 2026-08 上线，做法是给每个用户一台**常驻云电脑**（Linux，带浏览器、文件系统、终端），名下所有 Bot 共用这台机器，所以 Cookie 和登录状态在 Bot 之间是互通的。
- 用户创建**有名字的 Bot**，一个 Bot 专做一件事；可以**示范一次**，之后按定时或手动重复执行。
- 客户端关掉以后任务照样跑。
- 不支持自托管 —— 这正是 Mac Bot 要补的空缺。

### 3.2 可借鉴的开源项目

| 项目 | 语言 | 借鉴什么 |
|------|------|----------|
| [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) | Python | 记忆系统：`MEMORY.md` / `USER.md` 两份有字数上限、由 Agent 自己维护的笔记，会话开始时以**冻结快照**注入（照顾 prompt cache）；会话全文检索（FTS）；可插拔的外部记忆 provider |
| [earendil-works/pi](https://github.com/earendil-works/pi) `packages/ai` | TS | Provider 层：按 API 类型（OpenAI Completions/Responses、Anthropic Messages、Google…）抽象；模型目录带能力和价格；鉴权解析；跨 provider 中途切换；任意 OpenAI 兼容端点 |
| [earendil-works/pi](https://github.com/earendil-works/pi) `packages/durable` | TS | 持久化 harness：transcript 由不可变 Entry 组成；所有修改走原子 Commit，**先落盘再展示**；Task 是逐步 checkpoint 的状态机，崩溃后 `resume()` 能接着跑；Submission 按 `requestId` 幂等；每个会话一个持久 sessionId（用于 prompt cache 亲和）；compaction |
| [openclaw](https://docs.openclaw.ai) | TS | **产品形态最接近**：自托管 Gateway + 手机/电脑以「节点」身份接入；WebSocket 配对，配对需要审批；敏感能力走白名单 |
| [trycua/cua](https://github.com/trycua/cua) + lume | Swift/Python | 在 Apple Silicon 上用 Virtualization.framework 跑 macOS/Linux 虚拟机做 Agent 沙箱；guest 里跑 computer-server 接收截图/点击指令 |
| browser-use | Python | 浏览器 Agent 的 DOM 精简、可交互元素标注 |
| UI-TARS-desktop | TS | GUI grounding 模型（截图→坐标）的接入方式 |
| [zed-industries/zed](https://github.com/zed-industries/zed) / longbridge/gpui-component | Rust | GPUI 写法和现成组件 |

> 注：pi 和 hermes 分别是 TS 和 Python，没法直接当依赖用。我们**在 Rust 里重写它们的设计**，不绑定实现。

## 4. 原始思路中尚未考虑到的问题（重点）

### 4.1 ⚠️ 锁屏下的桌面控制（最大技术风险）
macOS 锁屏后，解锁界面有一组特殊的安全限制：往当前会话注入 `CGEvent` 和用 ScreenCaptureKit 截图都可能被拦，显示器睡眠后截图也会失败。**直接控制宿主机桌面，在锁屏时基本不可靠。** 我们给出三层方案：

| 层 | 方式 | 锁屏可用 | 适用 |
|----|------|---------|------|
| L1 | **浏览器自动化（CDP）**：Mac Bot 自己管理一个 Chrome，使用持久 profile，headful 模式并关闭后台节流，或使用 new-headless | ✅ 不依赖屏幕 | 80% 的需求：X、网页、表单、信息汇总 |
| L2 | **VM 沙箱**：用 Virtualization.framework 跑 macOS 或 Linux guest，guest 里放一个 agent 负责截图和输入 | ✅ VM 的显示和宿主锁屏无关 | 需要完整桌面或原生 App 的任务；同时起到隔离作用 |
| L3 | 宿主机桌面直控（ScreenCaptureKit + CGEvent + AX） | ❌ 仅限未锁屏时 | 可选能力，默认关闭 |

**建议：MVP 只做 L1，L2 放第二阶段。** L2 的代价：macOS guest 要占 4–8 GB 内存和 30 GB 以上磁盘，Apple 还限制每台宿主最多同时跑 2 个 macOS VM。需要先确认你 Mac mini 的内存。

### 4.2 断电或重启后无人登录
服务需要截图或操作 GUI 时，必须以 **LaunchAgent**（用户 GUI 会话）身份运行，不能用 LaunchDaemon。但开着 FileVault 时，重启后会停在预启动登录界面，没有任何用户会话，Bot 就整个离线了。
- 可选方案：关闭 FileVault 并开启自动登录（安全性下降）；或计划内重启时使用 `fdesetup authrestart`；在系统设置里打开「断电后自动开机」。
- L1 和 L2 本身不需要 GUI 会话也能跑，可以拆出一个 LaunchDaemon 版本的核心服务，这样重启后无人登录也能工作（需要验证）。

### 4.3 暴露端口 = 暴露一台能远程执行任意操作的电脑
这个 Bot 有执行 shell、操控浏览器的能力。端口一旦被扫到又没有鉴权，就等于把电脑交出去。
- 强制 TLS（首次启动自签证书，客户端按指纹 pin，类似 SSH 的 TOFU）。
- **设备配对**：服务端显示二维码或配对码，客户端扫码后拿到设备令牌；设备可以吊销。
- 公网访问优先推荐 Tailscale / ZeroTier；用 frp 时推荐 `stcp/xtcp` 模式，不要直接 `tcp` 映射到公网。
- 审计日志：谁、在什么时候、让 Bot 做了什么。

### 4.4 人在回路（Grok Bot 也有）
- **实时画面**：手机上能看到 Bot 正在操作的浏览器或 VM 画面（先推 JPEG 帧，以后再上 WebRTC）。
- **接管**：遇到验证码、2FA、短信验证码时，暂停任务并推送通知，让你在手机上接管操作。
- **审批**：发帖、私信、付款、删除这类动作，执行前需要你确认（策略可配置）。

### 4.5 账号风控
X 对自动化很敏感。具体措施：使用真实 Chrome 和持久 profile；**第一次登录由你通过实时画面手动完成**（Bot 不保存密码）；操作节奏做拟人化限速；只读任务（刷帖、总结）优先。另外要提醒：这类自动化可能违反平台服务条款，存在账号风险。

### 4.6 自定义模型的能力差异
- 桌面或视觉操作需要模型同时支持**视觉输入和工具调用**。纯文本模型只能走 DOM 或 AX 树文本模式。
- 可选「双模型」：主模型负责规划，grounding 模型（如 UI-TARS 类）负责把截图里的目标映射成点击坐标。
- 截图很耗 token，需要做缩放、裁剪和历史截图淘汰，并在界面上展示花费。

### 4.7 一台桌面只有一个鼠标
多个 Bot 或任务同时要操作桌面时，需要资源锁和排队。浏览器标签页可以并行，VM 画面同一时间只能给一个任务用。

### 4.8 Android 端：GPUI 不支持 Android
GPUI 官方只支持 macOS、Windows、Linux。社区的 `gpui-mobile` 还很早期，不可用于生产。备选：
- **方案 A（推荐）**：Rust 共享核心（协议、加密、会话同步）通过 **UniFFI** 导出给 Kotlin，UI 用 Jetpack Compose 原生写。性能好，Android 体验地道。
- 方案 B：Android 用 Dioxus 或 Tauri Mobile（Rust，但不是 GPUI）。
- 方案 C：服务端顺带提供 Web/PWA 页面作为兜底。成本低，但不满足「高性能原生」的要求。

### 4.9 小米 / HyperOS 推送与保活
国行小米 17 没有 GMS，FCM 用不了。任务完成或需要审批的通知需要：前台服务长连接（引导用户关闭电池优化、允许自启动），或者接入小米推送（需要开发者账号，服务端要能访问小米推送）。MVP 先做前台服务长连接。

### 4.10 一键安装的隐性成本
- macOS：.app 需要用 Developer ID 签名并公证（每年 $99），否则 Gatekeeper 会拦截。**屏幕录制和辅助功能权限必须由用户手动授权**，没法自动化，只能用首次启动向导引导。macOS 15 以后屏幕录制权限还会定期要求重新确认（如果只用 L1 / L2，就基本不需要这两个权限）。
- Windows：没有代码签名证书会被 SmartScreen 警告。
- Android：APK 侧载即可。
- 自动更新，以及客户端和服务端的**协议版本协商**。

### 4.11 其他
- **密钥存储**：API Key 放 macOS 钥匙串，不要明文写进 SQLite。
- **防睡眠**：用 `IOPMAssertion` 阻止系统睡眠，但允许显示器睡眠。
- **记忆可见、可编辑**：用户能在界面里查看、修改、删除 Bot 记住的内容（隐私）。
- **定时任务（Routines）**：「每天早上 9 点总结 X 时间线」是这类产品的核心场景，需要调度器。
- **任务回放**：保存每一步截图和动作的时间线，方便排查。
- **备份**：一个 SQLite 文件加浏览器 profile 目录。
- **单用户假设**：v1 只支持一个主人多台设备，不做多租户。

## 5. 总体架构

```
┌──────────────── Mac mini（服务端 macbot-server）────────────────┐
│                                                                  │
│  Gateway (axum + WebSocket/TLS, 固定端口如 7788)                  │
│     │  配对/鉴权 · 协议版本协商 · 事件流推送                        │
│     ▼                                                            │
│  Bot Runtime ── Durable Engine (pi-durable 思路)                  │
│     │            Conversation / Entry / Commit / Task / Resume    │
│     ├── Providers (pi-ai 思路)  OpenAI兼容/Responses/Anthropic/…   │
│     ├── Memory (hermes 思路)    USER.md/MEMORY.md + FTS5 检索      │
│     ├── Scheduler               定时 Routines                     │
│     └── Tools                                                    │
│          ├ browser  (CDP, chromiumoxide)      ← L1              │
│          ├ vm       (Virtualization.framework) ← L2              │
│          ├ desktop  (SCK + CGEvent + AX)       ← L3 可选          │
│          ├ shell / fs                                            │
│          └ mcp      (外部工具扩展)                                 │
│  Store: SQLite (rusqlite + FTS5, 可选 sqlite-vec)                 │
│  Secrets: macOS Keychain                                         │
└──────────────────────────────────────────────────────────────────┘
          ▲ wss://host:port               ▲
          │ (LAN / Tailscale / frp)       │
 ┌────────┴─────────┐           ┌─────────┴──────────┐
 │ 桌面客户端 GPUI   │           │ Android 客户端      │
 │ macOS / Windows  │           │ Compose + Rust core │
 │ (mac 上兼任服务端 │           │ (UniFFI)            │
 │  的设置/菜单栏)   │           │                    │
 └──────────────────┘           └────────────────────┘
```

### 5.1 仓库结构（Cargo workspace）

```
mac-bot/
├── crates/
│   ├── macbot-protocol     # 客户端/服务端消息定义（serde），版本号
│   ├── macbot-store        # SQLite、迁移、FTS5
│   ├── macbot-durable      # Conversation/Entry/Commit/Task/Submission 引擎
│   ├── macbot-providers    # LLM provider 抽象与实现、模型目录
│   ├── macbot-memory       # 记忆：curated notes + 会话检索 + 工具
│   ├── macbot-tools        # browser / shell / fs / mcp
│   ├── macbot-vm           # Virtualization.framework 封装（objc2）
│   ├── macbot-server       # 守护进程 bin
│   └── macbot-client-core  # 协议客户端、重连、本地缓存（桌面与 Android 共用）
├── apps/
│   ├── desktop/            # GPUI 客户端（mac / windows）
│   └── android/            # Kotlin + Compose，通过 UniFFI 调用 client-core
└── docs/
```

### 5.2 关键设计

**Durable 引擎（Rust 版 pi-durable）**
- 表结构大致为：`conversations`、`entries`（追加写、不可变，带 kind 和 JSON 内容）、`documents`（per-conversation 的 typed state：agent、usage、inbox…）、`tasks`（状态机 + checkpoint JSON）、`submissions`（`request_id` 唯一）。
- 每次 Commit 是一个 SQLite 事务；先提交，再通过 broadcast 推给客户端。
- 启动时扫描未完成的 task 并 resume。工具调用如果有副作用，要标记为「不可安全重放」，resume 时改为询问用户。
- Compaction：transcript 过长时生成摘要 Entry，再插入一个 reset 点。

**Provider 层（Rust 版 pi-ai）**
- 按 `ApiKind` 抽象：`OpenAiCompletions`、`OpenAiResponses`、`AnthropicMessages`、`GoogleGenerative`。前两种就能覆盖 Ollama、vLLM、LM Studio、DeepSeek、Qwen、MiMo 等绝大多数自定义模型。
- `Model { provider, id, api, base_url, caps{vision, tools, reasoning}, context_window, cost }`，用户在界面里新增「自定义 provider」时只需要填 base_url、key 和模型 id。
- 统一的流式事件：`TextDelta`、`ThinkingDelta`、`ToolCallDelta`、`Usage`、`Stop`。

**记忆（hermes 思路）**
1. **Curated memory**：`user_profile`（相当于 USER.md）和 `agent_notes`（相当于 MEMORY.md），各有字数上限，由 Agent 通过 `memory` 工具做增、改、删；会话开始时以快照形式注入 system prompt。
2. **Episodic recall**：所有消息进 FTS5 索引，Agent 可以调用 `session_search` 工具检索「我之前说过什么」。以后可以加 sqlite-vec 做语义检索。
3. **Procedural memory**：「示范一次」得到的 Routine 存成 skill（步骤加注意事项），可以复用。
4. 客户端提供「记忆」页，可以查看、编辑、删除。

**协议**
- WebSocket over TLS，JSON 帧，`{v, id, type, payload}`；请求/响应和服务端推送事件并存。
- 断线重连靠 `last_seq` 补齐事件，因为 durable 引擎天然有序。
- 实时画面用独立的二进制帧通道（JPEG），按需订阅。

## 6. 客户端界面（参考 Grok Bot）

- **左栏**：Bot 列表（名字、头像、状态：空闲/运行/等待审批）。
- **中栏**：对话 + 任务进度（工具调用卡片、截图缩略图）。
- **右栏或抽屉**：Bot 的电脑，即实时画面（可以接管）。
- **其他页**：Routines（定时任务）、Memory（记忆）、Models（模型与 provider）、Devices（已配对设备）、Settings。
- Android 端是同样的信息架构，换成移动端布局；首次使用「扫码配对」。

## 7. 里程碑

| 阶段 | 内容 | 验收 |
|------|------|------|
| **M0 Spike** | ① 锁屏下 CDP 控制 Chrome 是否稳定 ② 宿主锁屏时 VM 是否能截图和输入 ③ GPUI 客户端骨架 ④ UniFFI → Android Hello | 写一份技术验证报告，定下 L1/L2 路线 |
| **M1 核心** | store、durable、providers（OpenAI 兼容 + Anthropic）、gateway（TLS + 配对）、GPUI 聊天 | 桌面客户端能连上服务端对话，服务端重启后会话可以恢复 |
| **M2 浏览器** | browser 工具、实时画面、接管、审批 | 端到端跑通「打开 X → 刷时间线 → 总结」（锁屏状态下） |
| **M3 记忆** | curated memory、FTS 检索、记忆管理页 | 跨会话记住用户偏好 |
| **M4 Android** | Compose 客户端、前台服务长连接、通知 | 小米 17 上完成 M2 的场景 |
| **M5 扩展** | Routines 调度、VM 沙箱（L2）、MCP、Windows 客户端打包 | 定时任务；完整桌面任务 |
| **M6 分发** | 签名、公证、dmg、自动更新、安装向导 | 双击安装即可使用 |

## 8. 待确认问题

1. Mac mini 的**内存和磁盘**是多少？这决定 L2（VM）是否可行，以及 guest 用 macOS 还是 Linux。
2. 「桌面控制」主要是为了网页任务（X 之类），还是也要操作原生 Mac App？
3. 能否接受 **Android 端用 Kotlin/Compose 写 UI**（Rust 共享核心）？还是坚持全 Rust？
4. 是否有 Apple Developer 账号（签名和公证）？
5. 公网访问打算用 frp 还是 Tailscale？
6. FileVault 是否开启？能否接受自动登录？
7. 主要使用哪些模型（是否支持视觉）？
8. 服务端的设置界面，是放在 Mac 上的 GPUI 菜单栏 App 里，还是只通过远程客户端配置？
