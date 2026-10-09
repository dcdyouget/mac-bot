# 各开发线的启动 prompt

> **用法**：为每条开发线各开一个 agent 会话，工作目录设为仓库根目录，把对应的 prompt 整段粘贴进去，四个会话同时启动。
> 每个 prompt 都覆盖 **S0–S5 全部阶段**，agent 会一直做到完成；只有在被别的开发线阻塞时才会停下来。
> prompt 只告诉 agent 去读哪些文档、按什么顺序做，规则本身都在文档里，这里不重复。

---

## 公共部分（四个 prompt 都包含）

```text
【必读，按顺序】
1. AGENTS.md：目录归属、分支、协议变更流程、打卡规则，必须遵守。
2. docs/PLAN.md 第 0 章：文档地图。每类规则只有一个权威来源：协议字段看 docs/PROTOCOL.md，界面和交互看 docs/DESIGN.md，服务端实现和开发计划看 docs/PLAN.md。
3. docs/PLAN.md 第 6 章：唯一的开发计划。你这条线在每个阶段要做的事，就是表格里你这一列。
4. docs/REFERENCES.md：参考项目和计划使用的库，可以用 gh api 阅读参考项目的源码。

【工作方式】
- 在自己的 git worktree 和分支上开发（命令见 AGENTS.md 第 2 节）。小步提交，频繁 rebase 到 origin/main 并合入 main。
- 目标是一次做完全部功能：按 S0 → S5 的顺序推进。每完成一个阶段，就在 COORDINATION.md 里打卡「<你的开发线> Sx 完成」，附上验证方式，然后直接开始下一个阶段，不要停下来等确认。
- 需要别的开发线配合，或者需要改协议时，写进 COORDINATION.md。在等待的时候，先做不依赖它的部分。
- 只有被阻塞、完全没有可以做的事时，才停下来汇报原因。
- 全部完成后汇报：每个阶段完成了什么、怎么验证的（命令、截图）、遗留问题。
```

---

## 1. server-mac

```text
你是 Mac Bot 项目 server-mac 开发线的负责人，负责服务端 macbotd 和协议的代码形式（protocol/rust、protocol/schema、protocol/fixtures）。其他三条开发线都依赖你的协议 crate 和 mock，所以 S0 是你最高的优先级。

<把上面「公共部分」粘贴在这里>

【重点阅读】PROTOCOL.md 全文；PLAN.md 第 5 章（重点是 5.2 存储、5.3 运行模型、5.4 路由、5.5 记忆与上下文、5.7 连接、5.8 浏览器、5.9 用量、5.10 工具与技能、5.11 运行轨迹）；DESIGN.md 第 3 章（完整场景，也就是 mock 场景和联调场景的剧本）。实现 durable 时参考 earendil-works/pi 的 packages/durable；实现工具时照搬 packages/coding-agent/src/core/tools。

【S0 的顺序（越早合入 main 越好）】
1. protocol/rust：macbot-protocol crate，实现 PROTOCOL.md 的全部类型（serde + schemars）；提供导出 JSON Schema 的命令；单独合入 main。
2. protocol/fixtures：每个对象、事件、块类型、TraceItem 类型至少一份示例；写 scenarios/login-feature.jsonl；加契约测试（所有 fixture 都能反序列化再序列化而不丢字段）。
3. server workspace 骨架（crate 划分见 PLAN 5.1）。
4. macbotd --mock：按 PROTOCOL 第 11 节实现全部方法和事件，包括运行轨迹和画面帧。完成后在 COORDINATION.md 写上启动命令。

【S1–S5】按 PLAN 第 6 章 server-mac 那一列推进。每个阶段的「联调验收」写成 scripts/e2e/<阶段>/ 下可以重复运行的场景脚本（通过 /api/v1/rpc 或 /ws 驱动，模型用 mock provider，另外提供一个真实模型的版本）。四条线都打卡某个阶段后，由你运行这个场景，把问题记到 COORDINATION.md。

【质量要求】cargo test、cargo clippy 通过；关键路径要有测试：存储崩溃恢复（截断最后一行、快照重建）、durable 恢复、send_msg 幂等、steer 送达状态、并发调度、trace 游标补发、画面的 ack 节流。
【真实模型】API Key 和 base_url 由用户提供；在拿到之前，用 mock provider 完成全部开发。
```

---

## 2. client-mac

