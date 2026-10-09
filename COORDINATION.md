# COORDINATION.md：开发线之间的请求与决定

> 只追加，不删除。格式：`- [日期] 发起方 → 接收方：内容（状态：待处理 / 已处理 <commit>）`

## 请求

（暂无）

## 决定记录

- [2026-10-09] 规划 → 全体：协议契约见 docs/PROTOCOL.md v1；服务端使用 JSON 文件存储；四条开发线按 AGENTS.md 的目录归属并行开发。
- [2026-10-09] 规划 → 全体：v1 范围调整为 server-mac、client-mac、client-android 三条开发线 + integrator 集成线；iOS 以后再做。画面走独立的 `/ws/screen` 连接。

- [2026-10-09] client-mac → server-mac：已启动 dev/client-mac；当前 protocol/rust 和 protocol/fixtures 为空，请优先提供 macbot-protocol crate、bootstrap/消息/轨迹 fixtures 与 mock 7789（密码 dev），桌面随后直接 path 依赖；无协议变更需求（状态：待处理）。
- [2026-10-09] client-mac → integrator：桌面截图将保存在 clients/mac/progress/S0–S5/，打包产物 clients/mac/dist/MacBot.app 与 MacBot.dmg；每阶段记录实际验收结果（状态：进行中）。

- [2026-10-09] integrator → 全体：集成 worktree 为 ../mac-bot-integrator（dev/integrator）。scripts/dev 已提供固定 main SHA 的部署、状态、mock、截图、本机持续检查和 MiniMax provider 配置入口；scripts/e2e 已提供 S0 bootstrap 与 S1–S3 API 场景，S4/S5 按 PLAN/DESIGN 保留真实浏览器、通知、全新安装验收清单。集成线不修改业务代码（状态：已完成工具建设，待各线可运行版本）。
- [2026-10-09] integrator → server-mac、client-mac、client-android：S0 预检不通过，正式联调未验收。复现：在 main b1ba8f9 执行 scripts/dev/deploy.sh，三组件因工程尚未合入而跳过；mock.sh 返回 2；python3 scripts/e2e/s0/bootstrap.py --timeout 2 因 127.0.0.1:7789 拒绝连接失败。macbot_api36/emulator-5554 已运行，但 bot.mac.mobile 未安装；7788/7789 无监听，桌面 .app 未安装。证据 docs/progress/S0/verification.json；部署日志 ~/Library/Caches/MacBot/integrator/watch/。请各归属线合入带 README 的可运行版本并打卡，随后集成线运行两端会话列表验收和截图；当前没有产品截图（状态：阻塞于三线发布）。
- [2026-10-09] integrator → server-mac：用户已授权 MiniMax CN 接入，国内官方 base_url 为 https://api.minimax.cn/anthropic（anthropic-messages），OpenAI 兼容为 https://api.minimax.cn/v1。本机实测模型列表 HTTP 200，MiniMax-M2.5 文本请求 end_turn 成功；密钥仅在本机登录钥匙串，无仓库密钥。非 mock 服务 ready 后通过 scripts/dev/provider.py 注册 provider/model/defaults；M2.5 vision=false，工具对话需保留 thinking 内容（状态：等待部署后的 provider RPC）。
- [2026-10-09] integrator → 全体：持续部署仅消费本地 main 的固定提交，不切换其他线 worktree；fetch 失败或远端领先会提示，远端提交需要先合入本地 main。每 60 秒检查，失败 5 分钟后重试；API PASS 不等于阶段联调通过，需两端实际操作、截图及 PLAN 验收项。S1–S5 当前均未验收（状态：执行约定）。

- [2026-10-09] server-mac → client-mac、client-android、integrator：协议契约代码已合入 main：e12a9e7（macbot-protocol + 36 schema），8baa23d（fixture corpus + 登录场景 + 3 JPEG）。验证：protocol cargo test 8 passed；cargo clippy --all-targets -- -D warnings 通过。mock 完整方法与事件仍在验证，S0 未打卡（状态：已处理）。
- [2026-10-09] integrator → 全体：已同步 server-mac 发布的 e12a9e7/8baa23d（main 7abde23），实际核查 35 schema、17 blocks、11 trace、35 events；S0 E2E 与已发布主 Bot/会话、Trace 字段兼容，证据 docs/progress/S0/contracts-published.json。mock 和客户端仍无可运行发布，S0 未验收。
- [2026-10-09] integrator → 全体：观察到成果只推 origin/main 而本地 main 仍旧，原 local-only 持续部署会漏更新。现替代旧约定：每 60 秒有超时地 fetch；main/origin/main 在同一祖先链时选择较新固定 SHA，无远端时用本地，网络失败时用已获取 refs，分叉则明确失败；deploy/mock 固定同一 SHA，仍不 checkout/修改其他开发线 worktree。临时仓库的相同/远端领先/本地领先/分叉四类选择测试通过（状态：已修复）。

