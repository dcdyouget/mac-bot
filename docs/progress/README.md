# Mac Bot 集成进度与使用说明

本文面向在 Mac mini（Apple M4，局域网地址 `192.168.31.162`）上查看 Mac Bot 效果的人。服务端运行在这台 Mac 上，Android 客户端运行在本机的 `macbot_api36` 模拟器里。v1 不包含 iOS。

## 现在的状态

最新进展：正式服务已固定部署 `094b7c5ab98c872ef261b8474a284e17e1c49c6b`，健康检查、认证探测和 LaunchAgent/监听 PID 一致；`907a05c` 的 ping/心跳证据仍作为历史契约修复保留。Android 在保留数据的签名 Release 上完成一次既有真实 AE 私聊、最终文本和 write/read/bash 轨迹回放（[证据](S1/production-094b7c5-cac5b12-android-private-trace-replay.json)及截图），不是新 live 流式、kill9 恢复或完整 S1。S2 旧三项任务/pending 保持原样；仅对 718 followup journal 的 root/descendants 做了 6 项显式 stop（另 1 项此前 done），没有 approve，也未修改原项目、PRD 或原建群请求。新 journal 对两个原群各发一次纠正续办请求并关联正确 Main→Coder；现已停在无效 `memory replace/kind=project_status` 审批，未批准。parallel 已有 `login.html`，first 仍仅 PRD；两次工作中 @Coder 被另建任务而非注入原 run，缺少 delivery/read/steer，[取证](S2/production-094b7c5-steer-events-readonly.json)已交 server-mac。S2 不通过；新 RPC 对未知 project UUID 已返回 `not_found`，旧错误 chat 的两个 JSONL 文件 hash 未变。旧04:22定时任务未观察到通知；容量修复 `cac5b12` 同签名覆盖更新后，新04:52通知出现在系统列表和原生通知栏，[点击通知](S4/production-7189395-cac5b12-notification-opened.json)进入正确私聊，手动滚动后可见 marker。更新前50→启动后1的原因未证，本轮不记满50原生容量通过。718 的纵向映射/Android 本地页触摸证据仍保留；桌面输入/X登录仍未验收。Mac 锁屏阻断桌面原生验收，S5 全新安装尚未开始。S0 是唯一整阶段通过。

正式服务 `094b7c5ab98c872ef261b8474a284e17e1c49c6b` 已从固定干净 archive 部署到 `7788`（当前 PID `43528`，可执行文件为 `~/Applications/MacBotServer.app/Contents/MacOS/macbotd`），[部署证据](S1/server-094b7c5-deployed.json)确认 LaunchAgent/监听 PID 一致、安装 source、公开健康检查和部署认证探测；固定 SHA 部署退出 0。MiniMax-M2.5 沿用本机 file backend。桌面当前仍是 `deploy.sh` 的 debug 开发安装 `7c5e480f`（PID `66995`，窗口 `58491`），Release DMG 仅完成候选校验；[历史现场截图](S4/20261009-235101-desktop.png)只记录 23:51 的“正在连接”页，不能代表当前 094 状态或 formal 已连接证据。`lsof` 的 7788 ESTABLISHED 只证明 TCP；CUA fullpath、Finder、Activity Monitor 均 `cgWindowNotFound`，native 连续帧和输入仍未验收。[3 秒样本](S5/desktop-dmg-7c5e480-deployment.json)不能据 AppKit idle/connect timer 判定冻结。Android 当前通知修复 Release 来源为 `cac5b12d68fbe409b41e763231c28f2e7baaff37`（发布记录 `6e1d7f8`），APK `2,111,890` bytes、SHA-256 `c1b2e2fb…2872f2a`，[保留数据覆盖安装记录](S4/android-notification-cac5b12-installed.json)核对与旧版本同签名、安装base.apk哈希一致，新PID `20943`，连接目标为正式 `10.0.2.2:7788`；owner 证据是局部 Production 检查，不等于集成线全场景 PASS。持续部署保留 UI 验证 hold，避免中断现场。

