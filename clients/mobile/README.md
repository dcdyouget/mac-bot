# clients/mobile/：Android + iOS 客户端（Kotlin Multiplatform + Compose Multiplatform）

| 目录 | 负责人 |
|------|--------|
| `shared/src/commonMain/.../core/`、`feature/{connect,chat,group,mainbot,approval,search,routines}`、`androidApp/`、Gradle 配置 | **client-android** |
| `shared/src/commonMain/.../feature/{trace,workbench,dashboard,skills,bots,settings,computer}`、`shared/src/iosMain/`、`iosApp/` | **client-ios** |

包名为 `bot.mac.mobile`，Android 的 applicationId 为 `bot.mac.mobile`。界面以 [DESIGN.md](../../docs/DESIGN.md) 第 5 章为准，协议以 [PROTOCOL.md](../../docs/PROTOCOL.md) 为准，归属规则见 [AGENTS.md](../../AGENTS.md)。
