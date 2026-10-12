# About 白屏修复验收 — 2026-10-12

## 根因与改动

- `AboutPage` 在 `SettingsDetail.tsx` 渲染版本行时对 `t('settings.about.version')` 结果直接调用 `.replace()`；`i18n.ts` 原先缺少该 key，运行时返回 `undefined`，造成 React 渲染异常和整页空白。
- 同一页面的 `settings.about.check_update`、`settings.about.export_diag` 也缺失。三个文案已按 `design/07-copy.md` 补齐。
- `aboutCopyKeys.test.mjs` 使用 TypeScript AST 从 `AboutPage` 提取实际静态 `t()` key，并检查 `i18n.ts` copy 表，防止此类 key 缺失再次导致页面白屏。

## QA 环境

- 源码基线：`20385c4`，改动位于独立 worktree `tmp/desktop-about-white-screen-fix`。
- QA bundle：`/private/tmp/macbot-about-qa/MacBotAboutQA.app`，Bundle ID `bot.mac.desktop.aboutqa`，QA 进程 PID `95400`。
- mock：`127.0.0.1:7813`，隔离数据根 `/private/tmp/macbot-about-qa/`；health 返回 protocol 1、version 0.1.0。
- 连接后从「设置 → 关于」进入成功，页面显示客户端 `0.1.0`、协议 `1`、Host `macbotd 0.1.0` 和 Host 名称，并出现「检查更新」「导出诊断信息」两个控件。
- 已从 About 导航返回「通用」设置页，返回正常。浅色、深色截图均为 QA 窗口截图：`about-light.png`、`about-dark.png`；返回页证据为 `settings-general-light.png`、`settings-general-dark.png`。
- 更新检查和诊断导出控件只验证显示，没有实际触发联网或文件保存。

## 验证

- `npm run test:i18n-keys`：通过（1/1）。
- `npm run build`：通过（TypeScript 与 Vite）。
- `npm run lint`：通过。
- `clients/mac`：`cargo test --workspace` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `clients/mac/app/src-tauri`：`cargo test --workspace`（56/56）与 strict clippy 通过。
- `npm run tauri -- build --bundles app`：通过。
- QA App 与端口 7813 mock 已关闭。