| 阶段 | 目标 | 联调状态 | 截图目录 |
|---|---|---|---|
| S0 | 桌面端和 Android 模拟器连接 mock 并看到会话列表 | 通过：两端连接 mock 并看到会话列表 | `docs/progress/S0/` |
| S1 | 真实服务端上的单 Bot 对话、工具和轨迹恢复 | 未通过：094 的 Android 既有 AE 私聊、最终文本和 write/read/bash 轨迹回放已局部通过（[私聊截图](S1/production-7189395-cac5b12-android-private-replay.png)、[轨迹截图](S1/production-7189395-cac5b12-android-trace-replay.png)、[工具回放截图](S1/production-094b7c5-cac5b12-android-trace-tools-replay.png)），但不是新 live 流式或 kill9 恢复；严格 one-call 检查仍因模型额外请求 `bash_job` 保留失败，桌面、实时流式和完整联合验收待验 | `docs/progress/S1/` |
| S2 | 主 Bot、群协作、插话和待验收 | 未通过：旧三项任务/pending 保持原样；718 followup 的 6 项显式 stop 已确认取消、另 1 项此前 done，未 approve 且未改原项目/PRD/原建群请求（[证据](S2/production-094b7c5-followup-task-convergence.json)）。首个 Main trace 的 assign 已关联 Coder；新 journal 对两个原群各发一次纠正续办请求，canonical→Main trace assign→新 Coder 已精确关联；现停在无效 memory replace/kind=project_status 审批，未批准。parallel 有 login.html、first 仍仅 PRD；工作中 @Coder 另建任务而非插话，已分派 server-mac；新未知 project UUID RPC 已 `not_found`，旧错误 chat 的两个 JSONL hash 未变（[证据](S2/production-094b7c5-invalid-chat-rpc.json)）。缺 demo/Tester/编码并发/插话闭环，相关旧审批保持未批准。 | `docs/progress/S2/` |
| S3 | 技能、仪表盘、搜索和记忆 | 未通过：Android [技能 CRUD](S3/production-d97e48f-android-skill-crud.json) 已局部验收 PASS；d43 [正文更新保持全局/per-Bot停用范围](S3/production-d43a2bb-skill-scope.json) API PASS；[只读用量聚合](S3/production-d97e48f-usage-readonly.json) PASS，为 `399,070` tokens、`99` requests，费用尚未定价；仍不等于双端 S3 全通过 | `docs/progress/S3/` |
| S4 | 浏览器画面、接管、定时任务和通知 | 未通过（局部通过）：718 的 [Android 原生本地页触摸提交/交还](S4/production-7189395-android-native-input.json)通过；907 的定时任务 API 按时完成，旧 [50 条容量阻断](S4/production-907a05c-android-notification-quota-metadata.json)已修并覆盖更新，新 [schedule通知](S4/production-7189395-cac5b12-android-notification-receipt.json)在系统与原生画面出现；[通知打开记录](S4/production-7189395-cac5b12-notification-opened.json)确认点击进入正确私聊，手动滚动看到 marker，但未测试审批动作。更新后实际数量未满50，仍不计满额 native 验收。桌面输入、X 登录及完整双端操作待验，Mac 锁屏仍阻断桌面 UI。 | `docs/progress/S4/` |
| S5 | pkg 全新安装、两端连接和完整场景 | 未验收：094 pkg 已完成[候选来源/载荷审计](S5/server-pkg-094b7c5-verified.json)，未做实际 installer/fresh install；桌面 Release DMG 仍仅包核验通过。 | `docs/progress/S5/` |

每个阶段只有在三条开发线都在 `COORDINATION.md` 打卡，并且集成线完成真实联调、保存两端截图后，才会标记为“通过”。

Android [通知权限/渠道只读元数据](S4/production-d43a2bb-android-notification-owner-metadata.json)确认系统通知权限已授予、渠道未删除、前台连接服务运行；新窗口的应用总开关、Bot/chat 过滤和目标 ledger 亦已核正常；ledger 记录不代表系统实际展示。正式 ping 已修复并完成独立 60 秒 transport 检查；[Android端口观测](S1/production-907a05c-android-heartbeat-observation.json)在采样期间保持原formal连接。旧[04:22定时任务](S4/production-907a05c-scheduled-routine.json)已 done 但通知仍 NOT_OBSERVED；[原生通知总开关](S4/production-907a05c-android-notifications-enabled.png)启用，Bot通知启用、聊天未静音。Android owner 已确认[系统日志](S4/production-907a05c-android-notification-quota-metadata.json)达到50条 App 通知上限并拒绝新增；容量管理/去重修复 `cac5b12` 已同签名覆盖安装：[新04:52定时任务](S4/production-7189395-notification-cac5b12-scheduled-routine.json)按时 done，[通知 receipt](S4/production-7189395-cac5b12-android-notification-receipt.json)和[原生通知栏](S4/production-7189395-cac5b12-native-notifications.png)出现唯一 marker 及完成通知。[打开记录](S4/production-7189395-cac5b12-notification-opened.json)确认 native tap 进入预期 bot/chat，手动滚动后看到 marker；审批动作未测。更新前50、启动后1，原因未证且未手动 cancel；本轮计消息通知打开局部 PASS，不计满50原生容量或完整 S4。旧窗口 NOT_OBSERVED 不回填因果。718 的[Android原生输入](S4/production-7189395-android-native-input.json)仍是本地 fixture 专项；不计 X 登录、桌面输入或整阶段。

