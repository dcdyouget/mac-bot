# macbotd 服务端

`server/` 是 macOS Apple Silicon 服务端 workspace，包含 JSON 文件存储、durable runtime、providers/tools/skills/memory/orchestrator、浏览器 sidecar、usage ledger 和 axum gateway。二进制 crate `macbotd` 同时提供守护进程与本机 CLI。

数据目录默认为 `~/MacBot`，可用 `MACBOT_HOME` 覆盖。用户级服务配置位于 `~/mac-bot/config.toml`，可用 `--config` 或 `MACBOT_CONFIG` 指定其他文件。正式服务默认监听 `0.0.0.0:28789`；mock 服务默认监听 `0.0.0.0:7789`。mock 与正式服务必须使用不同的数据目录，Store 的单写锁会拒绝同目录的第二个进程。

## 编译和运行

在仓库根目录执行：

```sh
cargo build --manifest-path server/Cargo.toml -p macbotd

# mock：不调用真实模型，供客户端联调；临时 config/home/端口与正式服务隔离
test_root="$(mktemp -d /tmp/macbot-mock.XXXXXX)"
: > "$test_root/config.toml"
cargo run --manifest-path server/Cargo.toml -p macbotd -- \
  --mock --config "$test_root/config.toml" --home "$test_root/home" \
  --port 27889 --password dev

# 正式服务：provider.create 之后才连接真实模型
cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 28789
```

`~/mac-bot/config.toml` 首次启动时自动创建，文件权限为 0600。可按需取消注释并编辑：

```toml
port = 28789
data_home = "~/MacBot"
host_name = "Mac Bot"
```

`MACBOT_HOME` 仍可覆盖数据目录；上面的临时启动示例显式传入 `--home` 和空测试 `--config`，避免复用正式数据或读取默认用户配置。若示例端口已被占用，请换一个空闲端口后重试，不要终止占用该端口的旧服务。

相对 `data_home` 路径按配置文件所在目录解析；`~` 指当前用户目录。配置优先级为命令行参数（例如 `--port`、`--home`、`--config`）> 对应环境变量（`MACBOT_PORT`、`MACBOT_HOME`、`MACBOT_CONFIG`）> `config.toml` > 旧版 `data/settings.json` > 默认值。旧版数据中仅保存了原默认端口 `7788` 时，只迁移一次到 `28789`，并在 settings 中写入迁移标记；迁移后用户通过管理页或 CLI 明确保存的 `7788` 不会再被改写。命令行、环境变量和 TOML 中显式设置的 `7788` 始终优先保留。管理页状态会显示端口来源；由启动参数、`MACBOT_PORT` 或 TOML 管理的端口不可在管理页保存，配置冲突返回 `409 port_managed_by_startup`，同一次请求中的其他设置仍会保存。非整数或越界端口明确返回 `400 invalid_port`，不会被静默忽略。`--password` 只在 `data/auth.json` 不存在时初始化，不覆盖已有密码。已有密码可用 `macbotd passwd --password <password>` 重置；密码只保存为 Argon2 哈希，文件权限为 0600。首次设置密码必须通过 loopback 的管理页面完成；健康检查公开，未完成设置时管理页仅向 loopback 返回设置状态，其他受保护接口拒绝访问。`/api/v1/health` 无需鉴权。

## HTTP/WebSocket

- `GET /api/v1/health`：公开健康检查。
- `POST /api/v1/rpc`：JSON RPC，使用 `Authorization: Bearer <password>`。
- `GET /ws`：控制连接，支持 query `?token=<password>` 或 Bearer header。
- `GET /ws/screen`：画面连接，同样需要鉴权；二进制帧为 4 字节大端 JSON 头加 JPEG，客户端 ACK 后才发送下一帧。
- `/api/v1/files`、`/api/v1/uploads`、`/api/v1/usage/export.csv`：鉴权后的文件与用量接口。
- `/` 和 `/admin`：同一个首次设置、登录和管理页面。首次设置仅允许 loopback 请求；设置后通过自定义密码表单登录，服务端签发 HttpOnly、SameSite=Strict、Path=/、12 小时 session cookie，浏览器打开服务根地址会显示对应的设置、登录或管理界面。session 仅保存在服务进程内，重启后失效；退出登录会撤销当前 session，修改密码会撤销全部 session。管理 HTTP 页面不使用浏览器 HTTP Basic 认证。

