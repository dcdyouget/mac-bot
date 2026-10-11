# clients/mac：macOS 桌面客户端

## Tauri + React 迁移批次 1

新界面工程位于 `app/`：Tauri 2 负责窗口与 Rust bridge，React/TypeScript/Vite 负责界面。bridge 直接复用 `crates/macbot-client-core`，连接状态和协议事件保留在 Rust。首次连接可使用 `127.0.0.1:7789`、密码 `dev`；客户端不启动 mock，需先运行 `macbotd --mock --port 7789 --password dev`。

```sh
cd clients/mac/app
npm ci
npm run dev                 # 仅启动 Vite 前端
npm run tauri -- dev        # 编译 Rust bridge 并打开桌面窗口
npm run lint                # ESLint
npm run build               # TypeScript 检查和 Vite 前端构建
npm run tauri -- build      # 原始 Tauri release bundle，产物位于 app/src-tauri/target/release/bundle/
cd .. && packaging/package-tauri.sh  # 可复现打包到 dist/MacBot.app 与 dist/MacBot.dmg
```

Tauri bundle id 为 `bot.mac.desktop`，并引用 `packaging/Info.plist` 注册 `macbot://`。原始 Tauri bundle 位于 `app/src-tauri/target/release/bundle/`；`packaging/package-tauri.sh` 会将 `.app`、带 Applications 拖拽链接的压缩 `.dmg` 输出到 `clients/mac/dist/MacBot.app` 和 `clients/mac/dist/MacBot.dmg`。未配置 `CODESIGN_IDENTITY` 时使用 ad hoc 签名；设置该变量可指定 Apple 签名身份。该脚本不执行 notarization。下文的 `packaging/package.sh` 仍只包装旧 GPUI 可执行文件。

当前 Tauri bridge 已依赖 `macbot-desktop-services`，连接成功后将 Host 元数据和密码写入旧格式的 `hosts.json` 与 macOS Keychain；可恢复 Host、节点状态缓存和待发消息，并提供本地偏好读写、窗口材质、Host 列表/删除/连接命令。待发消息使用稳定 request ID，连接恢复后重试。Rust 命令已注册，但 React 尚未全部切换到这些命令：现有页面的主题、材质和所选会话仍有 WebView `localStorage` 读写；菜单栏启动和更新服务也尚未在 Tauri bridge 接线。

旧数据兼容：Bundle ID 是 `bot.mac.desktop`；Keychain generic-password service 是 `bot.mac.desktop.host-password`，account 使用 Host ID；默认数据根为 `~/Library/Application Support/MacBot`，`MACBOT_CLIENT_DATA_DIR` 可用绝对路径覆盖。Host 元数据保存在 `hosts.json`，密码不写入该文件。Tauri 服务 bridge 复用这些相同格式与路径，因此旧 GPUI 安装保存的 Host、Keychain 密码、状态缓存、待发消息和本地偏好可由 Tauri 端读取。该兼容性来自共享存储实现；UI 尚未接线的设置项仍不会从 React 页面呈现或修改。

以下 Cargo workspace、`packaging/run.sh` 和 `packaging/package.sh` 命令针对保留的旧 GPUI 客户端；Tauri 使用独立 Cargo workspace 和 `packaging/package-tauri.sh`。

这是独立的 Cargo workspace，包含 `macbot-client-core`、旧 `macbot-desktop` 和 `macbot-desktop-services`。旧运行时界面使用 Rust + GPUI；Tauri bridge 位于独立 workspace `app/src-tauri/`。

界面和交互遵循 [DESIGN.md](../../docs/DESIGN.md) 第 4 章，协议遵循 [PROTOCOL.md](../../docs/PROTOCOL.md)。编译需要 Xcode 的 Metal 工具链（本机已安装）。

桌面主题 token、消息块与交互细则同步采用新版 `design/` 规范。外观支持浅色、深色和跟随系统，并提供遵循 macOS「降低透明度」设置的半透明材质开关；唯一提示音 `04-bubble-pop.m4a` 会被打包到 App 的 `Contents/Resources/Sounds/`。远程 Home 浏览使用 Host 的文件 API；客户端没有远程打开 Finder 的能力，因此提供路径复制。

通知总开关和四类通知偏好保存在本机私有设置目录；诊断导出只包含客户端版本、平台、连接状态类别和实体数量，不导出 Host 地址、消息正文或凭据。登录时启动通过用户 LaunchAgent 实现。当前 GPUI 只支持整窗系统模糊，不能逐个表面设置设计中的 blur 半径和 saturate 值；半透明主题通过系统窗口材质显示，降低透明度开启时自动退回不透明。

