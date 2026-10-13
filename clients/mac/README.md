# clients/mac：macOS 桌面客户端

## Tauri 2 + React 客户端

界面工程位于 `app/`：Tauri 2 负责窗口与 Rust bridge，React/TypeScript/Vite 负责界面；bridge 复用 `crates/macbot-client-core`，连接状态和协议事件保留在 Rust。客户端不会启动 mock；先在终端启动服务端 mock，再在客户端连接页输入地址和密码。

```sh
# 从仓库根目录启动隔离的 mock 数据目录
MACBOT_HOME=/tmp/macbot-mock cargo run --manifest-path server/Cargo.toml -p macbotd -- --mock --port 7789 --password dev
```

开发运行与前端检查：

```sh
cd clients/mac/app
npm ci
npm run tauri -- dev        # 启动 Vite、编译 Rust bridge 并打开桌面窗口
npm run lint                # ESLint
npm run build               # TypeScript 检查和 Vite 前端构建
npm run tauri -- build      # Tauri 原始 release bundle，位于 app/src-tauri/target/release/bundle/
```

首次连接 mock 时，在连接页输入 `127.0.0.1:7789` 和密码 `dev`。正式服务的默认端口是 `7788`；地址支持任意 `host:port`、IP、域名及 `ws://`/`wss://`。Tauri UI 不读取旧 GPUI 使用的 `MACBOT_HOST`、`MACBOT_PASSWORD` 环境变量。

正式 `.app` 和 `.dmg` 使用仓库打包脚本生成：

```sh
cd clients/mac
packaging/package-tauri.sh
open dist/MacBot.dmg             # 在打开的 DMG 中将 MacBot.app 拖到 Applications
open /Applications/MacBot.app
# 也可直接打开本地产物：open dist/MacBot.app
```

`packaging/package-tauri.sh` 运行 `npm ci` 和 Tauri release 构建，先让 Tauri 生成 app bundle，再复制为 `clients/mac/dist/MacBot.app` 并用系统 `hdiutil` 生成带 Applications 拖拽链接的压缩 DMG `clients/mac/dist/MacBot.dmg`。默认使用 ad hoc 签名，签名 identifier 从 `Info.plist` 读取并固定为 bundle ID `bot.mac.desktop`，避免通知授权随临时构建路径变化；设置 `CODESIGN_IDENTITY="Developer ID Application: …"` 可指定签名身份。该脚本不执行 notarization。直接运行 `npm run tauri -- build` 的原始 bundle 在 `app/src-tauri/target/release/bundle/`，其 app 名称为 `Mac Bot.app`；交付路径以 `dist/` 中的 `MacBot.app` 和 `MacBot.dmg` 为准。

Tauri bridge 使用 `macbot-desktop-services` 读写 Host、凭据、本地偏好和待发 outbox；待发消息使用稳定 request ID，连接恢复后重试。React 已接入 Host 连接/管理、协议请求、通用设置（外观、字号、通知分类、提示音、登录启动、菜单栏驻留）、附件、电脑画面、轨迹及设置页的更新检查和诊断导出。主题和材质由 Rust 本地偏好恢复，但主题、材质及所选会话也会写入 WebView `localStorage`；所选会话目前仍以 WebView 存储为准。Tauri 通知按本机偏好和 `message.created` / `approval.requested` 筛选；已用打包 `.app` 验证后台 mock DM 可进入 Notification Center。系统通知首次请求 macOS 授权；无自定义签名身份时，打包脚本使用稳定的 bundle ID 作为 ad-hoc 签名 identifier。提示音以唯一音源 `design/sounds/04-bubble-pop.wav` 为准。

兼容与数据位置：正式 Bundle ID 保持 `bot.mac.desktop`；`packaging/Info.plist` 注册 `macbot://` 消息链接 scheme。默认本地数据根为 `~/Library/Application Support/MacBot`，`MACBOT_CLIENT_DATA_DIR` 可用绝对路径覆盖。Host 元数据在 `hosts.json`；连接密码存于 `host-passwords.enc`，由同目录中权限为 0600 的随机密钥文件 `host-passwords.key` 使用 AES-256-GCM 加密，父目录权限为 0700。Tauri 服务复用旧 GPUI 的 Host 元数据、状态缓存、outbox 和 Rust 本地偏好格式；Host 密码不再使用 Keychain，也不会迁移旧 Keychain 项目，因此升级后需重新连接并保存密码。WebView 的所选会话仍单独存在 `localStorage`，并非旧 Rust 偏好的兼容数据。

