# server/：macbotd（负责人：server-mac）

独立的 Cargo workspace，只支持 macOS（Apple Silicon）。设计见 [PLAN.md](../docs/PLAN.md) 第 5 章，协议见 [PROTOCOL.md](../docs/PROTOCOL.md)。

```
server/
├── crates/
│   ├── macbot-store         # JSON 文件存储（JSONL 日志 + 快照、文件锁、内存索引）
│   ├── macbot-durable       # Entry / Commit / Job / Inbox / Resume / Compaction
│   ├── macbot-providers     # 模型接入（OpenAI 兼容 / Anthropic …）与模型目录
│   ├── macbot-tools         # 工具框架和内置工具
│   ├── macbot-skills        # 技能扫描、索引、加载
│   ├── macbot-memory        # 三种记忆、上下文组装、压缩、整理（只在服务端）
│   ├── macbot-orchestrator  # 主 Bot、群、任务流转、路由、并发调度
│   ├── macbot-browser       # agent-browser sidecar：每个 Bot 一个会话、按任务分标签页、画面
│   ├── macbot-usage         # 用量记账和汇总
│   └── macbot-gateway       # axum：/ws（含画面）、/api/v1、/admin
└── macbotd/                 # 二进制：守护进程 + CLI（macbot）+ --mock
```

数据目录：`~/MacBot/`（开发时可以用 `MACBOT_HOME` 环境变量指向别的目录）。