正式画面连接会读取 Bot 和设置中的浏览器配置，服务重启后可直接恢复旧 Computer，无需先发送模型请求。无头模式会核对保存的任务标签页与真实 sidecar 页面，缺失时恢复保存的 URL；attach 模式不会自动重建用户已关闭的标签页。

管理页可修改主机名称和监听端口；端口变更写入 `data/settings.json`，重启 LaunchAgent 后生效。配置文件若明确设置同一字段，会覆盖管理页写入的旧版 setting。客户端继续使用既有 `/ws`、`/ws/screen` 和 `/api/v1/*` 路径；服务端只监听一个端口，不额外拆分管理端口或添加 `/client` 前缀。

正式模式的 provider 配置通过 `provider.create` 写入，API key 由 Mac Bot 自行使用 AES-256-GCM 加密，保存到 `data/provider-secrets/`；同目录的 `.provider-secrets-key` 为随机 256 位密钥。目录权限为 0700、文件权限为 0600，凭据文件采用原子替换；存储后端拒绝 Git checkout 内的目录，不使用 macOS Keychain。旧版 Keychain 凭据不会自动迁移，升级后需要重新输入 API key；备份和恢复时必须同时保留 `.provider-secrets-key` 与密文文件。`MACBOT_SECRET_BACKEND=file` 保留为测试/开发配置，仍使用同一加密格式；可用 `MACBOT_SECRET_DIR` 指定隔离目录。provider 响应、事件、运行日志和配置快照均不包含密钥值。

请求幂等收据使用 `data/idempotency-signing.key` 中的随机 256 位密钥生成 HMAC-SHA256，密钥文件为 0600，创建时原子落盘；WAL 只保存签名，不保存请求中的敏感值。备份和恢复时必须保留该文件。若密钥丢失，旧签名会导致同一请求返回 `conflict`，服务不会把它当成新请求重新执行。旧版没有签名的 WAL 收据只能按已有 method/params 尽力兼容；其中被 redaction 隐藏的设置 secret 无法核实，相关旧请求会返回 `conflict`，需要使用新的 `client_request_id` 重试。

首次打开 `/admin` 设置访问密码后，登录页会显示「模型接入」。选择 DeepSeek、OpenAI API、智谱 GLM、阶跃星辰 StepFun、Anthropic Claude、MiniMax 或 Google Gemini，输入该服务商的 API key 与模型 ID，即可添加 provider/model 并设为 Main Bot 与普通 Bot 的默认模型。OpenAI Codex 与 OpenCode 是客户端/编排器，不是独立模型 API provider；OpenAI preset 使用 API key，不接受 ChatGPT 订阅登录。

```sh
MACBOT_SECRET_BACKEND=file MACBOT_SECRET_DIR="$HOME/MacBot-dev-secrets" \
  cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 28789
```

LaunchAgent 不继承当前 shell 的环境变量。开发期通过 LaunchAgent 使用独立目录时，需要在安装后的 plist 的 `EnvironmentVariables` 中显式设置 `MACBOT_SECRET_BACKEND=file` 和仓库外的 `MACBOT_SECRET_DIR`，再重载该 LaunchAgent。没有环境变量时，正式服务使用 `MACBOT_HOME/data/provider-secrets/`。

## CLI

```sh
macbotd status
macbotd passwd --password 'new-password'
macbotd settings --host-name '办公 Mac mini' --port 28789
macbotd logs [-f]
macbotd restart
macbotd update
```

守护进程在 `MACBOT_HOME/data/macbotd.sock` 创建权限为 0600 的 Unix 控制 socket；`status`、`passwd`、`settings`、`logs`（非 follow）、`restart` 和 `update` 通过它操作，不依赖远程密码鉴权。`settings --port` 写入配置并在重启后生效，`logs -f` 直接跟随日志文件。安装后 `macbotd` 和 `macbot` 都是指向 `MacBot Server.app/Contents/MacOS/macbotd` 的用户级 symlink，CLI 与守护进程仍是同一个二进制。

