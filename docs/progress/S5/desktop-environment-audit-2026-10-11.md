# Desktop environment audit (2026-10-11)

结论：当前证据不支持把事件桥接处理饥饿或 16/16ms batch 调参认定为根因；窗口遮挡/GPUI macOS display-link 未运行仍是候选环境因素，尚未证实为客户端缺陷。

证据：

- 运行实例为 `/Applications/MacBot.app/Contents/MacOS/macbot-desktop`，PID 50057。只读采样保存在 `/tmp/macbot-desktop_2026-10-11_005640_7Fvs.sample.txt`。3 秒采样中主线程持续位于 `NSApplication nextEventMatchingMask` → CoreFoundation run loop → `mach_msg`，未见 Rust event bridge 占用；进程 CPU 约 0.5%。
- `CGWindowListCopyWindowInfo` 显示 Mac Bot 主窗口 ID 62655，`onscreen=true`、`alpha=1`、`1280x853`；AX 只读状态为 `visible=true`、`AXMinimized=false`、`AXFocused=true`，但应用 `frontmost=false`。
- 同一窗口列表发现 BetterDisplay 两个全屏 alpha=1 的高层窗口，layer `2147483629`。它们位于 Mac Bot 之上，可能造成 AppKit occlusion；`CGWindow onscreen` 和 AX 可见状态不能替代进程内 `NSWindow.occlusionState`，因此只能记录为候选。
- GPUI 0.3.8 源码显示 `AsyncApp::refresh` 只刷新 dirty window；Apple 平台默认 `frame_waker=None`、`schedule_frame()` 空实现；macOS `start_display_link` 在 `NSWindowOcclusionStateVisible` 缺失时停止 display link。相关源码位于 cargo registry 的 `gpui-pre-0.3.8/src/app/async_context.rs:150`、`gpui-pre-0.3.8/src/window.rs:2650`、`gpui-pre-apple-0.3.8/vendor/gpui/src/platform.rs:1205,1224`、`gpui-pre-macos-0.3.8/src/window.rs:834`。

处理：本轮仅撤销未提交的 `clients/mac/crates/macbot-desktop/src/app.rs` 16/16 实验 patch；已提交的 9965/bdc 内容未改动，也未重新构建、部署或控制 UI。下一步应在无 BetterDisplay 遮挡且 Mac Bot 真正前台的条件下复测，再决定是否需要平台级修复。
