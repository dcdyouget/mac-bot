# Mac Bot 集成进度与使用说明

最新结论（2026-10-10）：**S0–S3通过；S4真实执行链路通过，接管卡状态缺陷修复中；S5 fresh未执行**。当前server `5b244ea`、桌面 `ba0e8f1`、Android `04b0c3d` 同签名Release。旧等待恢复、新请求落盘、同run交还完成及工作台问题关闭已实际通过；错误审批引用已修复并两端实看，桌面已回答问题按钮/新消息显示仍在修复；正式数据正常重启测得103秒，929MB操作日志整体解析热点正在修复。历史版本与失败证据保留，不覆盖本段。

S2两个原项目已由用户授权结束5个历史等待分支并用原UUID确认done，双端完成状态已实看；其余7pending未改变，见 [S2确认记录](S2/production-9c20fc4-confirm-done.json)。

S4用户已允许本次Chrome远程调试连接。旧sidecar 0.38.2的2秒握手超时已通过升级0.39.0修复，原Bot真实打开已登录X首页；Android显示实际画面、接管、只读滑动、交还，原run恢复并done，driver为bot→user→bot。新增X页已关闭，原登录页保留，Bot恢复headless。见 [本次X验收](S4/production-be64802-authorized-x.json)。交还后原消息block仍pending属于服务端状态投影缺陷，正在修复，未据执行成功提前记整阶段通过。

Android长聊天初始定位竞态已修复；新版首次进入显示最新消息，上滑后保持位置，142条群历史已实际到达seq1，加载结束保持首条锚点，见 [定位回归](S4/production-04b0c3d-chat-scroll.json)。S5保留历次pkg审计，但最终server尚未冻结，接管修复后需重建匹配包；fresh未开始。安装需要用户管理员认证，管理页新密码须按CUA凭据变更规则由用户亲自输入、确认和提交。

| 当前阶段 | 结论与主要证据 |
| --- | --- |
| S0 | 已通过，两端 mock 会话列表验收见 `S0/` |
| S1 | 已通过：[真实双端流式、轨迹、同run kill-9恢复](S1/production-aa2c99a-joint-recovery.json) |
| S2 | 已通过：[两原项目与精确旧分支确认闭环](S2/production-9c20fc4-confirm-done.json) |
| S3 | 已通过：[真实双端数据、技能、记忆与搜索结论](S3/production-6eaa91a-stage-conclusion.json) |
| S4 | 收尾中：[证据与剩余显示缺陷](S4/stage-conclusion.json) |
| S5 | 未开始：[可逆安装计划](S5/fresh-install-plan.md)、[写RPC审计](S5/fresh-rpc-journal-audit.md)、[客户端配置隔离](S5/fresh-client-config-plan.md) |

本文面向在 Mac mini（Apple M4，局域网地址 `192.168.31.162`）上查看 Mac Bot 效果的人。服务端运行在这台 Mac 上，Android 客户端运行在本机的 `macbot_api36` 模拟器里。v1 不包含 iOS。

## 历史现场（ee7111a，最新状态以上方结论为准）

当前接手续办：Mac 已解锁。正式服务 `ee7111a`（PID/source 见 [部署校验](S4/production-attach-no-copy-upgrade-after.json)），桌面 `cd81c23`，Android APK `cac5b12d`。修复了浏览器动作/参数和审批恢复、运行互斥、RPC事件扫描、搜索暴露内部对象、全局轨迹序号缺口及实时轨迹/退订路由；attach不复制Chrome profile的修复通过18浏览器与192网关测试。桌面最新修复长消息在右侧Bot信息面板下被裁切，构建与严格Clippy通过，原X摘要在面板打开及窄窗口实际换行通过（[证据](S5/desktop-cd81c23-message-width.json)）。仅更新桌面，服务端与Android保持。服务端严格clippy为此前fe360a2验证；此前协议、34桌面核心和Kotlin生成检查保持。

实时序号缺口已在正式现场闭环：旧桌面缓存停在4952；server-only更新后，无手动刷新自动显示review/Tester完成。标准库WebSocket观察143条重放游标、8条实时游标无缺号，未订阅连接无轨迹正文（[证据](S1/production-07a3245-live-cursor-after.json)）。旧pending、checkpoint、run request、浏览器session哈希保持。

两群现均有原Tester的真实浏览器执行证据及报告：parallel的TEST-browser.md已纠正草稿错误、保留三项未测/证据不足，主Bot已汇总并置review；first的index.html已实测登录、刷新、退出、375px布局，TEST-index-browser.md已提交，原login.html失败保留且两份demo均未手改。两群主Bot均已汇总并置review，主私聊两张待验收卡各含页面和报告链接，未点击确认完成。first公告有同路径重复登记，保留原记录。任务done和报告不代替真实双端全阶段通过。

S1新增真实专项：安全checkpoint后kill‑9，原run自动恢复读取原文件并完成，Bash未重放（[证据](S1/production-07a3245-owned-recovery.json)）。修复history固定live=false后，桌面实际观察正文逐步增长及轨迹追加到run.end，重开可回放；标准库观察41个正文片段、52个轨迹片段（[证据](S1/production-7f0a5dd-real-stream.json)）。退订释放及8路容量实测通过；原生“收起”按钮未可靠验证，返回键已能关闭。Android未参加本轮专项，不能计S1整阶段通过。

桌面持续画面、低清切换及本地fixture真实键鼠输入/释放已局部通过。用户确认后临时开启Chrome远程调试，原Bot真实读取登录后的X首页，桌面显示连续视频；首次摘要编造链接，失败原文保留，同Bot重新读取DOM后更正，三条链接逐一匹配实际href，桌面更正摘要已实看（[完整证据](S4/production-ee7111a-x-acceptance-summary.json)）。验收后远程调试关闭、9222不再监听、Bot恢复headless，原用户X标签保留。此为桌面X只读专项通过，不计手机接管或S4整阶段。`ee7111a`将profile复制限定于headless_profile；18浏览器、192网关测试通过。Android仍无CUA控制面，最新只读adb截图超时；adb替代操作及保留数据重启待用户明确授权。