隔离 QA 只需设置独立数据目录：`open -n --env MACBOT_CLIENT_DATA_DIR=/tmp/macbot-client-qa dist/MacBot.app`；该变量必须是绝对路径。客户端始终使用上述加密 vault，无需选择凭据后端。不要把真实密码传入命令行、日志或截图。下文的 `packaging/package.sh`、`packaging/run.sh` 针对旧 GPUI 客户端。

以下 Cargo workspace、`packaging/run.sh` 和 `packaging/package.sh` 命令针对保留的旧 GPUI 客户端；Tauri 使用独立 Cargo workspace 和 `packaging/package-tauri.sh`。

这是独立的 Cargo workspace，包含 `macbot-client-core`、旧 `macbot-desktop` 和 `macbot-desktop-services`。旧运行时界面使用 Rust + GPUI；Tauri bridge 位于独立 workspace `app/src-tauri/`。

界面和交互遵循 [DESIGN.md](../../docs/DESIGN.md) 第 4 章，协议遵循 [PROTOCOL.md](../../docs/PROTOCOL.md)。编译需要 Xcode 的 Metal 工具链（本机已安装）。

桌面主题 token、消息块与交互细则同步采用新版 `design/` 规范。外观支持浅色、深色和跟随系统，并提供遵循 macOS「降低透明度」设置的半透明材质开关。唯一提示音源为 `design/sounds/04-bubble-pop.wav`；通知 API 使用 `afconvert` 转出的 `BubblePop.caf`（IMA4），位于 App 的 `Contents/Resources/` 根目录。远程 Home 浏览使用 Host 的文件 API；客户端没有远程打开 Finder 的能力，因此提供路径复制。

Tauri 诊断导出包含应用/系统版本、连接状态、当前 Host 名称、脱敏后的 Host 地址，以及最近客户端连接/传输错误类别；不包含实体数量、消息正文或凭据。旧 GPUI 客户端的通知偏好保存在本机私有设置目录，登录时启动通过用户 LaunchAgent 实现；GPUI 只支持整窗系统模糊，降低透明度开启时半透明主题自动退回不透明。

macOS App 注册 `macbot://` URL scheme。消息链接格式为 `macbot://message/<message_id>?chat_id=<chat_id>&host_id=<node_id>`；打开后，客户端等待 Host 连接并核对 `host_id`，匹配时加载会话历史并定位消息，不匹配时显示提示且不会跳到其他 Host。链接在连接前到达时会排队；客户端通过 `chat.history.before_seq` 最多向前加载 20 页（最多 2,000 条），找到消息、到达历史起点或达到上限时结束并显示对应状态。此 scheme 由打包后的 `.app` 的 `Info.plist` 注册，直接运行可执行文件不会向系统注册。

S0 已在 main 安装版完成桌面与 Android 的 mock 联调。客户端已接入私聊、全部消息块、轨迹、群协作、工作台、Bot 管理、技能、仪表盘、搜索、Agent Computer 和定时任务。后续阶段的客户端原生验收证据见 `progress/S1` 至 `progress/S5`。旧 GPUI 更新器在未配置 manifest URL 时为 `disabled`；Tauri 设置页固定查询 GitHub Releases latest API，GitHub 返回 404 时显示 `unsupported`。本地产出的 DMG 已通过独立目录内的安装替换测试；正式签名尚未配置。

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

连接密码始终写入本地加密 vault：`host-passwords.enc` 保存 AES-256-GCM 密文，`host-passwords.key` 保存由系统安全随机源生成的 32 字节本地密钥；两者及锁文件权限为 0600，数据目录权限为 0700。密文头包含格式版本和每次写入新生成的 12 字节 nonce，认证失败或未知版本会拒绝读取。更新通过同目录临时文件、同步和原子替换完成。该方案不调用 macOS Keychain，首次切换不会尝试迁移旧 Keychain 密码；清空或更换数据目录后需要重新连接 Host。

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

## 旧 GPUI 截图和 manifest 更新检查

本节的 `MACBOT_UPDATE_URL`、`update.json`、manifest 校验和替换脚本仅属于旧 GPUI 更新器，不配置 Tauri 设置页。Tauri 更新检查固定调用 GitHub Releases latest API；API 返回 404 时状态为 `unsupported`，网络或其他 HTTP 错误会显示检查失败。

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

旧 GPUI 的 `UpdateClient::download_and_stage` 会先下载并校验 digest，再生成缓存目录中的可复核 `install-<version>.sh`。该客户端的设置页检查、下载和安装事件已接线；只有用户点击安装后才执行脚本。脚本会重新校验 SHA-256，以只读方式挂载 DMG，校验 `bot.mac.desktop` 和版本，使用 `ditto` 写入目标父目录，等待旧进程退出后原子替换，并在替换失败时恢复应用备份。备份只保留应用 bundle，不删除其他用户文件。重开 replacement 时只通过 `open --env` 透传已设置的 `MACBOT_CLIENT_DATA_DIR`；不会透传密码、Host 或用户 API key。若程序是直接运行的 Rust binary 而不是 `.app`，不会生成替换脚本；应打开已校验的 DMG 手动安装。旧 GPUI manifest 尚未配置公开发布 URL；正式签名/notarization 也未配置；本机生成的实际 DMG 已完成独立目录内的替换验收。