Android [交回记录](S5/android-80d8a42-handoff.json)与[24份归属线截图/性能资料清单](S5/android-80d8a42-artifact-manifest.json)已归档至各阶段目录，文件名带 `android-owner-80d8a42-`。这些是客户端 mock/专项证据，包含历史截图，不代表由集成线在最终 APK 上重新完成场景。Release 为 2,111,878 bytes；集成线拉取已安装 `base.apk` 的 SHA-256 与归属线一致，未重复安装或重启。性能 JSON 保留不同源码修订及软件/硬件模拟器限制；QEMU 曾 exit139，不能据样本认定真机性能或稳定性通过。

历史 Android continuation owner 证据已按 [2ccfc88e Release 记录](S5/android-2ccfc88-continuation-release.json)归档；当前安装来源为 cac5b12，见[覆盖安装记录](S4/android-notification-cac5b12-installed.json)；历史对应的 [fresh-install](S5/android-owner-2ccfc88-release-fresh-install.png)、[connected](S5/android-owner-2ccfc88-release-connected.png)、[dark-theme](S5/android-owner-2ccfc88-dark-theme.png) 和 [swipe-actions](S5/android-owner-2ccfc88-swipe-actions.png) 截图来自 owner 工作流。它确认签名 Release、模拟器安装哈希、Production/Mock Host 配置和局部真实聊天/trace/dashboard 检查；不替代 PLAN 第 6 章的集成线 S0–S5 双端联合验收，也不将 owner 截图记为 root 全场景通过。 [Android 正式新鲜度观察](S2/production-1f771c7-android-live-project.json)对比 23:51 基线与 00:01:42 当前画面，确认 1f 升级后 formal 侧栏出现新 marker `macbot-e2e-s2-login-6770085010b5-登录功能` 及 project `01a12164-6b92-732d-9ecb-ed48e8a94ca8`；这只证明正式侧栏收到新项目，不证明聊天、轨迹、协作或 S2 全场 UI。最新[Android 当前只读截图](S2/production-7968e7e-android-current.png)及[记录](S2/production-7968e7e-android-current.json)仍显示 Production 侧栏原有两个项目，名称未变；不据此判断 live freshness 或新增 Question。d97e48f 的 [登录/聊天截图](S2/production-d97e48f-android-login-chat.png)、[Product 详情截图](S2/production-d97e48f-android-product-details.png)、[群公告截图](S2/production-d97e48f-android-announcement.png)和[native记录](S2/production-d97e48f-android-native.json)已归档；详情 tap 不能证明导航完成。

当前阻断：094 formal 已部署，ping/心跳和认证通过；Android 094 现场是既有会话/工具轨迹回放，不是新 live 流式或 kill9 恢复，严格 one-call 仍因额外 bash_job 失败。旧04:22通知未观察到保留；容量修复更新后新04:52通知已显示，普通通知点击进入正确私聊并经手动滚动看到 marker，但满50原生容量、审批动作和完整 S4 仍待证。S2 旧三项任务/pending 保持原样；718 followup 的选定任务已收敛，两个原群的新纠正请求已因无效 memory 审批停止；parallel 有 login.html、first 缺实现，工作中插话被另建任务，server-mac 正修，未知 project UUID 的新 RPC 已拒绝，旧错误 chat JSONL 未改；完整 demo/Tester/并发/插话证据仍缺，旧审批保持未批准。Android 技能 CRUD 与用量只读聚合是局部证据，不能替代双端全场景。718 的 Android 图像流/接管/交还及本地页触摸提交证据保留，桌面输入/X 登录未验，Mac 锁屏仍阻断桌面原生 UI；S5 094 pkg 仅候选审计，未 fresh install。

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

S0 两端会话列表验收已归档：[记录](S0/main-s0-current.json)、[桌面](S0/main-eb088fa-desktop-sessions.png)、[Android](S0/main-08462a4-android.png)。当前 formal `094b7c5` 服务和桌面均已部署；该 S0 记录属于历史 mock 验收，不替代当前 formal Android UI。
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
