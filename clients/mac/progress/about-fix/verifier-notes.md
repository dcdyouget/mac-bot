# About 修复独立复验 — 2026-10-12

## 结果

通过聚焦复验。当前集成工作树的 About 页面在独立 QA App 中正常渲染；从「设置 → About」进入可见 `客户端 0.1.0 · 协议 1 · Host macbotd 0.1.0`、Host 名称、「检查更新」和「导出诊断信息」。随后返回「通用」设置页正常。上述路径在深色、浅色主题各执行一次，未复现白屏。

- [About 深色](verifier-about-dark.png)
- [通用设置 深色](verifier-general-dark.png)
- [About 浅色](verifier-about-light.png)
- [通用设置 浅色](verifier-general-light.png)

## QA 身份

- Bundle 从本次当前工作树构建的 `clients/mac/app/src-tauri/target/release/bundle/macos/Mac Bot.app` 复制到 `/private/tmp/macbot-aboutfix-verifier/MacBotVerify.app`；仅改副本的 Bundle ID 为 `bot.mac.desktop.aboutfixverify` 并 ad-hoc 重签名。副本可执行文件 SHA-256：`642889c31ba391bd43fd1b2ee6f80c486bf5b716e6f2a84321e7815373ab6e92`。
- QA 进程 PID `2929`；数据根 `/private/tmp/macbot-aboutfix-verifier/client-data`，file secrets `/private/tmp/macbot-aboutfix-verifier/secrets`。Mock 监听 `127.0.0.1:7814`，`MACBOT_HOME=/private/tmp/macbot-aboutfix-verifier/server-data`；健康检查返回 `ok=true`、protocol `1`、version `0.1.0`。
- 已在 macOS Accessibility tree 中确认版本行和两个按钮。截图是 QA 窗口区域截图（2560×1640 像素）。

## 验证 gates

- `npm run test:i18n-keys`：通过，1/1。
- `npm run build`：通过，TypeScript 与 Vite 构建成功；Vite 提示一个压缩后 chunk 超过 500 kB。
- `npm run lint`：通过。
- `clients/mac`: `cargo test --workspace` 通过（39 + 31 + 33 = 103 tests）；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `clients/mac/app/src-tauri`: `cargo test --workspace` 通过（56 tests）；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `npm run tauri -- build --bundles app`：通过，产出用于本轮 QA 的 App bundle。

修复后的 About 测试和文案已出现在集成工作树中；本轮只新增独立验收报告与截图，未改产品代码，也未触发更新检查或诊断导出操作。
