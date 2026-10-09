# Mac Bot 客户端 ↔ 服务端协议 v1

> **本文是协议的唯一权威来源**，四条开发线（server-mac、client-mac、client-android、client-ios）共同遵守。
> - 机器可读的定义由 server-mac 从 `protocol/rust` 导出到 `protocol/schema/`，必须与本文一致；两者冲突时以本文为准，并修正 schema。
> - 修改协议走 AGENTS.md 第 3 节的流程。
> - 下面的类型用 TypeScript 语法描述：`?` 表示可以省略，`| null` 表示值可以为 null。JSON 字段名一律为 `snake_case`。

---

## 1. 连接

### 1.1 地址

- 客户端可以填写**任意 IP 或域名**加端口：`192.168.1.20:7788`、`macmini.local:7788`、`bot.example.com:443`。
- 填 `host:port` 时使用 `ws://` 和 `http://`。填 `wss://…` 或 `https://…` 时使用 TLS；TLS 由用户自己的反向代理或 frp 提供，服务端本身只监听明文。
- 一台 Host 可以保存多个地址，客户端按顺序尝试。用 `hello.node_id` 判断两个地址是不是同一台 Host。

### 1.2 通道（同一个端口）

| 通道 | 用途 | 生命周期 |
|------|------|----------|
| **`/ws`**（WebSocket） | 主连接：请求和响应、所有事件、心跳、断线补发、运行轨迹推送 | 客户端在前台时常驻，每台 Host 一条 |
| **`/ws/screen`**（WebSocket） | Agent Computer 画面：下行 JPEG 二进制帧，上行确认和输入（见第 8 节） | **只在打开画面时建立**，关闭就断开 |
| `/api/v1/*`（HTTP） | 大块数据：文件和产物下载（支持 Range）、附件上传、被截断的工具输出全文、CSV 导出；另外 `/api/v1/rpc` 可以用 HTTP 调用任意方法，方便脚本使用 | 按需 |
| `/admin` | Web 管理页（不属于客户端协议） | — |

**为什么画面单独一条连接**：画面每秒 100–700 KB，如果和消息共用一条 TCP 连接，一旦丢包，重传期间后面的心跳和消息都要排队（TCP 队头阻塞），应用层的优先级处理不了这个问题。分开之后，丢包只影响画面自己。两条连接用同一个端口、同一个密码，代理（frp 等）不需要额外配置。

**以后可选**：如果在公网或手机网络上看画面仍然卡顿，再给画面增加一个 QUIC 通道作为加速，连不上 UDP 时自动退回 `/ws/screen`。v1 不做。

### 1.3 流量参考

| 场景 | 流量 |
|------|------|
| 后台挂着，只收事件 | 每分钟几 KB |
| 群消息（Bot 用 `send_msg` 发的完整消息，不流式） | 每条几百字节到几 KB |
| 私聊，Bot 的回复流式显示 | 1–3 KB/s 的短时峰值 |
| 打开某个 Bot 的运行轨迹（**只有打开时才推送**） | 平均 2–10 KB/s |
| 打开 Agent Computer 画面（走 `/ws/screen`） | 100–700 KB/s（只在打开时产生） |

文本帧启用 `permessage-deflate` 压缩。

### 1.4 鉴权

- `/ws`、`/ws/screen` 握手和每个 HTTP 请求都带请求头 `Authorization: Bearer <访问密码>`。如果某个平台无法设置请求头，可以改用查询参数 `?token=<访问密码>`。
- 密码错误：返回 HTTP 401，body 为 `{"error":"unauthorized"}`。
- 服务端还没有设置密码：返回 HTTP 403，body 为 `{"error":"setup_required"}`。客户端应提示「请先在 Bot 主机上打开 localhost:7788/admin 设置密码」。
- `GET /api/v1/health` 不需要鉴权。

### 1.5 握手、心跳、断线补发

```text
客户端                                          服务端
  │── WebSocket 握手（带密码）─────────────────────▶│
  │◀─ evt hello: Hello
  │── req session.resume {last_seq, client} ──────▶│
  │◀─ res {mode: "replay" | "reset"}
  │◀─ （replay 时）依次补发 seq > last_seq 的持久事件
  │◀─ evt sync.done {seq}
  │   … 正常收发 …
  │── WebSocket ping（每 20 秒）──────────────────▶│   服务端 60 秒收不到任何帧就断开
```

- `last_seq`：客户端已经处理过的最后一个**持久事件**的序号，每台 Host 单独保存。首次连接时传 `0`，服务端返回 `reset`。
- 服务端保留最近 7 天或最近 100,000 个持久事件，以先到者为准。客户端落后更多时，服务端返回 `reset`。
- 收到 `reset` 后，客户端调用 `bootstrap` 重建本地状态，并把 `last_seq` 设为返回值里的 `seq`。
- 重连退避：1s、2s、4s … 最长 30s，带 ±20% 随机抖动。
- 无法发送 WebSocket ping 的平台，改为每 20 秒调用一次 `ping` 方法。
- 主连接断开时，运行轨迹的订阅全部失效，重连后需要重新订阅。画面连接独立重连。

---

## 2. 帧格式

### 2.1 文本帧

```ts
// 请求（客户端 → 服务端）
{ v: 1, kind: "req", id: string, method: string, params: object }
// 响应（服务端 → 客户端）
{ v: 1, kind: "res", id: string, ok: true,  result: object }
{ v: 1, kind: "res", id: string, ok: false, error: ErrorBody }
// 持久事件（有全局 seq，断线后补发）
{ v: 1, kind: "evt", seq: number, event: string, data: object }
// 临时事件（没有 seq，不补发）
{ v: 1, kind: "evt", event: string, data: object }
```

- `id` 由客户端生成，在本连接内唯一即可。
- 所有**有写操作**的方法都接受 `client_request_id: string`（UUID）。服务端按它做幂等处理：重连后重发同一个请求，返回第一次的结果，不会重复执行。
- 请求超时：客户端等待 30 秒没有响应，视为失败；写操作需要用同一个 `client_request_id` 重试。

