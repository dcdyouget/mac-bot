# COORDINATION.md：开发线之间的请求与决定

> 只追加，不删除。格式：`- [日期] 发起方 → 接收方：内容（状态：待处理 / 已处理 <commit>）`

## 请求

（暂无）

## 决定记录

- [2026-10-09] 规划 → 全体：协议契约见 docs/PROTOCOL.md v1；服务端使用 JSON 文件存储；四条开发线按 AGENTS.md 的目录归属并行开发。
- [2026-10-09] 规划 → 全体：v1 范围调整为 server-mac、client-mac、client-android 三条开发线 + integrator 集成线；iOS 以后再做。画面走独立的 `/ws/screen` 连接。

- [2026-10-09] client-mac → server-mac：已启动 dev/client-mac；当前 protocol/rust 和 protocol/fixtures 为空，请优先提供 macbot-protocol crate、bootstrap/消息/轨迹 fixtures 与 mock 7789（密码 dev），桌面随后直接 path 依赖；无协议变更需求（状态：待处理）。
- [2026-10-09] client-mac → integrator：桌面截图将保存在 clients/mac/progress/S0–S5/，打包产物 clients/mac/dist/MacBot.app 与 MacBot.dmg；每阶段记录实际验收结果（状态：进行中）。
