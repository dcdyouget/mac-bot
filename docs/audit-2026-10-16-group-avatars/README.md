# 群头像与 Main 成员一致性复核（2026-10-16）

本轮接续 `7a08bad`，起始工作区干净。修复提交为 `a98aa53`（成员一致性）和 `60bc06e`（双端头像）。范围是用户指出的少成员群头像和“只有普通 Bot、没有 Main”的群；不重复宣称上一轮系统审查覆盖。

## 行为与缺陷

| 编号 | 影响与要求 | 根因、修复与证据 |
|---|---|---|
| GA-01 / P2 | 少成员头像挤在网格中，不符合侧向展开要求 | 桌面 `GroupAvatar.tsx` / `styles.css` 与 Android `GroupScreen.kt` 共用头像入口改为：1 个完整头像，2–3 个侧向堆叠，Main 左前；4+ 保留网格与 +N。桌面实际组件矩阵截图覆盖 1–6 人、5 种尺寸。 |
| GA-02 / P2 | mock 官网改版群缺少 Main，项目成员和聊天成员不一致，误导产品判断 | `generate-fixtures.rs` 原样例负责人为 bot_code、项目成员为空、聊天只有 bot_code。现为 Main + code，登录项目成员也与聊天一致；重新生成 fixtures 与 Android 资源，新增唯一成员、Main、worker 数量、负责人和 project/chat 一致性回归。 |
| GA-03 / P2 | mock 允许移除 Main，成员修改后聊天仍显示旧成员 | `mock.rs` 对齐真实创建与成员管理约束，自动加入 Main、禁止移除、限制 1–6 个初始 worker，并同步 chat 与事件。49 项契约测试及独立 mock RPC 通过。 |

产品依据：`docs/DESIGN.md` 的项目群定义和建群流程。Main + 1 个普通 Bot 是合法群；用户手动创建和 Main 创建都自动加入 Main。生产实现允许后续移除全部普通 Bot、仅留下 Main，本次没有新增禁止该状态的规则。只有一个普通 Bot 且没有 Main 不符合约定。

已静态复核生产 `Orchestrator::create_project`、Main 建群工具、恢复规范化和移除保护，以及两端成员展示；未发现正常建群遗漏 Main 的路径。本次修复的是确认存在的 mock/fixture 问题，不是已复现的真实 Host 创建缺陷。具体用户所见群尚无群名/Host 来源，不能确认创建者；fixture 的 `created_by:user` 只是样例元数据。

## 实际界面追加发现

GA-04 / P2：Android 独立 mock 截图发现消息列表群行仍显示普通 Bot 默认单头像，并未使用已修改的群头像组件。`App.kt` 的项目聊天行已接入 `GroupAvatar`，`ChatScreen.kt` 的项目聊天顶栏也补齐入口。新增成员选择 helper 以当前 chat 成员集合为准，project/announcement 只补充匹配 ID 的元数据，避免旧公告复活已移除成员；不伪造缺失 Main。修复提交 `aadc2d8`。作者 Android main 编译通过，独立成员回归 5/5、增量 APK 构建通过；最终 mock 界面列表/群顶栏/群信息已截图，列表 2–5 人布局经主 agent 审图通过。首张矩阵图使用旧 mock，重连时缓存官网仍为单头像；换为独立验证的 e1296de… binary 后，连接完成 ready，官网列表/聊天/群信息均为 Main + code。这是旧 binary/cache 的证据混用问题，已分开归档。

桌面另做只读入口检查：活动/已完成群行、群顶栏、关注项目、消息 ProjectCard 均调用 GroupAvatar；工作台分组仅标题与任务数。逐个成员头像保留个人语义。

## 验证与限制

- 桌面作者构建及 lint 通过。独立验证使用真实 React 组件、CSS、成员排序 helper，最终图见 [头像矩阵](group-avatar-final.png)；[范围报告](group-avatar-review.json)。Tauri 原生窗口截图受 `cgWindowNotFound` 阻塞，因此该图不是原生应用或真实 RPC 页面验收。
- Android 作者 `GroupMemberLogicTest` 3/3、`FixtureContractTest` 4/4，`compileAndroidMain` 通过；独立最终构建与成员回归 5/5 通过；隔离 emulator-5570、capacitytest 包通过 adb reverse 连接 mock。
- 协议独立 fixtures 测试 10/10，完整协议测试、Kotlin 生成检查和生成器测试 6 项通过；生成输出一致。见 [协议复核](protocol-review.md)、[定向日志](fixtures-test.log)。未修改协议字段、公共类型或 schema。
- 独立 mock 契约测试 49/49、隔离 daemon 构建及真实 RPC 通过：单 worker 创建得到 Main + worker；移除 Main 返回 forbidden 且前后状态相同；官网项目和聊天成员一致。见 [报告](mock-project-members-qa.md)、[RPC 结果](mock-project-members-rpc.json)。这证明 mock 行为，不代替生产调度测试。
- 所有新运行使用隔离数据、配置、端口及测试包；没有改实际 Host 数据或部署到正式客户端。证据内 `/tmp` 路径保留执行上下文，不保证长期存在，本目录保存关键结果。

## 交付状态

源码修复及证据已提交。初次 `git fetch origin` 报 `Recv failure: Operation timed out`，首次限时推送也超时；最终重试成功，将 `7a08bad..c320cde` 推送至 `origin/main`。没有强制推送。

## Android 图像范围

- [最终新 mock 列表](android/09-list-current.png)、[官网聊天顶栏](android/10-website-chat-header.png)、[官网群信息](android/12-website-info.png)：修复后的 binary，证明官网包含 Main + code，且列表不再误用单 Bot 头像。
- [最终客户端 2–5 人列表矩阵](android/01-list.png)、[五人聊天](android/02-five-chat.png)、[五人信息](android/04-five-info.png)：最终客户端配旧 mock 的 QA 创建群，证明布局；列表处于重连缓存状态，不能据此判断官网 fixture 已修复。
- [三人聊天](android/06-three-member-chat-header-pre-m08.png)、[仅 Main 聊天](android/09-one-main-chat-header-pre-m08.png)：列表入口补修前的群内组件，组件几何与最终版一致；仅 Main 通过移除 worker 得到，并非创建无 worker 群。
- 本轮 Followup 入口未完成独立实际点击验收；共享组件已有截图，不能替代该页面操作证据。