最新server pkg `fe360a2`、release DMG `71fddde`已审计，未fresh安装。S0仍唯一整阶段通过，S1–S4完整双端验收及S5 fresh未完成。详见[接手续办记录](S2/2026-10-10-takeover-closeout.md)。

S3补充桌面原生核对：同一固定时间范围，仪表盘9,636,069 Token、718请求、费用未知、1完成任务与正式summary/breakdown一致（[证据](S3/production-ee7111a-desktop-dashboard.json)）；技能列表4项、3启用1停用与API相符，但名称被省略，未重验增删改（[证据](S3/production-ee7111a-desktop-skills.json)）。Android同范围原生核对仍待完成，不计S3整阶段通过。

### 历史记录（7481d8e 及此前，以下 PID/锁屏状态不是当前现场）

7481d8e 历史进展（下述锁屏/PID为当时现场）：正式服务已固定部署 `7481d8e`；取消任务遗留审批自动过期，原 checkpoint/工具参数/轨迹不变、没有重放，14个有效待审批项保持（[证据](S2/production-7481d8e-cancelled-approval-after.json)）。此前 `d799dda` 的真实专项保持：旧无效 memory 审批已过期，原参数/run 保持，经同 run 参数错误后生成合法新调用；仅该新 project/add 审批经精确范围和 checkpoint 核对后单次批准。新 @Tester 用户消息真实走 `chat.send`，在原 assignment/run 内完成 queued→delivered→read、applied/trace，未另建任务且未自动放行审批（[证据](S2/production-d799dda-busy-tester-steer.json)）。parallel 的 `login.html` 已在隔离真实浏览器验证正确/错误密码、刷新保持、退出和390px布局（[证据与截图](S2/production-d799dda-parallel-demo.json)）；first 续办已真实生成 login.html；浏览器检查发现登录成功后表单仍同时可见，已反馈原项目 Coder 修复，失败证据保留；完整双群/Tester/验收闭环未完成。Android 私聊工具回放、仪表盘、技能、通知打开及本地页触摸均是局部通过；桌面原生验收仍被 Mac 锁屏阻断。S0 是唯一整阶段通过，S5 全新安装尚未开始。

正式服务 `7481d8e4b988e866bb3feade18db7e603b6aba5e` 已从固定干净 archive 部署到 `7788`（当前 PID `86386`，可执行文件为 `~/Applications/MacBotServer.app/Contents/MacOS/macbotd`），[部署证据](S1/server-7481d8e-deployed.json)确认 LaunchAgent/监听 PID 一致、安装 source、公开健康检查和部署认证探测；固定 SHA 部署退出 0。MiniMax-M2.5 沿用本机 file backend。桌面当前仍是 `deploy.sh` 的 debug 开发安装 `7c5e480f`（PID `66995`，窗口 `58491`），Release DMG 仅完成候选校验；[历史现场截图](S4/20261009-235101-desktop.png)只记录 23:51 的“正在连接”页，不能代表当前 094 状态或 formal 已连接证据。`lsof` 的 7788 ESTABLISHED 只证明 TCP；CUA fullpath、Finder、Activity Monitor 均 `cgWindowNotFound`，native 连续帧和输入仍未验收。[3 秒样本](S5/desktop-dmg-7c5e480-deployment.json)不能据 AppKit idle/connect timer 判定冻结。Android 当前通知修复 Release 来源为 `cac5b12d68fbe409b41e763231c28f2e7baaff37`（发布记录 `6e1d7f8`），APK `2,111,890` bytes、SHA-256 `c1b2e2fb…2872f2a`，[保留数据覆盖安装记录](S4/android-notification-cac5b12-installed.json)核对与旧版本同签名、安装base.apk哈希一致，新PID `20943`，连接目标为正式 `10.0.2.2:7788`；owner 证据是局部 Production 检查，不等于集成线全场景 PASS。持续部署保留 UI 验证 hold，避免中断现场。

本轮新增：原忙碌 Coder 接收修复插话并在同 run 内完成项目 `index.html` 编辑；[独立实浏览器复验](S2/production-d799dda-first-index-demo.json)确认错误输入提示、登录后仅成功区、刷新保持、退出及375px布局通过。原 `login.html` 未被修改，其表单同时显示的失败记录保留；这是新 `index.html` 的产物专项，不替代 Tester 实测、原文件修复或完整 S2。已取消任务残留 pending 审批的 P1 已在正式 `7481d8e` 升级复验通过，未点击该审批。Mac 锁屏仍为 true，桌面 UI 和 S5 fresh 尚未恢复验收。

### 历史阶段结论（7481d8e 及此前，不代表当前状态）