### 2.2 错误

```ts
interface ErrorBody { code: ErrorCode; message: string; details?: object }
type ErrorCode =
  | "unauthorized" | "setup_required" | "version_unsupported"
  | "invalid_params" | "not_found" | "forbidden"   // forbidden：例如删除主 Bot
  | "conflict"                                     // 状态冲突：例如对已结束的任务调用 stop
  | "rate_limited" | "busy" | "unavailable"         // unavailable：例如没有配置 APNs
  | "internal"
```

### 2.3 二进制帧

主连接 `/ws` 上没有二进制帧。二进制帧只出现在 `/ws/screen` 上，格式见第 8 节。

---

## 3. 通用类型

```ts
type Id = string          // UUIDv7，带前缀：bot_ chat_ msg_ prj_ asg_ art_ apr_ que_ run_ rtn_ rrn_ prv_ dev_ upl_
type Time = string        // RFC 3339，UTC，例如 "2026-10-09T10:19:02.312Z"
type DateStr = string     // "YYYY-MM-DD"（按 Host 时区）
type ModelRef = string    // "<provider_id>/<model_id>"，例如 "prv_01…/claude-sonnet-4-5"
type Money = number | null // 单位是 settings.currency；模型没有配置单价时为 null

interface UsageTotals {
  input_tokens: number; output_tokens: number;
  cache_read_tokens: number; cache_write_tokens: number;
  requests: number; cost: Money;
}

type Sender =
  | { kind: "user" }
  | { kind: "bot"; bot_id: Id }
  | { kind: "system" }
```

---

## 4. 对象

### 4.1 Hello

```ts
interface Hello {
  protocol: 1;
  server_version: string;      // 例如 "0.1.0"
  node_id: string;             // 这台 Host 的唯一标识
  host_name: string;
  server_time: Time;
  last_seq: number;            // 服务端当前最新的持久事件序号
  timezone: string;            // Host 的时区，例如 "Asia/Shanghai"
  currency: string;            // 例如 "CNY"
  features: string[];          // 可选能力，例如 ["browser", "apns", "web_search"]
}
```

### 4.2 Bot

```ts
interface Bot {
  id: Id;
  name: string;                // 1–32 个字符
  label: string;               // 头衔，0–64 个字符
  description: string;         // 长期规则，0–4000 个字符
  avatar: Avatar;
  is_main: boolean;            // 主 Bot：全局唯一，不能删除
  model: ModelRef | null;      // null = 使用 settings.models 里的默认值
  max_parallel: number;        // 1–8；主 Bot 忽略这个字段
  tools: ToolToggles;          // 主 Bot 忽略这个字段
  browser_mode: BrowserMode;
  pinned: boolean;
  hidden: boolean;
  notifications: boolean;
  dm_chat_id: Id;              // 和这个 Bot 的私聊会话
  created_at: Time;
  updated_at: Time;
  status: BotStatus;
}
type Avatar =
  | { kind: "bean"; color: number }            // 0–9，对应 DESIGN 里的 10 种颜色
  | { kind: "emoji"; emoji: string }
  | { kind: "image"; file: FileRef }
interface ToolToggles { files: boolean; bash: boolean; browser: boolean; subagent: boolean; web: boolean; mcp: boolean }
type BrowserMode = "headless" | "headless_profile" | "attach"
interface BotStatus {
  summary: "idle" | "working" | "waiting_user" | "blocked";   // 综合状态，用于侧栏
  active: number;      // 正在执行的任务数
  queued: number;      // 排队中的任务数
  waiting: number;     // 挂起等待中的任务数（等用户或等其他 Bot）
}
```

### 4.3 Chat

```ts
interface Chat {
  id: Id;
  kind: "main" | "direct" | "project" | "bot_dm";
    // main = 和主 Bot 的私聊；direct = 和其他 Bot 的私聊；project = 群；bot_dm = Bot 之间的私信（只读）
  title: string;               // 私聊为 Bot 名字；群为项目名；bot_dm 为 "A ↔ B"
  bot_id: Id | null;           // main / direct：对应的 Bot
  project_id: Id | null;       // project：对应的项目
  member_bot_ids: Id[];        // project / bot_dm 的成员
  last_message: MessagePreview | null;
  last_seq: number;            // 会话内最新的消息序号
  last_read_seq: number;
  unread: number;
  attention: "none" | "unread" | "working" | "waiting_user" | "blocked" | "review";
  pinned: boolean;
  muted: boolean;
  updated_at: Time;
}
interface MessagePreview { message_id: Id; sender: Sender; text: string; created_at: Time }  // text 是纯文本，最多 120 个字符
```

### 4.4 Message

```ts
interface Message {
  id: Id;
  chat_id: Id;
  seq: number;                 // 会话内单调递增
  sender: Sender;
  created_at: Time;
  edited_at: Time | null;
  deleted: boolean;
  reply_to: Id | null;         // 回复（线程）的根消息
  thread_count: number;        // 以这条消息为根的回复数
  mentions: Mention[];
  blocks: Block[];
  fallback_text: string;       // 纯文本版本，用于通知、搜索，以及渲染不认识的块
  intent: Intent | null;       // Bot 通过 send_msg 发出的消息才有
  assignment_id: Id | null;    // 这条消息属于哪个任务
  streaming: boolean;          // 只有私聊里 Bot 正在流式输出的回复为 true
  delivery: Delivery[];        // 只出现在用户 @ Bot 的消息上，见 4.5
  reactions: Reaction[];
}
type Mention =
  | { kind: "bot"; bot_id: Id; instruction: string | null }
  | { kind: "main" }
  | { kind: "user" }
  | { kind: "everyone" }
type Intent = "ack" | "progress" | "decision" | "done" | "blocked"
interface Reaction { emoji: string; by: Sender[] }
```

**群里 Bot 发出的每条消息都来自它调用的 `send_msg` 工具**（定义见 PLAN 5.3.2），每条都是完整的，**不会有流式片段**。intent 与消息、任务状态的对应关系：