macOS App 注册 `macbot://` URL scheme。消息链接格式为 `macbot://message/<message_id>?chat_id=<chat_id>&host_id=<node_id>`；打开后，客户端等待 Host 连接并核对 `host_id`，匹配时加载会话历史并定位消息，不匹配时显示提示且不会跳到其他 Host。链接在连接前到达时会排队；客户端通过 `chat.history.before_seq` 最多向前加载 20 页（最多 2,000 条），找到消息、到达历史起点或达到上限时结束并显示对应状态。此 scheme 由打包后的 `.app` 的 `Info.plist` 注册，直接运行可执行文件不会向系统注册。

S0 已在 main 安装版完成桌面与 Android 的 mock 联调。客户端已接入私聊、全部消息块、轨迹、群协作、工作台、Bot 管理、技能、仪表盘、搜索、Agent Computer 和定时任务。后续阶段的客户端原生验收证据见 `progress/S1` 至 `progress/S5`。自动更新未配置 manifest URL 时为 disabled；本地产出的 DMG 已通过独立目录内的安装替换测试，公开发布源与签名尚未配置。

## 编译、测试和检查

在仓库根目录执行：

```sh
cd clients/mac
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

本 README 不宣称上述命令在每次改动后都已运行通过。

仅做编译静态检查（不运行测试）：

```sh
cd clients/mac
cargo check --workspace --locked
```

只运行桌面程序时（不设置环境变量时优先恢复已保存 Host；无记录时显示连接页）：

```sh
cd clients/mac
MACBOT_HOST=127.0.0.1:7789 MACBOT_PASSWORD=dev cargo run -p macbot-desktop
```

客户端不会在未设置环境变量时自动连接 mock；下面的命令是显式启动 mock 的示例。正式本机服务使用 `MACBOT_HOST=127.0.0.1:7788`。Host 也可以是任意 IP、域名或 `ws(s)://` 地址。mock 服务由 server-mac 提供，例如：

```sh
macbotd --mock --port 7789 --password dev
```

开发期凭据后端：默认使用 macOS Keychain。若本机开发环境无法使用 Keychain，可显式设置 `MACBOT_SECRET_BACKEND=file`；此时密码写入仓库外的 `~/Library/Application Support/MacBot/development-secrets.json`，若设置 `MACBOT_CLIENT_DATA_DIR` 则改写入该目录下的 `development-secrets.json`；文件权限 0600、父目录权限 0700，并通过原子替换更新。该开关只用于开发联调，日志、截图和 Git 产物都不得包含密码；未设置开关时不会写入该文件。

## 旧 GPUI 客户端打包和运行

`packaging/package.sh` 会编译桌面 crate，并生成 `clients/mac/dist/MacBot.app` 与 `clients/mac/dist/MacBot.dmg`。DMG 同时包含 `Applications` 拖拽安装链接。debug 包使用 `debug` 配置：

```sh
cd clients/mac
packaging/package.sh release       # 编译、生成 .app 和 .dmg
packaging/package.sh debug app     # 只生成 debug .app
SKIP_BUILD=1 packaging/package.sh release all  # 复用已有 target/release 二进制
MACBOT_SOURCE_COMMIT=<完整源码SHA> packaging/package.sh debug app  # 干净 git archive 构建
open dist/MacBot.app
```

打包支持无 `.git` 的干净 archive；可以用 `MACBOT_SOURCE_COMMIT` 显式传入快照 SHA，写入 `Contents/Resources/source-commit`。普通 worktree 自动记录 HEAD 与未提交源码标记；没有 SHA 时写 `unknown`，不会阻断打包。

打包默认对完整 App 使用本机 ad hoc 签名；可用 `CODESIGN_IDENTITY="Developer ID Application: ..." packaging/package.sh release all` 指定签名身份。所有 fixtures、源码标记和 Info.plist 写入后才签名，并在生成 DMG 前对 App 及复制后的 bundle 执行 `codesign --verify --deep --strict`，失败时停止打包。`MACBOT_ADHOC_SIGN=1` 旧调用仍兼容默认行为。`Info.plist` 的 Bundle ID 是 `bot.mac.desktop`，可执行文件名是 `macbot-desktop`。

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

设置页保存的更新源写入 `~/Library/Application Support/MacBot/update.json`（权限 0600）；独立 QA 设置 `MACBOT_CLIENT_DATA_DIR` 时改用该目录下的 `update.json`。`MACBOT_UPDATE_URL` 存在时优先使用环境变量，适合临时覆盖。远程 HTTP、无效 JSON、非 64 位十六进制摘要都会被拒绝。

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