| 阶段 | 目标 | 联调状态 | 截图目录 |
|---|---|---|---|
| S0 | 桌面端和 Android 模拟器连接 mock 并看到会话列表 | 通过：两端连接 mock 并看到会话列表 | `docs/progress/S0/` |
| S1 | 真实服务端上的单 Bot 对话、工具和轨迹恢复 | 未通过：094 的 Android 既有 AE 私聊、最终文本和 write/read/bash 轨迹回放已局部通过（[私聊截图](S1/production-7189395-cac5b12-android-private-replay.png)、[轨迹截图](S1/production-7189395-cac5b12-android-trace-replay.png)、[工具回放截图](S1/production-094b7c5-cac5b12-android-trace-tools-replay.png)），但不是新 live 流式或 kill9 恢复；严格 one-call 检查仍因模型额外请求 `bash_job` 保留失败，桌面、实时流式和完整联合验收待验 | `docs/progress/S1/` |
| S2 | 主 Bot、群协作、插话和待验收 | 未通过：d799 旧无效 memory 同 run 纠错、新 @Tester queued→delivered→read 且无新任务已局部通过；parallel demo 实浏览器检查通过，Tester 原 run done，TEST.md 4220 bytes 含插话 marker/明确未实测，Android 原群[文字与 marker 可见](S2/production-d799dda-cac5b12-android-tester-report.json)；错误日期保留，不计实际 Tester 测试通过；first 续办 canonical→Main assignment/run→assign精确绑定并生成 demo，但登录后表单未隐藏，待原项目 Coder 修复，双群完整协作/验收未闭环。旧插话 siblings 与未知审批保持，不重发或套旧授权。 | `docs/progress/S2/` |
| S3 | 技能、仪表盘、搜索和记忆 | 未通过：094 Android 原生仪表盘与同范围 RPC 摘要/明细通过，桌面同窗口待解锁；Android [技能 CRUD](S3/production-d97e48f-android-skill-crud.json) 已局部验收 PASS；d43 [正文更新保持全局/per-Bot停用范围](S3/production-d43a2bb-skill-scope.json) API PASS；[只读用量聚合](S3/production-d97e48f-usage-readonly.json) PASS，为 `399,070` tokens、`99` requests，费用尚未定价；仍不等于双端 S3 全通过 | `docs/progress/S3/` |
| S4 | 浏览器画面、接管、定时任务和通知 | 未通过（局部通过）：718 的 [Android 原生本地页触摸提交/交还](S4/production-7189395-android-native-input.json)通过；907 的定时任务 API 按时完成，旧 [50 条容量阻断](S4/production-907a05c-android-notification-quota-metadata.json)已修并覆盖更新，新 [schedule通知](S4/production-7189395-cac5b12-android-notification-receipt.json)在系统与原生画面出现；[通知打开记录](S4/production-7189395-cac5b12-notification-opened.json)确认点击进入正确私聊，手动滚动看到 marker，但未测试审批动作。更新后实际数量未满50，仍不计满额 native 验收。桌面输入、X 登录及完整双端操作待验，Mac 锁屏仍阻断桌面 UI。 | `docs/progress/S4/` |
| S5 | pkg 全新安装、两端连接和完整场景 | 未验收：7481d8e pkg 已完成[来源/载荷审计](S5/server-pkg-7481d8e-verified.json)，未实际 installer/fresh；桌面 Release DMG 仍仅包核验通过。 | `docs/progress/S5/` |

每个阶段只有在三条开发线都在 `COORDINATION.md` 打卡，并且集成线完成真实联调、保存两端截图后，才会标记为“通过”。

Android [通知权限/渠道只读元数据](S4/production-d43a2bb-android-notification-owner-metadata.json)确认系统通知权限已授予、渠道未删除、前台连接服务运行；新窗口的应用总开关、Bot/chat 过滤和目标 ledger 亦已核正常；ledger 记录不代表系统实际展示。正式 ping 已修复并完成独立 60 秒 transport 检查；[Android端口观测](S1/production-907a05c-android-heartbeat-observation.json)在采样期间保持原formal连接。旧[04:22定时任务](S4/production-907a05c-scheduled-routine.json)已 done 但通知仍 NOT_OBSERVED；[原生通知总开关](S4/production-907a05c-android-notifications-enabled.png)启用，Bot通知启用、聊天未静音。Android owner 已确认[系统日志](S4/production-907a05c-android-notification-quota-metadata.json)达到50条 App 通知上限并拒绝新增；容量管理/去重修复 `cac5b12` 已同签名覆盖安装：[新04:52定时任务](S4/production-7189395-notification-cac5b12-scheduled-routine.json)按时 done，[通知 receipt](S4/production-7189395-cac5b12-android-notification-receipt.json)和[原生通知栏](S4/production-7189395-cac5b12-native-notifications.png)出现唯一 marker 及完成通知。[打开记录](S4/production-7189395-cac5b12-notification-opened.json)确认 native tap 进入预期 bot/chat，手动滚动后看到 marker；审批动作未测。更新前50、启动后1，原因未证且未手动 cancel；本轮计消息通知打开局部 PASS，不计满50原生容量或完整 S4。旧窗口 NOT_OBSERVED 不回填因果。718 的[Android原生输入](S4/production-7189395-android-native-input.json)仍是本地 fixture 专项；不计 X 登录、桌面输入或整阶段。

Android [交回记录](S5/android-80d8a42-handoff.json)与[24份归属线截图/性能资料清单](S5/android-80d8a42-artifact-manifest.json)已归档至各阶段目录，文件名带 `android-owner-80d8a42-`。这些是客户端 mock/专项证据，包含历史截图，不代表由集成线在最终 APK 上重新完成场景。Release 为 2,111,878 bytes；集成线拉取已安装 `base.apk` 的 SHA-256 与归属线一致，未重复安装或重启。性能 JSON 保留不同源码修订及软件/硬件模拟器限制；QEMU 曾 exit139，不能据样本认定真机性能或稳定性通过。

历史 Android continuation owner 证据已按 [2ccfc88e Release 记录](S5/android-2ccfc88-continuation-release.json)归档；当前安装来源为 cac5b12，见[覆盖安装记录](S4/android-notification-cac5b12-installed.json)；历史对应的 [fresh-install](S5/android-owner-2ccfc88-release-fresh-install.png)、[connected](S5/android-owner-2ccfc88-release-connected.png)、[dark-theme](S5/android-owner-2ccfc88-dark-theme.png) 和 [swipe-actions](S5/android-owner-2ccfc88-swipe-actions.png) 截图来自 owner 工作流。它确认签名 Release、模拟器安装哈希、Production/Mock Host 配置和局部真实聊天/trace/dashboard 检查；不替代 PLAN 第 6 章的集成线 S0–S5 双端联合验收，也不将 owner 截图记为 root 全场景通过。 [Android 正式新鲜度观察](S2/production-1f771c7-android-live-project.json)对比 23:51 基线与 00:01:42 当前画面，确认 1f 升级后 formal 侧栏出现新 marker `macbot-e2e-s2-login-6770085010b5-登录功能` 及 project `01a12164-6b92-732d-9ecb-ed48e8a94ca8`；这只证明正式侧栏收到新项目，不证明聊天、轨迹、协作或 S2 全场 UI。最新[Android 当前只读截图](S2/production-7968e7e-android-current.png)及[记录](S2/production-7968e7e-android-current.json)仍显示 Production 侧栏原有两个项目，名称未变；不据此判断 live freshness 或新增 Question。d97e48f 的 [登录/聊天截图](S2/production-d97e48f-android-login-chat.png)、[Product 详情截图](S2/production-d97e48f-android-product-details.png)、[群公告截图](S2/production-d97e48f-android-announcement.png)和[native记录](S2/production-d97e48f-android-native.json)已归档；详情 tap 不能证明导航完成。