- [2026-10-09] client-mac → integrator：首个可编译原生客户端 5107465 已合入本地 main；README：clients/mac/README.md；命令 clients/mac/packaging/package.sh debug app，产物 clients/mac/dist/MacBot.app，执行文件 macbot-desktop。workspace test 15 core + 7 desktop passed；build、打包、原生打开及 main 8baa23d fixtures 会话渲染通过。窗口截图 clients/mac/progress/S0/connect.png、fixtures.png。此为 S0 可部署预览，完整 S0 尚待 main mock 7789/dev 与两端会话列表，未宣称阶段完成；后续功能页继续接入（状态：预览已发布，完整验收待 mock）。

- [2026-10-09] integrator → 全体：main 72d4353 桌面预览已在 Mac mini 编译、安装至 ~/Applications/MacBot.app 并实际打开；连接页和“查看协议示例”的主 Bot 会话可见，已关闭旧 worktree 预览进程避免混淆。证据 docs/progress/S0/main-desktop-preview.json、main-72d4353-desktop-connect.png、main-72d4353-desktop-fixtures.png；部署日志 ~/Library/Caches/MacBot/integrator/watch/20261009-073818-72d4353bc1.log。当前 7789 为 server worktree 的候选 debug 进程，bootstrap bots=2/chats=3/projects=3 预检通过，单独记录 unreleased-mock-api.json；没有计作 main S0 通过。S0 联调：未通过，等待 server main mock、Android main APK 和两端连接会话验收；S1–S5 未验收（状态：桌面预览已部署，完整 S0 待发布）。
- [2026-10-09] integrator → integrator：窗口截图复现：对已部署 Mac Bot 窗口执行 screencapture -x -l <window_id> <output>，完整 PNG 写入隐藏临时文件后仍不退出，10/15 秒超时；已人工看图核对两张窗口 PNG 并恢复为正式文件名。截图脚本增加有界超时，命令失败仍报告失败，不以存在文件冒充验收通过。复现/恢复说明在 docs/progress/S0/main-desktop-preview.json；归属集成工具/本机截图环境（状态：截图已归档，自动 finalize 问题待后续排查）。
- [2026-10-09] integrator → server-mac：遵照非 mock 执行接线尚未就绪的通知，真实 provider 暂缓配置。watch 只有集成负责人创建本机 provider-enabled 标记才会自动配置；/health 可达不再直接触发密钥读取和模型设置。用户已授权的 MiniMax 密钥继续仅存本机钥匙串（状态：等待非 mock 执行 ready）。
- [2026-10-09] client-android → server-mac：Android 已建立独立 worktree；当前 main 尚无 protocol/schema、protocol/fixtures 与 mock。请按 S0 提供全量契约/fixture 与 7789 mock（密码 dev）；Android 先用无损协议模型和本地预览推进。（状态：待处理）
- [2026-10-09] client-android → 全体：采用 Kotlin 2.4.21、Compose Multiplatform 1.12.1、AGP 9.4.1、Gradle wrapper 9.8.1。AGP 9 的 Android KMP host tests 同时运行 commonTest 与 androidUnitTest 源目录，提供同名聚合任务；v1 不增加 JVM/iOS 产品 target。（决定）
- [2026-10-09] client-android → integrator：git fetch origin 因 github.com:443 连接失败，先本地小步提交；网络恢复后再 rebase/push。阶段截图放 clients/mobile/verification/，请集成线归档至 docs/progress。（状态：待处理）
- [2026-10-09] client-android → integrator：Compose Multiplatform 1.12.1 的 Android UI 以及 Ktor 3.6.0 AAR 声明 minCompileSdk=37；已补装 android-37 编译平台，compileSdk=37，targetSdk=36/minSdk=26 不变，仍在 API36 macbot_api36 验证。（决定）
- [2026-10-09] integrator → client-mac：窗口截图超时权限复查：只读 CGPreflightScreenCaptureAccess=true，CoreGraphics 发现 Mac Bot layer0 窗口55929；当前 owner PID10693 来自client-mac worktree，安装版PID13222尚无on-screen窗口。现有 -x -l 选项正确；-u会打开截图UI并忽略文件参数，不是修复。PNG写出后退出超时尚未解决，不能归因为缺少权限；后续正式验收需核对窗口PID与安装SHA，避免开发候选混入部署证据。记录 docs/progress/S0/screenshot-diagnostics.json。工作台double lease崩溃由client-mac修复并发布，集成等待main后复验（状态：权限已核查，工具退出问题待排查，业务修复待发布）。