更新比较使用运行中 `.app/Contents/Info.plist` 的 `CFBundleShortVersionString`；直接运行 target binary 时回退到 `CARGO_PKG_VERSION`。可对现有 DMG 做隔离替换验收（测试会在系统临时目录创建 `.app`、设置 `MACBOT_TEST_UPDATE_NO_OPEN=1`，不打开应用）：

```sh
MACBOT_TEST_UPDATE_DMG="$PWD/dist/MacBot.dmg" \
  cargo test -p macbot-desktop isolated_dmg_install_when_requested -- --nocapture
```

该测试只在显式设置 `MACBOT_TEST_UPDATE_DMG` 时执行，验证 SHA-256、Bundle ID、版本、原子替换和旧 bundle 备份；不会触碰 `~/Applications` 或工作树应用。独立替换测试已验证本机实际 DMG，测试模式跳过重新打开 GUI；公开源下载和真实安装版重启仍由发布集成验收。

旧 GPUI 当前状态：`.app`/`.dmg` 打包、DMG 安装脚本、manifest 校验、截图入口和设置页更新入口已集成；manifest 地址未配置时 `MACBOT_UPDATE_URL` 留空，旧 GPUI 检查结果为 `disabled`。README 中的命令用于独立验证该旧客户端的配置和脚本。

## 原生验收与性能

`progress/` 的开发窗口截图来自本 worktree；独立 mock 测试使用已发布服务端 bundle、独立数据目录和 `127.0.0.1:7790`，不修改共享 `7789` 或 Android 会话。测试记录必须注明服务端 source commit、客户端 commit、窗口 PID 和连接地址。

消息与轨迹使用虚拟列表并测量可见行高度；Host 缓存按捕获顺序异步写入，断线待发消息保存稳定请求 ID。`macbot-client-core/examples/state_bench.rs` 提供大状态合并基准（`cargo run -p macbot-client-core --example state_bench --release`）；结果是状态层基准，不代表 GUI 帧率。

常用快捷键：`⌘0` 秘书长，`⌘1…9` 已固定会话，`⌘N` 新建，`⌘K` 搜索，`⌘,` 设置，`⌘⇧W/U/S` 工作台/仪表盘/技能，`⌘\` 侧栏，`⌘⇧\` 上下文，`Esc` 返回，`⌘Q` 退出。

“关闭窗口后留在菜单栏”可在“设置 → 通用 → 启动”切换，默认关闭，并保存在 `local-menubar.json`；保存后对后续关窗立即生效。启用后关闭主窗口会隐藏窗口并保留会话；菜单栏左键菜单提供打开和退出，Dock 在没有可见窗口时会重新打开或重建主窗口。TrayIcon 安装失败时，关闭主窗口会正常关闭而不会被隐藏。上述生命周期已实现，但真实 macOS GUI 操作尚未验收。

独立 QA 必须同时使用独立 Bundle ID 和客户端数据目录：

```sh
open -n --env MACBOT_CLIENT_DATA_DIR=/tmp/macbot-client-qa-data \
  --env MACBOT_HOST=127.0.0.1:7790 --env MACBOT_PASSWORD=dev \
  dist/MacBotQA.app
```

`MACBOT_CLIENT_DATA_DIR` 必须为绝对路径，覆盖 Host 列表与当前 Host 恢复所用记录、加密连接凭据 vault 和密钥、会话状态缓存、待发 outbox、本机偏好、更新配置和更新缓存。未设置时仍使用正常 Application Support 路径；`MACBOT_UPDATE_CACHE` 显式覆盖更新缓存时优先。QA 使用独立数据目录，因此不会共享正式安装版的连接凭据；开机启动等系统设置仍由系统管理，QA 不应启用。仅修改测试 bundle 的 `CFBundleIdentifier` 为 `bot.mac.desktop.qa`，正式产物继续使用 `bot.mac.desktop`。

独立 QA bundle 同时将 `CFBundleName` 和 `CFBundleDisplayName` 设为 `MacBotQA`，这样 CoreGraphics 窗口 owner 与正式安装版可区分；截图时使用 `packaging/screenshot-window.sh progress/S5/qa.png MacBotQA <PID>`，或通过 `MACBOT_WINDOW_PID=<PID>` 过滤该进程。