## LaunchAgent 和分发包

源码安装会为当前用户构建 release，安装无界面的 `~/Applications/MacBot Server.app`（`LSUIElement=true`）、`~/.local/bin/macbotd`/`macbot`，并加载 `~/Library/LaunchAgents/com.macbot.server.plist`。源码安装和 `.pkg` 都将 agent-browser 0.39.0 sidecar 及其 Apache-2.0 许可证放入 App；该版本包含 Chrome 144+ 远程调试确认框的等待和重复弹窗修复。首次构建下载固定版本并校验 SHA256，以后复用 `server/target/sidecars/` 缓存。可设置 `MACBOT_BROWSER_BIN` 使用已有 sidecar：

```sh
server/macbotd/packaging/install-launchagent.sh
# install.sh 是同一源码安装流程的短入口
server/macbotd/packaging/install.sh
server/macbotd/packaging/uninstall-launchagent.sh
```

源码安装和 `.pkg` 默认不向 LaunchAgent 注入 `MACBOT_HOME`，所以服务会读取 `~/mac-bot/config.toml` 中的 `data_home`，未设置时使用 `~/MacBot`。如需在安装时固定数据目录，可显式执行 `MACBOT_HOME="$HOME/MacBot" server/macbotd/packaging/install-launchagent.sh`；LaunchAgent 此时会固定使用该目录，日志文件也写入该目录。

直接从源码运行浏览器功能时，可先准备 sidecar 并指定路径：

```sh
server/macbotd/packaging/prepare-sidecar.sh server/target/sidecars/agent-browser
MACBOT_BROWSER_BIN="$PWD/server/target/sidecars/agent-browser" \
  cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 28789
```

更新脚本先 fast-forward 当前 checkout，再构建 release 并重载 LaunchAgent：

```sh
server/macbotd/packaging/update.sh
```

源码安装的 `macbot update` 通过 Unix socket 调用该脚本。`.pkg` 安装包内置 `Contents/Resources/update-installed.sh` 和 `update-manifest.json`；构建发布包时必须提供 `MACBOT_UPDATE_URL` 和对应的 `MACBOT_UPDATE_SHA256`，之后 `macbot update` 会下载、校验 SHA-256 与可执行版本并原子替换 App 内二进制，再 kickstart LaunchAgent。没有 URL 的开发包会明确返回 `update_unavailable`，不会替换当前程序。

构建 `.pkg`（需要 Xcode Command Line Tools 的 `pkgbuild`）：

```sh
server/macbotd/packaging/build-pkg.sh target/MacBot-Server.pkg
# git archive 没有 .git 时，显式传入完整的固定源码 SHA
MACBOT_SOURCE_COMMIT=<40位SHA> server/macbotd/packaging/build-pkg.sh target/MacBot-Server.pkg
```

包内 `Contents/Resources/source-commit.txt`、`build-info.json` 和 Info.plist 的 `MacBotSourceCommit` 标识源码；build-info 同时记录源码 dirty 状态及 daemon/sidecar SHA256。

`.pkg` 安装 `/Applications/MacBot Server.app`，由 postinstall 复制到当前登录用户的 `~/Applications`、创建用户级 CLI，并通过 `launchctl bootstrap gui/<uid>` 注册 LaunchAgent；没有登录用户时只安装文件。

包的 `components.plist` 禁用 App 级别重定位，确保磁盘上同 bundle ID 的旧版或备份不会被选作安装目标。包校验会检查 `PackageInfo` 中没有 `relocate/bundle`；仅检查顶层 `relocatable=false` 不足以验证这一点。postinstall 在修改用户路径前核对固定位置的 payload。

## 验证

```sh
cargo test --manifest-path server/Cargo.toml --workspace
cargo clippy --manifest-path server/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path protocol/rust/Cargo.toml
python3 server/macbotd/tests/smoke_packaging.py --pkg target/MacBot-Server.pkg
```

`server/macbotd/tests/smoke_mock.py` 验证隔离 mock 的协议、事件补发、轨迹游标和画面 ACK。正式模式的 `smoke_runtime.py`、`smoke_collaboration.py`、`smoke_dispatch.py`、`smoke_features.py`、`smoke_screen.py` 使用本机 fake provider 与独立数据目录；运行参数见各脚本 `--help`。开发测试必须显式选择 file secrets 的临时目录，使用不同端口，不连接真实模型。