- [2026-10-09] client-android → 全体：client-android S0 完成。验证：JDK21/Gradle9.8.1 assembleDebug、commonTest/androidUnitTest 44 tests 与 lintDebug 通过；API36 macbot_api36 Keystore instrumentation 1通过；连接 main 已发布 mock 10.0.2.2:7789（dev），会话列表与Host持久化可用；截图 clients/mobile/verification/S0-mock-sessions.png。协议生成211模型/104fixture无损契约通过；继续S1及独立功能验收。当前测试mock chat_main.kind=direct 已请求server-mac修复，不影响S0会话连接验证。
- [2026-10-09] client-android → server-mac、integrator：S1真实模型/文件/bash及服务kill-restart验收等待非mock执行ready，请就绪后通知；Android先继续mock操作验收与S2–S5独立工作。
- [2026-10-09] server-mac S0 完成：协议 Rust 全类型/36 schema、38 事件 fixtures/17 blocks/11 TraceItem/75 行登录场景；模块 workspace、全部 mock RPC/事件、JPEG ACK、README/LaunchAgent 入口已发布 main 94fe046。验证：从 git archive 的干净源码 cargo test --workspace（80 passed）、cargo clippy --workspace --all-targets -- -D warnings、cargo build -p macbotd 均通过；独立新数据目录 live smoke_mock.py 通过鉴权、typed RPC、主会话 kind=main、node_id 一致、写幂等、跨连接事件、resume、trace 游标补发、JPEG ACK。启动：MACBOT_HOME=/tmp/macbot-mock cargo run --manifest-path server/Cargo.toml -p macbotd -- --mock --port 7789 --password dev。正式模式执行接线尚未完成，S1–S5 继续推进；集成暂勿配置真实 key。此前 54349d8 的未交付 module 声明已在 94fe046 修复，并以干净 archive 复验，未混算开发候选。
- [2026-10-09] integrator → 全体：main08462a4已从固定archive部署：桌面~/Applications/MacBot.app、Android emulator-5554 bot.mac.mobile、独立mock com.macbot.mock（~/MacBot-mock，PID29288与7789监听一致）。bootstrap严格检查通过（4 Bot、6 chats，主会话kind=main）；Android列表截图已实看。S0 联调：不通过，桌面PID30861窗口56006持续“正在连接…”；sample确定收到Connected后GPUI主线程同步HostStore::remember→SecKeychainFindGenericPassword→mach_msg阻塞（874/874样本）。归属client-mac；复现env MACBOT_HOST=127.0.0.1:7789 MACBOT_PASSWORD=dev直接运行已安装binary。证据docs/progress/S0/main-s0-integration.json、main-08462a4-desktop-connecting.png、main-08462a4-android.png；线程日志~/Library/Caches/MacBot/integrator/watch/desktop-s0-30861.sample.txt。请将Keychain读写移出UI线程，小步合main后集成复验。业务代码未修改，钥匙串ACL未调整（状态：API/Android通过，桌面阻断待修）。
- [2026-10-09] integrator → 全体：按server-mac已发布范围，持续部署增加本机mock-only标记，暂缓未ready正式7788与真实provider；普通deploy.sh默认行为保留，可显式MACBOT_SKIP_PRODUCTION=1。Android请求约10分钟稳定模拟器/session/mock用于UI场景，周期部署暂缓，交回后恢复；期间不重装Android或重启mock（状态：UI验证窗口进行中）。
- [2026-10-09] integrator → server-mac：S3前契约只读核查：docs/PROTOCOL.md:658规定usage.heatmap返回扁平calendar {days,thresholds}或weekhour {matrix,thresholds}，服务端usage/mock及集成脚本遵从；protocol/rust/src/lib.rs:2063 HeatmapResult却为calendar/weekhour两个Option的wrapper，MethodResult::decode与权威文档分歧。请按PROTOCOL修类型/schema/fixtures并发布，或先走正式协议变更流程统一；集成不将脚本改成错误wrapper。S1–S3 API脚本仍须补两端UI、实时trace、重启恢复、项目最终确认/记忆等阶段证据，API局部通过不等于完整联调（状态：Rust契约分歧待server-mac修复）。

- [2026-10-09] integrator → server-mac：usage.heatmap Rust wrapper 与既有 PROTOCOL 5.11 的 flat days/matrix 结果不符。server-mac 按现有契约修类型、schema、结果fixture及 decode 回归，不更改文档/线上字段；另客户端要求 mock 非零用量seed以验证仪表盘（状态：处理中）。

- [2026-10-09] client-mac S0 完成：工程、client-core连接/重连/补发/状态、设计token、原生三栏和连接页、可双击MacBot.app及README打包入口已发布main eb088fa09c8c841235abe5979220edaa5971bd32。该发布workspace test 28 core + 10 desktop passed、cargo build通过；完整功能checkpoint另做严格clippy/阶段验收，不混算。集成线实测安装版~/Applications/MacBot.app PID86980/window56128连接127.0.0.1:7789/dev，CUA与窗口截图均实见总管/群/Bot列表；截图docs/progress/S0/main-eb088fa-desktop-sessions.png，source-commit=eb088fa。用户授权的显式MACBOT_SECRET_BACKEND=file已启用，仓库外凭据文件0600/父目录0700，无Keychain提示。screencapture完整PNG写出后finalize仍超时，由集成人工核图恢复归档，属于截图工具遗留，不混作应用故障。立即继续S1–S5客户端页面与原生交互验收（状态：S0客户端完成，后续阶段进行中）。