`UpdateClient::download_and_stage` 会先下载并校验 digest，再生成缓存目录中的可复核 `install-<version>.sh`。设置页的检查、下载和安装事件已由主应用接线；只有用户点击安装后才执行脚本。脚本会重新校验 SHA-256，以只读方式挂载 DMG，校验 `bot.mac.desktop` 和版本，使用 `ditto` 写入目标父目录，等待旧进程退出后原子替换，并在替换失败时恢复应用备份。备份只保留应用 bundle，不删除其他用户文件。重开 replacement 时只通过 `open --env` 透传已设置的 `MACBOT_CLIENT_DATA_DIR` 和 `MACBOT_SECRET_BACKEND`，因此 QA 数据根和显式 file 凭据后端会保留；不会透传密码、Host 或用户 API key。若程序是直接运行的 Rust binary 而不是 `.app`，不会生成替换脚本；应打开已校验的 DMG 手动安装。当前尚未配置公开发布 URL，正式签名/notarization 也未配置；本机生成的实际 DMG 已完成独立目录内的替换验收。

更新比较使用运行中 `.app/Contents/Info.plist` 的 `CFBundleShortVersionString`；直接运行 target binary 时回退到 `CARGO_PKG_VERSION`。可对现有 DMG 做隔离替换验收（测试会在系统临时目录创建 `.app`、设置 `MACBOT_TEST_UPDATE_NO_OPEN=1`，不打开应用）：

```sh
MACBOT_TEST_UPDATE_DMG="$PWD/dist/MacBot.dmg" \
  cargo test -p macbot-desktop isolated_dmg_install_when_requested -- --nocapture
```

该测试只在显式设置 `MACBOT_TEST_UPDATE_DMG` 时执行，验证 SHA-256、Bundle ID、版本、原子替换和旧 bundle 备份；不会触碰 `~/Applications` 或工作树应用。独立替换测试已验证本机实际 DMG，测试模式跳过重新打开 GUI；公开源下载和真实安装版重启仍由发布集成验收。

当前状态：`.app`/`.dmg` 打包、DMG 安装脚本、manifest 校验、截图入口和设置页更新入口已集成；更新地址尚未发布时保持 `MACBOT_UPDATE_URL` 未配置，检查结果为 `disabled`。README 中的命令用于独立验证配置和脚本。

## 原生验收与性能

`progress/` 的开发窗口截图来自本 worktree；独立 mock 测试使用已发布服务端 bundle、独立数据目录和 `127.0.0.1:7790`，不修改共享 `7789` 或 Android 会话。测试记录必须注明服务端 source commit、客户端 commit、窗口 PID 和连接地址。

消息与轨迹使用虚拟列表并测量可见行高度；Host 缓存按捕获顺序异步写入，断线待发消息保存稳定请求 ID。`macbot-client-core/examples/state_bench.rs` 提供大状态合并基准（`cargo run -p macbot-client-core --example state_bench --release`）；结果是状态层基准，不代表 GUI 帧率。

常用快捷键：`⌘0` 总管，`⌘1…9` 已固定会话，`⌘N` 新建，`⌘K` 搜索，`⌘,` 设置，`⌘⇧W/U/S` 工作台/仪表盘/技能，`⌘\` 侧栏，`⌘⇧\` 上下文，`Esc` 返回，`⌘Q` 退出。

设置页的“关闭窗口后留在菜单栏”默认关闭，并单独保存在 `local-menubar.json`；旧版本机偏好没有此设置时也按关闭处理。启用后，关闭主窗口会隐藏应用但保留会话；菜单栏的 Mac Bot 菜单可重新打开窗口或显式退出应用。若系统未能创建状态栏项目，关闭窗口沿用普通关闭行为。

独立 QA 必须同时使用独立 Bundle ID 和客户端数据目录：

```sh
open -n --env MACBOT_CLIENT_DATA_DIR=/tmp/macbot-client-qa-data \
  --env MACBOT_SECRET_BACKEND=file \
  --env MACBOT_HOST=127.0.0.1:7790 --env MACBOT_PASSWORD=dev \
  dist/MacBotQA.app
```

`MACBOT_CLIENT_DATA_DIR` 必须为绝对路径，覆盖 Host 列表与当前 Host 恢复所用记录、file 后端凭据、会话状态缓存、待发 outbox、本机偏好、更新配置和更新缓存。未设置时仍使用正常 Application Support 路径；`MACBOT_UPDATE_CACHE` 显式覆盖更新缓存时优先。QA 使用 `file` 后端，避免与安装版共享 Keychain；开机启动等系统设置仍由系统管理，QA 不应启用。仅修改测试 bundle 的 `CFBundleIdentifier` 为 `bot.mac.desktop.qa`，正式产物继续使用 `bot.mac.desktop`。

独立 QA bundle 同时将 `CFBundleName` 和 `CFBundleDisplayName` 设为 `MacBotQA`，这样 CoreGraphics 窗口 owner 与正式安装版可区分；截图时使用 `packaging/screenshot-window.sh progress/S5/qa.png MacBotQA <PID>`，或通过 `MACBOT_WINDOW_PID=<PID>` 过滤该进程。
