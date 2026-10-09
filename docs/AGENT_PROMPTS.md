# 各开发线的启动 prompt

> 用法：为每条开发线各开一个 agent 会话，工作目录设为仓库根目录（`/Users/gongshaojie/Project/mac-bot`），把下面对应的 prompt 整段粘贴进去。四个会话可以同时启动。
> 每个 prompt 只覆盖 **S0（契约与骨架）+ S1（单 Bot 跑通）**。完成后 agent 会停下来汇报，确认后再下发 S2 的 prompt。

---

## 1. server-mac

```text
你是 Mac Bot 项目 server-mac 开发线的负责人，负责服务端 macbotd 和协议契约。四条开发线（server-mac、client-mac、client-android、client-ios）正在并行开发，你的进度决定了其他三条线能不能尽早联调。

【先读】
1. AGENTS.md（目录归属、分支、协议变更流程，必须遵守）
2. docs/PROTOCOL.md（你负责实现的契约）
3. docs/PLAN.md：第 5 章（重点是 5.2 JSON 存储、5.3 运行模型、5.5 上下文、5.7 协议、5.10 工具与技能、5.11 运行轨迹）以及 7.1 的阶段表
4. docs/REFERENCES.md（参考项目和计划使用的 crate）。工具的实现照搬 earendil-works/pi 的 packages/coding-agent/src/core/tools；durable 部分参考 pi 的 packages/durable，可以用 gh api 查看源码。

【你负责的目录】server/、protocol/rust/、protocol/schema/、protocol/fixtures/。不要修改其他目录；需要别人配合的事写进 COORDINATION.md。

【工作方式】
- 使用 git worktree：git worktree add ../mac-bot-server -b dev/server-mac，在那里开发。
- 小步提交，每完成一个能运行的部分就 rebase 到 origin/main 并合入 main、推送。提交前缀用 server: 或 protocol:。
- server/ 是独立的 Cargo workspace，只支持 macOS。crates.io 走 tuna 镜像，cargo search 需要加 --registry crates-io。

【S0：契约与骨架（最高优先级，越早合入 main 越好）】
1. protocol/rust：建 macbot-protocol crate，用 serde + schemars 定义 PROTOCOL.md 里的全部内容：帧（req/res/evt）、所有 method 的参数和返回值、所有事件、核心对象（Bot、Chat、Message 及全部消息块、Project、Announcement、Assignment 等）、TraceItem 和画面通道的消息。要求：
   - snake_case 字段名；带标签的枚举用 "type" 字段区分。
   - 未知字段可以容忍，未知的消息块能落到 Unknown 变体（客户端用 fallback_text 显示）。
   - 提供一个导出命令，把 JSON Schema 写到 protocol/schema/。
   这一步要尽快单独合入 main，因为 client-mac 会直接依赖这个 crate。
2. protocol/fixtures：每种对象、事件、消息块、TraceItem 至少一份示例 JSON；scenarios/login-feature.jsonl 按时间顺序回放「登录功能」完整场景（见 DESIGN.md 第 3 章）：主 Bot 私聊 → 建群 → 开场 → 产品收到并干活（附带 trace 事件和子代理）→ 完成报告 → 编码收到 → 用户插话 → … → 待验收卡片。另外加一个测试：每个 fixture 都能反序列化再序列化，不丢字段。
3. server/ workspace 骨架：按 server/README.md 建好各 crate（先只有模块划分和文档注释）。
4. macbotd --mock：axum 监听 7788 端口，密码用 --password 参数。需要实现：
   - /ws：hello、session.resume（含 replay/reset）、ping；chat.list / bot.list / project.list / workbench.get 返回 fixtures；chat.send 时回放场景；trace.subscribe / trace.history 返回场景里的轨迹。
   - /ws/screen：循环推送几张静态 JPEG。
   - /api/v1/health。
   让三个客户端可以不依赖真实模型进行联调。
完成 S0 后，在 COORDINATION.md 记一条「mock 可用」，并写明启动命令。

【S1：单 Bot 跑通（真实服务端）】
- macbot-store：按 PLAN 5.2 实现 JSON 文件存储。JSONL 只追加、写完 fsync，启动时截掉不完整的最后一行；快照先写临时文件再 rename；data/.lock 文件锁；内存索引；JSONL 的 seq→字节偏移稀疏索引；持久事件日志用于断线补发。数据目录默认 ~/MacBot，可以用 MACBOT_HOME 覆盖。
- macbot-durable：执行线程、Entry、run、断点恢复（重启后继续 working 状态的 run）、插话注入点。
- macbot-providers：OpenAI 兼容（chat/completions）和 Anthropic Messages 两种 API 类型，流式 SSE、工具调用、usage；provider 和模型配置通过 provider.* / model.list 接口管理，API Key 存 macOS 钥匙串（security-framework）；另外实现一个用于测试的 mock provider。
- macbot-tools：按 PLAN 5.10.1 / 5.10.2 实现工具框架，以及 read、write、edit、ls、find、grep、bash、bash_job。截断规则是 2,000 行或 50 KB，完整输出写入 runs/。出错时返回 is_error 结果，不要抛异常。审批钩子先做成默认放行，但要留好接口。
- macbot-skills：扫描 ~/MacBot/skills 下的 SKILL.md（遵循 Agent Skills 规范），实现 skill 工具；系统提示里只放技能的名字、描述和路径。
- 一个默认的普通 Bot（例如「助手」）：在私聊里可以对话、调用上面的工具、流式回复（message.delta → message.updated）。注意：私聊是对话模式，回复可以流式；**群里是工作模式，Bot 只通过 send_msg 工具发完整消息，不流式**（PLAN 5.3）。send_msg 和群协作属于 S2，但 durable run 的设计（落盘、恢复、steer 注入、挂起等待）要在 S1 就按 5.3.4 和 5.3.6 预留好。
- 运行轨迹：按 PLAN 5.11 和 PROTOCOL 第 6 节，把 TraceItem 写入 entries.jsonl，并实现 trace.subscribe / trace.history，按 (run_id, tseq) 排序。只给订阅了的客户端推送；没有订阅时只落盘。
- 用量：每次模型调用往 usage/raw 写一行。
- 鉴权：auth.json 存 argon2 哈希，每个连接都要求 Bearer 密码；没设置密码时返回 setup_required。
- /admin：首次设置密码页（设置之前只允许本机访问）+ 状态页，单个 HTML 文件嵌入二进制（rust-embed）。
- CLI：macbot status、macbot passwd、macbot logs、macbot restart（通过本地 Unix socket 与守护进程通信）。
- server/macbotd/packaging：LaunchAgent plist 和一个开发用的安装脚本（打包 .pkg 留到以后）。

【S1 验收】
- 用真实模型（API Key 和 base_url 由用户提供；没有的话先用 mock provider 跑通，并在汇报里说明）完成以下流程：在私聊里让 Bot 新建一个文件并运行 ls，客户端能实时看到流式回复和运行轨迹。
- kill -9 杀掉 macbotd 再重启，未完成的 run 能接着跑，客户端用 last_seq 补齐期间的事件。
- cargo test 和 cargo clippy 都通过。

【完成后】停下来汇报：完成了什么、没完成什么、对协议做了哪些变更（以及是否已经同步到 PROTOCOL.md 和 fixtures）、需要其他开发线配合的事。
```