当前阻断：first 项目已生成 demo，但登录成功后的表单隐藏有实际浏览器缺口，parallel 实现和真实浏览器检查已完成，Tester 原 run 已 done 并写入 TEST.md，但报告明确未实测且日期错误，两项目并发编码和最终验收未闭环；忙碌 @Tester 插话及无效 memory 纠错已局部通过，不重写旧失败历史。新 scoped 工具仍逐项核验，未知旧审批不批。Mac 锁屏阻断桌面连续帧/输入和双端同期核对，S5 fresh 尚未执行。

历史 `ae3009b` 证据（保留，不代表当前 094b7c5）：[实际部署来源](S1/server-ae3009b-deployed.json)、[旧6消息原 ID/seq/时间戳与文字恢复](S1/production-ae3009b-history-repaired.json)、[新 write/read/bash 与已知 Markdown](S1/production-ae3009b-private-chat.json)、[原生只读消息与 trace](S1/production-af14e69-ae3009b-native-read.json)、[文字截图](S1/20261009-214932-desktop.png)、[同连接接管/交还与低画质](S4/production-ae3009b-screen-transport.json)、[桌面 Computer 专项](S4/production-af14e69-ae3009b-native-computer.json)、[包来源与哈希](S5/server-pkg-ae3009b-verified.json)。首次 browser 工具结束后即读到流式占位的记录保留为 early snapshot incomplete，[最终只读复核](S4/production-ae3009b-browser-final.json)已确认文字和 URL。API、桌面专项和候选包校验均不替代真实双端整体验收。最新[桌面摘要](S3/production-af14e69-ae3009b-dashboard-summary.json)与同range RPC一致；[heatmap只读核查](S3/production-af14e69-ae3009b-heatmap-check.json)有Oct9非零数据，不以灰图推断数据缺失。[原生技能新建](S3/production-af14e69-ae3009b-native-skill.json)已落盘，保存按钮在页底，工具滚动限制使修改尚未触发；源码未确认误绑，样例已停用保留复现。

`9e70`最新证据：[write/read/bash、seq1→2和 after_seq](S1/production-9e70d88-private-chat.json)、[2178→5282 同 run 恢复](S1/production-9e70d88-recovery.json)、[pending 与 Workbench 一致](S1/production-9e70d88-workbench.json)、[技能/用量/搜索/跨 DM 偏好](S3/production-9e70d88-api.json)、[偏好实际落盘](S3/production-9e70d88-memory-persisted.json)、[真实 schedule 与 canonical 结果 ID](S4/production-9e70d88-scheduled-routine.json)、[接管 RPC 成功但 driver 广播超时](S4/production-9e70d88-takeover-api.json)。这些是 API 或专项证据，均不替代双端 UI 和完整场景。

历史专项证据仍保留：[22b3 同 run 工具](S1/production-22b3b10-tools.json)、[断线补发](S1/production-bd6e7c2-connection-replay.json)、[浏览器 transport](S4/production-22b3b10-screen-transport.json)、[low 画质限制](S4/production-22b3b10-screen-low-contract-failed.json)、[旧接管 OS2](S4/production-22b3b10-screen-takeover-failed.json)、[22b3 pkg 载荷校验](S5/server-pkg-22b3b10-verified.json)。这些保留为历史证据，不覆盖当前 094b7c5 状态，也不计联合验收。


桌面 [正式摘要数值](S3/production-3f7c046-dashboard-summary.json)与[截图](S3/20261009-210958-desktop.png)独立核对一致，Android 用量一致性待验。S2 [首次场景失败与待审批](S2/production-9e70d88-login-scene-partial.json)已保留，未误记通过。

桌面归属线的 mock 专项证据仍在：[历史截图](S1/main-222fda9-desktop-history.png)、[技能修改](S3/main-222fda9-skill-edited.png)、[接管](S4/main-222fda9-computer-takeover.png)、[窗口重开](S5/main-222fda9-window-reopened.png)及[清单](S5/desktop-222fda9-owner-manifest.json)。历史正式桌面 `af14e69` 记录；前版现场为 [3f7c046 Computer 画面](S4/20261009-210338-desktop.png) 与[窗口元数据](S4/20261009-210338-desktop.txt)及[搜索与绘制断言](S4/production-3f7c046-native-search-computer.json)；[最新 af14e69 Release DMG](S5/desktop-dmg-af14e69-candidate.json)已独立核对源码、哈希与 hdiutil，仍只计候选；S5 [9e70 pkg 来源与载荷校验](S5/server-pkg-9e70d88-verified.json)已通过，尚未全新安装。

共享 mock `11bc831` 的[契约预检](S2/mock-11bc831-contract-precheck.json)通过：Workbench 扁平返回、登录群4成员、2个 pending 引用和PRD搜索。历史失败截图保留，最新结论以 `COORDINATION.md` 为准。

历史桌面 `50f4c08` 曾从 main 干净 archive 构建并部署，不能视为当前安装状态；[热图复验](S3/production-50f4c08-ae3009b-heatmap-data.json)：仅 Oct9 非零 203858 tokens/60 requests，三个阈值相同，原生显示绿色首档、零态仍灰；[截图](S3/20261009-231302-desktop.png)。[技能复验](S3/production-50f4c08-ae3009b-native-skill.json)：实际可见 Save 后 v2 已落盘且预览更新，停用操作保留 v3 未保存草稿、服务器仍为 v2；保存响应在飞时继续输入的严格时序尚未取证。窗口 Raise 后外层滚动已成功，旧 offscreen 尝试保留为历史未完成。正文更新将原停用样例变为启用，已交 server-mac 隔离核查；样例已原生恢复停用。

[50f4c08 候选包](S5/desktop-dmg-50f4c08-candidate.json)哈希与 hdiutil 通过，但缺完整 bundle 签名；[91a18ad 打包修复候选](S5/desktop-dmg-91a18ad-candidate.json)默认完整 ad hoc 签名且 strict verify 通过。91a 只改打包/README，功能源码与50f相同；候选校验不等于 S5 全新安装。