`smoke_routine_queue.py` 使用本机可控 provider 和随机端口，验证并发上限下的 routine 排队、原 run 身份、真实投递会话、用量归属及重启后的末次运行摘要。`smoke_routine_recovery.py` 在已批准 bash 产生隔离副作用后 kill9，验证原 run 保持 unsafe/suspended、不创建替代 run、不重复调用 provider。两者自行启动服务，必须给一个尚不存在的 `--home`，例如：

```sh
python3 server/macbotd/tests/smoke_routine_queue.py --home /tmp/macbot-routine-queue-check
python3 server/macbotd/tests/smoke_routine_recovery.py --home /tmp/macbot-routine-recovery-check
```

`smoke_conversation.py` 验证连续多轮 provider 请求、known text Markdown 与重启后的事件正文、共享消息序号及 `after_seq`、混合 write/read/bash 审批、私聊待处理工作台和已读状态。`smoke_recovery.py` 验证安全私聊 kill9 后以同一 run 自动恢复，并确认不安全工具暂停等待审批、不会自动重放。

`python3 server/macbotd/tests/smoke_profile_language.py` 自动创建独立 home 和本机 fake provider，验证首次初始化工具传入“简体中文”后保存 `zh-CN`、非法语言更新没有副作用、空语言保存为 `null`，以及重启后的档案一致。需要先构建 `server/target/debug/macbotd`，也可用 `--binary` 指定产物；只批准该场景的 `set_user_profile`。明确授权真实 MiniMax 回归时才传 `--minimax-key-file /path/to/credential.key`，可用 `--minimax-url` 和 `--minimax-model` 选择服务与模型；脚本不打印密钥，结束时停止自身进程并删除隔离目录里的凭据副本。

以下场景自行启动和关闭隔离服务；每次使用新的 `--home`。Python 依赖为 `websockets` 和 `jsonschema`，可安装在仓库外的 venv。`smoke_loop.py` 验证防循环暂停、继续、结束及 canonical message ID；`smoke_question.py` 验证私聊提问、工作台和同 run 回答恢复。`smoke_trace.py` 验证进行中的文本与游标补发，`smoke_routines.py` 验证调度、通知和禁用；定时场景使用隔离日志中的到期时间，不改变正式服务的最短周期。

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

`smoke_screen.py` 启动本机标记页面并驱动真实 Chrome；断言实际 JPEG/URL、low 宽度上限、同连接接管/交还状态，以及按帧缩放的点击和键盘回显。它不使用外部网站或真实账号。

画面输入按实际 JPEG 内容范围映射，低画质缩放保留原始内容宽高；不把 sidecar 的设备高度作为页面可见高度。独立鼠标/触摸提交回归用 DOM 按钮位置计算输入点，并记录原始 sidecar 元数据、JPEG 和实际 DOM 事件：

```sh
python3 server/macbotd/tests/smoke_screen_input.py \
  --daemon-command '/path/to/macbotd --port 7863 --password dev' \
  --home /tmp/macbot-screen-input-low --browser-bin /path/to/agent-browser --quality low
```

再用新的 home 和 `--quality high` 验证原始画质。默认严格要求 `touchStart`＋空点集 `touchEnd` 及鼠标点击都触发实际 submit；`--diagnose` 仅保存失败证据，不用于验收通过。脚本只启动并清理自己的 headless 浏览器、本机页面和 fake provider。

主 Bot 协调与私聊的模型调用按每个 Bot、每种模式每分钟 60 次限速，不占任务并发名额。滑动窗口保存于 `data/limits/model-calls.json`；达到上限时等待，已有 durable checkpoint 和 run ID 保留，停止请求可取消等待。

生产协作路径会生成项目、任务和待验收卡片。主 Bot 先建立或复用项目群，再通过 `assign` 将普通任务投递到该群；Bot 间通过群消息交接，同群活跃任务收到再次 `@` 时将消息交给当前 run 处理。worker 完成后唤醒主 Bot，由主 Bot 汇总产物并请求验收；用户提出修改意见后重新唤醒主 Bot。项目只能由用户显式调用 `project.confirm_done` 完成，确认后保存群总结及项目记忆。

