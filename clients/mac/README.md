# clients/mac：macOS 桌面客户端

这是独立的 Cargo workspace，包含 `macbot-client-core` 和 `macbot-desktop`。运行时界面使用 Rust + GPUI，组件只依赖与之匹配的 `gpui-kit` 0.7；协议类型通过 path 依赖引用 `../../protocol/rust`。

界面和交互遵循 [DESIGN.md](../../docs/DESIGN.md) 第 4 章，协议遵循 [PROTOCOL.md](../../docs/PROTOCOL.md)。编译需要 Xcode 的 Metal 工具链（本机已安装）。

当前发布范围：打包、截图、连接配置、Host persistence 和 update helper 已具备可调用实现；主应用的页面/RPC 接线仍由 client-mac 集成线推进，S0 需要先用 server-mac 的 mock 联调验收。README 中的 update 安装流程描述 helper 契约，不代表设置页按钮已经完成端到端接通。

## 编译、测试和检查

在仓库根目录执行：

```sh
cd clients/mac
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

只运行桌面程序时（不设置环境变量会显示连接页，由用户填写 Host 和密码）：

```sh
cd clients/mac
MACBOT_HOST=127.0.0.1:7789 MACBOT_PASSWORD=dev cargo run -p macbot-desktop
```

客户端不会在未设置环境变量时自动连接 mock；下面的命令是显式启动 mock 的示例。正式本机服务使用 `MACBOT_HOST=127.0.0.1:7788`。Host 也可以是任意 IP、域名或 `ws(s)://` 地址。mock 服务由 server-mac 提供，例如：

```sh
macbotd --mock --port 7789 --password dev
```

## 打包和运行

`packaging/package.sh` 会编译桌面 crate，并生成 `clients/mac/dist/MacBot.app` 与 `clients/mac/dist/MacBot.dmg`。DMG 同时包含 `Applications` 拖拽安装链接。debug 包使用 `debug` 配置：

```sh
cd clients/mac
packaging/package.sh release       # 编译、生成 .app 和 .dmg
packaging/package.sh debug app     # 只生成 debug .app
SKIP_BUILD=1 packaging/package.sh release all  # 复用已有 target/release 二进制
open dist/MacBot.app
```

可选签名：`CODESIGN_IDENTITY="Developer ID Application: ..." packaging/package.sh release all`；本地临时签名使用 `MACBOT_ADHOC_SIGN=1`。未设置签名变量时保留未签名 bundle，便于开发机直接查看。`Info.plist` 的 Bundle ID 是 `bot.mac.desktop`，可执行文件名是 `macbot-desktop`。

也可以直接运行 bundle 或 target 二进制，并覆盖连接配置：

```sh
MACBOT_HOST=127.0.0.1:7789 MACBOT_PASSWORD=dev packaging/run.sh debug
MACBOT_HOST=127.0.0.1:7788 MACBOT_PASSWORD="$MACBOT_PASSWORD" packaging/run.sh release
# 不设置 MACBOT_HOST/MACBOT_PASSWORD：打开连接页
packaging/run.sh debug
```

## 截图和更新检查

窗口截图脚本使用 CoreGraphics 查找可见的 Mac Bot 窗口，再调用系统 `screencapture`；默认写入 `clients/mac/progress/S5/macbot-window.png`：

```sh
packaging/screenshot-window.sh
packaging/screenshot-window.sh progress/S5/connect.png "Mac Bot"
```

截图默认最多等待 15 秒；可用 `MACBOT_SCREENSHOT_TIMEOUT=30` 调整，系统未授予 Screen Recording 权限时会返回错误而不是一直挂起。

截图脚本输出的 `clients/mac/progress/` 可直接纳入阶段打卡提交；`dist/` 仍是本地产物目录，不提交。

更新清单使用下面的 JSON 结构，`sha256` 必须是下载文件的完整 SHA-256。更新地址和 artifact 地址默认只允许 HTTPS；本机联调允许 `http://localhost`、`http://127.0.0.1` 或 `http://[::1]`。没有设置 `MACBOT_UPDATE_URL` 时检查状态为 `disabled`，这是预期的未配置状态。

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

`UpdateClient::download_and_stage` 会先下载并校验 digest，再生成缓存目录中的可复核 `install-<version>.sh`。只有设置页在用户点击安装后才应执行该脚本；脚本会重新校验 SHA-256，以只读方式挂载 DMG，校验 `bot.mac.desktop` 和版本，使用 `ditto` 写入目标父目录，等待旧进程退出后原子替换，并在替换失败时恢复应用备份。备份只保留应用 bundle，不删除其他用户文件。若程序是直接运行的 Rust binary 而不是 `.app`，不会生成替换脚本；应打开已校验的 DMG 手动安装。

当前状态：`.app`/`.dmg` 打包、DMG 安装脚本、manifest 校验、截图入口已集成；更新地址尚未发布时保持 `MACBOT_UPDATE_URL` 未配置，检查结果为 `disabled`。设置页的“检查更新/下载并安装”事件由主应用接通后才会触发上述 Rust helper，README 中的命令可先独立验证打包和本机测试清单。