| intent | 生成的块 | 任务状态 | 其他效果 |
|--------|---------|---------|---------|
| `ack` | `text` | 不变（派发时已经是 `working`） | — |
| `progress` | `progress` | 不变 | — |
| `decision` | `text`；带选项时为 `question` | `waiting_user` 或 `waiting_bot` | 被 @ 的 Bot 收到一个回答问题的任务；@ 了用户时推送 |
| `done` | `completion` | `done` | 被 @ 的 Bot 收到新任务（交接）；@主 Bot 时主 Bot 被唤醒，去汇总并提醒用户验收 |
| `blocked` | `blocked` | `blocked` | 主 Bot 被唤醒跟进；@ 了用户时推送 |

### 4.5 插话的送达状态

```ts
interface Delivery { bot_id: Id; assignment_id: Id | null; state: "queued" | "delivered" | "read"; at: Time }
```

用户在群里 @ 一个正在干活或正在挂起等待的 Bot 时，这条消息会作为 steer 进入它的任务。服务端**不会立即生成回复**，而是通过 `message.updated` 依次更新 `delivery`：
- `queued`：已收到，Bot 正在执行当前步骤。
- `delivered`：已经注入任务，下一次模型调用就会看到。
- `read`：这次模型调用已经包含了这条消息。

如果 Bot 在这个群里没有进行中的任务，服务端会创建一个新任务，`assignment_id` 指向它，`state` 直接为 `delivered`。

### 4.6 Block（消息块）

每个块都有 `type` 字段。客户端遇到不认识的 `type`，用 `Message.fallback_text` 渲染。

```ts
type Block =
  | { type: "text"; markdown: string }
  | { type: "image"; file: FileRef; width: number | null; height: number | null }
  | { type: "file"; file: FileRef }
  | { type: "task_card"; assignment_id: Id }                  // 系统生成，按 Assignment 的状态渲染
  | { type: "completion"; summary: string; artifacts: ArtifactRef[];
      next: { bot_id: Id; instruction: string }[]; notify_main: boolean }
  | { type: "progress"; text: string }
  | { type: "blocked"; reason: string }
  | { type: "question"; question_id: Id }                     // 按 Question 对象渲染
  | { type: "project_card"; project_id: Id }                  // 新群卡片
  | { type: "review_card"; project_id: Id; artifacts: ArtifactRef[];
      state: "pending" | "confirmed" | "changes_requested" }  // 待验收卡片
  | { type: "delegation"; bot_id: Id; assignment_id: Id }    // 「↪ 交给 调研」
  | { type: "approval"; approval_id: Id }                     // 审批卡片，只出现在私聊里
  | { type: "approval_ref"; approval_id: Id; chat_id: Id }   // 群里的「⚑ 在私聊里等待你审批」
  | { type: "takeover_request"; bot_id: Id; reason: string; state: "pending" | "active" | "done" }
  | { type: "bot_dm_ref"; chat_id: Id; count: number }       // 「✉ A 私信了 B」
  | { type: "system"; code: SystemCode; text: string }
  | { type: "loop_paused"; root_message_id: Id; hops: number; state: "paused" | "continued" | "ended" }

type SystemCode = "member_added" | "member_removed" | "project_status" | "renamed" | "task_stopped"
                | "task_no_report" | "info"
interface FileRef { root: "project" | "bot" | "upload"; root_id: Id; path: string; name: string; size: number; mime: string }
interface ArtifactRef { artifact_id: Id; title: string; path_or_url: string }
```

### 4.7 Project 与 Announcement

```ts
interface Project {
  id: Id;
  chat_id: Id;                 // 一个项目对应一个群
  name: string;                // 也是群名
  slug: string;                // Home 目录名
  goal: string;
  flow: string[];              // 例如 ["产品", "编码", "测试"]，只用于展示
  deadline: DateStr | null;
  home_path: string;           // 例如 "~/MacBot/projects/login/"
  status: "active" | "review" | "done" | "archived";
  lead_bot_id: Id;             // 负责人，默认是主 Bot
  members: ProjectMember[];    // 主 Bot 加上 1–6 个 Bot
  created_by: Sender;
  created_at: Time;
  updated_at: Time;
  done_at: Time | null;
}
interface ProjectMember { bot_id: Id; role_note: string; joined_at: Time }

interface Announcement {      // 公告：服务端实时拼出来，客户端只读
  project_id: Id;
  members: {
    bot_id: Id; role_note: string;
    state: "idle" | "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked" | "done";
    current_assignment_id: Id | null;
    since: Time | null;
  }[];
  artifacts: Artifact[];
  highlights: { text: string; at: Time }[];   // 项目要点：来自服务端维护的项目记忆，只读，最多 20 条
  updated_at: Time;
}
```

### 4.8 Assignment（任务）

```ts
interface Assignment {
  id: Id;
  project_id: Id | null;       // null = 主 Bot 转交的小事，或者私聊里产生的任务
  origin_chat_id: Id;          // 任务所在的会话
  bot_id: Id;
  title: string;
  instruction: string;
  from: Sender;                // 谁派的
  trigger_message_id: Id | null;
  parent_assignment_id: Id | null;   // 交接链上的上一个任务
  status: AssignmentStatus;
  queue_reason: "bot_parallel_limit" | "global_limit" | "serial_in_project" | null;
  wait: { reason: "decision" | "blocked" | "approval" | "takeover"; message_id: Id | null } | null;
  created_at: Time;
  started_at: Time | null;
  finished_at: Time | null;
  usage: UsageTotals;
  subagents_active: number;
  steers: { message_id: Id; text: string; at: Time; applied_at: Time | null }[];
  result_message_id: Id | null;     // done / blocked 那条 send_msg 消息
  model: ModelRef;
}
type AssignmentStatus = "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
                      | "done" | "failed" | "cancelled"

interface Artifact {
  id: Id; project_id: Id; bot_id: Id; assignment_id: Id;
  title: string; path_or_url: string; kind: "file" | "dir" | "url";
  created_at: Time; updated_at: Time;
}
```

### 4.9 Approval、Question