`smoke_collaboration.py` 验证真实 provider 工具链的项目群交接、审批/问答、产物写入、修改后复审及显式用户验收；`smoke_dispatch.py` 验证并发上限下排队、跨群并行、同群插话并入当前 run、问答恢复和停止任务。两个脚本都使用本机 fake provider，但通过正式服务执行工具；请传入新的 `--home`、独立 `--config`、唯一端口和测试凭据，不预写待验收产物。

`smoke_product_closure.py` 是 Main-Bot 产品闭环场景，使用 `--help` 查看参数；传入独立 `--temp-root` 和已构建的 `--daemon-binary`，脚本会创建隔离 config、home 和临时 provider，要求 worker 通过生产 `write` 工具生成待验收文件。

以下 admission 回归均使用本机可控 provider、自动选择的 loopback 端口和独立 `--home`；home 必须是不存在的新路径，脚本会在其中写入测试 config 并启动/关闭自己的 daemon。先构建 `server/target/debug/macbotd`，再运行：

```sh
test_root="$(mktemp -d /tmp/macbot-admission.XXXXXX)"
daemon_bin="$PWD/server/target/debug/macbotd"

# 回答 A 时 Bot 容量仍被 B 占用：A 应排队；在答案已持久化的恢复窗口重启后，沿原 run 恢复且只注入一次答案。
python3 server/macbotd/tests/smoke_admission_answer.py --home "$test_root/answer" --binary "$daemon_bin"

# 分别覆盖 Bot 并发上限、全局并发上限、同项目串行、跨项目并行。
python3 server/macbotd/tests/smoke_admission_matrix.py --home "$test_root/bot-cap" --binary "$daemon_bin" --case bot_cap
python3 server/macbotd/tests/smoke_admission_matrix.py --home "$test_root/global-cap" --binary "$daemon_bin" --case global_cap
python3 server/macbotd/tests/smoke_admission_matrix.py --home "$test_root/serial-project" --binary "$daemon_bin" --case serial_project
python3 server/macbotd/tests/smoke_admission_matrix.py --home "$test_root/cross-project" --binary "$daemon_bin" --case cross_project_parallel

# unsafe Bash 已获准并造成副作用后崩溃；重启后重复同一 approval.decide 不会再次执行副作用。
python3 server/macbotd/tests/smoke_approval_uncertain_replay.py --home "$test_root/approval-replay" --binary "$daemon_bin"

# 验证安全 decision/blocked 等待可通过 message.steer 续接原 run，unsafe approval 等待仍需审批且不会执行工具。
python3 server/macbotd/tests/smoke_message_steer_waiting.py --home "$test_root/message-steer" --binary "$daemon_bin"

# 这些 smoke 的短路径降低 macOS Unix socket 路径超限风险。
steer_root="$(mktemp -d /tmp/mbs.XXXXXX)"

# 同一个 waiting run 上并发 question.answer 与 message.steer；验证原消息文本和答案均被原 run 消费一次。
python3 server/macbotd/tests/smoke_answer_steer_race.py --home "$steer_root/race" --binary "$daemon_bin" --case answer_steer_race

# 已 claimed 的 message continuation 遇到迟到答案和旧 done 响应；分别覆盖纯文本、多选无补充文本、多选带补充文本。
python3 server/macbotd/tests/smoke_claimed_answer_steer.py --home "$steer_root/text" --binary "$daemon_bin"
python3 server/macbotd/tests/smoke_claimed_answer_steer.py --home "$steer_root/multi" --binary "$daemon_bin" --multi-select
python3 server/macbotd/tests/smoke_claimed_answer_steer.py --home "$steer_root/multi-text" --binary "$daemon_bin" --multi-select-with-text

# 模拟 question.answer canonical 状态已提交、inbox WAL append 因 EISDIR 失败后进程崩溃；重启应沿原 run 将答案作为 role=user 输入恰好消费一次。
python3 server/macbotd/tests/smoke_answer_inbox_append_recovery.py --home "$steer_root/answer-inbox-recovery" --binary "$daemon_bin"
```