[7c5e480f Release 候选包](S5/desktop-dmg-7c5e480-candidate.json)的 source、binary、DMG 哈希、strict 签名和 hdiutil 校验均通过，integrator 已用 `git ls-remote` 确认 origin 精确提交；[Mac-only debug 开发安装记录](S5/desktop-dmg-7c5e480-deployment.json)显示已安装并运行，但不计为 Release DMG 实装、S5 全新安装或 native UI/input 通过。

历史 [91a 开发安装](S5/desktop-91a18ad-development-deployed.json)已核对 source/签名与服务监听 PID；仍是 debug 更新，不代表当前安装状态或 Release DMG 实装。[本轮浏览器输入复验](S4/production-91a18ad-ae3009b-native-input.json)：新精确 URL 已有真实 browser_open 和最终文字，低画质可见本地页、接管/交还通过；输入未验证，自动画质白画布与低画质偶发白帧已交 client-mac 核查，正常 raw JPEG 保留供对照。新增标准库 `scripts/e2e/s3/skill_update_scope.py` 用于正文更新保持全局/per-Bot停用状态，待服务端累计修复后实跑。

## 快速查看效果

在包含最新 `main` 的工作区执行：

```sh
cd /Users/gongshaojie/Project/mac-bot
./scripts/dev/deploy.sh
./scripts/dev/status.sh
```

`deploy.sh` 会按当前代码可用性编译并部署 `macbotd`、桌面 `.app` 和 Android APK；缺少某条开发线产物时会跳过并打印提示。正式服务使用端口 `7788`，数据目录为 `~/MacBot`，访问密码只从本机文件 `~/.macbot-dev-password` 读取或由部署流程设置，密码内容不写入仓库。

S0 两端会话列表验收已归档：[记录](S0/main-s0-current.json)、[桌面](S0/main-eb088fa-desktop-sessions.png)、[Android](S0/main-08462a4-android.png)。当前 formal `7481d8e` 服务和桌面 `7c5e480f` 均已部署；该 S0 记录属于历史 mock 验收，不替代当前 formal Android UI。
桌面端：双击打开 `~/Applications/MacBot.app`，Host 填 `127.0.0.1:7788`，密码取自 `~/.macbot-dev-password`；当前可查看真实消息和工具轨迹，旧消息文字修复已复验。需要 mock 时填 `127.0.0.1:7789`，密码 `dev`。

Android 模拟器：需要时先执行：

```sh
./scripts/dev/deploy.sh
```

部署脚本会启动 AVD `macbot_api36`、等待 `sys.boot_completed=1` 并安装 APK。打开 App 后，mock 地址填写 `10.0.2.2:7789`、密码 `dev`；formal 地址填写 `10.0.2.2:7788`，密码与桌面端相同。这是模拟器访问 Mac 本机的地址；换成同一局域网中的真机时填写 `192.168.31.162:7788`。

查看服务状态、进程、端口和最近日志：

```sh
./scripts/dev/status.sh
```

需要验证 mock 或 S0 骨架时，启动开发 mock（端口 `7789`）：

```sh
./scripts/dev/mock.sh
python3 scripts/e2e/s0/bootstrap.py
python3 scripts/e2e/mock_contract.py --json
```

mock 的客户端地址是桌面端 `127.0.0.1:7789`、模拟器 `10.0.2.2:7789`，密码为 `dev`。mock 只用于联调，不替代正式服务的 `7788`。

`mock_contract.py` 只读核对扁平工作台、登录群4成员、pending引用和PRD搜索；通过不代表 S1–S5 真实服务联调通过。

阶段验收时，先打开桌面客户端和模拟器中的 App，再使用集成截图脚本保存画面：

```sh
./scripts/dev/capture.sh S0
```

若screencapture finalize卡住而隐藏PNG完整，脚本保留图片并输出需要核图的提示；文件存在不代表UI验收通过。

截图会放在 `docs/progress/S0/`；后续阶段将 `S0` 替换为对应阶段名。桌面截图由 `screencapture` 生成，Android 截图由 `adb exec-out screencap -p` 生成。

## 当前开发线的运行入口

client-mac 已提供实际打包入口，生成带 fixture 资源的 App：

```sh
cd clients/mac
packaging/package.sh debug app
open dist/MacBot.app
```

服务端和 Android 编译运行命令已发布：

```sh
cargo build --release --manifest-path server/Cargo.toml -p macbotd
cd clients/mobile
./gradlew :androidApp:assembleDebug
```

LaunchAgent、独立 MACBOT_HOME、APK 安装与 Activity 命令见 `server/README.md`、`clients/mobile/README.md`。Android 编译使用 JDK 21 与 API 37.0，模拟器和 target 仍为 API 36；`deploy.sh` 会自动探测产物。需要保留 mock 预览时使用 `MACBOT_SKIP_PRODUCTION=1 ./scripts/dev/deploy.sh` 和 `./scripts/dev/mock.sh`；正式访问密码仍只存于 `~/.macbot-dev-password`。


## 真实模型

真实服务端通过 `/api/v1/rpc` 的 `provider.create` 配置 provider。按用户最新授权，开发期使用仓库外、权限 `0600` 的本机凭据文件；不写入 README、脚本、日志、截图或 Git。MiniMax 配置已通过 `provider.test`，正式服务已有实际 Bot 执行记录；如需重新配置，使用下方 `provider.py` 命令。

## 持续部署与模型配置

集成线可安装本机定时检查，每 60 秒获取并检查 `main` 与 `origin/main` 的提交：

```sh
python3 scripts/dev/watch.py --install
```

Android 当前是已交回的签名 Release，允许同签名更新；Debug 签名不同，不卸载或清空数据。本机 `watch/ui-validation-hold` 仍用于保护集成窗口和模拟器现场，未得到主线程确认前不要移除。保留桌面可用 `MACBOT_SKIP_DESKTOP=1`；只部署桌面可显式指定 `MACBOT_SKIP_PRODUCTION=1 MACBOT_SKIP_ANDROID=1`。

