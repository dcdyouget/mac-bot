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

`--port`、`--password`、`--home` 可覆盖默认值；`--home` 优先于 `MACBOT_HOME`。`--password` 只在 `data/auth.json` 不存在时初始化，不覆盖已有密码。已有密码可用 `macbotd passwd --password <password>` 重置；密码只保存为 Argon2 哈希，文件权限为 0600。首次设置密码必须通过 loopback 的 `/admin` 完成；健康检查公开，其余接口在 `setup_required` 状态拒绝访问。`/api/v1/health` 无需鉴权。

## HTTP/WebSocket

- `GET /api/v1/health`：公开健康检查。
- `POST /api/v1/rpc`：JSON RPC，使用 `Authorization: Bearer <password>`。
- `GET /ws`：控制连接，支持 query `?token=<password>` 或 Bearer header。
- `GET /ws/screen`：画面连接，同样需要鉴权；二进制帧为 4 字节大端 JSON 头加 JPEG，客户端 ACK 后才发送下一帧。
- `/api/v1/files`、`/api/v1/uploads`、`/api/v1/usage/export.csv`：鉴权后的文件与用量接口。
- `/admin`：首次设置密码和管理入口；已设置密码后使用 HTTP Basic Auth。

管理页可修改主机名称和监听端口；端口变更写入 `data/settings.json`，重启 LaunchAgent 后生效。

正式模式的 provider 配置通过 `provider.create` 写入，API key 默认进入 macOS Keychain。开发期可显式设置 `MACBOT_SECRET_BACKEND=file`，密钥以明文保存到仓库外的 `~/MacBot-dev-secrets`，也可用 `MACBOT_SECRET_DIR` 覆盖。目录权限为 0700、文件为 0600，写入采用原子替换；后端拒绝 Git checkout 内的目录。provider 响应、事件、运行日志和配置快照均不包含密钥值。

```sh
MACBOT_SECRET_BACKEND=file MACBOT_SECRET_DIR="$HOME/MacBot-dev-secrets" \
  cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 7788
```

## CLI

```sh
macbotd status
macbotd passwd --password 'new-password'
macbotd settings --host-name '办公 Mac mini' --port 7788
macbotd logs [-f]
macbotd restart
macbotd update
```

守护进程在 `MACBOT_HOME/data/macbotd.sock` 创建权限为 0600 的 Unix 控制 socket；`status`、`passwd`、`settings`、`logs`（非 follow）、`restart` 和 `update` 通过它操作，不依赖远程密码鉴权。`settings --port` 写入配置并在重启后生效，`logs -f` 直接跟随日志文件。安装后 `macbotd` 和 `macbot` 都是指向 `MacBot Server.app/Contents/MacOS/macbotd` 的用户级 symlink，CLI 与守护进程仍是同一个二进制。

## LaunchAgent 和分发包

源码安装会为当前用户构建 release，安装无界面的 `~/Applications/MacBot Server.app`（`LSUIElement=true`）、`~/.local/bin/macbotd`/`macbot`，并加载 `~/Library/LaunchAgents/com.macbot.server.plist`。源码安装和 `.pkg` 都将 agent-browser 0.38.2 sidecar 及其 Apache-2.0 许可证放入 App；首次构建下载固定版本并校验 SHA256，以后复用 `server/target/sidecars/` 缓存。可设置 `MACBOT_BROWSER_BIN` 使用已有 sidecar：

```sh
MACBOT_HOME="$HOME/MacBot" server/macbotd/packaging/install-launchagent.sh
# install.sh 是同一源码安装流程的短入口
server/macbotd/packaging/install.sh
server/macbotd/packaging/uninstall-launchagent.sh
```

直接从源码运行浏览器功能时，可先准备 sidecar 并指定路径：

```sh
server/macbotd/packaging/prepare-sidecar.sh server/target/sidecars/agent-browser
MACBOT_BROWSER_BIN="$PWD/server/target/sidecars/agent-browser" \
  cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 7788
```

更新脚本先 fast-forward 当前 checkout，再构建 release 并重载 LaunchAgent：

```sh
server/macbotd/packaging/update.sh
```

源码安装的 `macbot update` 通过 Unix socket 调用该脚本。`.pkg` 安装包内置 `Contents/Resources/update-installed.sh` 和 `update-manifest.json`；构建发布包时必须提供 `MACBOT_UPDATE_URL` 和对应的 `MACBOT_UPDATE_SHA256`，之后 `macbot update` 会下载、校验 SHA-256 与可执行版本并原子替换 App 内二进制，再 kickstart LaunchAgent。没有 URL 的开发包会明确返回 `update_unavailable`，不会替换当前程序。

构建 `.pkg`（需要 Xcode Command Line Tools 的 `pkgbuild`）：

```sh
server/macbotd/packaging/build-pkg.sh target/MacBot-Server.pkg
```

`.pkg` 安装 `/Applications/MacBot Server.app`，由 postinstall 复制到当前登录用户的 `~/Applications`、创建用户级 CLI，并通过 `launchctl bootstrap gui/<uid>` 注册 LaunchAgent；没有登录用户时只安装文件。

## 验证

```sh
cargo test --manifest-path server/Cargo.toml --workspace
cargo clippy --manifest-path server/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path protocol/rust/Cargo.toml
python3 server/macbotd/tests/smoke_packaging.py --pkg target/MacBot-Server.pkg
```

`server/macbotd/tests/smoke_mock.py` 验证隔离 mock 的协议、事件补发、轨迹游标和画面 ACK。正式模式的 `smoke_runtime.py`、`smoke_collaboration.py`、`smoke_features.py`、`smoke_screen.py` 使用本机 fake provider 与独立数据目录；运行参数见各脚本 `--help`。开发测试必须显式选择 file secrets 的临时目录，使用不同端口，不连接真实模型。

以下场景自行启动和关闭隔离服务；每次使用新的 `--home`。Python 依赖为 `websockets` 和 `jsonschema`，可安装在仓库外的 venv。`smoke_trace.py` 验证进行中的文本与游标补发，`smoke_routines.py` 验证调度、通知和禁用；定时场景使用隔离日志中的到期时间，不改变正式服务的最短周期。

```sh
python3 server/macbotd/tests/smoke_runtime.py \
  --daemon-command 'server/target/debug/macbotd --port 7791 --password dev' \
  --home /tmp/macbot-runtime-check
python3 server/macbotd/tests/smoke_features.py \
  --daemon-command 'server/target/debug/macbotd --port 7798 --password dev' \
  --home /tmp/macbot-features-check
python3 server/macbotd/tests/smoke_trace.py \
  --daemon-command 'server/target/debug/macbotd --port 7840 --password dev' \
  --home /tmp/macbot-trace-check
python3 server/macbotd/tests/smoke_screen.py \
  --daemon-command 'server/target/debug/macbotd --port 7830 --password dev' \
  --home /tmp/macbot-screen-check --browser-bin "$PWD/server/target/sidecars/agent-browser"
```
