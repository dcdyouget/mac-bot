# clients/mac/：macOS 桌面客户端（负责人：client-mac）

独立的 Cargo workspace：Rust + GPUI，界面组件只依赖 `gpui-kit`（它会固定匹配的 GPUI 版本），协议类型通过 path 依赖引用 `../../protocol/rust`。

界面以 [DESIGN.md](../../docs/DESIGN.md) 第 4 章为准，协议以 [PROTOCOL.md](../../docs/PROTOCOL.md) 为准。gpui-kit 有给 AI 编程助手用的 skills：`npx skills add longbridge/gpui-kit`。

编译需要 Xcode 的 Metal 工具链（这台机器上已经装好）。