Android 部署用 `MACBOT_ANDROID_VARIANT=debug|release` 选择构建，未指定时沿用本机 `android-installed-variant` 记录，否则默认 Debug；本机当前记录为 Release。Release 从 `~/.local/share/macbot/android-signing/release.env` 加载签名环境，可用 `MACBOT_ANDROID_SIGNING_ENV` 指定其他仓库外文件。仅安装 `androidApp-release.apk`；缺少签名或安装签名不匹配时失败并保留已有 App/数据，不卸载、不安装 unsigned APK。变体逻辑已用临时假 Gradle/假签名验证，归属线已验证并交回实际 Release，集成线已核对安装哈希，并完成 cac5b12 同签名保留数据覆盖更新；该专项不代替全部版本组合的部署验收。

源码从 `main` 分支的固定提交导出到 `~/Library/Caches/MacBot/integrator/source/`，不会切换其他开发线的 worktree。远端 `origin/main` 若领先则部署其固定 SHA；本地 main 若领先则部署本地 SHA。二者分叉时停止部署并报告，避免自动选错版本；网络失败时使用已获取的 main 引用。部署与初步 S0 API 检查日志保存在 `~/Library/Caches/MacBot/integrator/watch/`，API 成功不代表两端界面已验收。停止持续检查：`python3 scripts/dev/watch.py --uninstall`。

编译 target 使用独立共享缓存：`~/Library/Caches/MacBot/integrator/target/server` 和 `~/Library/Caches/MacBot/integrator/target/desktop`。每个 main 快照下的 `server/target`、`clients/mac/target` 会链接到对应缓存，减少重复编译；缓存属于本机开发数据，不提交 Git。部署锁会串行化使用同一缓存的构建。

MiniMax CN 国内官方 [Anthropic 接口](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) 的 base URL 为 `https://api.minimax.cn/anthropic`。本机已实测 `MiniMax-M2.5` 文本请求成功，开发凭据现存于 `~/MacBot-dev-secrets/minimax-cn.key`。可用 `MINIMAX_API_KEY_FILE` 指向新的本机文件，或设置 `MINIMAX_API_KEY`；部署服务已配置 MiniMax provider 并通过 provider.test。

本机 `watch/file-secrets` 标记让部署使用 `MACBOT_SECRET_BACKEND=file`，服务端 provider 凭据保存在仓库外的本机目录。服务端访问密码只从 `~/.macbot-dev-password` 获取；此模式不访问钥匙串。用户后续可替换本机 MiniMax 凭据文件，再重新运行 provider 配置命令。

需要重新配置真实模型时，运行：

```sh
python3 scripts/dev/provider.py --set-defaults
```

该命令通过 `provider.create/update` 写入密钥、`provider.test` 检查接入、`model.upsert` 注册模型，再设置主 Bot 和普通 Bot 默认模型。持续部署是否自动执行 provider 配置仍由集成负责人通过本机 gate 标记控制；现有 Bot 如果显式指定了其他模型，仍需在客户端设置中改为默认模型。M2.5 不支持图片输入，配置中的 `vision` 为 false。

## 2026-10-10 Android 恢复与产物下载修复

用户授权 adb 点击/滑动/输入后，macbot_api36 已自行恢复，无需重启；保留 cac5b12 签名 Release 与全部应用数据。实际进入原并行群，确认待验收状态及两个产物链接，未 confirm_done。点击 TEST-browser.md 复现 HTTP 400：产物为绝对 Host 路径，文件接口原先拒绝所有绝对路径。server 6afbff9 仅允许选定根目录内的绝对路径，保留跨项目、父目录及符号链接越界限制；193 项 gateway 测试通过。仅部署 server 后，Android 原报告实际打开成功，下载内容哈希与原文件一致，12 条 pending 审批（按 ID 比较）及全部 job 文件保持不变。证据见 S2/production-6afbff9-artifact-upgrade.json；失败截图同目录保留。

S3 已实际核对 Android 技能页四项及 3 开/1 关，与桌面相同，见 S3/production-ee7111a-android-skills.json。以上为局部验收；S1–S5 均未整阶段通过，fresh-install 未执行。主仓库 COORDINATION.md 与权威文档有其他会话未提交修改，本轮协调记录暂记此处，避免混入他人改动。

## 2026-10-10 群身份修复、双端报告下载与模拟器重启

正式 server 已更新至 `d398bc4`：历史群消息现在区分当前 Bot 与其他 Bot/system，避免测试 Bot 把主 Bot 历史发言当作自己的 assistant 上下文；195 项 gateway 测试通过。12 条旧 pending 及全部 job 文件保持不变。桌面更新至 `a0b9ddf`，保留产物绝对路径，实际下载 TEST-browser.md 的哈希与源文件一致。默认 Markdown 查看器 Typora 因试用过期不能显示，随后通过 TextEdit 实际打开同一下载文件；未修改产物。证据见 S2/production-d398bc4-group-context-upgrade.json、production-a0b9ddf-desktop-artifact-download.json 与对应截图。

原并行项目 Tester 已实测补齐无效邮箱原生验证、正确登录后 360×800 卡片边界，以及调用原 alert 时的真实错误密码消息；报告由原 Tester 修改，早先被拒绝或测量不准的记录保留。主 Bot 最新汇总已在 Android 实际显示，项目仍待验收，未 confirm_done。报告轨迹编号及弹窗观察范围已由原 Tester 修正；旧测量历史的概括仍需保守解读，不能以其“全部通过”文字替代阶段验收。见 production-d398bc4-parallel-alert-observer.json、production-d398bc4-parallel-report-precision.json、production-d398bc4-android-main-review.png。

模拟器随后自行退出（原因未知），按用户授权重启同一 macbot_api36，未 wipe、卸载或清空数据。冷启动后点击已保存 AndroidWifi 恢复网络，Production 配置及原群保留；截图已查看，见 production-d398bc4-android-reconnected.json。Android 新一轮真实流式测试已有服务端事件，最终消息及标记搜索后的轨迹回放已实际看到；最初打开页面时尚无 live run，不能据旧画面判定丢事件，实时 UI 仍需专项验证（S1/production-d398bc4-android-real-stream.json）。

当前仅 S0 整阶段通过；S1/S3/S4 双端补验及 S5 fresh 仍待完成。GitHub 推送遇到网络超时，当前改动已合本地 main，尚未确认远端更新。主仓库根文档的其他会话改动保持不动。

## 2026-10-10 Android 实时轨迹开关修复

