# Mac Bot 客户端 ↔ 服务端协议 v1（契约草案，修订 2）

> 本文是 **server-mac、client-mac、client-android、client-ios 四条开发线共同遵守的契约**。
> 修订 2：群消息不流式（只来自 Bot 的 `send_msg` 工具调用）；去掉发言模型；插话带送达状态；运行轨迹只在用户打开时推送。
> - 修改协议要走「协议变更」流程（见 AGENTS.md）：先改本文，再由 server-mac 更新 Rust 类型和 JSON Schema，最后各客户端跟进。
> - 机器可读的定义以 `protocol/schema/*.json`（由 Rust 类型导出）为准；示例数据在 `protocol/fixtures/`。
> - 字段命名使用 `snake_case`；时间统一用 RFC 3339 UTC 字符串；ID 用 UUIDv7 字符串。

---

## 1. 连接

### 1.1 地址

客户端填写的地址可以是**任意 IP 或域名**加端口：`192.168.1.20:7788`、`macmini.local:7788`、`bot.example.com:443`。
- 用户填 `host:port` 时默认使用 `ws://`；填 `wss://…` 或 `https://…` 时使用 TLS（TLS 由用户的反向代理或 frp 提供，服务端本身只监听明文）。
- 客户端可以为同一台 Host 保存多个地址（例如局域网地址 + 公网域名），按顺序尝试；用握手返回的 `node_id` 判断是不是同一台 Host。

### 1.2 通道一览（同一个端口）

| 通道 | 用途 | 生命周期 |
|------|------|----------|
| **`/ws` 主连接**（WebSocket，文本帧 JSON） | 请求/响应、所有业务事件、心跳、断线补发、**运行轨迹流式推送（订阅制）** | 客户端在前台时常驻，一个客户端只开一条 |
| `/ws/screen?bot_id=&project_id=` | Agent Computer 实时画面：下行 JPEG 二进制帧，上行鼠标、键盘、触摸 JSON | **按需**：打开画面时建立，关闭时断开 |
| `/api/v1/*`（HTTP） | 大块数据：文件和产物下载（支持 Range）、上传附件、仪表盘查询、CSV 导出 | 按需 |
| `/admin` | Web 管理页 | — |

**为什么不全部塞进一条 TCP 连接**：普通消息、心跳、事件、运行轨迹都很小，走 `/ws` 一条连接完全够用。但画面流每秒几百 KB，和它们共用一条 TCP 时，一帧大图会让后面的心跳和消息排队（队头阻塞）。所以画面单独开连接，文件走 HTTP。

### 1.3 流量估算

| 场景 | 主连接流量 |
|------|-----------|
| 后台挂着，只收群消息和状态 | 每分钟几 KB |
| 打开一个群：Bot 用 `send_msg` 发的完整消息（**不流式**） | 每条几百字节到几 KB |
| 和 Bot 私聊：回复流式显示 | 1–3 KB/s 的短时峰值 |
| 点开某个 Bot 的工作详情，看它的运行轨迹（执行中流式推送；**只有打开时才推**） | 平均 2–10 KB/s；工具输出过长时只推前 8 KB，其余按需用 HTTP 取 |
| 打开 Agent Computer（单独的画面连接） | 100–700 KB/s，取决于画质、尺寸和帧率；只在打开时产生 |

主连接上的文本帧启用 `permessage-deflate` 压缩。

### 1.4 鉴权

- 建立 `/ws`、`/ws/screen` 和 HTTP 请求时，都在请求头里带 `Authorization: Bearer <访问密码>`。
- 无法设置请求头的场景，可以在查询参数里带 `?token=<访问密码>`，作为后备方案。
- 密码错误时：WebSocket 握手返回 HTTP 401；HTTP 请求返回 401。
- 服务端还没设置密码时，返回 HTTP 403 和 `{"error":"setup_required"}`，客户端提示「请先在 Bot 主机上打开 localhost:7788/admin 设置密码」。

### 1.5 握手、心跳、断线补发

```text
客户端                                         服务端
  │── WebSocket 握手（带密码）────────────────────▶│
  │◀─ evt hello {protocol, server_version, node_id, host_name, server_time, last_seq}
  │── req session.resume {last_seq, client}──────▶│
  │◀─ res {mode:"replay"} + 依次补发 seq > last_seq 的事件
  │      或 res {mode:"reset"}（落后太多：客户端重新拉快照）
  │◀─ evt sync.done {seq}
  │   … 正常收发 …
  │── WebSocket ping（每 20 秒）─────────────────▶│  服务端 60 秒收不到任何帧就断开
```