```ts
interface Approval {
  id: Id; bot_id: Id; assignment_id: Id | null;
  chat_id: Id;                 // 审批卡片所在的私聊
  tool: string;                // 例如 "bash"、"browser_act"
  risk: "write" | "exec" | "external";
  summary: string;             // 一句话说明
  detail: string;              // Markdown，例如完整的命令
  state: "pending" | "allowed_once" | "always_allowed" | "denied" | "expired";
  created_at: Time; decided_at: Time | null;
}
interface Question {
  id: Id; bot_id: Id; assignment_id: Id; chat_id: Id;
  text: string;
  options: string[];           // 可以为空（只能自由回答）
  allow_free_text: boolean;
  state: "pending" | "answered";
  answer: { option_index: number | null; text: string | null; at: Time } | null;
}
```

### 4.10 Skill、Routine

```ts
interface Skill {
  name: string;                // 小写字母、数字和连字符，最多 64 个字符
  description: string;         // 最多 1024 个字符
  source: "builtin" | "user" | "imported" | "draft";   // draft：Bot 生成的草稿，用户确认后才生效
  path: string;                // 技能目录
  files: string[];             // 目录下除 SKILL.md 以外的文件（相对路径）
  enabled: boolean;
  disabled_bot_ids: Id[];
  invocations_7d: { total: number; by_bot: { bot_id: Id; count: number }[] };
  updated_at: Time;
}
interface SkillDetail extends Skill { content: string }   // SKILL.md 全文

interface Routine {
  id: Id; bot_id: Id; project_id: Id | null;
  name: string; instructions: string;
  schedules: { cron: string; label: string }[];   // label 例如 "工作日 09:00"
  timezone: string;
  enabled: boolean;
  next_run_at: Time | null;
  last_run: RoutineRun | null;
  created_at: Time; updated_at: Time;
}
interface RoutineRun {
  id: Id; routine_id: Id; assignment_id: Id | null;
  trigger: "schedule" | "test";
  status: "running" | "done" | "failed" | "skipped";
  started_at: Time; finished_at: Time | null; error: string | null;
}
```

### 4.11 Provider、Model、Settings、Device

```ts
interface Provider {
  id: Id; name: string;
  api_kind: "openai-completions" | "openai-responses" | "anthropic-messages" | "google-generative";
  base_url: string;
  has_key: boolean;            // API Key 只写入，不会返回给客户端
  headers: Record<string, string>;
  created_at: Time; updated_at: Time;
}
interface Model {
  ref: ModelRef; provider_id: Id; model_id: string;
  display_name: string;
  context_window: number; max_output: number;
  caps: { vision: boolean; tools: boolean; reasoning: boolean };
  price: { input_per_mtok: number; output_per_mtok: number;
           cache_read_per_mtok: number; cache_write_per_mtok: number } | null;   // 每百万 token 的价格
  enabled: boolean;
}
interface Settings {
  host_name: string;
  timezone: string;
  currency: string;
  concurrency: { global: number; bot_default: number; subagent_per_run: number;
                 subagent_global: number; loop_hops: number };
  models: { bot_default: ModelRef | null; main: ModelRef | null;
            subagent: ModelRef | "inherit"; maintenance: ModelRef | null };   // maintenance：记忆整理和压缩
  main_bot: { auto_create_project: boolean };
  approvals: { mode: "require" | "always_allow"; rules: ApprovalRule[] };
  browser: { default_mode: BrowserMode; chrome_profile: string;
             stream: { desktop: StreamQuality; mobile: StreamQuality } };
  skills: { extra_dirs: string[] };
  trace: { save_full_requests: boolean };
  web_search: { provider: "brave" | "tavily" | "searxng" | null; endpoint: string | null; has_key: boolean };
  push: { apns_configured: boolean };   // 只读
}
interface ApprovalRule { id: Id; kind: "ask_first" | "auto_allow"; text: string; created_at: Time }
interface StreamQuality { max_width: number; quality: number; max_fps: number }

interface Device {
  id: Id; platform: "macos" | "android" | "ios";
  app_version: string; device_name: string;
  push_token: string | null; last_seen_at: Time;
}
```

### 4.12 TraceItem（运行轨迹）

```ts
interface TraceItem {
  assignment_id: Id | null;    // 私聊对话时为 null，用 chat_id 定位
  chat_id: Id;
  run_id: Id;
  aseq: number;                // 在同一个任务（或私聊线程）内单调递增，用于排序、去重、做游标
  at: Time;
  type: TraceType;
  data: object;                // 见下表
}
```

| type | data | 说明 |
|------|------|------|
| `run.start` | `{ phase: "chat"\|"work"\|"subagent"\|"memory"\|"compact", model: ModelRef, parent_run_id: Id\|null, subagent_task: string\|null }` | 一次运行开始；子代理的 run 带 `parent_run_id` |
| `llm.request` | `{ request_id: Id, model: ModelRef, context: { l0: number, l1: number, l2: number, l3: number, l4: number, total: number }, tools: string[], prompt_ref: string\|null }` | 一次模型调用开始；context 是各层的 token 估算；只有打开了「保存完整请求」才有 `prompt_ref` |
| `llm.response` | `{ request_id, text: string, thinking: string\|null, tool_calls: { call_id, name, args: object }[], stop_reason: string, usage: UsageTotals, latency_ms: number, ttft_ms: number }` | 一次模型调用结束，包含全文 |
| `tool.start` | `{ call_id, name, args: object }` | |
| `tool.end` | `{ call_id, is_error: boolean, preview: string, details: object, truncated: boolean, full_output: FileRef\|null, duration_ms: number }` | `preview` 最多 8 KB；`details` 供界面渲染，例如 edit 的 diff、bash 的退出码、截图 |
| `send_msg` | `{ call_id, intent: Intent, message_id: Id, chat_id: Id }` | Bot 发了一条消息，可以链接过去 |
| `steer` | `{ message_id: Id, text: string, from: Sender }` | 插话被注入 |
| `run.wait` | `{ reason: "decision"\|"blocked"\|"approval"\|"takeover", message_id: Id\|null }` | 挂起等待 |
| `run.resume` | `{ by_message_id: Id\|null }` | 被唤醒 |
| `compaction` | `{ reason: string, before_tokens: number, after_tokens: number }` | 开了一个新段 |
| `run.end` | `{ status: "done"\|"failed"\|"cancelled"\|"suspended", error: string\|null }` | |

