# protocol/：客户端 ↔ 服务端契约

**负责人：server-mac**（`protocol/kotlin/` 除外，由 client-android 负责）。人读的契约文档在 [docs/PROTOCOL.md](../docs/PROTOCOL.md)。

| 目录 | 内容 | 谁来改 |
|------|------|--------|
| `rust/` | `macbot-protocol` crate：所有帧、请求、事件、对象、消息块、TraceItem 的 serde 类型。server 和 client-mac 都通过 path 依赖使用 | server-mac |
| `schema/` | 从 Rust 类型导出的 JSON Schema（生成物，需要提交到仓库） | server-mac（运行导出命令） |
| `fixtures/` | 每种对象、事件、消息块各一份示例 JSON；`scenarios/*.jsonl` 是 `macbotd --mock` 回放用的场景 | server-mac |
| `kotlin/` | 由 schema 生成 Kotlin（kotlinx.serialization）数据类的脚本，输出到 `clients/mobile/shared/.../core/protocol/` | client-android |

变更流程见 [AGENTS.md](../AGENTS.md) 第 3 节。
