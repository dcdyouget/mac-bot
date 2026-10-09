# clients/mac：macOS 桌面客户端

这是独立的 Cargo workspace，包含 `macbot-client-core` 和 `macbot-desktop`。运行时界面使用 Rust + GPUI，组件只依赖与之匹配的 `gpui-kit` 0.7；协议类型通过 path 依赖引用 `../../protocol/rust`。

界面和交互遵循 [DESIGN.md](../../docs/DESIGN.md) 第 4 章，协议遵循 [PROTOCOL.md](../../docs/PROTOCOL.md)。编译需要 Xcode 的 Metal 工具链（本机已安装）。

S0 已在 main 安装版完成桌面与 Android 的 mock 联调。客户端已接入私聊、全部消息块、轨迹、群协作、工作台、Bot 管理、技能、仪表盘、搜索、Agent Computer 和定时任务。后续阶段的客户端原生验收证据见 `progress/S1` 至 `progress/S5`；真实模型执行和两端场景验收由集成线依赖服务端推进。自动更新未配置 manifest URL 时为 disabled；本地产出的 DMG 已通过独立目录内的安装替换测试，公开发布源与签名尚未配置。

## 编译、测试和检查

在仓库根目录执行：

```sh
cd clients/mac
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

上述命令是集成线的验证入口；本 README 不宣称它们在每次改动后都已运行通过。

只运行桌面程序时（不设置环境变量时优先恢复已保存 Host；无记录时显示连接页）：

```sh
cd clients/mac
MACBOT_HOST=127.0.0.1:7789 MACBOT_PASSWORD=dev cargo run -p macbot-desktop
```

客户端不会在未设置环境变量时自动连接 mock；下面的命令是显式启动 mock 的示例。正式本机服务使用 `MACBOT_HOST=127.0.0.1:7788`。Host 也可以是任意 IP、域名或 `ws(s)://` 地址。mock 服务由 server-mac 提供，例如：

```sh
macbotd --mock --port 7789 --password dev
```

开发期凭据后端：默认使用 macOS Keychain。若本机开发环境无法使用 Keychain，可显式设置 `MACBOT_SECRET_BACKEND=file`；此时密码写入仓库外的 `~/Library/Application Support/MacBot/development-secrets.json`，文件权限 0600、父目录权限 0700，并通过原子替换更新。该开关只用于开发联调，日志、截图和 Git 产物都不得包含密码；未设置开关时不会写入该文件。

## 打包和运行

`packaging/package.sh` 会编译桌面 crate，并生成 `clients/mac/dist/MacBot.app` 与 `clients/mac/dist/MacBot.dmg`。DMG 同时包含 `Applications` 拖拽安装链接。debug 包使用 `debug` 配置：

```sh
cd clients/mac
packaging/package.sh release       # 编译、生成 .app 和 .dmg
packaging/package.sh debug app     # 只生成 debug .app
SKIP_BUILD=1 packaging/package.sh release all  # 复用已有 target/release 二进制
MACBOT_SOURCE_COMMIT=<完整源码SHA> packaging/package.sh debug app  # 干净 git archive 构建
open dist/MacBot.app
```

打包支持无 `.git` 的干净 archive；集成线用 `MACBOT_SOURCE_COMMIT` 显式传入快照 SHA，写入 `Contents/Resources/source-commit`。普通 worktree 自动记录 HEAD 与未提交源码标记；没有 SHA 时写 `unknown`，不会阻断打包。

可选签名：`CODESIGN_IDENTITY="Developer ID Application: ..." packaging/package.sh release all`；本地临时签名使用 `MACBOT_ADHOC_SIGN=1`。未设置签名变量时保留未签名 bundle，便于开发机直接查看。`Info.plist` 的 Bundle ID 是 `bot.mac.desktop`，可执行文件名是 `macbot-desktop`。

也可以直接运行 bundle 或 target 二进制，并覆盖连接配置：

```sh
MACBOT_HOST=127.0.0.1:7789 MACBOT_PASSWORD=dev packaging/run.sh debug
MACBOT_HOST=127.0.0.1:7788 MACBOT_PASSWORD="$MACBOT_PASSWORD" packaging/run.sh release
# 不设置 MACBOT_HOST/MACBOT_PASSWORD：恢复已保存 Host；无记录时打开连接页
packaging/run.sh debug
```

## 截图和更新检查

窗口截图脚本使用 CoreGraphics 查找可见的 Mac Bot 窗口，再调用系统 `screencapture`；默认写入 `clients/mac/progress/S5/macbot-window.png`：

```sh
packaging/screenshot-window.sh
packaging/screenshot-window.sh progress/S5/connect.png "Mac Bot"
# 第三个参数按 PID 过滤，适合同时运行 dev/dist 和已安装实例：
packaging/screenshot-window.sh progress/S5/connect.png "Mac Bot" 12345
# 或：MACBOT_WINDOW_PID=12345 packaging/screenshot-window.sh progress/S5/connect.png
```

