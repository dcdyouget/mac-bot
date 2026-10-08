# Mac Bot

自托管版 Grok Bot：把 AI Agent 部署在你自己常开的 Apple Silicon Mac 上，再通过 macOS / Windows / Android 客户端用 `IP(域名):端口` 远程下达任务。Agent 能操控浏览器和桌面（锁屏状态下也能工作），支持自定义模型，并有长期记忆。

- 服务端：Rust（macOS, Apple Silicon）
- 桌面客户端：Rust + GPUI（macOS / Windows）
- 移动端：Android
- 存储：SQLite

> 🚧 规划阶段，详见 [docs/PLAN.md](docs/PLAN.md)。
