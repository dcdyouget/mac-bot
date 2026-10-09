# Mac Bot

自托管版 Grok Bot：在你自己常开的 Apple Silicon Mac 上运行一支 Bot 团队。每个 Bot 有名字、职责和独立工作间，Bot 之间可以拉群、互发消息、交接任务。你日常只和主 Bot 对话，它负责拉群、派活、提醒你验收；macOS / Android / iOS 客户端（Windows 以后支持）用任意 IP 或域名加访问密码就能连上。支持自定义模型、长期记忆、子代理、技能，以及用系统 Chrome 的登录状态操作网页。

| 端 | v1 | 以后 |
|----|----|------|
| **Server**（`macbotd`，Rust，无界面守护进程，JSON 文件存储） | macOS（Apple Silicon） | — |
| **Client** 桌面（Rust + GPUI） | macOS | Windows |
| **Client** 移动（Kotlin + Compose Multiplatform） | Android、iOS | |

## 文档

每类规则只有一个权威来源：

| 文档 | 内容 |
|------|------|
| [docs/PLAN.md](docs/PLAN.md) | 技术方案和**唯一的开发计划**（第 0 章是文档地图） |
| [docs/DESIGN.md](docs/DESIGN.md) | 产品行为与界面（含线框图） |
| [docs/PROTOCOL.md](docs/PROTOCOL.md) | 客户端 ↔ 服务端协议（全部字段） |
| [docs/REFERENCES.md](docs/REFERENCES.md) | 参考的开源项目和依赖库 |
| [AGENTS.md](AGENTS.md) · [docs/AGENT_PROMPTS.md](docs/AGENT_PROMPTS.md) | 多 agent 并行开发的约定和启动 prompt |

## License

[MIT](LICENSE)