已在原私聊 Bot 上看到 Android 实时追加模型请求、read 工具结果及正文。原 cac5b12 的实时思考片段不受“显示思考”开关约束；`d613819` 修复实时 thinking/tool 输出过滤，56 项 shared 测试通过，同签名 Release `adb install -r` 成功且保留两个 Host 与旧群。新版原生截图显示关闭思考时仍能实时追加正文，见 S1/production-d613819-android-live-verified.json 与 live-partial.png。一次完成态截图误导航到聊天输入框，失败截图保留，不计完成态证据；此前新运行的最终消息及轨迹回放已另存。S1 整阶段仍未通过，Android kill‑9 恢复等仍待验。

Mac 再次锁屏，已请求用户解锁；桌面截图/输入暂停，未将其判作客户端冻结。模拟器 AndroidWifi 被系统标记 NETWORK_SELECTION_DISABLED_NO_INTERNET_PERMANENT，重新连接可短暂访问主机，正在处理其持续连接；不清空应用或 AVD 数据。

## 2026-10-10 正式服务状态计数性能修复

`09622d7` 已 server-only 部署，PID 62810，桌面仍 a0b9ddf、Android d613819。状态发布复用单次计数和共享 durable 状态；磁盘 job 文件集合、size/mtime 不一致时回退磁盘，终态 job 不再读取 run_request。gateway 195/195、durable 5/5 及同 ID 外部状态变更专项通过。部署前后 12 条 pending ID、149 个 job 文件哈希完全一致，无任务重放。实际本机 bootstrap 从部署前单次 5230ms 降至部署后三次 89/93/87ms，approval.list 从 2709ms 降至104ms；这是当前现场抽样，不是通用性能保证。证据见 S1/production-09622d7-upgrade-{before,after}.json。

Android 网络受限提示已通过系统通知选择“仍然使用”并对该既有 AndroidWifi 记住选择（noInternetAccessExpected=true），随后重连与恢复验收保持连接。S1 原 Bot 新运行的 Android kill‑9 恢复专项通过：SIGKILL 前核对正式 PID 62810 与安全 checkpoint，LaunchAgent 重启为 PID 65439；同一 run 恢复后 read/marker/done，原 Bash 没有重放，标记文件哈希保持。手机原生页实际显示完整恢复链；12 条旧 pending 与149个旧 job 哈希保持。见 production-09622d7-android-recovery.json 与 running/after.png；整阶段结论仍需对照矩阵复核。

## 2026-10-10 解锁后双端对账与新增缺陷

已核对解锁，桌面进程36391和服务65439保持。CUA键盘与AX可更新页面，但坐标点击返回noWindowsAvailable，截图有时保持旧画面；原生窗口zoom后取得当前截图。代码检查未发现缺少cx.notify的确定性缺陷，暂记工具或窗口可见性限制，不判客户端冻结，不计连续实时验收通过。模拟器已退出，再次按授权启动同一macbot_api36，未wipe、卸载或清数据，既有Production和AndroidWifi恢复。

S3固定日期2026-10-09T00:00:00Z至23:59:59Z：桌面a0b9ddf与Android d613819实际显示5,703,826 token、540次请求、费用未知或未定价，与09622d7 API一致。Android模型明细输入5,531,382、输出172,444、缓存读3,685,100一致；截图均已查看。见S3/production-09622d7-closed-day-usage.json与production-d613819-android-closed-day.json，以及closed-day-detail.png。这是摘要与Android明细专项通过，不是S3整阶段。

S2双端已实际看到原两个项目待验收卡，未confirm_done；原Tester最新alert-observer和report-precision成功证据继续有效，不重复测试。发现Android卡片不显示项目名称，多个待验收卡无法可靠辨识；失败截图和服务端两张review_card保存在S2/production-09622d7-joint-review.json。另查明S3底层draft/publish存在，但模型工具只有只读skill、没有生成草稿入口；正在补受审批约束的草稿工具。两项缺陷修复尚未部署，本记录不宣称解决。


## 2026-10-10 双端修复部署与真实技能草稿

Android `98fb4a9` 已同签名 Release 更新，58 项 shared 测试通过；原两项目待验收卡现在实际显示项目名称，截图已查看，未 confirm_done。主 Bot 多次汇总仍重复登记同路径产物，正在修复服务端登记和历史卡片投影；不删除旧报告或重跑 Tester。

桌面 `e14d0a8` 已安装干净归档的 Release，55 项测试及签名/DMG校验通过。修复仪表盘维度切换被旧30天范围覆盖；实际 Bot/项目维度均保持2026-10-09 UTC与小时趋势，失败截图保留。两端当天摘要与模型明细已对齐：5,703,826 token、540请求，输入5,531,382、输出172,444、缓存读3,685,100，费用未知。见 S3/production-e14d0a8-*；这是局部验收，不是 fresh install。

服务 `8b257ca` 新增受审批的 worker skill_draft 工具。200项 gateway 测试及追加5项针对性测试通过；严格clippy仍受4项既有adapter type-complexity阻断，允许该既有lint后通过。server-only升级保持12条旧pending和150个旧job哈希。真实Bot已生成 integrator-draft-e297d435ad：逐项核准参数、effective target、run和checkpoint后，Android系统通知点击“允许一次”成功恢复同run并done；旧12pending保持。Android技能页和编辑预览已实际看见草稿、路径、完整正文，未保存编辑、未发布。见 S3/production-8b257ca-real-skill-draft.json 与 S4/production-98fb4a9-skill-notification-action.json。

桌面工具随后报cgWindowNotFound，核验当前系统 IOConsoleLocked=Yes，已告知用户需保持解锁，不判客户端冻结。Android草稿详情另发现部分Bot启停开关名称空白，正在定位；桌面草稿查看和发布、最终S1/S3/S4联合复核、S5fresh仍待完成。仅S0整阶段通过。主仓库根文档有其他会话改动，继续保留，协调事实暂记录此处。

## 2026-10-10 技能名称、附件去重与搜索新缺口

Android `5e2fbcc` 已同签名 Release 更新，60项 shared 测试通过。技能详情空白 label 现在回退到 name/id，草稿隐藏启用范围开关并提示发布后设置；真实截图已查看，现有技能范围未修改，真实 Bot 草稿仍未发布。见 S3/production-5e2fbcc-android-skill-scope-fix.json。