上述各 `--home` 子目录每次运行都必须是新路径；脚本自行选择临时 daemon/provider 端口，并使用本机可控的真实 provider，不使用 `--mock`。独立的 `/tmp/mbs.XXXXXX` 根目录让 Unix socket 路径保持较短，避免 macOS socket 路径长度限制。若端口分配失败或本机 provider 端口不可用，请重新运行，不要停止其他服务。

`send_msg.chat_id` 必须是已存在的聊天 ID，项目 ID 不能代替项目群的 `chat_id`。省略时使用当前聊天；显式错误目标不自动改写，模型收到工具错误后可在同一 run 用正确 ID 重试。校验在 durable send receipt、消息、事件和派发之前执行，合法 Bot 私聊、`to: {bot}` 和明确跨群目标保持。旧版已落盘的错误目标消息、任务、参数及审批不迁移、不重放。

`smoke_chat_targets.py` 使用独立 production runtime 与本机 fake provider，验证无 assignment 的 Main 群请求传错项目 ID 后零发送副作用、同 run 正确重试，以及 Bot DM 和跨群路由。每次使用新的隔离 home：

```sh
python3 server/macbotd/tests/smoke_chat_targets.py \
  --daemon-command '/path/to/macbotd --port 7864 --password dev' \
  --home /tmp/macbot-chat-target-check
```

阻塞、失败、完成但未汇报，以及两小时没有新 `ack/progress` 的项目任务会生成系统提醒并唤醒主 Bot。提醒标记随 orchestrator 恢复，取消任务不会触发提醒。公告、产物、任务晋升和待验收状态均通过持久事件补发。

旧版 decision 等待的启动迁移只接受 durable Waiting/Suspended checkpoint 与 canonical Message、run request 的一致映射。消息已保存非空 options 时可以重建丢失的 Question，保留原消息 ID/seq/时间；不从正文推断选项、不调用模型或重复派发。已有 Question 用 `question.answer {question_id, option_index}` 或 `{question_id, text}` 回答；旧无 options 的等待用 `chat.send {chat_id, text, mentions:[], reply_to:<原 decision 消息 ID>}` 明确回复。

Question 已为 `answered` 而 durable job 仍安全等待时，启动会补齐原消息与原 qid 的关联，并自动把已保存的 `answer.text` 或 `options[answer.option_index]` 送达原 run。原 Question、答案时间和消息 ID/seq/时间均保留，不重开问题、不创建新 run，也不批准工具；后续工具仍走正常审批。没有可用答案、映射冲突或存在未决工具的 job 不自动恢复。

`smoke_decision_migration.py` 验证隔离进程 kill9 后丢失 Question/wait 的修复、工作台与事件、未回答时恢复期间无模型请求、回答后同 run 完成；还覆盖已回答但未送达的旧版边界，断言启动自动送达原答案、原 qid/答案时间不变、重复重启不重复调用模型。参数与其他 runtime smoke 相同。

`memory(scope=project)` 必须显式提供 `project_id`，`scope=bot` 必须提供 `bot_id`，不从运行上下文猜目标。`memory` 与 `memory_search` 的目标必须存在于权威 Bot/项目快照中；项目名字、场景 marker 或不存在的 ID 在审批前成为工具错误，模型可在同一 run 修正。有效的显式跨目标请求仍遵循原有角色/成员授权规则，不强制等于当前 run 的项目。旧版已挂起的无效调用，仅在 approval-map、原 pending call、run request 和当前任务路由唯一一致时使审批过期，返回参数错误并续接原 run；不批准、不取消任务、不改旧参数。同批未执行调用收到 deferred 错误，必须重新请求并经过正常审批。过期回执先持久化，覆盖过期后尚未续接的崩溃边界。

文件工具支持 `~` 和 `~/` 展开为当前用户 HOME（不是 `MACBOT_HOME`），仍限制实际目标位于当前工作目录内。审批 detail 保留原始参数，并记录 `resolved_path` 与 `path_resolution=home-v1`，与执行共用解析函数。旧版缺少该元数据的 `~/` write/edit 审批不沿用授权：启动后使其过期、向原 run 返回路径语义错误，要求模型显式绝对路径重试并重新审批。已完成的旧写入及其 receipt 不迁移、不重放。

