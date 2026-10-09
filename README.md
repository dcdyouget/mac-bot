# Mac Bot

自托管版 Grok Bot：在你自己常开的 Apple Silicon Mac 上运行一支 Bot 团队。每个 Bot 有名字、职责和独立工作间，Bot 之间可以拉群、互发消息、交接任务。macOS / Android / iOS 客户端（Windows 放到 v2）填入 `host:port` 和访问密码后，就可以随时给它们派活。支持自定义模型、长期记忆，后续还会支持用系统浏览器操作网页。

| 端 | v1 | 以后 |
|----|----|------|
| **Server**（`macbotd`，Rust，无界面守护进程，JSON 文件存储） | macOS（Apple Silicon） | — |
| **Client** 桌面（Rust + GPUI） | macOS | Windows |
| **Client** 移动（Kotlin + Compose Multiplatform） | Android、iOS | |

> 🚧 规划阶段：
> - [docs/DESIGN.md](docs/DESIGN.md)：组件、界面与交互设计（含线框图）
> - [docs/PLAN.md](docs/PLAN.md)：架构、数据模型、里程碑
> - [docs/PROTOCOL.md](docs/PROTOCOL.md)：客户端 ↔ 服务端协议契约
> - [docs/REFERENCES.md](docs/REFERENCES.md)：各组件参考的开源项目
> - [AGENTS.md](AGENTS.md)：多 agent 并行开发约定 · [docs/AGENT_PROMPTS.md](docs/AGENT_PROMPTS.md)：各开发线的启动 prompt

## License

[MIT](LICENSE)
