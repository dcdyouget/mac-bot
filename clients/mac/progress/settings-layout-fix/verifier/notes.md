# 设置主内容区修复：独立验证

状态：通过。前端、Rust workspace 门槛和独立 GUI 核验均完成。

## 设计核对

依据 `design/prototype/mac.html` 的 `settingsShell`、`set_general`、`set_memory`、`set_about`，设置应保留应用侧栏，在主区显示 260px 左侧设置导航和右侧标题/内容区。`design/03-macos.md` 第 99、126、172-186 节规定内容宽度、通用/About 页面和用户记忆页面行为。

审查集成差异：设置覆盖层改为主区布局，导航项切换仍留在设置；记忆页挂入设置内容区并保持 Host 范围加载/事件刷新；About 页面位于设置内。

## 前端门槛

- `npm run build`：通过（Vite 提示入口 bundle 超过 500KB）。
- `npm run lint`：通过。
- `npm run test:memory`：2/2 通过。
- `npm run test:summary`：2/2 通过。
- `npm run test:i18n-keys`：1/1 通过。

## Rust 门槛

- `git diff --check`（仅设置布局相关的 App.tsx/CSS）：通过。
- `cargo test --workspace`：通过，39 core + 31 desktop + 33 desktop-services 测试通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。Cargo 报告依赖 `block 0.1.6` 有未来版本兼容提醒，不影响本次 gate。

## 独立 GUI 核验

- 使用新构建的 `MacBotQA.app`（Bundle ID `bot.mac.desktop.qa`），客户端数据目录 `/tmp/macbot-settings-layout-qa-20261012/data`，file 凭据后端；连接独立 mock `127.0.0.1:7790`（`MACBOT_HOME=/tmp/macbot-settings-layout-qa-20261012/mock`）。测试结束后已退出 App 并停止 mock。
- 连接后进入设置，确认应用侧栏仍在；设置占据主内容区；左侧设置导航在通用、用户记忆和 About 间切换时保持打开。AX 树分别显示「用户记忆」档案/用量/记忆列表，以及 About 版本信息和两个操作按钮。
- 按 `mac.html` 的 `settingsShell`、`set_general`、`set_memory`、`set_about` 对照：两栏导航与内容区、52px 标题栏、通用分组卡片、用户记忆档案和条目列表、About 居中图标/版本/操作均可见。四种外观下内容区与导航都保留；透明材质开启时主区不显示底下聊天文字，关闭时表面不透明。
- 点击设置页「关闭」后 AX 树恢复总管私聊及消息列表，已保存 `chat-after-close.png`。
- 每个页面都覆盖浅/深 × 半透明/不透明，共 12 张截图，另有关闭返回聊天截图。`packaging/screenshot-window.sh` 的隐藏中间文件 `.development-mock.png` 写入失败；直接用 `screencapture -x -l 66766` 截取相同 QA 窗口成功，图片均为 2784×1864 PNG。

截图：

- 通用：`general-light-translucent.png`、`general-dark-translucent.png`、`general-light-opaque.png`、`general-dark-opaque.png`
- 用户记忆：`memory-light-translucent.png`、`memory-dark-translucent.png`、`memory-light-opaque.png`、`memory-dark-opaque.png`
- About：`about-light-translucent.png`、`about-dark-translucent.png`、`about-light-opaque.png`、`about-dark-opaque.png`
- 返回聊天：`chat-after-close.png`