另外有两种**临时**的流式片段，不落盘、没有 `aseq`，只在订阅时通过事件推送（见 7.1）：
- `trace.delta`：`{ request_id, channel: "text" | "thinking" | "tool_args", call_id: Id | null, text }`。同一个请求结束时，由 `llm.response` 给出全文，客户端用它替换已经收到的片段。
- `trace.tool_output`：`{ call_id, chunk }`。bash 等长时间运行的工具的实时输出，每次调用最多推 8 KB。

---

## 5. 方法

> 写法：`方法名(params) → result`。所有写操作都可以带 `client_request_id`，下面不再重复列出。

### 5.1 会话与设备

| 方法 | 参数 | 返回 |
|------|------|------|
| `session.resume` | `{ last_seq: number, client: { platform: "macos"\|"android"\|"ios", app_version: string, device_name: string, device_id: string } }` | `{ mode: "replay"\|"reset" }` |
| `ping` | `{}` | `{ server_time: Time }` |
| `bootstrap` | `{}` | `{ seq: number, hello: Hello, bots: Bot[], chats: Chat[], projects: Project[], settings: Settings, pending: PendingItems }` |
| `device.register` | `{ device_id: string, platform, app_version, device_name, push_token: string\|null }` | `{ device: Device }` |

```ts
interface PendingItems { approvals: Approval[]; questions: Question[]; reviews: Id[] /* 待验收的项目 */ }
```

### 5.2 会话与消息

| 方法 | 参数 | 返回 |
|------|------|------|
| `chat.list` | `{ include_archived?: boolean }` | `{ chats: Chat[] }` |
| `chat.get` | `{ chat_id }` | `{ chat: Chat }` |
| `chat.history` | `{ chat_id, before_seq?: number, after_seq?: number, limit?: number /* 默认 50，最大 100 */ }` | `{ messages: Message[], has_more: boolean }`（按 seq 升序） |
| `chat.thread` | `{ chat_id, root_message_id }` | `{ root: Message, replies: Message[] }` |
| `chat.send` | `{ chat_id, text: string, mentions: Mention[], reply_to?: Id, attachments?: Id[] /* upload_id */ }` | `{ message: Message }` |
| `chat.mark_read` | `{ chat_id, seq: number }` | `{}` |
| `chat.react` | `{ message_id, emoji: string, on: boolean }` | `{ message: Message }` |
| `chat.set_pinned` | `{ chat_id, pinned: boolean }` | `{ chat: Chat }` |
| `chat.set_muted` | `{ chat_id, muted: boolean }` | `{ chat: Chat }` |

- `chat.send` 里用户可以用的 mention 只有 `bot`、`main`、`everyone`（`instruction` 必须为 null）。服务端按 PLAN 5.4 的规则路由。
- `bot_dm` 会话是只读的，对它调用 `chat.send` 返回 `forbidden`。
- **强制使用技能**：`text` 以 `/<技能名>` 开头时（例如 `/web-auth-patterns 按这个规范做`），服务端把这个技能的全文加载进本次任务，其余文字作为指令。技能名不存在时，按普通文本处理。

### 5.3 Bot

| 方法 | 参数 | 返回 |
|------|------|------|
| `bot.list` | `{ include_hidden?: boolean }` | `{ bots: Bot[] }` |
| `bot.get` | `{ bot_id }` | `{ bot: Bot }` |
| `bot.create` | `{ name, label?, description?, avatar?, model?, max_parallel?, tools?, browser_mode? }` | `{ bot: Bot, dm_chat: Chat }` |
| `bot.update` | `{ bot_id, patch: Partial<Pick<Bot, "name"\|"label"\|"description"\|"avatar"\|"model"\|"max_parallel"\|"tools"\|"browser_mode"\|"pinned"\|"hidden"\|"notifications">> }` | `{ bot: Bot }` |
| `bot.duplicate` | `{ bot_id, name }` | `{ bot: Bot, dm_chat: Chat }`（复制资料、技能停用设置和定时任务，不复制记忆和历史） |
| `bot.delete` | `{ bot_id }` | `{}`（主 Bot 返回 `forbidden`；有进行中任务时返回 `conflict`） |
| `bot.templates` | `{}` | `{ templates: { id: string, name: string, description: string, bots: { name, label, description, avatar }[] }[] }`（服务端内置的团队模板，例如「产品 + 编码 + 测试」「调研 + 写作」） |
| `bot.create_from_template` | `{ template_id: string }` | `{ bots: Bot[], dm_chats: Chat[] }`（同名 Bot 已经存在时跳过） |

### 5.4 项目（群）

| 方法 | 参数 | 返回 |
|------|------|------|
| `project.list` | `{ status?: Project["status"][] }` | `{ projects: Project[] }` |
| `project.get` | `{ project_id }` | `{ project: Project, announcement: Announcement }` |
| `project.create` | `{ name, goal, member_bot_ids: Id[] /* 1–6 个，不含主 Bot */, flow?: string[], deadline?: DateStr }` | `{ project: Project, chat: Chat }`（主 Bot 自动加入并担任负责人，随后在群里发开场） |
| `project.update` | `{ project_id, patch: { name?, goal?, flow?, deadline? } }` | `{ project: Project }` |
| `project.add_member` | `{ project_id, bot_id, role_note? }` | `{ project: Project }` |
| `project.remove_member` | `{ project_id, bot_id }` | `{ project: Project }`（有进行中任务时先停止） |
| `project.confirm_done` | `{ project_id }` | `{ project: Project }`（`review` 状态下是确认验收；`active` 状态下是用户直接标记完成，进行中的任务会被停止） |
| `project.request_changes` | `{ project_id, text: string }` | `{ message: Message }`（意见会作为一条消息发到群里并 @主 Bot，群回到 `active`） |
| `project.archive` | `{ project_id }` | `{ project: Project }` |
| `project.reopen` | `{ project_id }` | `{ project: Project }` |