server `14a3b18` 已仅服务升级，PID60527；12条旧pending和151个job哈希保持。项目产物按项目/路径更新并保留实际交付者，私人Bot路径另按Bot隔离；历史公告/卡片仅读时去重，不重写原记录。orchestrator 58、gateway 202项原有及新增初版测试通过，追加Bot/项目隔离与旧记录不变等5项release回归通过。原两个公告由6/12条收敛为各2条，Android同屏已实际显示两项目名称和各两附件，未confirm_done；桌面补验仍受锁屏阻断。见 S2/production-14a3b18-artifact-dedup-upgrade-after.json 与 android-review-deduplicated.png。代码已推至远端main。

S3已只读核对原回忆run request，问句确实为“What exact preference did I ask you to remember?”，未提供偏好marker；Android实际展示原问句和正确回忆，未重发模型或写记忆。见 production-8b257ca-memory-request-audit.json、production-5e2fbcc-android-memory-recall.png。搜索此测试marker时新发现内部memory记录误分类成“群”，失败截图与RPC保留在 production-14a3b18-search-memory-misclassified.*，正在定位，不能计搜索通过。S0仍唯一整阶段通过，S5fresh未执行。

搜索缺陷已由 `aa2c99a` 修复并 server-only 部署，PID67422：限制业务数据来源，移除路径子串分类；8项release搜索回归通过。同一测试关键词从4条误分类内部记录+6条真实消息变为仅6条真实消息；Android实际显示消息，“群”筛选为空，截图已查看。12条旧pending和151个旧job哈希保持。本机单次查询从4.158秒降到0.034秒，仅为现场抽样。见 S3/production-aa2c99a-search-path-upgrade-after.json。桌面仍需解锁，最短剩余见 [remaining-native-acceptance](2026-10-10-remaining-native-acceptance.md)。

后续桌面已解锁并恢复截图/输入。桌面已实看两张去重待验收卡、6条真实搜索结果、真实Bot技能草稿，并通过原生发布按钮发布；Android仅对“画面联调-9e70”停用该技能，桌面复核一致。相关截图及操作journal已补入S2/S3。桌面范围标题语义修复6eaa91a已产出Release候选，尚待安装。S1联合恢复继续原run，双端已显示write成功及Bash待审批；未重发请求、尚未kill。S2确认完成会取消5个历史waiting_user assignment，暂不执行。S0仍是唯一整阶段通过；S5fresh未执行。

S1联合恢复现已完成：精确审批write/bash后，在安全checkpoint kill正式PID67422；LaunchAgent拉起79740，同run继续read并done。双端真实实时轨迹、重连提示、部分流式输出→完成及回放已截图实看；Bash成功一次，marker不变，12条旧pending及151个旧job哈希保持。见S1/production-aa2c99a-joint-recovery.json。按PLAN第6章联调门槛，S1现判通过（server aa2c99a）；启动监听约54秒作为独立性能缺陷修复，后续服务版本仍需升级回归。模型输出的“无重启”等未经证实文字不作为验收依据。桌面6eaa91a已安装并实看范围标题修复，原记忆回忆答案及搜索群筛选为空也已补证。当前通过阶段为S0、S1，S2–S5未整阶段通过。

S3按最新真实双端证据判功能联调通过，见S3/production-6eaa91a-stage-conclusion.json；当前通过S0、S1、S3。S4通知容量已在隔离应用包运行真实NotificationManager专项：50→40、FGS/summary保护、marker/ledger/pending断言通过，正式包3条通知keys不变，隔离包已移除。见S4/production-962fb53-isolated-capacity.json；不将其描述为真实provider50连发。Android X仍待Chrome本次连接确认。server 4f4893a已安装且最终健康，12pending/154jobs保持；30秒部署健康期限曾失败，采样发现剩余operation修复热点，继续修复，不隐去失败。

server `9c20fc4` 已仅服务升级并完成空闲现场正常重启回归：PID91534→94543，正确 `/api/v1/health` 8.673秒恢复，12条旧pending、154个旧job哈希全部保持；见 S1/production-9c20fc4-upgrade.json。首次观测误用 `/health` 的超时已明确排除，不作性能证据；一次7.789秒bootstrap后续两次为0.081/0.058秒，未确定其单次慢响应原因。最终9c20fc4开发pkg审计已归档，仍未执行fresh。Chrome权限弹窗现已消失，不能据此认定授权；Android X未通过，测试Bot已恢复原headless模式。S2精确5个旧分支结束确认仍待用户答复。当前通过S0/S1/S3，剩余S2/S4/S5。


## 2026-10-10 接管问题卡与实时游标收尾

S0–S3通过，S4尚未整阶段关闭。server90e637a流式读取启动日志，server-only部署已在30秒健康期限内恢复，旧审批和任务保持；不把秒级时间戳测得0秒解释为精确启动性能。桌面53abef3首次打包误复用旧二进制，失败产物和截图保留；随后实际Release构建并重装，历史问题卡提交按钮已消失。3933e1a补状态标题，508f325补sync.done缺口恢复，新的实际Release正在构建。

当前原接管run仍等待，服务端已落盘question/card；桌面停在6387，内部memory.updated占用6388导致后续事件缺口。正在补服务端live和旧历史重放的游标投影，并沿用原run验证，不重发请求或修改数据游标。S5未开始，离线备份目录为空，旧业务现场完整。


S4最终通过：server3635008补齐内部memory事件的live与replay游标，旧桌面从6387自动显示原等待卡；独立重放无缺口且memory正文未下发。升级前后7pending、164jobs一致。桌面508f325实际Release安装后从原生接管并交还原run，trace297恢复、299工具结束、302 done；两端接管按钮消失，桌面显示已回答，workbench等待归零，仅该job发生预期变化。磁盘cache6411为5秒节流快照，实际落盘早于尾部6412–6415；并非新的live缺口。截图均实看，详见S4/production-508f325-native-takeover.json、stage-conclusion.json。当前S0–S4通过，S5 fresh未开始。冻结候选server3635008/pkg、desktop508f325/DMG、Android04b0c3d/APK；开发签名边界保持。
