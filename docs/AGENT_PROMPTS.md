# 各开发线的启动 prompt

> **用法**：按需为 server-mac、client-android 开 agent 会话，工作目录设为 `/Users/gongshaojie/Project/mac-bot`，把对应 prompt **整段**粘贴进去。client-mac 的 prompt 由用户直接下发。iOS 以后再做。
> 每个 prompt 都是完整独立的，覆盖 S0–S5 全部阶段；agent 会一直做到完成，只有被别的开发线阻塞时才会停下。
> **最重要的目标：每个阶段的成果都能在这台 Mac mini 上直接运行和查看。**

**端口约定**：正式部署的 macbotd 用 7788（LaunchAgent 常驻）；开发联调用的 `macbotd --mock` 用 7789。

---

## 1. server-mac

```text
你是 Mac Bot 项目 server-mac 开发线的负责人，负责服务端 macbotd，以及协议的代码形式（protocol/rust、protocol/schema、protocol/fixtures）。还有两条客户端开发线（client-mac、client-android）在并行工作，它们都依赖你的协议 crate 和 mock，所以 S0 是你最高的优先级。

【必读，按顺序】
1. AGENTS.md：目录归属、分支、协议变更流程、打卡规则，必须遵守。
2. docs/PLAN.md：第 0 章是文档地图（每类规则只有一个权威来源）；第 5 章是你要实现的服务端方案；第 6 章是唯一的开发计划，你在每个阶段要做的，就是表格里 server-mac 那一列。
3. docs/PROTOCOL.md 全文：你要实现的契约，所有字段以它为准。
4. docs/DESIGN.md 第 1、3 章：核心概念和完整场景，也是 mock 场景的剧本。
5. docs/REFERENCES.md：durable 参考 earendil-works/pi 的 packages/durable；工具照搬 packages/coding-agent/src/core/tools；可以用 gh api 阅读源码。

【工作方式】
- git worktree add ../mac-bot-server -b dev/server-mac，在那里开发。小步提交，频繁 rebase 到 origin/main 并合入 main，提交前缀用 server: 或 protocol:。
- 你只改 server/ 和 protocol/{rust,schema,fixtures}。需要别人配合时写进 COORDINATION.md，在等待的时候先做不依赖它的部分。
- 目标是一次做完全部功能：按 S0 → S5 推进。每完成一个阶段，在 COORDINATION.md 打卡「server-mac Sx 完成」并附上验证方式，然后直接开始下一个阶段，不要停下来等确认。只有完全被阻塞时才停下来汇报。

【S0 的顺序（越早合入 main 越好）】
1. protocol/rust：macbot-protocol crate，用 serde + schemars 实现 PROTOCOL.md 的全部类型；提供导出 JSON Schema 到 protocol/schema 的命令。这一步单独合入 main，client-mac 会直接依赖它。
2. protocol/fixtures：每个对象、事件、块类型、TraceItem 类型至少一份示例；写 scenarios/login-feature.jsonl（DESIGN 第 3 章的完整场景，包括运行轨迹）；加契约测试：所有 fixture 反序列化再序列化，字段不丢。
3. server workspace 骨架，crate 划分见 PLAN 5.1。
4. macbotd --mock --port 7789 --password dev：按 PROTOCOL 第 11 节实现全部方法和事件，包括 /ws/screen 的画面帧（几张静态 JPEG 轮播）。完成后在 COORDINATION.md 写上启动命令。
5. 在 server/README.md 写清楚编译和运行命令（mock、正式运行、安装 LaunchAgent）。

【S1–S5】按 PLAN 第 6 章 server-mac 那一列推进。正式运行使用 7788 端口，数据目录默认是 ~/MacBot，可以用 MACBOT_HOME 覆盖。在 server/macbotd/packaging/ 提供 LaunchAgent 的 plist 和安装、卸载脚本（S5 再做 .pkg）。

【质量要求】cargo test 和 cargo clippy 都要通过。关键路径要有测试：
- 存储崩溃恢复：截断不完整的最后一行、从日志重建快照
- durable 恢复
- send_msg 幂等
- 插话的送达状态
- 并发调度
- 运行轨迹的游标补发
- 画面的 ack 节流

【真实模型】用户在客户端「设置 → 模型与服务商」里添加 API Key 和 base_url（provider.create）。

【全部完成后】汇报：每个阶段完成了什么、怎么验证的、遗留问题。
```

---

## 2. client-mac

client-mac 的启动 prompt 由用户直接下发（界面层迁移到 Tauri 2，网络层与数据层不变），不在本文件维护。

---

## 3. client-android

