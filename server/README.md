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

正式画面连接会读取 Bot 和设置中的浏览器配置，服务重启后可直接恢复旧 Computer，无需先发送模型请求。无头模式会核对保存的任务标签页与真实 sidecar 页面，缺失时恢复保存的 URL；attach 模式不会自动重建用户已关闭的标签页。

管理页可修改主机名称和监听端口；端口变更写入 `data/settings.json`，重启 LaunchAgent 后生效。

正式模式的 provider 配置通过 `provider.create` 写入，API key 默认进入 macOS Keychain。开发期可显式设置 `MACBOT_SECRET_BACKEND=file`，密钥以明文保存到仓库外的 `~/MacBot-dev-secrets`，也可用 `MACBOT_SECRET_DIR` 覆盖。目录权限为 0700、文件为 0600，写入采用原子替换；后端拒绝 Git checkout 内的目录。provider 响应、事件、运行日志和配置快照均不包含密钥值。

```sh
MACBOT_SECRET_BACKEND=file MACBOT_SECRET_DIR="$HOME/MacBot-dev-secrets" \
  cargo run --manifest-path server/Cargo.toml -p macbotd -- --port 7788
```

LaunchAgent 不继承当前 shell 的环境变量。开发期通过 LaunchAgent 使用 file 后端时，需要在安装后的 plist 的 `EnvironmentVariables` 中显式设置 `MACBOT_SECRET_BACKEND=file` 和仓库外的 `MACBOT_SECRET_DIR`，再重载该 LaunchAgent；默认安装仍使用 Keychain。

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

源码安装会为当前用户构建 release，安装无界面的 `~/Applications/MacBot Server.app`（`LSUIElement=true`）、`~/.local/bin/macbotd`/`macbot`，并加载 `~/Library/LaunchAgents/com.macbot.server.plist`。源码安装和 `.pkg` 都将 agent-browser 0.39.0 sidecar 及其 Apache-2.0 许可证放入 App；该版本包含 Chrome 144+ 远程调试确认框的等待和重复弹窗修复。首次构建下载固定版本并校验 SHA256，以后复用 `server/target/sidecars/` 缓存。可设置 `MACBOT_BROWSER_BIN` 使用已有 sidecar：

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
# git archive 没有 .git 时，显式传入完整的固定源码 SHA
MACBOT_SOURCE_COMMIT=<40位SHA> server/macbotd/packaging/build-pkg.sh target/MacBot-Server.pkg
```

包内 `Contents/Resources/source-commit.txt`、`build-info.json` 和 Info.plist 的 `MacBotSourceCommit` 标识源码；build-info 同时记录源码 dirty 状态及 daemon/sidecar SHA256。

`.pkg` 安装 `/Applications/MacBot Server.app`，由 postinstall 复制到当前登录用户的 `~/Applications`、创建用户级 CLI，并通过 `launchctl bootstrap gui/<uid>` 注册 LaunchAgent；没有登录用户时只安装文件。

## 验证

```sh
cargo test --manifest-path server/Cargo.toml --workspace
cargo clippy --manifest-path server/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path protocol/rust/Cargo.toml
python3 server/macbotd/tests/smoke_packaging.py --pkg target/MacBot-Server.pkg
```

`server/macbotd/tests/smoke_mock.py` 验证隔离 mock 的协议、事件补发、轨迹游标和画面 ACK。正式模式的 `smoke_runtime.py`、`smoke_collaboration.py`、`smoke_features.py`、`smoke_screen.py` 使用本机 fake provider 与独立数据目录；运行参数见各脚本 `--help`。开发测试必须显式选择 file secrets 的临时目录，使用不同端口，不连接真实模型。

`smoke_conversation.py` 验证连续多轮 provider 请求、known text Markdown 与重启后的事件正文、共享消息序号及 `after_seq`、混合 write/read/bash 审批、私聊待处理工作台和已读状态。`smoke_recovery.py` 验证安全私聊 kill9 后以同一 run 自动恢复，并确认不安全工具暂停等待审批、不会自动重放。

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

生产协作路径会生成项目、任务、委派和待验收卡片。项目任务投递到该项目群；不建群的委派在主 Bot 私聊报告结果。Bot 间私信写入独立的只读 `bot_dm` 会话，在源群显示引用。worker 完成后通知主 Bot，由主 Bot 汇总产物请求验收；用户提出修改意见后重新唤醒主 Bot。确认完成会保存群总结及项目记忆。

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
