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