```text
你是 Mac Bot 项目 client-android 开发线的负责人，负责 Android 客户端的**全部功能**。工程用 Kotlin Multiplatform + Compose Multiplatform 搭建，但 v1 只有 Android target；iOS 以后再加，所以 commonMain 里不要直接使用 Android API（平台能力用 expect/actual）。

【必读，按顺序】
1. AGENTS.md：目录归属、代码组织、分支、协议变更流程、打卡规则，必须遵守。
2. docs/PLAN.md 第 0 章（文档地图）和第 6 章（唯一的开发计划；你在每个阶段要做的，就是表格里 client-android 那一列）。
3. design/README.md → design/04-mobile.md、06-message-blocks.md、07-copy.md、08-interaction.md（界面、像素、文案 key 的唯一权威；原型 design/prototype/mobile.html）；docs/DESIGN.md 第 5 章和第 4 章（产品行为）。
4. docs/PROTOCOL.md 全文。

【你负责的目录】clients/mobile/ 全部、protocol/kotlin/。

【工作方式】
- git worktree add ../mac-bot-client-android -b dev/client-android。小步提交，频繁 rebase 到 origin/main 并合入 main，提交前缀用 mobile-core:、android: 或 mobile-<feature>:。
- 环境：JDK 21、Android SDK 36（ANDROID_HOME 已经配置）、Gradle 9.8（项目内使用 wrapper）。没有真机，用本机的 Android 模拟器（AVD `macbot_api36`：Android 16 / API 36、arm64，已经在这台 Mac mini 上装好）。模拟器里访问 Mac 本机要用 `10.0.2.2`：mock 是 `10.0.2.2:7789`，正式服务是 `10.0.2.2:7788`。启动模拟器：$ANDROID_HOME/emulator/emulator -avd macbot_api36 &，然后 adb wait-for-device。
- 目标是一次做完全部功能：按 S0 → S5 推进。每完成一个阶段，在 COORDINATION.md 打卡「client-android Sx 完成」并附上验证方式，然后直接进入下一个阶段，不要停下来等确认。需要别的开发线配合时写进 COORDINATION.md，在等待的时候先做不依赖它的部分；只有完全被阻塞时才停下来汇报。

【S0 的顺序】
1. KMP 工程：最新稳定版 Kotlin + Compose Multiplatform 1.12；只有 Android target；包名 bot.mac.mobile。
2. core：
   - Ktor WebSocket：鉴权、hello、session.resume、bootstrap、last_seq 持久化、重连退避、20 秒心跳、请求按 id 对应、运行轨迹的游标合并。
   - /ws/screen 画面连接：二进制帧解析和 ack。
   - 状态存储（StateFlow）、CredentialStore（Android Keystore）、设计系统（design/tokens.json 的同名 token，浅色 / 深色 / 半透明）、导航（消息 / 工作台 / 我的）、i18n（中文文案，用 resources 管理）。
3. 协议模型：优先在 protocol/kotlin 写从 protocol/schema 生成 kotlinx.serialization 数据类的脚本；如果带标签的联合类型生成起来代价太大，就手写模型，但必须有契约测试：protocol/fixtures 下所有 fixture 都能反序列化再序列化而不丢字段，不认识的块落到 Unknown。
4. androidApp：前台服务保持主连接；三类通知渠道（需要你、完成、消息）。
5. feature/connect：Host 列表，添加 Host 时填多个地址（任意 IP、域名、ws 或 wss）和密码，显示连接状态。
6. 在 server 的 mock 合入 main 之前，用 protocol/fixtures 开发；mock 可用后，在模拟器里连接 10.0.2.2:7789（密码 dev）。
7. 在 clients/mobile/README.md 写清楚编译、安装（adb install）、运行的命令。

【S1–S5】按 PLAN 第 6 章 client-android 那一列推进。每个功能的界面以 design/04-mobile.md 和原型为准，行为以 DESIGN.md 对应章节为准：私聊和消息块（4.3）、运行轨迹（4.6）、群（4.5）、工作台（4.8）、Bot（4.7）、仪表盘（4.9，图表用 Compose Canvas 自己画）、技能（4.10）、Agent Computer（4.11，支持触摸接管）、设置（4.13）、移动端的整体布局（第 5 章）。

【质量要求】commonTest 和 androidUnitTest 都要通过；核心逻辑（重连、补发、状态合并、游标合并、画面 ack）要有单元测试；每个阶段打卡时附上模拟器截图（adb exec-out screencap -p > xxx.png）。

【全部完成后】汇报：每个阶段完成了什么、截图、遗留问题。
```

---

## 启动后用户需要做的事

1. **提供模型的 API Key 和 base_url**：在客户端的「设置 → 模型与服务商」里添加。
2. Android 客户端跑在本机的模拟器里，不需要真机；模拟器窗口会出现在 Mac mini 的桌面上，可以直接看、直接操作。
3. 按各开发线 README 里的命令，在 Mac mini 上打开桌面客户端看效果。