---

## 2. client-mac

```text
你是 Mac Bot 项目 client-mac 开发线的负责人，负责 macOS 桌面客户端（Rust + GPUI）。四条开发线正在并行开发；服务端会先提供协议 crate 和 macbotd --mock。

【先读】
1. AGENTS.md（目录归属、分支、协议变更流程，必须遵守）
2. docs/DESIGN.md：重点是第 2 章设计语言、第 4 章桌面客户端，包括线框图
3. docs/PROTOCOL.md（连接、帧、事件、运行轨迹、画面通道）
4. clients/mac/README.md
5. gpui-kit 的用法：可以先运行 npx skills add longbridge/gpui-kit 安装它给 AI 编程助手准备的 skills；也可以用 gh api 阅读 longbridge/gpui-kit 的 README、examples 和 story。

【你负责的目录】只有 clients/mac/。协议类型通过 path 依赖使用 ../../protocol/rust（由 server-mac 维护，不要修改；需要改协议时写进 COORDINATION.md）。

【工作方式】
- 使用 git worktree：git worktree add ../mac-bot-client-mac -b dev/client-mac。
- 小步提交，频繁 rebase 到 origin/main 并合入 main。提交前缀用 client-mac:。
- clients/mac 是独立的 Cargo workspace，只依赖 gpui-kit 0.7（不要单独依赖 gpui）。编译需要 Metal 工具链，这台机器上已经装好。

【S0：骨架】
1. 在 workspace 里建两个 crate：
   - macbot-client-core：无界面的协议客户端。用 tokio-tungstenite 建立 /ws 连接，带 Bearer 密码；处理 hello、session.resume 和 last_seq（last_seq 要持久化）；请求和响应按 id 对应；重连退避（1s、2s、4s … 最长 30s，带抖动）；每 20 秒发 ping；向界面暴露事件流；本地状态存储（会话、消息、Bot、群、任务）。需要单元测试。
   - macbot-desktop：GPUI 应用。
2. 设计 token：按 DESIGN.md 第 2 章实现，支持浅色和深色。
3. 主窗口三栏布局（DESIGN 4.1、4.2）：侧栏包括 Host 切换、主 Bot、群、Bot、工作台、仪表盘、技能、设置；中间是会话区；右侧是可以收起的面板，用页面栈管理。
4. 连接页：添加 Host 时填名称、多个地址（任意 IP 或域名加端口，也支持 ws:// 和 wss://）和密码，密码存 macOS 钥匙串；显示连接状态（在线、离线、重连中）。
5. fixtures 模式：设置环境变量后，直接读取 protocol/fixtures 渲染界面，不需要连接服务端。server-mac 的 mock 合入 main 之后，改为连接 macbotd --mock --password dev。

【S1：单 Bot 跑通】
- 私聊界面：消息列表用虚拟滚动；Markdown 渲染；流式显示 message.delta（只有私聊有流式；群消息都是完整消息）；用户气泡和 Bot 气泡样式按 DESIGN 第 2 章；输入框支持 Shift+Enter 换行；按 Bot 发送和停止。
- 消息块渲染：text、system、memory_note、approval（含三个按钮）、question、file、image；不认识的块用 fallback_text 显示。
- 运行轨迹（DESIGN 4.6）：右侧面板的简版，加上全屏轨迹视图。只在用户打开时订阅，关闭时退订。先 trace.history 分页，再 trace.subscribe 接收实时推送，按 (run_id, tseq) 合并去重；支持「跟随最新」；模型请求、工具调用、子代理可以折叠和展开；工具输出被截断时通过 HTTP 获取全文。任务结束后同一个页面用于回放。
- 设置：模型与服务商页（provider.list / save / test、model.list）。
- 错误和断线状态：按 DESIGN 4.14。

【S1 验收】对着真实服务端（或者在服务端 S1 完成之前对着 mock）：添加 Host → 和 Bot 私聊 → 看到流式回复 → 打开运行轨迹，能实时看到模型输出和工具调用 → 任务结束后能回放。断网重连后消息不丢、不重复。cargo test 和 cargo clippy 都通过。

【完成后】停下来汇报：完成项、未完成项、截图（用 screencapture 截取应用窗口）、对协议的诉求、需要其他开发线配合的事。
```