### 5.5 任务与运行轨迹

| 方法 | 参数 | 返回 |
|------|------|------|
| `assignment.list` | `{ project_id?, bot_id?, status?: AssignmentStatus[], cursor?: string, limit?: number /* 默认 50 */ }` | `{ items: Assignment[], next_cursor: string\|null }`（按 created_at 倒序） |
| `assignment.get` | `{ assignment_id }` | `{ assignment: Assignment }` |
| `assignment.stop` | `{ assignment_id }` | `{ assignment: Assignment }` |
| `trace.history` | `{ assignment_id?: Id, chat_id?: Id /* 二选一 */, before_aseq?: number, after_aseq?: number, tail?: boolean, limit?: number /* 默认 200，最大 500 */ }` | `{ items: TraceItem[], first_aseq: number\|null, last_aseq: number\|null, has_more_before: boolean, live: boolean }` |
| `trace.subscribe` | `{ assignment_id?: Id, chat_id?: Id, since_aseq: number }` | `{ stream: string, in_flight: { request_id, text: string, thinking: string }[] }` |
| `trace.unsubscribe` | `{ stream: string }` | `{}` |

### 5.6 审批、提问、防循环、接管

| 方法 | 参数 | 返回 |
|------|------|------|
| `approval.list` | `{ state?: Approval["state"][] }` | `{ approvals: Approval[] }` |
| `approval.decide` | `{ approval_id, decision: "allow_once"\|"always_allow"\|"deny" }` | `{ approval: Approval }`（always_allow 会新增一条 auto_allow 规则） |
| `question.answer` | `{ question_id, option_index?: number, text?: string }` | `{ question: Question }` |
| `loop.resolve` | `{ root_message_id, action: "continue"\|"end" }` | `{}` |
| `takeover.start` | `{ bot_id }` | `{}`（之后 `/ws/screen` 上的输入才会被接受；客户端需要已经打开该 Bot 的画面连接） |
| `takeover.release` | `{ bot_id, note?: string }` | `{}`（note 会作为插话交给 Bot） |

### 5.7 工作台

| 方法 | 参数 | 返回 |
|------|------|------|
| `workbench.get` | `{}` | `Workbench` |

```ts
interface Workbench {
  running: number; global_limit: number; subagents_running: number;
  waiting: (
    | { kind: "review"; project_id: Id; since: Time }
    | { kind: "approval"; approval: Approval }
    | { kind: "question"; question: Question }
    | { kind: "takeover"; bot_id: Id; assignment_id: Id; reason: string }
  )[];
  bots: { bot_id: Id; active: number; max_parallel: number; assignments: Assignment[] /* working、queued、waiting_* */ }[];
  done_today: Assignment[];
}
```

客户端可以在本地按 Bot、群或状态重新分组，服务端只提供这一种结构。

### 5.8 技能

| 方法 | 参数 | 返回 |
|------|------|------|
| `skill.list` | `{}` | `{ skills: Skill[] }` |
| `skill.get` | `{ name }` | `{ skill: SkillDetail }` |
| `skill.create` | `{ name, content: string /* SKILL.md 全文 */ }` | `{ skill: Skill }`（name 冲突时返回 `conflict`） |
| `skill.update` | `{ name, content: string }` | `{ skill: Skill }`（builtin 技能返回 `forbidden`） |
| `skill.delete` | `{ name }` | `{}`（builtin 技能返回 `forbidden`） |
| `skill.set_enabled` | `{ name, enabled: boolean, bot_id?: Id /* 只对某个 Bot 生效 */ }` | `{ skill: Skill }` |
| `skill.publish` | `{ name }` | `{ skill: Skill }`（把 `draft` 转为 `user` 并启用；只能用于草稿） |
| `skill.import` | `{ source: { kind: "git"; url: string; subdir?: string } \| { kind: "upload"; upload_id: Id } \| { kind: "path"; path: string } }` | `{ skills: Skill[] }`（zip 先用 HTTP 上传，拿到 `upload_id`） |

### 5.9 定时任务

| 方法 | 参数 | 返回 |
|------|------|------|
| `routine.list` | `{ bot_id?: Id }` | `{ routines: Routine[] }` |
| `routine.create` | `{ bot_id, project_id?: Id, name, instructions, schedules: { cron, label }[], timezone?: string }` | `{ routine: Routine }`（每个 Bot 最多 50 个；两次运行间隔至少 5 分钟） |
| `routine.update` | `{ routine_id, patch: { name?, instructions?, schedules?, timezone?, project_id? } }` | `{ routine: Routine }` |
| `routine.delete` | `{ routine_id }` | `{}` |
| `routine.set_enabled` | `{ routine_id, enabled: boolean }` | `{ routine: Routine }` |
| `routine.test_run` | `{ routine_id }` | `{ run: RoutineRun }` |
| `routine.runs` | `{ routine_id }` | `{ runs: RoutineRun[] }`（最近 20 次） |

### 5.10 模型与设置

| 方法 | 参数 | 返回 |
|------|------|------|
| `provider.list` | `{}` | `{ providers: Provider[], models: Model[] }` |
| `provider.create` | `{ name, api_kind, base_url, api_key?: string, headers?: Record<string,string> }` | `{ provider: Provider }` |
| `provider.update` | `{ provider_id, patch: { name?, base_url?, api_key?, headers? } }` | `{ provider: Provider }` |
| `provider.delete` | `{ provider_id }` | `{}`（还有 Bot 在使用它的模型时返回 `conflict`） |
| `provider.test` | `{ provider_id }` | `{ ok: boolean, latency_ms: number, error: string\|null }` |
| `model.refresh` | `{ provider_id }` | `{ models: Model[] }`（从服务商的 `/models` 拉取列表） |
| `model.upsert` | `{ ref?: ModelRef, provider_id, model_id, display_name?, context_window?, max_output?, caps?, price?, enabled? }` | `{ model: Model }` |
| `model.delete` | `{ ref: ModelRef }` | `{}` |
| `settings.get` | `{}` | `{ settings: Settings }` |
| `settings.update` | `{ patch: DeepPartial<Settings> }` | `{ settings: Settings }`（`push` 字段只读；API Key 类字段用单独的 `*_key` 参数写入） |

