# macbotd 服务端

`server/` 是 macOS Apple Silicon 服务端 workspace，包含 JSON 文件存储、durable runtime、providers/tools/skills/memory/orchestrator、浏览器 sidecar、usage ledger 和 axum gateway。二进制 crate `macbotd` 同时提供守护进程与本机 CLI。

数据目录默认为 `~/MacBot`，可用 `MACBOT_HOME` 覆盖。正式服务默认监听 `0.0.0.0:7788`；mock 服务默认监听 `0.0.0.0:7789`。mock 与正式服务必须使用不同的数据目录，Store 的单写锁会拒绝同目录的第二个进程。

## 编译和运行

在仓库根目录执行：

```sh
cargo build --manifest-path server/Cargo.toml -p macbotd

# mock：不调用真实模型，供客户端联调
MACBOT_HOME=/tmp/macbot-mock \
  cargo run --manifest-path server/Cargo.toml -p macbotd -- \
  --mock --port 7789 --password dev

# 正式服务：provider.create 之后才连接真实模型
cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 7788
```

`--port`、`--password`、`--home` 可覆盖默认值；`--home` 优先于 `MACBOT_HOME`。`--password` 只在 `data/auth.json` 不存在时初始化，不覆盖已有密码。已有密码可用 `macbotd passwd --password <password>` 重置；密码只保存为 Argon2 哈希，文件权限为 0600。首次设置密码前，除健康检查外的请求只接受 loopback；`/api/v1/health` 无需鉴权。

## HTTP/WebSocket

- `GET /api/v1/health`：公开健康检查。
- `POST /api/v1/rpc`：JSON RPC，使用 `Authorization: Bearer <password>`。
- `GET /ws`：控制连接，支持 query `?token=<password>` 或 Bearer header。
- `GET /ws/screen`：画面连接，同样需要鉴权；二进制帧为 4 字节大端 JSON 头加 JPEG，客户端 ACK 后才发送下一帧。
- `/api/v1/files`、`/api/v1/uploads`、`/api/v1/usage/export.csv`：鉴权后的文件与用量接口。
- `/admin`：首次设置密码和管理入口；已设置密码后使用 HTTP Basic Auth。

正式模式的 provider 配置通过 `provider.create` 写入，API key 只进入 macOS Keychain，不写入仓库或配置文件。开发和测试使用 `--mock`。

## CLI

```sh
macbotd status
macbotd passwd --password 'new-password'
macbotd logs [-f]
macbotd restart
macbotd update
```

当前 CLI 行为：`status` 请求本机 `7788` 健康接口；`passwd` 更新 `MACBOT_HOME/data/auth.json`；`logs` 读取 `MACBOT_HOME/data/macbot.log`；`restart` 请求当前用户的 `com.macbot.server` LaunchAgent；`update` 执行 `packaging/update.sh`。安装后可将同一二进制另名为 `macbot`。CLI 不依赖远程密码鉴权。

## LaunchAgent 和分发包

源码安装会为当前用户构建 release，安装无界面的 `~/Applications/MacBot Server.app`（`LSUIElement=true`）、`~/.local/bin/macbotd`/`macbot`，并加载 `~/Library/LaunchAgents/com.macbot.server.plist`：

```sh
MACBOT_HOME="$HOME/MacBot" server/macbotd/packaging/install-launchagent.sh
server/macbotd/packaging/uninstall-launchagent.sh
```

更新脚本先 fast-forward 当前 checkout，再构建 release 并重载 LaunchAgent：

```sh
server/macbotd/packaging/update.sh
```

构建 `.pkg`（需要 Xcode Command Line Tools 的 `pkgbuild`）：

```sh
server/macbotd/packaging/build-pkg.sh target/MacBot-Server.pkg
```

`.pkg` 安装 `/Applications/MacBot Server.app`，由 postinstall 复制到当前登录用户的 `~/Applications`、创建用户级 CLI，并通过 `launchctl bootstrap gui/<uid>` 注册 LaunchAgent；没有登录用户时只安装文件。