---

## 3. client-android

```text
你是 Mac Bot 项目 client-android 开发线的负责人，负责 Kotlin Multiplatform 工程的 core 层和 Android 端。client-ios 与你共用 clients/mobile/shared，并且依赖你尽早提供 core 的接口。

【先读】
1. AGENTS.md（尤其是第 1 节 commonMain 的目录归属表，必须遵守）
2. docs/DESIGN.md：第 2 章设计语言、第 5 章移动端，以及第 4 章里对应页面的线框图
3. docs/PROTOCOL.md
4. clients/mobile/README.md、protocol/README.md

【你负责的目录】
- clients/mobile 的 Gradle 配置、shared/src/commonMain/kotlin/bot/mac/mobile/core/、feature/{connect,chat,group,mainbot,approval}、shared/src/androidMain/、androidApp/、protocol/kotlin/。
- 不要修改 client-ios 负责的部分：feature/{trace,workbench,dashboard,skills,memory,settings,computer}、iosMain、iosApp。

【工作方式】
- 使用 git worktree：git worktree add ../mac-bot-client-android -b dev/client-android。
- 小步提交，频繁合入 main。提交前缀用 mobile-core:、android: 或 mobile-<feature>:。
- 环境：JDK 21、Android SDK 36（ANDROID_HOME 已经配置）、Gradle 9.8（项目内使用 wrapper）。调试真机是小米 17（adb），没有装模拟器。

【S0：骨架（第一要务：尽早把 core 的接口合入 main，client-ios 要依赖它）】
1. 建 KMP 工程：使用最新稳定版 Kotlin 和 Compose Multiplatform 1.12；targets 为 android、iosArm64、iosSimulatorArm64；包名 bot.mac.mobile；shared 模块输出 iOS framework，供 iosApp 使用。
2. core/api：先定义接口并合入 main。包括 MacBotClient（连接、请求、事件 Flow）、ConnectionState、各个 Repository（chats、messages、bots、projects、assignments、trace、usage）、CredentialStore（expect/actual，Android 用 Keystore 实现，iOS 的实现由 client-ios 负责）、Fixtures 数据源。
3. core 实现：
   - Ktor WebSocket 客户端：Bearer 密码、hello、session.resume、持久化 last_seq、请求按 id 对应、重连退避、20 秒心跳。
   - 状态存储：StateFlow。
   - 设计系统：DESIGN 第 2 章的 token，支持浅色和深色。
   - 导航框架：底部三个 Tab，消息 / 工作台 / 我的。
   - i18n：使用 Compose Multiplatform 的 resources，中文文案。
4. 协议模型：优先在 protocol/kotlin 里写脚本，从 protocol/schema 生成 kotlinx.serialization 数据类。如果带标签的联合类型生成起来代价太大，就手写模型，但必须加契约测试：protocol/fixtures 下每个 JSON 都要能反序列化再序列化而不丢字段。不认识的消息块落到 Unknown。
5. androidApp：应用外壳；前台服务保持主连接（App 在后台时也能收到消息）；通知渠道（需要你、完成、消息三类）。
6. feature/connect：Host 列表；添加 Host 时填多个地址（任意 IP、域名、ws 或 wss）和密码；显示连接状态。
7. 先用 fixtures 开发；server-mac 的 macbotd --mock 合入后，改为连接 mock。手机需要访问 Mac 的局域网 IP。

【S1：单 Bot 跑通】
- feature/chat：会话列表（主 Bot 固定在最上面，然后是「群」「Bot」两组，带注意力状态）；私聊界面（流式消息、Markdown、长按菜单；只有私聊有流式）；用户消息下方的送达状态（delivery 字段，见 PROTOCOL 3.3）；消息块渲染：text、system、memory_note、approval、question、file、image，不认识的块用 fallback_text；输入框。
- feature/approval：私聊里的审批卡片，以及通知里直接点「允许一次」或「拒绝」。

【S1 验收】在小米 17 上：添加 Host（填 Mac 的局域网 IP）→ 和 Bot 私聊 → 看到流式回复；App 切到后台再回来，消息不丢；断网重连后不重复。commonTest 和 androidUnitTest 都通过；iOS 目标能编译（./gradlew :shared:linkDebugFrameworkIosSimulatorArm64）。

【完成后】停下来汇报：完成项、未完成项、真机截图（adb exec-out screencap -p）、core 接口的说明（给 client-ios 看）、对协议的诉求、需要其他开发线配合的事。
```