### 5.11 仪表盘

| 方法 | 参数 | 返回 |
|------|------|------|
| `usage.summary` | `{ from: Time, to: Time }` | `{ current: UsageTotals & { tasks_done: number }, previous: UsageTotals & { tasks_done: number } }`（previous 是紧挨着的同长度周期） |
| `usage.heatmap` | `{ mode: "calendar"\|"weekhour", from: Time, to: Time, metric: "tokens"\|"cost"\|"requests" }` | calendar：`{ days: { date: DateStr, value: number, tokens: number, cost: Money, requests: number, top_bot_id: Id\|null }[], thresholds: [number, number, number] }`；weekhour：`{ matrix: number[][] /* 7×24，周一在前 */, thresholds: [number, number, number] }` |
| `usage.timeseries` | `{ from, to, granularity: "auto"\|"hour"\|"day"\|"week", dimension: "model"\|"bot"\|"project", metric: "tokens"\|"cost"\|"requests", split_io?: boolean, top?: number /* 默认 6 */ }` | `{ granularity: "hour"\|"day"\|"week", buckets: Time[], series: { key: string, label: string, values: number[], input_values: number[]\|null, output_values: number[]\|null, total: number }[] }` |
| `usage.breakdown` | `{ from, to, dimension: "model"\|"bot"\|"project", drill?: { bot_id: Id } \| { project_id: Id } }` | `{ rows: { key: string, label: string, usage: UsageTotals, sparkline: number[], phases: Record<"chat"\|"work"\|"subagent"\|"coordinate"\|"memory"\|"compact", number> }[] }` |

- `granularity: "auto"` 的规则：时间范围不超过 7 天按小时，不超过 90 天按天，更长按周。
- `top` 之外的维度值合并为 `key: "other"`。`project` 维度另有两个特殊 key：`"dm"`（私聊和转交的小事）和 `"routine"`（不属于群的定时任务）。
- `thresholds` 是热力图 4 档颜色的分界值，按非零值的 25%、50%、75% 分位数计算。

### 5.12 搜索

| 方法 | 参数 | 返回 |
|------|------|------|
| `search` | `{ query: string, kinds?: ("message"\|"chat"\|"bot"\|"artifact"\|"routine")[], limit?: number /* 默认 20 */ }` | `{ results: { kind, id: Id, chat_id: Id\|null, title: string, snippet: string, at: Time\|null }[] }` |

### 5.13 画面

画面不走主连接，见第 8 节。主连接上只有和接管相关的两个方法（5.6）：`takeover.start`、`takeover.release`。

---

## 6. 事件

### 6.1 持久事件（有 `seq`，断线后补发）

| 事件 | data |
|------|------|
| `chat.created` / `chat.updated` | `{ chat: Chat }` |
| `chat.deleted` | `{ chat_id: Id }` |
| `read.updated` | `{ chat_id: Id, last_read_seq: number }`（多端同步已读） |
| `message.created` / `message.updated` | `{ message: Message }` |
| `message.deleted` | `{ chat_id: Id, message_id: Id }` |
| `bot.created` / `bot.updated` | `{ bot: Bot }` |
| `bot.deleted` | `{ bot_id: Id }` |
| `project.created` / `project.updated` | `{ project: Project }` |
| `announcement.updated` | `{ announcement: Announcement }` |
| `assignment.created` / `assignment.updated` | `{ assignment: Assignment }` |
| `artifact.registered` | `{ artifact: Artifact }` |
| `approval.requested` / `approval.resolved` | `{ approval: Approval }` |
| `question.asked` / `question.answered` | `{ question: Question }` |
| `skill.updated` | `{ skill: Skill }` |
| `skill.deleted` | `{ name: string }` |
| `routine.updated` | `{ routine: Routine }` |
| `routine.deleted` | `{ routine_id: Id }` |
| `routine.run` | `{ run: RoutineRun }` |
| `provider.updated` | `{ provider: Provider, models: Model[] }` |
| `provider.deleted` | `{ provider_id: Id }` |
| `settings.updated` | `{ settings: Settings }` |

### 6.2 临时事件（没有 `seq`）

| 事件 | 发给谁 | data |
|------|--------|------|
| `hello` | 刚连上的客户端 | `Hello` |
| `sync.done` | 刚补发完的客户端 | `{ seq: number }` |
| `message.delta` | 所有在线客户端 | `{ chat_id, message_id, text }`：**只用于私聊**里 Bot 回复的流式片段，结束时由 `message.updated`（`streaming: false`）给出全文 |
| `typing` | 所有在线客户端 | `{ chat_id, bot_id, on: boolean }`：**只用于私聊** |
| `bot.status` | 所有在线客户端 | `{ bot_id, status: BotStatus }` |
| `usage.tick` | 所有在线客户端 | `{ assignment_id, usage: UsageTotals }`：每个任务每 10 秒最多一次，用于工作台 |
| `host.status` | 所有在线客户端 | `{ running: number, queued: number, global_limit: number, subagents_running: number }` |
| `trace.item` | **只发给订阅者** | `{ stream, item: TraceItem }` |
| `trace.delta` | **只发给订阅者** | `{ stream, request_id, channel: "text"\|"thinking"\|"tool_args", call_id: Id\|null, text }` |
| `trace.tool_output` | **只发给订阅者** | `{ stream, call_id, chunk }` |

---

## 7. 运行轨迹的查看流程

### 7.1 先拉历史，再订阅（带游标，不会漏）