- `last_seq` 是客户端已经处理过的最后一个**持久事件**的序号。首次连接时传 `0`，服务端返回 `reset`。
- 服务端保留最近 7 天或最近 100,000 个持久事件，以先到者为准；更早的只能 `reset`。
- `reset` 时客户端调用快照接口（`chat.list`、`bot.list`、`project.list`、`workbench.get`）重建本地状态。
- 重连退避：1s、2s、4s … 最长 30s，带随机抖动。
- 不支持 WebSocket ping 的平台，改为每 20 秒发 `req ping`。

---

## 2. 帧格式

所有 `/ws` 文本帧都是一个 JSON 对象：

```jsonc
// 请求（客户端 → 服务端）
{ "v": 1, "kind": "req", "id": "c-42", "method": "chat.send", "params": { } }

// 响应（服务端 → 客户端）
{ "v": 1, "kind": "res", "id": "c-42", "ok": true,  "result": { } }
{ "v": 1, "kind": "res", "id": "c-42", "ok": false, "error": { "code": "not_found", "message": "chat not found" } }

// 持久事件（有全局 seq，会补发）
{ "v": 1, "kind": "evt", "seq": 10293, "event": "message.created", "data": { } }

// 临时事件（没有 seq，不补发；只发给在线或订阅了的客户端）
{ "v": 1, "kind": "evt", "event": "trace.event", "stream": "trace:asg_0192…", "data": { } }
```

- `id` 由客户端生成，在本连接内唯一即可。
- 会产生写操作的请求带 `client_request_id`（UUID），服务端据此**幂等**处理，重连后重发不会重复执行。
- 错误码：`unauthorized`、`setup_required`、`version_unsupported`、`invalid_params`、`not_found`、`conflict`、`rate_limited`、`busy`、`internal`。

---

## 3. 核心数据对象

> 下面是客户端可见的形态（与服务端存储的 JSON 基本一致，但会去掉内部字段）。完整字段以 JSON Schema 为准。

```jsonc
// Bot
{ "id": "bot_…", "name": "编码", "label": "写代码、部署", "description": "…", "avatar": {"kind":"bean","color":3},
  "is_main": false, "model": "anthropic/claude-sonnet",
  "max_parallel": 3, "tools": {"files":true,"bash":true,"browser":true,"subagent":true,"web":true,"mcp":false},
  "pinned": false, "hidden": false,
  "status": { "summary": "working", "active": 2, "queued": 1, "waiting_user": 0 } }

// Chat（会话：主 Bot 私聊 / 某个 Bot 私聊 / 群 / Bot 间私信）
{ "id": "chat_…", "kind": "main|direct|project|bot_dm", "title": "登录功能", "project_id": "prj_…|null",
  "member_bot_ids": ["bot_…"], "last_message": {…}, "unread": 3, "attention": "none|unread|working|blocked|waiting_user|review" }

// Message
{ "id": "msg_…", "chat_id": "chat_…", "seq": 812, "sender": {"kind":"user|bot|system","id":"bot_…"},
  "created_at": "2026-10-09T10:19:02Z", "reply_to": "msg_…|null", "mentions": ["bot_…"],
  "blocks": [ /* 见 3.1 */ ],
  "intent": "ack|progress|decision|done|blocked|null",   // Bot 通过 send_msg 发出的消息才有，见 3.2
  "assignment_id": "asg_…|null",                          // 这条消息属于哪个任务
  "streaming": false,                                     // 只有私聊里的 Bot 回复可能为 true
  "delivery": [ {"bot_id": "bot_…", "state": "queued|delivered|read", "assignment_id": "asg_…"} ] }  // 只出现在用户 @ Bot 的消息上

// Project（= 群）
{ "id": "prj_…", "chat_id": "chat_…", "name": "登录功能", "slug": "login", "goal": "…", "flow": ["产品","编码","测试"],
  "deadline": "2026-10-12", "home_path": "~/MacBot/projects/login/", "status": "active|review|done|archived",
  "lead_bot_id": "bot_main", "members": [{"bot_id":"bot_…","role_note":"PRD、原型"}] }

// Announcement（公告，服务端实时拼出来）
{ "project_id": "prj_…", "members": [{"bot_id":"…","role_note":"…","state":"idle|queued|working|done|blocked|waiting_user","current_assignment_id":"…|null","since":"…"}],
  "artifacts": [{"id":"art_…","title":"PRD","path":"product/prd.md","bot_id":"…","updated_at":"…"}],
  "memory": [{"id":"mem_…","content":"只做邮箱登录","source":{"kind":"user","at":"…"}}] }

// Assignment（任务）
{ "id": "asg_…", "project_id": "prj_…|null", "origin_chat_id": "chat_…", "bot_id": "bot_…", "title": "实现登录",
  "instruction": "…", "from": {"kind":"user|bot","id":"…"}, "status": "queued|working|waiting_user|waiting_bot|blocked|done|failed|cancelled",
  "queue_reason": "bot_parallel_limit|global_limit|serial_in_project|null",
  "started_at": "…", "finished_at": null, "usage": {"input_tokens": 61200, "output_tokens": 26800, "cost": 0.42},
  "subagents_active": 2, "steers": [{"text":"只做邮箱登录","at":"…","applied_at":"…"}] }
```

