# Mac Bot

自托管版 Grok Bot：在你自己常开的 Apple Silicon Mac 上运行一支 Bot 团队。每个 Bot 有名字、职责和独立工作间，Bot 之间可以拉群、互发消息、交接任务。macOS / Windows / Android 客户端填入 `host:port` 并完成配对后，就可以随时给它们派活。支持自定义模型、长期记忆，后续还会支持用系统浏览器操作网页。

- 服务端：Rust（macOS, Apple Silicon），SQLite
- 桌面客户端：Rust + GPUI（macOS / Windows）
- 移动端：Kotlin + Jetpack Compose（Android）

> 🚧 规划阶段，详见 [docs/PLAN.md](docs/PLAN.md)。
