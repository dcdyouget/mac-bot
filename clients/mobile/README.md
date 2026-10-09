# clients/mobile/：Android 客户端（Kotlin Multiplatform + Compose Multiplatform）

**负责人：client-android。** v1 只有 Android target；iOS 以后再加（届时增加 iOS target 和 `iosApp/` 外壳，`commonMain` 的代码直接复用）。

- 包名和 applicationId：`bot.mac.mobile`
- 代码组织见 [AGENTS.md](../../AGENTS.md) 第 1 节；界面以 [DESIGN.md](../../docs/DESIGN.md) 第 5 章为准；协议以 [PROTOCOL.md](../../docs/PROTOCOL.md) 为准。
- 开发和演示使用本机的 Android 模拟器 `macbot_api36`（Android 16 / API 36、arm64）：`$ANDROID_HOME/emulator/emulator -avd macbot_api36`。模拟器里访问 Mac 本机用 `10.0.2.2`（mock：`10.0.2.2:7789`；正式服务：`10.0.2.2:7788`）。
- 编译、安装（adb install）、运行的命令由 client-android 写在这里。