升级回归（只使用隔离 home 与本机 fake provider）：

```sh
python3 server/macbotd/tests/smoke_invalid_tool_recovery.py \
  --url http://127.0.0.1:7858 --home "$HOME/Library/Caches/macbot-invalid-tools-smoke" \
  --legacy-command '/path/to/c6ffcfc/macbotd --port 7858 --password dev' \
  --new-command '/path/to/new/macbotd --port 7858 --password dev'
```

使用尚不存在、位于真实用户 HOME 下的隔离数据目录，以覆盖新合法 `~/` 写入；不改进程 HOME。覆盖旧缺目标审批、过期回执的重启恢复、旧 tilde 审批和新缺参调用；断言无未授权副作用、修正后必须新审批、原 run 完成、重复重启幂等。

不存在的显式 Bot/项目 ID 升级回归：

```sh
python3 server/macbotd/tests/smoke_memory_targets.py \
  --url http://127.0.0.1:7861 --home /tmp/macbot-memory-target-smoke \
  --legacy-command '/path/to/d43a2bb/macbotd --port 7861 --password dev' \
  --new-command '/path/to/new/macbotd --port 7861 --password dev'
```

覆盖旧非空错误目标审批的精确过期、剩余 batch 不执行、新写入/检索无效目标不生成审批，以及修正参数新审批后原 run 完成；再次重启不重复返回错误或调用模型。

正式 RPC `ping` 返回当前 `server_time`，不等待业务写锁、不写操作日志；可通过 HTTP 或现有主 WebSocket 调用。主连接 20 秒 fallback 心跳回归：

```sh
python3 server/macbotd/tests/smoke_ping.py \
  --url http://127.0.0.1:7862 --home /tmp/macbot-ping-smoke \
  --daemon-command '/path/to/macbotd --port 7862 --password dev'
```

只使用一个 WebSocket，连续四次 `ping` 跨三个 20 秒间隔，再取 bootstrap；不以重连掩盖持活失败。该测试不配置模型或调用 provider。

后台 Bash 的工具调用与进程生命周期分开：`background=true` 返回 job_id 后立即产生 tool.end 并让模型继续；输出继续按原 run/call 推送给对应 trace 订阅，直到输出 EOF。前台 Bash 仍等待输出结束。取消任务或正常结束时清理本 run 的非 persistent 进程，已经注册为本地产物服务的保留规则不变。

旧版已经启动但卡在输出 EOF 的 Bash 不自动恢复执行：其 Running/unsafe checkpoint 在启动后变为 Suspended，保持原 pending call；不会重放命令或补造成功 tool.end。旧 manager 的 job_id/PID 没有写入该 checkpoint，不能仅凭 command/cwd 推断进程归属并接管。读取精确白名单：

```sh
python3 server/macbotd/tests/inspect_background_checkpoint.py --home "$HOME/MacBot" \
  --assignment-id asg_id_1 --assignment-id asg_id_2
```

输出原 run/asg/job/call、状态、unsafe、head 一致性、cwd 与参数哈希，不打印命令正文/密钥、不修改文件或进程。升级前后应核同一身份、Running/unsafe→Suspended、无新增该 call 的执行/进程启动。保留已有审批 journal 与 PID/cwd/进程组证据；不再次批准旧 call。若明确终止旧任务，只通过 `assignment.stop({assignment_id})` 将 run/assignment 收敛为 cancelled，不伪造 done；该操作不会追溯接管重启前的未知 PID。旧进程的处置必须单独核定归属，不能自动杀服务；如需继续检查已启动服务，使用明确的只读新任务，不再启动原命令。

独立旧版升级、实际 WebSocket 输出与取消回归：

```sh
python3 server/macbotd/tests/smoke_background_bash.py \
  --url http://127.0.0.1:7861 --home /tmp/macbot-background-smoke \
  --legacy-command '/path/to/d97e48f/macbotd --port 7861 --password dev' \
  --new-command '/path/to/new/macbotd --port 7861 --password dev'
```