---

## 4. client-ios

```text
你是 Mac Bot 项目 client-ios 开发线的负责人，负责 iOS 外壳，以及 KMP shared 模块里分给你的功能。client-android 负责 core 层（网络、协议模型、状态、设计系统）；你基于它提供的接口开发，在接口合入之前先用 fixtures 和临时的假数据。

【先读】
1. AGENTS.md（尤其是第 1 节 commonMain 的目录归属表，必须遵守）
2. docs/DESIGN.md：第 2 章设计语言、第 5 章移动端；第 4.6 节工作详情与运行轨迹、4.8 工作台、4.9 仪表盘、4.10 技能、4.12 记忆、4.13 设置的线框图和说明
3. docs/PROTOCOL.md（尤其是第 6 节运行轨迹、第 7 节画面通道、device.register）
4. clients/mobile/README.md

【你负责的目录】
- clients/mobile/iosApp/、shared/src/iosMain/。
- shared/src/commonMain/kotlin/bot/mac/mobile/feature/ 下的 trace、workbench、dashboard、skills、memory、settings、computer。
- 不要修改 core/ 和 client-android 负责的其他 feature，也不要改 Gradle 配置；需要改动时写进 COORDINATION.md。

【工作方式】
- 使用 git worktree：git worktree add ../mac-bot-client-ios -b dev/client-ios。
- 小步提交，频繁合入 main。提交前缀用 ios: 或 mobile-<feature>:。
- 环境：Xcode 26.6，iOS 26.5 模拟器（iPhone 17 Pro 等）。真机签名由用户负责，你只需要保证模拟器上能跑。
- 如果 client-android 还没把 KMP 工程合入 main，先用 git fetch 关注进度；这段时间可以先建 iosApp 的 Xcode 工程，并在 feature 目录下用临时数据类做界面。

【S0：骨架】
1. iosApp：Xcode 工程，引入 shared framework，入口是 Compose Multiplatform 的 ComposeUIViewController；能在 iOS 26.5 模拟器上启动。
2. iosMain：
   - CredentialStore 的 actual 实现（Keychain）。
   - App 生命周期：进入后台时断开主连接；回到前台时调用 core 的重连，用 last_seq 补齐。
   - 本地通知权限。
   - APNs 注册：拿到 device token 后，通过协议的 device.register 上报（服务端支持之前，先把 token 保存下来）。
3. feature/workbench 和 feature/dashboard 的界面先用 fixtures 做：
   - 工作台：按 Bot、群、状态切换；等你验收和等你审批置顶。
   - 仪表盘：指标卡；日历热力图（53 周 × 7 天，4 档颜色，可以横向滑动）；星期 × 小时热力图；每日折线图，维度可以在模型、Bot、项目之间切换，最多 6 条线，其余合并为「其他」；明细列表。图表用 Compose Canvas 自己画。

【S1：单 Bot 跑通】
- feature/trace（DESIGN 4.6 和第 5 章）：工作详情全屏页和历史任务列表。
  - 只在用户打开时订阅，离开页面时退订。先 trace.history 分页，再 trace.subscribe 接收实时推送，按 (run_id, tseq) 合并去重。
  - 支持「跟随最新」；模型请求、工具调用、子代理可以折叠和展开；手机上默认折叠思考和工具输出；工具输出被截断时通过 HTTP 获取全文。
  - 任务结束后同一个页面用于回放。
  - 提供 [停止] [@ 它] 按钮。
- feature/settings：外观、语言、通知设置，以及「我的」页的框架（Host 管理的入口复用 client-android 的 feature/connect）。

【S1 验收】在 iOS 模拟器上：连接服务端（mock 或真实）→ 打开某个任务的运行轨迹，能实时滚动显示模型输出和工具调用 → 结束后能回放；App 切到后台再回来，能自动重连并补齐消息；工作台和仪表盘能用 fixtures 正常显示。iosSimulatorArm64 能编译，commonTest 通过。

【完成后】停下来汇报：完成项、未完成项、模拟器截图（xcrun simctl io booted screenshot）、对 core 接口和协议的诉求、需要其他开发线配合的事。
```

---

## 启动顺序建议

1. **同时启动四个会话。** server-mac 的 S0 第 1 步（协议 crate）和 client-android 的 S0 第 2 步（core 接口）是其他开发线的前置条件，它们的 prompt 里已经标成最高优先级。
2. client-mac 和 client-ios 在前置条件就绪之前，先做不依赖它们的部分：工程骨架、设计 token、布局，以及基于 fixtures 的界面。
3. 四条线都完成 S1 并汇报之后，在 main 上联调一次（PLAN 7.1 的「汇合点」），再下发 S2 的 prompt。