### 3.1 消息块（Message.blocks）

> 群里 Bot 发出的每条消息都来自它调用的 `send_msg` 工具（见 3.2），是一条完整的消息，**不会有流式片段**。

客户端按 `type` 渲染；不认识的类型显示为纯文本兜底（`fallback_text` 字段一定存在）。

| type | 用途 | 主要字段 |
|------|------|----------|
| `text` | Markdown 正文 | `markdown` |
| `image` / `file` | 附件 | `url`（HTTP 路径）、`name`、`size`、`mime` |
| `task_card` | 任务卡片（实时更新） | `assignment_id` |
| `completion` | 完成报告（`send_msg` intent=done） | `summary`、`artifacts[]`、`handoff: [{bot_id, instruction}]` |
| `blocked` | 卡住报告（intent=blocked） | `reason`、`mentions[]` |
| `progress` | 阶段进展（intent=progress） | `text` |
| `project_card` | 新群卡片 | `project_id` |
| `review_card` | 待验收卡片 | `project_id`、`artifacts[]`、`actions: [confirm, request_changes]` |
| `delegation` | 「↪ 交给 调研」 | `bot_id`、`assignment_id` |
| `approval` | 审批卡片（只在私聊里出现） | `approval_id`、`tool`、`summary`、`detail`、`state` |
| `approval_ref` | 群里的「⚑ 在私聊里等待你审批」 | `approval_id`、`chat_id` |
| `question` | 需要决策（intent=decision 且带 options） | `question_id`、`text`、`options[]`、`state` |
| `takeover_request` | 请求接管 | `bot_id`、`project_id`、`reason` |
| `bot_dm_ref` | 「✉ A 私信了 B」 | `chat_id`、`count` |
| `memory_note` | 「✎ 记住了」 | `memory_id`、`scope`、`content` |
| `system` | 系统事件 | `text` |
| `loop_paused` | 防循环横幅 | `root_message_id`、`hops` |

### 3.2 `send_msg` 与消息的对应关系

Bot 在干活过程中调用 `send_msg(text, intent, to?, mentions?, artifacts?, handoff?, options?)`（定义见 PLAN 5.3.2），服务端据此生成一条 Message：

| intent | 生成的 Message | 任务状态变化 |
|--------|---------------|--------------|
| `ack` | `text` 块 | 不变（派发时已经是 `working`） |
| `progress` | `progress` 块 | 不变 |
| `decision` | `text` 块；有 options 时为 `question` 块 | `waiting_user` 或 `waiting_bot`（run 挂起） |
| `done` | `completion` 块（含产物和交接） | `done`；为每个交接对象创建新任务 |
| `blocked` | `blocked` 块 | `blocked`（run 挂起） |

客户端根据 `Message.intent` 渲染不同样式；任务状态以 `assignment.updated` 为准。

### 3.3 插话的送达状态

用户在群里 @ 一个正在干活或挂起中的 Bot 时，这条消息会作为 steer 进入该任务的 run（见 PLAN 5.3.4），服务端**不会立即生成回复**。消息的 `delivery` 字段通过 `message.updated` 依次更新：
- `queued`：已收到，正在排队（Bot 正在执行一个步骤）
- `delivered`：已经注入 run，下一次模型调用就会看到
- `read`：Bot 已经处理了这条消息（模型调用已经包含它）

客户端在用户气泡下面显示「已送达 · 等下一步」或「编码 已读取」。

---

## 4. 请求（methods）