```text
用户点开某个 Bot 的工作详情（任务卡片、状态条成员、工作台）
client ── req trace.history {assignment_id, tail: true, limit: 200} ───▶ server
       ◀─ res {items, first_aseq, last_aseq, has_more_before, live}
       （live=false：任务已经结束，只是回放，不用订阅）
       ── req trace.subscribe {assignment_id, since_aseq: last_aseq} ──▶   （live=true 时）
       ◀─ res {stream, in_flight}      ← in_flight：正在进行中的模型调用已经输出的部分
       ◀─ evt trace.item（先补发 aseq > since_aseq 的条目，再推送实时条目）
       ◀─ evt trace.delta / trace.tool_output（实时片段）
       往上滚动：req trace.history {assignment_id, before_aseq: first_aseq}
       关闭页面：req trace.unsubscribe {stream}
```

- **不会漏数据**：`trace.subscribe` 带上历史的最后一个 `aseq`，服务端先补发它之后的条目，再推送实时条目。
- 客户端按 `aseq` 排序和去重。`trace.delta` 用 `request_id` 归组；收到同一个 `request_id` 的 `llm.response` 后，用其中的全文替换片段。
- 每个客户端最多同时订阅 8 个 stream。
- **没有任何客户端订阅时，服务端只落盘，不推送。**

---

## 8. Agent Computer 画面（`/ws/screen`）

- **每个 Bot 一个浏览器会话**（登录状态在这个 Bot 的所有任务之间共享）。同一个 Bot 并行的多个任务各用一个标签页；`ScreenState.tabs` 里的 `assignment_id` 标明每个标签页属于哪个任务。
- **建立连接**：`GET /ws/screen?bot_id=<id>&quality=auto|high|low&tab_id=<可选>`，带 `Authorization: Bearer <密码>`。一条连接看一个 Bot 的画面；要同时看多个 Bot，就开多条。
- **连接建立后**，服务端先发一个文本帧 `{ "type": "state", "state": ScreenState }`；之后每当状态变化（谁在操作、标签页切换、URL 变化），都再发一个这样的文本帧。

```ts
interface ScreenState {
  bot_id: Id;
  driver: "bot" | "user" | "idle";
  tabs: { tab_id: string; title: string; url: string; assignment_id: Id | null; active: boolean }[];
  width: number; height: number;
}
```

- **画面帧**（二进制）：

```text
二进制帧 = [4 字节大端整数：头部长度 N][N 字节 UTF-8 JSON 头部][JPEG 数据]
JSON 头部 = { seq: number, tab_id: string, w: number, h: number, ts: number /* 毫秒 */, url: string }
```

- **ack 节流**：客户端每画完一帧，就回一个文本帧 `{ "type": "ack", "seq": N }`。服务端收到上一帧的 ack 之前不发下一帧，并且总是发送最新的一帧，积压的旧帧直接丢弃。
- **画质**：`auto` 时桌面默认最大宽度 1280、画质 70、15 fps，手机默认最大宽度 720、画质 50、10 fps；`high` 是 1600、85、20 fps；`low` 是 640、30、8 fps。
- **切换标签页**（只在用户接管时可用）：`{ "type": "switch_tab", "tab_id": "…" }`。
- **输入**（只有在主连接上调用 `takeover.start` 之后才会被接受）：

```ts
{ type: "input", event: ScreenInput }
type ScreenInput =
  | { type: "mouse"; action: "move" | "down" | "up" | "click"; x: number; y: number; button: "left" | "right" | "middle"; click_count: number }
  | { type: "wheel"; x: number; y: number; dx: number; dy: number }
  | { type: "key"; action: "down" | "up" | "press"; key: string; code: string; text: string | null; modifiers: string[] }
  | { type: "touch"; action: "start" | "move" | "end"; points: { x: number; y: number }[] }
// 坐标是相对于帧画面的像素坐标；服务端换算后转发给 agent-browser
```

- 断开：客户端直接关闭 WebSocket。服务端在没有任何画面连接时，停止向 agent-browser 请求 screencast。

---

## 9. HTTP 接口

| 方法 | 路径 | 参数 | 返回 |
|------|------|------|------|
| GET | `/api/v1/health` | — | `{ ok: true, protocol: 1, version: string, setup_required: boolean }`（不需要鉴权） |
| POST | `/api/v1/rpc` | body：`{ method, params }` | 与 `/ws` 上的同名方法相同：`{ ok, result \| error }` |
| GET | `/api/v1/files` | `root=project\|bot\|upload`、`root_id`、`path` | 文件内容，支持 Range；只能访问项目 Home、Bot 目录和上传区 |
| GET | `/api/v1/files/list` | `root`、`root_id`、`path` | `{ entries: { name, path, is_dir, size, modified_at }[] }` |
| POST | `/api/v1/uploads` | multipart，字段名 `file` | `{ upload_id: Id, file: FileRef }`（单个文件最大 100 MB，24 小时内没有被消息引用就自动删除） |
| GET | `/api/v1/trace/output` | `run_id`、`call_id` | 被截断的工具输出全文（text/plain） |
| GET | `/api/v1/usage/export.csv` | 与 `usage.breakdown` 相同 | CSV 文件 |

## 10. 版本

- `protocol` 是一个整数，当前为 `1`。服务端同时支持 `N` 和 `N-1`。客户端发现 `hello.protocol` 比自己支持的版本高 2 个以上时，提示升级。
- 只新增字段、方法、事件或块类型，属于兼容变更，不升版本。客户端必须忽略不认识的字段和事件，不认识的块用 `fallback_text` 显示。

## 11. 开发用的 mock

- `macbotd --mock --password <pw>`（由 server-mac 提供）：不调用模型，按 `protocol/fixtures/scenarios/*.jsonl` 回放场景。实现本文的全部方法：读类方法返回 fixtures，写类方法返回合理的假结果并发出相应事件。
- `login-feature.jsonl` 覆盖 DESIGN 第 3 章的完整场景，包括 trace 条目和画面帧（几张静态 JPEG 轮播）。
- `protocol/fixtures/` 里每个对象、事件、块类型和 TraceItem 类型至少有一份示例；客户端的契约测试必须能把所有 fixture 反序列化再序列化而不丢字段。