截图默认最多等待 15 秒；可用 `MACBOT_SCREENSHOT_TIMEOUT=30` 调整。脚本通过 CoreGraphics 的 `CGPreflightScreenCaptureAccess` 只读检查 Screen Recording 权限并报告结果，不会自动修改系统权限；未授权时 macOS 的 `screencapture` 可能超时，需在“系统设置 → 隐私与安全性 → 屏幕录制”中手动授权后重试。截图先写入同目录隐藏的 `.development-mock.png` 中间文件，成功后原子替换目标；失败只清理本次中间文件，不删除历史截图。

截图脚本输出的 `clients/mac/progress/` 可直接纳入阶段打卡提交；`dist/` 仍是本地产物目录，不提交。

更新清单使用下面的 JSON 结构，`sha256` 必须是下载文件的完整 SHA-256。更新地址和 artifact 地址默认只允许 HTTPS；本机联调允许 `http://localhost`、`http://127.0.0.1` 或 `http://[::1]`。没有设置 `MACBOT_UPDATE_URL` 时检查状态为 `disabled`，这是预期的未配置状态。

设置页保存的更新源写入 `~/Library/Application Support/MacBot/update.json`（权限 0600）；`MACBOT_UPDATE_URL` 存在时优先使用环境变量，适合集成线临时覆盖。远程 HTTP、无效 JSON、非 64 位十六进制摘要都会被拒绝。

```json
{
  "version": "0.2.0",
  "url": "https://downloads.example.test/MacBot-0.2.0.dmg",
  "sha256": "64 个十六进制字符"
}
```

例如本机临时 HTTP 服务可以这样验证连接配置：

```sh
MACBOT_UPDATE_URL=http://127.0.0.1:7789/update.json packaging/check-update.sh
```

`UpdateClient::download_and_stage` 会先下载并校验 digest，再生成缓存目录中的可复核 `install-<version>.sh`。设置页的检查、下载和安装事件已由主应用接线；只有用户点击安装后才执行脚本。脚本会重新校验 SHA-256，以只读方式挂载 DMG，校验 `bot.mac.desktop` 和版本，使用 `ditto` 写入目标父目录，等待旧进程退出后原子替换，并在替换失败时恢复应用备份。备份只保留应用 bundle，不删除其他用户文件。若程序是直接运行的 Rust binary 而不是 `.app`，不会生成替换脚本；应打开已校验的 DMG 手动安装。当前尚未配置公开发布 URL；本机生成的实际 DMG 已完成独立目录内的替换验收。

更新比较使用运行中 `.app/Contents/Info.plist` 的 `CFBundleShortVersionString`；直接运行 target binary 时回退到 `CARGO_PKG_VERSION`。可对现有 DMG 做隔离替换验收（测试会在系统临时目录创建 `.app`、设置 `MACBOT_TEST_UPDATE_NO_OPEN=1`，不打开应用）：

```sh
MACBOT_TEST_UPDATE_DMG="$PWD/dist/MacBot.dmg" \
  cargo test -p macbot-desktop isolated_dmg_install_when_requested -- --nocapture
```

该测试只在显式设置 `MACBOT_TEST_UPDATE_DMG` 时执行，验证 SHA-256、Bundle ID、版本、原子替换和旧 bundle 备份；不会触碰 `~/Applications` 或工作树应用。独立替换测试已验证本机实际 DMG，测试模式跳过重新打开 GUI；公开源下载和真实安装版重启仍由发布集成验收。

当前状态：`.app`/`.dmg` 打包、DMG 安装脚本、manifest 校验、截图入口和设置页更新入口已集成；更新地址尚未发布时保持 `MACBOT_UPDATE_URL` 未配置，检查结果为 `disabled`。README 中的命令用于独立验证配置和脚本，真实发布验收由集成线执行。

## 原生验收与性能

`progress/` 的开发窗口截图来自本 worktree；S0 正式安装证据由集成线归档于 `docs/progress/S0/`。独立 mock 测试使用已发布服务端 bundle、独立数据目录和 `127.0.0.1:7790`，不修改共享 `7789` 或 Android 会话。测试记录必须注明服务端 source commit、客户端 commit、窗口 PID 和连接地址。

消息与轨迹使用虚拟列表并测量可见行高度；Host 缓存按捕获顺序异步写入，断线待发消息保存稳定请求 ID。`macbot-client-core/examples/state_bench.rs` 提供大状态合并基准（`cargo run -p macbot-client-core --example state_bench --release`）；结果是状态层基准，不代表 GUI 帧率。

常用快捷键：`⌘0` 总管，`⌘1…9` 已固定会话，`⌘N` 新建，`⌘K` 搜索，`⌘,` 设置，`⌘⇧W/U/S` 工作台/仪表盘/技能，`⌘\` 侧栏，`⌘⇧\` 上下文，`Esc` 返回，`⌘Q` 退出。