| 分组 | 方法 | 说明 |
|------|------|------|
| 会话 | `session.resume` `ping` | 见 1.5 |
| 设备 | `device.register {platform: macos|android|ios, app_version, device_name, push_token?}` | 登记客户端；iOS 用 `push_token` 接收 APNs 推送（服务端需要配置 APNs 密钥） |
| 快照 | `chat.list` `bot.list` `project.list` `workbench.get` | 重建本地状态 |
| 消息 | `chat.history {chat_id, before_seq?, limit≤100}`<br>`chat.send {chat_id, client_request_id, text, mentions[], reply_to?, attachments[]}`<br>`chat.mark_read {chat_id, seq}` | 附件先用 HTTP 上传，拿到 `upload_id` 后放进 `attachments` |
| Bot | `bot.get` `bot.create` `bot.update` `bot.delete` | 主 Bot 不能删除 |
| 群 | `project.get`（含公告）`project.create` `project.update` `project.add_member` `project.remove_member`<br>`project.confirm_done` `project.request_changes {text}` `project.archive` | |
| 任务 | `assignment.list {project_id?, bot_id?, status?}` `assignment.get` `assignment.stop` | |
| **运行轨迹** | `trace.subscribe {assignment_id}` → 返回 `{stream, backlog_cursor}`<br>`trace.unsubscribe {stream}`<br>`trace.history {assignment_id, cursor?, limit}` | 见第 6 节 |
| 交互 | `approval.decide {approval_id, decision: allow_once|always_allow|deny}`<br>`question.answer {question_id, option?|text}`<br>`takeover.start {bot_id, project_id}` `takeover.release` | |
| 工作台 | `workbench.get {group_by: bot|project|status}` | |
| 记忆 | `memory.list {scope, bot_id?, project_id?}` `memory.upsert` `memory.delete` | |
| 技能 | `skill.list` `skill.get` `skill.save` `skill.set_enabled` `skill.import {source}` | |
| 定时任务 | `routine.list` `routine.save` `routine.delete` `routine.test_run` | |
| 设置 | `provider.list` `provider.save` `provider.delete` `provider.test` `model.list` `settings.get` `settings.update` | |
| 仪表盘 | `usage.summary` `usage.heatmap` `usage.timeseries` `usage.breakdown` | 参数见 PLAN 5.5.9；也可以用 HTTP 调用 |
| 搜索 | `search {query, kinds[]}` | 搜消息、群、Bot、产物、定时任务 |

## 5. 事件（events）

**持久事件**（有 `seq`，断线后补发）：

`chat.created` `chat.updated` · `message.created` `message.updated` `message.deleted` · `bot.created` `bot.updated` `bot.deleted` · `project.created` `project.updated` · `announcement.updated` · `assignment.created` `assignment.updated` · `artifact.registered` · `approval.requested` `approval.resolved` · `question.asked` `question.answered` · `memory.updated` · `skill.updated` · `routine.updated` `routine.run` · `settings.updated`

**临时事件**（没有 `seq`）：

| 事件 | 发给谁 | 内容 |
|------|--------|------|
| `message.delta` | 所有在线客户端 | **只用于私聊**：Bot 回复的流式片段 `{message_id, text}`；结束时由 `message.updated`（`streaming:false`）给出完整内容。群消息没有 delta |
| `typing` | 所有在线客户端 | **只用于私聊**：`{chat_id, bot_id}` |
| `bot.status` | 所有在线客户端 | 综合状态和并发数，变化时推送 |
| `usage.tick` | 所有在线客户端 | `{assignment_id, input_tokens, output_tokens, cost}`，每个任务每 10 秒最多一次（用于工作台；群里的任务卡片不显示 token） |
| `trace.event` | **只发给订阅了该 stream 的客户端** | 见第 6 节 |
| `host.status` | 所有在线客户端 | 运行中的任务数、全局并发占用 |

---

## 6. 运行轨迹（Trace）：Bot 干活过程的流式查看和回放

Bot 执行任务的 durable 过程（模型请求、模型输出、思考、工具调用和结果、`send_msg`、子代理、插话、挂起和恢复、压缩、审批等待）全部**持久化**在服务端的线程日志里。

**只有用户点开某个 Bot 的工作详情时**，客户端才订阅；任务执行中流式推送，结束后用 history 回放。没有订阅时，服务端只落盘，不推送。

### 6.1 订阅流程

```text
client ── req trace.subscribe {assignment_id} ─────────────▶ server
       ◀─ res {stream:"trace:asg_…", backlog_cursor:"c_…", live:true|false}
       ── req trace.history {assignment_id, cursor:null} ──▶   （先拉已经发生的部分，分页）
       ◀─ res {items:[TraceItem…], next_cursor}
       ◀─ evt trace.event {stream, data: TraceItem}  … 实时推送（live 时）
       ── req trace.unsubscribe {stream} ─────────────────▶   （关闭面板时）
```