```text
你是 Mac Bot 项目 client-mac 开发线的负责人，负责 macOS 桌面客户端（Rust + GPUI，只依赖 gpui-kit 0.7）。

<把上面「公共部分」粘贴在这里>

【重点阅读】DESIGN.md 全文（第 2 章设计语言，第 4 章桌面客户端的全部页面和线框图）；PROTOCOL.md 全文（你直接依赖 protocol/rust 里的类型）。gpui-kit 的用法：先运行 npx skills add longbridge/gpui-kit 安装它的 skills，或者用 gh api 阅读 longbridge/gpui-kit 的 README、examples、story。

【你负责的目录】clients/mac/（独立的 Cargo workspace，包含 macbot-client-core 和 macbot-desktop 两个 crate）。

【S0】按 PLAN 第 6 章 client-mac 那一列做。在 server 的 mock 合入 main 之前，用 protocol/fixtures 渲染界面；mock 可用后，连接 macbotd --mock --password dev。
【S1–S5】继续按 PLAN 第 6 章 client-mac 那一列推进。每个页面都以 DESIGN.md 对应章节的线框图和规则为准。

【质量要求】
- macbot-client-core 要有单元测试：重连、补发、状态合并、trace 游标合并、画面 ack。
- cargo test、cargo clippy 通过。
- 每个阶段在 COORDINATION.md 打卡时附上应用截图（用 screencapture 截取窗口）。
```

---

## 3. client-android

```text
你是 Mac Bot 项目 client-android 开发线的负责人，负责 Kotlin Multiplatform 工程的 core 层、Android 外壳，以及 AGENTS.md 里归你的 feature。client-ios 依赖你的 core 接口，所以 S0 里要**最先**把 core 接口合入 main。

<把上面「公共部分」粘贴在这里>

【重点阅读】DESIGN.md 第 2 章（设计语言）、第 5 章（移动端），以及第 4 章里和你的 feature 对应的页面；PROTOCOL.md 全文。

【你负责的目录】见 AGENTS.md 第 1 节：Gradle 配置、shared 的 core/、feature/{connect,chat,group,mainbot,approval,search,routines}、androidMain、androidApp、protocol/kotlin。

【S0 的顺序】
1. KMP 工程：最新稳定版 Kotlin + Compose Multiplatform 1.12；targets 为 android、iosArm64、iosSimulatorArm64；包名 bot.mac.mobile。
2. core 接口，单独合入 main：MacBotClient（连接、请求、事件 Flow）、ConnectionState、各个 Repository、CredentialStore（expect/actual）、Fixtures 数据源。接口说明写进 COORDINATION.md，方便 client-ios 使用。
3. core 实现：Ktor WebSocket（鉴权、hello/resume/bootstrap、last_seq 持久化、重连退避、心跳、请求按 id 对应、trace 游标合并、画面二进制帧解析和 ack）、状态存储、设计系统、导航（消息 / 工作台 / 我的）、i18n。
4. 协议模型：优先写 protocol/kotlin 里的生成脚本；如果生成带标签的联合类型代价太大，就手写模型，但必须有覆盖所有 fixture 的契约测试。
5. androidApp（前台服务保持连接、三类通知渠道）和 feature/connect。

【S1–S5】按 PLAN 第 6 章 client-android 那一列推进。

【质量要求】commonTest 和 androidUnitTest 通过；iOS 目标能编译；每个阶段在 COORDINATION.md 打卡时附上真机截图（小米 17，adb exec-out screencap -p）。
```

---

## 4. client-ios

```text
你是 Mac Bot 项目 client-ios 开发线的负责人，负责 iOS 外壳，以及 KMP shared 模块里归你的 feature。core 层（网络、协议模型、状态、设计系统）由 client-android 提供，你基于它的接口开发；接口合入之前，先用 fixtures 和临时的数据类做界面。

<把上面「公共部分」粘贴在这里>

【重点阅读】DESIGN.md 第 2 章、第 5 章，以及和你的 feature 对应的 4.6（运行轨迹）、4.7（Bot）、4.8（工作台）、4.9（仪表盘）、4.10（技能）、4.11（Agent Computer）、4.13（设置）；PROTOCOL.md 全文（重点是第 7 章运行轨迹、第 8 章画面、device.register）。

【你负责的目录】见 AGENTS.md 第 1 节：iosApp/、shared/src/iosMain/、feature/{trace,workbench,dashboard,skills,bots,settings,computer}。不要修改 core/ 和 Gradle 配置，需要时写进 COORDINATION.md。

【S0】iosApp 工程（ComposeUIViewController，能在 iOS 26.5 模拟器上启动）；iosMain：CredentialStore 的 Keychain 实现、前后台切换（进入后台时断开，回到前台时重连并补齐）、通知权限、APNs 注册（拿到 token 后用 device.register 上报）；feature/workbench 和 feature/dashboard 先用 fixtures 做界面（图表用 Compose Canvas 自己画）。
【S1–S5】按 PLAN 第 6 章 client-ios 那一列推进。

【质量要求】iosSimulatorArm64 能编译；commonTest 通过；每个阶段在 COORDINATION.md 打卡时附上模拟器截图（xcrun simctl io booted screenshot）。真机签名由用户负责。
```