只使用新隔离 home 和本机 fake provider。覆盖旧 unsafe 调用不重放、逐项审批和同 response 后续后台调用、tool.end 后持续输出、跨群 run/call 隔离和取消仅终止本 run 的非 persistent 子进程；测试结束清理其自有进程。

用户 `chat.send` 的 `@Bot` 会按实际群 ID 和 Bot 查找现有 working/waiting/blocked 任务：有任务时进入该 run 的 durable inbox，没有任务时创建一次新任务。原用户消息的 `delivery` 经 `message.updated` 从 queued 推进到 delivered/read；更新保留原 ID、seq、created_at、mentions 和 reply_to，同一 client_request_id 重放不重复插话。挂起的 decision/blocked 只续接精确关联的原 run，工具审批等待不会被插话自动批准。私聊的新请求仍使用流式对话模式。

`memory` 的 action、kind、必填 id/content 与 scope 在审批前统一校验；例如项目记忆的 kind 仅接受 `project`，不接受 `project_status`。合法的默认 user owner 和省略 action（默认 add）保持。旧版无效 kind 的 pending 仅沿已有唯一 approval/map/call/run 校验使审批 expired，向同 run 返回工具错误；不修改原参数、不沿用授权、同批剩余调用不执行。模型必须显式纠正，合法新写入仍需新审批。

真实用户 RPC 入口及旧版升级回归（隔离 home 必须不存在，端口禁止 7788/7789）：

```sh
python3 server/macbotd/tests/smoke_user_steer_memory.py \
  --url http://127.0.0.1:7866 --home /tmp/macbot-user-steer-memory-smoke \
  --old-command '/path/to/094b7c5/macbotd --port 7866 --password dev' \
  --new-command '/path/to/new/macbotd --port 7866 --password dev'
```

只使用本机 fake provider；不调用 `assignment.steer`、不在 `chat.send` 传入内部 assignment_id。验证工作中用户插话、canonical delivery 与重放去重、同 run 参数纠正、旧审批过期和未执行 batch。旧版已经派出的插话 siblings 不迁移、不取消、不重放，本补丁只修新消息的入口。

`assignment.stop` 在取消任务时将其关联 pending 审批收敛为 `expired`，并发布 `approval.resolved`；其他任务与无 assignment 的私聊审批保持。重复停止/重启不改决定时间、不重复发解决事件，过期审批不能被 allow_once/always_allow/deny 再次决定。旧版遗留的 terminal assignment + pending approval 在启动时按已有 assignment_id 修复，不重放工具或恢复任务。正常 pending 的 deny 仍记录 denied、取消 durable job，并保留失败任务和一次停止系统消息。

隔离取消与旧 d799 升级回归（本机 fake provider，禁止 7788/7789，不执行任何待批写入）：

```sh
python3 server/macbotd/tests/smoke_cancelled_approvals.py \
  --url http://127.0.0.1:7796 --home /tmp/macbot-cancelled-approvals \
  --old-command '/path/to/d799dda/macbotd --port 7796 --password dev' \
  --new-command '/path/to/new/macbotd --port 7796 --password dev'
```

记忆开发场景：`--mock` 默认提供至少 6 条用户记忆，覆盖群整理、秘书长私聊整理、用户手动添加、旧 Bot 数据、版本冲突和接近配额上限的条目。第一次编辑 `mem_mock_conflict` 会先模拟其他设备将版本从 1 更新到 2，再返回 conflict；`memory.user.create` 默认返回带 entries 目录的 quota_full。可用 mock-only RPC `mock.set_memory_scenario({"quota_full":false,"conflict_once":false})` 关闭这两个场景，正常创建和编辑便可继续；`mock.use_complete_profile({})` 切换到「小明 / zh-CN / 产品经理」完整档案，默认 profile 为空并触发 onboarding。

服务端记忆文件位于 `data/memory/state.json`、只追加修改日志 `data/memory/changelog.jsonl` 与段游标 `data/memory/segments/{run_id}.json`；段日志保留策略在服务启动维护时执行。记忆与网关测试及 lint：

```sh
cargo test --manifest-path server/Cargo.toml -p macbot-memory -p macbot-gateway
cargo clippy --manifest-path server/Cargo.toml -p macbot-memory -p macbot-gateway -p macbot-providers -- -D warnings
```
