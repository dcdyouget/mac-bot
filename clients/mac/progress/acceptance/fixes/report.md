# 桌面修复独立验收

日期：2026-10-12
范围：当前主工作树未提交的桌面通知与公告空态修复。只验证，不修改产品代码。

## 结果

| 项目 | 结果 | 证据 |
|---|---|---|
| server workspace tests | PASS | cargo test --workspace 全部通过；无失败。 |
| server clippy | PASS | cargo clippy --workspace --all-targets --all-features -- -D warnings 通过。 |
| clients/mac workspace tests | PASS | cargo test --workspace 通过：core 39、services 33，均无失败。 |
| clients/mac clippy | PASS | cargo clippy --workspace --all-targets --all-features -- -D warnings 通过。 |
| Tauri tests | PASS | cargo test --manifest-path app/src-tauri/Cargo.toml --workspace：56 passed。 |
| Tauri clippy | PASS | cargo clippy --manifest-path app/src-tauri/Cargo.toml --workspace --all-targets --all-features -- -D warnings 通过。 |
| 前端 build / lint / summary tests | PASS | npm run build（tsc + Vite）、npm run lint 通过；npm run test:summary：2/2 passed。Build 有 624.33 kB JS chunk 体积提示。 |
| 打包 | PASS | packaging/package-tauri.sh 生成 dist/MacBot.app 和 MacBot.dmg；DMG checksum 验证通过。 |
| 正式包签名 | PASS | codesign --verify --deep --strict 通过；Plist 与签名均为 bot.mac.desktop，TeamIdentifier 未设置（adhoc）；BubblePop.caf 已打入资源。 |
| QA bundle 工作台→群聊与公告空态 | PASS（渲染/可达性） | 独立 QA 副本在 7799 隔离 mock 上启动，工作台打开“官网登录入口改版”后进入“官网改版”群聊，无灰屏；公告为空时显示设计文案。 |
| 与原型内容对照 | PARTIAL | 结构和空公告状态可核对；mock 只有 0 成员、0 个制品和一条旧消息，无法对应原型中的成员、制品和多消息内容。本轮按主 agent 指示只以群聊可达为 gate。 |
| 通知后台送达 | PARTIAL（本轮独立复测未触发） | 通知实现 agent 的上一轮正式包隔离实送达报告仍记录 Authorized、alert=2、notificationCenter=2、add callback 无错误、delivered count=2 且匹配；本轮独立复测因连接前出现 macOS“Zeron 想要访问其他 App 的数据”权限提示而未进入 connected，未触发新 DM，因此不计为独立送达通过。正式包 ID 仍复核为 `bot.mac.desktop`。系统“播放通知声音”关闭，实际可听性未验证。 |

## GUI 灰屏调查

使用唯一 QA bundle MacBotFixQA（bot.mac.desktop.fixesqa）做渲染验证；它是从当前 dist/MacBot.app 复制的独立副本，临时 adhoc 签名，仅用于 GUI 渲染验证，不代表正式 bot.mac.desktop 包的签名或通知行为。窗口 AX 地址为 tauri://localhost。启动欢迎页显示正常，之后按“工作台”→“官网登录入口改版”→“打开”到群聊，页面和公告空态均正常。

首次检查正式同 bundle 时出现过灰色画面，但系统中存在多个同名 bot.mac.desktop 进程，CUA 按 app 路径选择无法唯一关联窗口。对本次启动的正式包 PID 采样显示主线程处于 AppKit 事件循环，WebCore/WebKit 线程存在；未发现近 10 分钟 crash report。QA 唯一 bundle 的冷启动和群聊点击都未复现灰屏。因此当前证据更支持同 bundle 多实例导致窗口选取/加载时态混淆，不能据此判定 WebView 启动或群聊导航存在产品灰屏缺陷；也没有可用的 app 内 DevTools/Console 输出可进一步证明 JS 错误。

## 截图与复现

- [群聊与右侧群信息面板](group-empty-dark-opaque-panel.png)：暗色、不透明，含公告空态、0 成员/制品信息。
- [群聊主视图](group-empty-dark-opaque.png)：暗色、不透明。
- 复现：启动 QA bundle → 连接 127.0.0.1:7799（隔离 mock，密码 dev）→ 工作台 → “官网登录入口改版”旁的“打开”→ “官网改版”。
- mock project.get(prj_web) 返回 announcement.text=""，前端显示“还没有公告。点「编辑」写下这件事要做什么。”
- packaging/screenshot-window.sh 保存时返回 screencapture: cannot write file to intended destination；系统屏幕录制权限预检为 authorized。改用 PID/窗口 ID 定向 screencapture 成功保存上述 PNG，因此不是 TCC 权限阻塞。

## 限制

先前通知送达 API 结果沿用 `../notification-verify/report.md`；本轮独立重测的连接表单被 macOS App 数据访问提示阻断，未授权系统权限、未连接 mock、未触发 DM。独立诊断日志见 [notification-independent.log](notification-independent.log)，仅记录状态和 ID，不含正文。实际声音播放未验证。QA bundle 和正式 bundle 的签名用途分别记录，不能将 QA bundle 的 adhoc 签名作为正式签名验收。