- 订阅之后，实时推送的条目和 history 里的条目用 `(run_id, tseq)` 去重和排序，客户端合并显示。
- 任务结束后（`live:false`），只用 `trace.history` 回放，界面和实时查看相同。
- 每个客户端同时最多订阅 8 个 stream。

### 6.2 TraceItem

```jsonc
{ "run_id": "run_…", "tseq": 37, "at": "2026-10-09T10:24:11.312Z", "type": "…", "data": { } }
```

| type | data | 说明 |
|------|------|------|
| `run.start` | `{phase: work|subagent|memory|compact, model, parent_run_id?, subagent_task?}` | 一次运行开始；子代理的 run 带 `parent_run_id` |
| `llm.request` | `{request_id, model, provider, context: {layers: {l0,l1,l2,l3,l4}, total_tokens_est}, tools: [name…]}` | 一次模型调用开始；默认只给各层的大小，不给完整提示词（设置里打开「保存完整请求」后才有 `prompt_ref`） |
| `llm.delta` | `{request_id, channel: text|thinking|tool_args, tool_call_id?, text}` | 流式片段（服务端每 50ms 合并一次再推送） |
| `llm.response` | `{request_id, stop_reason, usage:{input,output,cache_read,cache_write}, cost, latency_ms, ttft_ms}` | 一次模型调用结束 |
| `tool.start` | `{call_id, name, args}` | |
| `tool.output` | `{call_id, chunk}` | 长时间运行的工具（bash、子代理）的流式输出；每次调用最多推 8 KB，超出部分只在结束时给出路径 |
| `tool.end` | `{call_id, is_error, preview, details, truncated, full_output_url?}` | `details` 供界面渲染（diff、退出码、截图等） |
| `steer` | `{text, from, message_id}` | 插话被注入（对应消息的 delivery 变为 `delivered`） |
| `send_msg` | `{call_id, intent, message_id, chat_id}` | Bot 往群里发了一条消息，客户端可以链接过去 |
| `run.wait` / `run.resume` | `{reason: decision|blocked|approval, message_id?}` / `{by_message_id?}` | run 挂起等待 / 被回复唤醒 |
| `approval.wait` / `approval.done` | `{approval_id, tool, decision?}` | 等待审批 |
| `compaction` | `{reason, before_tokens, after_tokens}` | 开新段 |
| `run.end` | `{status: done|failed|cancelled, error?}` | |

---

## 7. 画面通道 `/ws/screen`

- 握手参数：`bot_id`、`project_id`（私聊和转交的小事用 `chat_id`）、`quality`（auto / high / low）、`max_fps`。
- 下行：二进制帧 = 4 字节大端头长度 + JSON 头 `{seq, w, h, ts, url}` + JPEG 数据；另外有文本帧 `{"type":"url","url":…}`、`{"type":"state","driver":"bot|user|idle"}`。
- 上行：`{"type":"input_mouse"|"input_keyboard"|"input_touch", …}`。格式与 agent-browser 相同，macbotd 原样转发；只有调用 `takeover.start` 之后才会被接受。
- 采用 ack 节流：客户端每收到一帧回 `{"type":"ack","seq":N}`，网络慢时不会堆积旧帧。

## 8. HTTP 接口

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/files?root=project:<id>|bot:<id>&path=<相对路径>` | 下载文件或产物，支持 Range；只允许访问 Home 和 Bot 目录 |
| GET | `/api/v1/files/list?root=…&path=…` | 列目录 |
| POST | `/api/v1/uploads` | 上传附件（multipart），返回 `upload_id` |
| GET | `/api/v1/trace/output?run_id=&call_id=` | 被截断的工具输出全文 |
| GET | `/api/v1/usage/{summary,heatmap,timeseries,breakdown,export.csv}` | 与 `usage.*` 方法等价 |
| GET | `/api/v1/health` | 不需要鉴权，返回 `{ok, protocol, version}` |

## 9. 版本

- `protocol` 是一个整数，当前为 `1`。服务端同时支持 `N` 和 `N-1`；客户端在 `hello` 里发现服务端版本比自己高 2 个以上时，提示升级。
- 新增字段、事件或消息块类型不升版本；客户端必须忽略不认识的字段和事件，不认识的块用 `fallback_text` 显示。

## 10. 开发用的模拟服务（mock）

- `macbotd --mock`（由 server-mac 提供）：不调用模型，按 `protocol/fixtures/scenarios/*.jsonl` 回放场景，例如「登录功能」完整流程：建群 → 接力 → 插话 → 待验收。包括 `trace.event` 和画面流（静态图片轮播）。
- 在 mock 可用之前，客户端先用 `protocol/fixtures/*.json` 里的静态示例开发界面。
