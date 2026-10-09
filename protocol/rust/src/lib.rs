//! Wire types for the Mac Bot v1 client/server protocol.
//!
//! The protocol deliberately keeps identifiers and timestamps as strings: IDs are
//! UUIDv7 values with a resource prefix and timestamps are RFC3339 UTC strings.
//! Unknown JSON fields are ignored by serde so newer servers remain compatible
//! with older clients.

use schemars::JsonSchema;
use serde::{
    de::{self, DeserializeOwned, Deserializer},
    Deserialize, Serialize,
};
use std::collections::BTreeMap;

pub type Id = String;
pub type Time = String;
pub type DateStr = String;
pub type ModelRef = String;
pub type Money = Option<f64>;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Sender {
    User,
    Bot { bot_id: Id },
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Avatar {
    Bean { color: u8 },
    Emoji { emoji: String },
    Image { file: FileRef },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ToolToggles {
    pub files: bool,
    pub bash: bool,
    pub browser: bool,
    pub subagent: bool,
    pub web: bool,
    pub mcp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    Headless,
    HeadlessProfile,
    Attach,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BotSummary {
    Idle,
    Working,
    WaitingUser,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotStatus {
    pub summary: BotSummary,
    pub active: u32,
    pub queued: u32,
    pub waiting: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub requests: u64,
    pub cost: Money,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FileRef {
    pub root: FileRoot,
    pub root_id: Id,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FileRoot {
    Project,
    Bot,
    Upload,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ArtifactRef {
    pub artifact_id: Id,
    pub title: String,
    pub path_or_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Hello {
    pub protocol: u32,
    pub server_version: String,
    pub node_id: String,
    pub host_name: String,
    pub server_time: Time,
    pub last_seq: u64,
    pub timezone: String,
    pub currency: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Bot {
    pub id: Id,
    pub name: String,
    pub label: String,
    pub description: String,
    pub avatar: Avatar,
    pub is_main: bool,
    pub model: Option<ModelRef>,
    pub max_parallel: u8,
    pub tools: ToolToggles,
    pub browser_mode: BrowserMode,
    pub pinned: bool,
    pub hidden: bool,
    pub notifications: bool,
    pub dm_chat_id: Id,
    pub created_at: Time,
    pub updated_at: Time,
    pub status: BotStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    Main,
    Direct,
    Project,
    BotDm,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MessagePreview {
    pub message_id: Id,
    pub sender: Sender,
    pub text: String,
    pub created_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    None,
    Unread,
    Working,
    WaitingUser,
    Blocked,
    Review,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Chat {
    pub id: Id,
    pub kind: ChatKind,
    pub title: String,
    pub bot_id: Option<Id>,
    pub project_id: Option<Id>,
    pub member_bot_ids: Vec<Id>,
    pub last_message: Option<MessagePreview>,
    pub last_seq: u64,
    pub last_read_seq: u64,
    pub unread: u64,
    pub attention: Attention,
    pub pinned: bool,
    pub muted: bool,
    pub updated_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Mention {
    Bot {
        bot_id: Id,
        instruction: Option<String>,
    },
    Main,
    User,
    Everyone,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Ack,
    Progress,
    Decision,
    Done,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Reaction {
    pub emoji: String,
    pub by: Vec<Sender>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Delivery {
    pub bot_id: Id,
    pub assignment_id: Option<Id>,
    pub state: DeliveryState,
    pub at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Queued,
    Delivered,
    Read,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Block {
    Text {
        markdown: String,
    },
    Image {
        file: FileRef,
        width: Option<u32>,
        height: Option<u32>,
    },
    File {
        file: FileRef,
    },
    TaskCard {
        assignment_id: Id,
    },
    Completion {
        summary: String,
        artifacts: Vec<ArtifactRef>,
        next: Vec<NextTask>,
        notify_main: bool,
    },
    Progress {
        text: String,
    },
    Blocked {
        reason: String,
    },
    Question {
        question_id: Id,
    },
    ProjectCard {
        project_id: Id,
    },
    ReviewCard {
        project_id: Id,
        artifacts: Vec<ArtifactRef>,
        state: ReviewState,
    },
    Delegation {
        bot_id: Id,
        assignment_id: Id,
    },
    Approval {
        approval_id: Id,
    },
    ApprovalRef {
        approval_id: Id,
        chat_id: Id,
    },
    TakeoverRequest {
        bot_id: Id,
        reason: String,
        state: TakeoverState,
    },
    BotDmRef {
        chat_id: Id,
        count: u32,
    },
    System {
        code: SystemCode,
        text: String,
    },
    LoopPaused {
        root_message_id: Id,
        hops: u32,
        state: LoopState,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NextTask {
    pub bot_id: Id,
    pub instruction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Pending,
    Confirmed,
    ChangesRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverState {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LoopState {
    Paused,
    Continued,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SystemCode {
    MemberAdded,
    MemberRemoved,
    ProjectStatus,
    Renamed,
    TaskStopped,
    TaskNoReport,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Message {
    pub id: Id,
    pub chat_id: Id,
    pub seq: u64,
    pub sender: Sender,
    pub created_at: Time,
    pub edited_at: Option<Time>,
    pub deleted: bool,
    pub reply_to: Option<Id>,
    pub thread_count: u32,
    pub mentions: Vec<Mention>,
    pub blocks: Vec<Block>,
    pub fallback_text: String,
    pub intent: Option<Intent>,
    pub assignment_id: Option<Id>,
    pub streaming: bool,
    pub delivery: Vec<Delivery>,
    pub reactions: Vec<Reaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Project {
    pub id: Id,
    pub chat_id: Id,
    pub name: String,
    pub slug: String,
    pub goal: String,
    pub flow: Vec<String>,
    pub deadline: Option<DateStr>,
    pub home_path: String,
    pub status: ProjectStatus,
    pub lead_bot_id: Id,
    pub members: Vec<ProjectMember>,
    pub created_by: Sender,
    pub created_at: Time,
    pub updated_at: Time,
    pub done_at: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Review,
    Done,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectMember {
    pub bot_id: Id,
    pub role_note: String,
    pub joined_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Announcement {
    pub project_id: Id,
    pub members: Vec<AnnouncementMember>,
    pub artifacts: Vec<Artifact>,
    pub highlights: Vec<Highlight>,
    pub updated_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AnnouncementMember {
    pub bot_id: Id,
    pub role_note: String,
    pub state: MemberState,
    pub current_assignment_id: Option<Id>,
    pub since: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MemberState {
    Idle,
    Queued,
    Working,
    WaitingUser,
    WaitingBot,
    Blocked,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Highlight {
    pub text: String,
    pub at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Assignment {
    pub id: Id,
    pub project_id: Option<Id>,
    pub origin_chat_id: Id,
    pub bot_id: Id,
    pub title: String,
    pub instruction: String,
    pub from: Sender,
    pub trigger_message_id: Option<Id>,
    pub parent_assignment_id: Option<Id>,
    pub status: AssignmentStatus,
    pub queue_reason: Option<QueueReason>,
    pub wait: Option<WaitState>,
    pub created_at: Time,
    pub started_at: Option<Time>,
    pub finished_at: Option<Time>,
    pub usage: UsageTotals,
    pub subagents_active: u32,
    pub steers: Vec<Steer>,
    pub result_message_id: Option<Id>,
    pub model: ModelRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentStatus {
    Queued,
    Working,
    WaitingUser,
    WaitingBot,
    Blocked,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QueueReason {
    BotParallelLimit,
    GlobalLimit,
    SerialInProject,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WaitState {
    pub reason: WaitReason,
    pub message_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    Decision,
    Blocked,
    Approval,
    Takeover,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Steer {
    pub message_id: Id,
    pub text: String,
    pub at: Time,
    pub applied_at: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Artifact {
    pub id: Id,
    pub project_id: Id,
    pub bot_id: Id,
    pub assignment_id: Id,
    pub title: String,
    pub path_or_url: String,
    pub kind: ArtifactKind,
    pub created_at: Time,
    pub updated_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    File,
    Dir,
    Url,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Approval {
    pub id: Id,
    pub bot_id: Id,
    pub assignment_id: Option<Id>,
    pub chat_id: Id,
    pub tool: String,
    pub risk: ApprovalRisk,
    pub summary: String,
    pub detail: String,
    pub state: ApprovalState,
    pub created_at: Time,
    pub decided_at: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Write,
    Exec,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    AllowedOnce,
    AlwaysAllowed,
    Denied,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Question {
    pub id: Id,
    pub bot_id: Id,
    pub assignment_id: Id,
    pub chat_id: Id,
    pub text: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
    pub state: QuestionState,
    pub answer: Option<QuestionAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    Pending,
    Answered,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct QuestionAnswer {
    pub option_index: Option<u32>,
    pub text: Option<String>,
    pub at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub path: String,
    pub files: Vec<String>,
    pub enabled: bool,
    pub disabled_bot_ids: Vec<Id>,
    pub invocations_7d: Invocations7d,
    pub updated_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Builtin,
    User,
    Imported,
    Draft,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Invocations7d {
    pub total: u64,
    pub by_bot: Vec<BotInvocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotInvocation {
    pub bot_id: Id,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillDetail {
    #[serde(flatten)]
    pub skill: Skill,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Routine {
    pub id: Id,
    pub bot_id: Id,
    pub project_id: Option<Id>,
    pub name: String,
    pub instructions: String,
    pub schedules: Vec<Schedule>,
    pub timezone: String,
    pub enabled: bool,
    pub next_run_at: Option<Time>,
    pub last_run: Option<RoutineRun>,
    pub created_at: Time,
    pub updated_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Schedule {
    pub cron: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineRun {
    pub id: Id,
    pub routine_id: Id,
    pub assignment_id: Option<Id>,
    pub trigger: RoutineTrigger,
    pub status: RoutineRunStatus,
    pub started_at: Time,
    pub finished_at: Option<Time>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RoutineTrigger {
    Schedule,
    Test,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RoutineRunStatus {
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Provider {
    pub id: Id,
    pub name: String,
    pub api_kind: ApiKind,
    pub base_url: String,
    pub has_key: bool,
    pub headers: BTreeMap<String, String>,
    pub created_at: Time,
    pub updated_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ApiKind {
    OpenaiCompletions,
    OpenaiResponses,
    AnthropicMessages,
    GoogleGenerative,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Model {
    pub r#ref: ModelRef,
    pub provider_id: Id,
    pub model_id: String,
    pub display_name: String,
    pub context_window: u64,
    pub max_output: u64,
    pub caps: ModelCaps,
    pub price: Option<ModelPrice>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelCaps {
    pub vision: bool,
    pub tools: bool,
    pub reasoning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_write_per_mtok: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Settings {
    pub host_name: String,
    pub timezone: String,
    pub currency: String,
    pub concurrency: Concurrency,
    pub models: ModelDefaults,
    pub main_bot: MainBotSettings,
    pub approvals: ApprovalSettings,
    pub browser: BrowserSettings,
    pub skills: SkillSettings,
    pub trace: TraceSettings,
    pub web_search: WebSearchSettings,
    pub push: PushSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Concurrency {
    pub global: u32,
    pub bot_default: u32,
    pub subagent_per_run: u32,
    pub subagent_global: u32,
    pub loop_hops: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelDefaults {
    pub bot_default: Option<ModelRef>,
    pub main: Option<ModelRef>,
    pub subagent: SubagentModel,
    pub maintenance: Option<ModelRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum SubagentModel {
    Ref(ModelRef),
    Inherit,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MainBotSettings {
    pub auto_create_project: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalSettings {
    pub mode: ApprovalMode,
    pub rules: Vec<ApprovalRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Require,
    AlwaysAllow,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalRule {
    pub id: Id,
    pub kind: ApprovalRuleKind,
    pub text: String,
    pub created_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRuleKind {
    AskFirst,
    AutoAllow,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BrowserSettings {
    pub default_mode: BrowserMode,
    pub chrome_profile: String,
    pub stream: StreamSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StreamSettings {
    pub desktop: StreamQuality,
    pub mobile: StreamQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StreamQuality {
    pub max_width: u32,
    pub quality: u8,
    pub max_fps: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillSettings {
    pub extra_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceSettings {
    pub save_full_requests: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WebSearchSettings {
    pub provider: Option<WebSearchProvider>,
    pub endpoint: Option<String>,
    pub has_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchProvider {
    Brave,
    Tavily,
    Searxng,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PushSettings {
    pub apns_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Device {
    pub id: Id,
    pub platform: Platform,
    pub app_version: String,
    pub device_name: String,
    pub push_token: Option<String>,
    pub last_seen_at: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Android,
    Ios,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
pub struct TraceItem {
    pub assignment_id: Option<Id>,
    pub chat_id: Id,
    pub run_id: Id,
    pub aseq: u64,
    pub at: Time,
    #[serde(rename = "type")]
    pub item_type: TraceType,
    pub data: TraceData,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum TraceType {
    #[serde(rename = "run.start")]
    RunStart,
    #[serde(rename = "llm.request")]
    LlmRequest,
    #[serde(rename = "llm.response")]
    LlmResponse,
    #[serde(rename = "tool.start")]
    ToolStart,
    #[serde(rename = "tool.end")]
    ToolEnd,
    SendMsg,
    Steer,
    #[serde(rename = "run.wait")]
    RunWait,
    #[serde(rename = "run.resume")]
    RunResume,
    Compaction,
    #[serde(rename = "run.end")]
    RunEnd,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum TraceData {
    RunStart {
        phase: TracePhase,
        model: ModelRef,
        parent_run_id: Option<Id>,
        subagent_task: Option<String>,
    },
    LlmRequest {
        request_id: Id,
        model: ModelRef,
        context: TraceContext,
        tools: Vec<String>,
        prompt_ref: Option<String>,
    },
    LlmResponse {
        request_id: Id,
        text: String,
        thinking: Option<String>,
        tool_calls: Vec<TraceToolCall>,
        stop_reason: String,
        usage: UsageTotals,
        latency_ms: u64,
        ttft_ms: u64,
    },
    ToolStart {
        call_id: Id,
        name: String,
        args: BTreeMap<String, serde_json::Value>,
    },
    ToolEnd {
        call_id: Id,
        is_error: bool,
        preview: String,
        details: BTreeMap<String, serde_json::Value>,
        truncated: bool,
        full_output: Option<FileRef>,
        duration_ms: u64,
    },
    SendMsg {
        call_id: Id,
        intent: Intent,
        message_id: Id,
        chat_id: Id,
    },
    Steer {
        message_id: Id,
        text: String,
        from: Sender,
    },
    RunWait {
        reason: WaitReason,
        message_id: Option<Id>,
    },
    Compaction {
        reason: String,
        before_tokens: u64,
        after_tokens: u64,
    },
    RunEnd {
        status: RunEndStatus,
        error: Option<String>,
    },
    RunResume {
        by_message_id: Option<Id>,
    },
}

fn decode_value<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

impl TraceData {
    /// Decode trace data only after the outer `type` has been read. This is
    /// deliberately explicit because several trace payloads contain optional
    /// fields and an untagged serde enum would otherwise accept the wrong one.
    pub fn decode(item_type: &TraceType, value: serde_json::Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct RunStartD {
            phase: TracePhase,
            model: ModelRef,
            parent_run_id: Option<Id>,
            subagent_task: Option<String>,
        }
        #[derive(Deserialize)]
        struct LlmRequestD {
            request_id: Id,
            model: ModelRef,
            context: TraceContext,
            tools: Vec<String>,
            prompt_ref: Option<String>,
        }
        #[derive(Deserialize)]
        struct LlmResponseD {
            request_id: Id,
            text: String,
            thinking: Option<String>,
            tool_calls: Vec<TraceToolCall>,
            stop_reason: String,
            usage: UsageTotals,
            latency_ms: u64,
            ttft_ms: u64,
        }
        #[derive(Deserialize)]
        struct ToolStartD {
            call_id: Id,
            name: String,
            args: BTreeMap<String, serde_json::Value>,
        }
        #[derive(Deserialize)]
        struct ToolEndD {
            call_id: Id,
            is_error: bool,
            preview: String,
            details: BTreeMap<String, serde_json::Value>,
            truncated: bool,
            full_output: Option<FileRef>,
            duration_ms: u64,
        }
        #[derive(Deserialize)]
        struct SendMsgD {
            call_id: Id,
            intent: Intent,
            message_id: Id,
            chat_id: Id,
        }
        #[derive(Deserialize)]
        struct SteerD {
            message_id: Id,
            text: String,
            from: Sender,
        }
        #[derive(Deserialize)]
        struct RunWaitD {
            reason: WaitReason,
            message_id: Option<Id>,
        }
        #[derive(Deserialize)]
        struct RunResumeD {
            by_message_id: Option<Id>,
        }
        #[derive(Deserialize)]
        struct CompactionD {
            reason: String,
            before_tokens: u64,
            after_tokens: u64,
        }
        #[derive(Deserialize)]
        struct RunEndD {
            status: RunEndStatus,
            error: Option<String>,
        }
        match item_type {
            TraceType::RunStart => decode_value::<RunStartD>(value).map(|x| Self::RunStart {
                phase: x.phase,
                model: x.model,
                parent_run_id: x.parent_run_id,
                subagent_task: x.subagent_task,
            }),
            TraceType::LlmRequest => decode_value::<LlmRequestD>(value).map(|x| Self::LlmRequest {
                request_id: x.request_id,
                model: x.model,
                context: x.context,
                tools: x.tools,
                prompt_ref: x.prompt_ref,
            }),
            TraceType::LlmResponse => {
                decode_value::<LlmResponseD>(value).map(|x| Self::LlmResponse {
                    request_id: x.request_id,
                    text: x.text,
                    thinking: x.thinking,
                    tool_calls: x.tool_calls,
                    stop_reason: x.stop_reason,
                    usage: x.usage,
                    latency_ms: x.latency_ms,
                    ttft_ms: x.ttft_ms,
                })
            }
            TraceType::ToolStart => decode_value::<ToolStartD>(value).map(|x| Self::ToolStart {
                call_id: x.call_id,
                name: x.name,
                args: x.args,
            }),
            TraceType::ToolEnd => decode_value::<ToolEndD>(value).map(|x| Self::ToolEnd {
                call_id: x.call_id,
                is_error: x.is_error,
                preview: x.preview,
                details: x.details,
                truncated: x.truncated,
                full_output: x.full_output,
                duration_ms: x.duration_ms,
            }),
            TraceType::SendMsg => decode_value::<SendMsgD>(value).map(|x| Self::SendMsg {
                call_id: x.call_id,
                intent: x.intent,
                message_id: x.message_id,
                chat_id: x.chat_id,
            }),
            TraceType::Steer => decode_value::<SteerD>(value).map(|x| Self::Steer {
                message_id: x.message_id,
                text: x.text,
                from: x.from,
            }),
            TraceType::RunWait => decode_value::<RunWaitD>(value).map(|x| Self::RunWait {
                reason: x.reason,
                message_id: x.message_id,
            }),
            TraceType::RunResume => decode_value::<RunResumeD>(value).map(|x| Self::RunResume {
                by_message_id: x.by_message_id,
            }),
            TraceType::Compaction => decode_value::<CompactionD>(value).map(|x| Self::Compaction {
                reason: x.reason,
                before_tokens: x.before_tokens,
                after_tokens: x.after_tokens,
            }),
            TraceType::RunEnd => decode_value::<RunEndD>(value).map(|x| Self::RunEnd {
                status: x.status,
                error: x.error,
            }),
        }
    }
}

impl<'de> Deserialize<'de> for TraceItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawTraceItem {
            assignment_id: Option<Id>,
            chat_id: Id,
            run_id: Id,
            aseq: u64,
            at: Time,
            #[serde(rename = "type")]
            item_type: TraceType,
            data: serde_json::Value,
        }
        let raw = RawTraceItem::deserialize(deserializer)?;
        let data = TraceData::decode(&raw.item_type, raw.data).map_err(de::Error::custom)?;
        Ok(Self {
            assignment_id: raw.assignment_id,
            chat_id: raw.chat_id,
            run_id: raw.run_id,
            aseq: raw.aseq,
            at: raw.at,
            item_type: raw.item_type,
            data,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TracePhase {
    Chat,
    Work,
    Subagent,
    Memory,
    Compact,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceContext {
    pub l0: u64,
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
    pub l4: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceToolCall {
    pub call_id: Id,
    pub name: String,
    pub args: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunEndStatus {
    Done,
    Failed,
    Cancelled,
    Suspended,
}

// ---- RPC methods ---------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ClientInfo {
    pub platform: Platform,
    pub app_version: String,
    pub device_name: String,
    pub device_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionResumeParams {
    pub last_seq: u64,
    pub client: ClientInfo,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionResumeResult {
    pub mode: ResumeMode,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeMode {
    Replay,
    Reset,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PingResult {
    pub server_time: Time,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BootstrapResult {
    pub seq: u64,
    pub hello: Hello,
    pub bots: Vec<Bot>,
    pub chats: Vec<Chat>,
    pub projects: Vec<Project>,
    pub settings: Settings,
    pub pending: PendingItems,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PendingItems {
    pub approvals: Vec<Approval>,
    pub questions: Vec<Question>,
    pub reviews: Vec<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DeviceRegisterParams {
    pub device_id: String,
    pub platform: Platform,
    pub app_version: String,
    pub device_name: String,
    pub push_token: Option<String>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}

/// Every mutating RPC accepts this id. The server stores the first result and
/// returns it when a client retries the request after reconnecting.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
pub struct WriteMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

/// A patch field has three wire states: omitted, explicit `null`, or a value.
/// Plain `Option<T>` cannot represent the first two states distinctly because
/// serde maps both a missing field and JSON null to `None`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Patch<T> {
    #[default]
    Unset,
    Null,
    Value(T),
}

impl<T> Patch<T> {
    pub fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }
}
impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Unset => serializer.serialize_none(),
            Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}
impl<'de, T: DeserializeOwned> Deserialize<'de> for Patch<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<T>::deserialize(deserializer)?;
        Ok(match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}
impl<T: JsonSchema> JsonSchema for Patch<T> {
    fn schema_name() -> String {
        format!("Patch{}", T::schema_name())
    }
    fn json_schema(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        generator.subschema_for::<Option<T>>()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DeviceRegisterResult {
    pub device: Device,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EmptyParams {}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EmptyResult {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IncludeArchivedParams {
    pub include_archived: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatListResult {
    pub chats: Vec<Chat>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatIdParams {
    pub chat_id: Id,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatGetResult {
    pub chat: Chat,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatHistoryParams {
    pub chat_id: Id,
    pub before_seq: Option<u64>,
    pub after_seq: Option<u64>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatHistoryResult {
    pub messages: Vec<Message>,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatThreadParams {
    pub chat_id: Id,
    pub root_message_id: Id,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatThreadResult {
    pub root: Message,
    pub replies: Vec<Message>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatSendParams {
    pub chat_id: Id,
    pub text: String,
    pub mentions: Vec<Mention>,
    pub reply_to: Option<Id>,
    pub attachments: Option<Vec<Id>>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatSendResult {
    pub message: Message,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatMarkReadParams {
    pub chat_id: Id,
    pub seq: u64,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatReactParams {
    pub message_id: Id,
    pub emoji: String,
    pub on: bool,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatReactResult {
    pub message: Message,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatPinnedParams {
    pub chat_id: Id,
    pub pinned: bool,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatPinnedResult {
    pub chat: Chat,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatMutedParams {
    pub chat_id: Id,
    pub muted: bool,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChatMutedResult {
    pub chat: Chat,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotListParams {
    pub include_hidden: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotListResult {
    pub bots: Vec<Bot>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotIdParams {
    pub bot_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotGetResult {
    pub bot: Bot,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotCreateParams {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<Avatar>,
    pub model: Option<ModelRef>,
    pub max_parallel: Option<u8>,
    pub tools: Option<ToolToggles>,
    pub browser_mode: Option<BrowserMode>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotCreateResult {
    pub bot: Bot,
    pub dm_chat: Chat,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Avatar>,
    /// `None` means omitted; `Some(None)` explicitly clears the model.
    #[serde(default, skip_serializing_if = "Patch::is_unset")]
    pub model: Patch<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolToggles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_mode: Option<BrowserMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotUpdateParams {
    pub bot_id: Id,
    pub patch: BotPatch,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotUpdateResult {
    pub bot: Bot,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotDuplicateParams {
    pub bot_id: Id,
    pub name: String,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotDuplicateResult {
    pub bot: Bot,
    pub dm_chat: Chat,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotTemplatesResult {
    pub templates: Vec<BotTemplate>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub bots: Vec<BotTemplateEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotTemplateEntry {
    pub name: String,
    pub label: String,
    pub description: String,
    pub avatar: Avatar,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CreateFromTemplateParams {
    pub template_id: String,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CreateFromTemplateResult {
    pub bots: Vec<Bot>,
    pub dm_chats: Vec<Chat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectListParams {
    pub status: Option<Vec<ProjectStatus>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectListResult {
    pub projects: Vec<Project>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectIdParams {
    pub project_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectGetResult {
    pub project: Project,
    pub announcement: Announcement,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectCreateParams {
    pub name: String,
    pub goal: String,
    pub member_bot_ids: Vec<Id>,
    pub flow: Option<Vec<String>>,
    pub deadline: Option<DateStr>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectCreateResult {
    pub project: Project,
    pub chat: Chat,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<Vec<String>>,
    /// `None` means omitted; `Some(None)` explicitly clears the deadline.
    #[serde(default, skip_serializing_if = "Patch::is_unset")]
    pub deadline: Patch<DateStr>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectUpdateParams {
    pub project_id: Id,
    pub patch: ProjectPatch,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectUpdateResult {
    pub project: Project,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectMemberParams {
    pub project_id: Id,
    pub bot_id: Id,
    pub role_note: Option<String>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectResult {
    pub project: Project,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectRemoveMemberParams {
    pub project_id: Id,
    pub bot_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectConfirmDoneParams {
    pub project_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectRequestChangesParams {
    pub project_id: Id,
    pub text: String,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProjectRequestChangesResult {
    pub message: Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssignmentListParams {
    pub project_id: Option<Id>,
    pub bot_id: Option<Id>,
    pub status: Option<Vec<AssignmentStatus>>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssignmentListResult {
    pub items: Vec<Assignment>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssignmentIdParams {
    pub assignment_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssignmentGetResult {
    pub assignment: Assignment,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AssignmentResult {
    pub assignment: Assignment,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceHistoryParams {
    pub assignment_id: Option<Id>,
    pub chat_id: Option<Id>,
    pub before_aseq: Option<u64>,
    pub after_aseq: Option<u64>,
    pub tail: Option<bool>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceHistoryResult {
    pub items: Vec<TraceItem>,
    pub first_aseq: Option<u64>,
    pub last_aseq: Option<u64>,
    pub has_more_before: bool,
    pub live: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceSubscribeParams {
    pub assignment_id: Option<Id>,
    pub chat_id: Option<Id>,
    pub since_aseq: u64,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InFlight {
    pub request_id: Id,
    pub text: String,
    pub thinking: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceSubscribeResult {
    pub stream: String,
    pub in_flight: Vec<InFlight>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceUnsubscribeParams {
    pub stream: String,
    #[serde(flatten)]
    pub meta: WriteMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalListParams {
    pub state: Option<Vec<ApprovalState>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalListResult {
    pub approvals: Vec<Approval>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalDecideParams {
    pub approval_id: Id,
    pub decision: ApprovalDecision,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    AlwaysAllow,
    Deny,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalResult {
    pub approval: Approval,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct QuestionAnswerParams {
    pub question_id: Id,
    pub option_index: Option<u32>,
    pub text: Option<String>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct QuestionResult {
    pub question: Question,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LoopResolveParams {
    pub root_message_id: Id,
    pub action: LoopAction,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LoopAction {
    Continue,
    End,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TakeoverStartParams {
    pub bot_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TakeoverReleaseParams {
    pub bot_id: Id,
    pub note: Option<String>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Workbench {
    pub running: u32,
    pub global_limit: u32,
    pub subagents_running: u32,
    pub waiting: Vec<WorkbenchWaiting>,
    pub bots: Vec<WorkbenchBot>,
    pub done_today: Vec<Assignment>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkbenchWaiting {
    Review {
        project_id: Id,
        since: Time,
    },
    Approval {
        approval: Approval,
    },
    Question {
        question: Question,
    },
    Takeover {
        bot_id: Id,
        assignment_id: Id,
        reason: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WorkbenchBot {
    pub bot_id: Id,
    pub active: u32,
    pub max_parallel: u32,
    pub assignments: Vec<Assignment>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WorkbenchResult {
    pub workbench: Workbench,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillNameParams {
    pub name: String,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillListResult {
    pub skills: Vec<Skill>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillGetResult {
    pub skill: SkillDetail,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillCreateParams {
    pub name: String,
    pub content: String,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillResult {
    pub skill: Skill,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillSetEnabledParams {
    pub name: String,
    pub enabled: bool,
    pub bot_id: Option<Id>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillImportParams {
    pub source: SkillImportSource,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillImportSource {
    Git { url: String, subdir: Option<String> },
    Upload { upload_id: Id },
    Path { path: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SkillImportResult {
    pub skills: Vec<Skill>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineListParams {
    pub bot_id: Option<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineListResult {
    pub routines: Vec<Routine>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineCreateParams {
    pub bot_id: Id,
    pub project_id: Option<Id>,
    pub name: String,
    pub instructions: String,
    pub schedules: Vec<Schedule>,
    pub timezone: Option<String>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineResult {
    pub routine: Routine,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineUpdateParams {
    pub routine_id: Id,
    pub patch: RoutinePatch,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutinePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<Schedule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default)]
    pub project_id: Patch<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineIdParams {
    pub routine_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineEnabledParams {
    pub routine_id: Id,
    pub enabled: bool,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineRunResult {
    pub run: RoutineRun,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RoutineRunsResult {
    pub runs: Vec<RoutineRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderListResult {
    pub providers: Vec<Provider>,
    pub models: Vec<Model>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderCreateParams {
    pub name: String,
    pub api_kind: ApiKind,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderResult {
    pub provider: Provider,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderUpdateParams {
    pub provider_id: Id,
    pub patch: ProviderPatch,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderIdParams {
    pub provider_id: Id,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderTestResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelRefreshResult {
    pub models: Vec<Model>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelUpsertParams {
    #[serde(default, skip_serializing_if = "Patch::is_unset")]
    pub r#ref: Patch<ModelRef>,
    pub provider_id: Id,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<ModelCaps>,
    #[serde(default, skip_serializing_if = "Patch::is_unset")]
    pub price: Patch<ModelPrice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelResult {
    pub model: Model,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModelDeleteParams {
    pub r#ref: ModelRef,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SettingsUpdateParams {
    pub patch: SettingsPatch,
    #[serde(flatten)]
    pub meta: WriteMeta,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<Concurrency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<ModelDefaults>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_bot: Option<MainBotSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals: Option<ApprovalSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchSettings>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SettingsResult {
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsagePeriodParams {
    pub from: Time,
    pub to: Time,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageWithTasks {
    #[serde(flatten)]
    pub usage: UsageTotals,
    pub tasks_done: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageSummaryResult {
    pub current: UsageWithTasks,
    pub previous: UsageWithTasks,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageHeatmapParams {
    pub mode: HeatmapMode,
    pub from: Time,
    pub to: Time,
    pub metric: UsageMetric,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HeatmapMode {
    Calendar,
    Weekhour,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UsageMetric {
    Tokens,
    Cost,
    Requests,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum HeatmapResult {
    Calendar(CalendarHeatmap),
    Weekhour(WeekhourHeatmap),
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CalendarHeatmap {
    pub days: Vec<CalendarDay>,
    pub thresholds: [f64; 3],
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CalendarDay {
    pub date: DateStr,
    pub value: f64,
    pub tokens: u64,
    pub cost: Money,
    pub requests: u64,
    pub top_bot_id: Option<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WeekhourHeatmap {
    pub matrix: Vec<Vec<f64>>,
    pub thresholds: [f64; 3],
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageTimeseriesParams {
    pub from: Time,
    pub to: Time,
    pub granularity: Granularity,
    pub dimension: UsageDimension,
    pub metric: UsageMetric,
    pub split_io: Option<bool>,
    pub top: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Auto,
    Hour,
    Day,
    Week,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UsageDimension {
    Model,
    Bot,
    Project,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageTimeseriesResult {
    pub granularity: Granularity,
    pub buckets: Vec<Time>,
    pub series: Vec<UsageSeries>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageSeries {
    pub key: String,
    pub label: String,
    pub values: Vec<f64>,
    pub input_values: Option<Vec<f64>>,
    pub output_values: Option<Vec<f64>>,
    pub total: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageBreakdownParams {
    pub from: Time,
    pub to: Time,
    pub dimension: UsageDimension,
    pub drill: Option<UsageDrill>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum UsageDrill {
    Bot { bot_id: Id },
    Project { project_id: Id },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageBreakdownResult {
    pub rows: Vec<UsageRow>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageRow {
    pub key: String,
    pub label: String,
    pub usage: UsageTotals,
    pub sparkline: Vec<f64>,
    pub phases: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SearchParams {
    pub query: String,
    pub kinds: Option<Vec<SearchKind>>,
    pub limit: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    Message,
    Chat,
    Bot,
    Artifact,
    Routine,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SearchResult {
    pub results: Vec<SearchHit>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SearchHit {
    pub kind: SearchKind,
    pub id: Id,
    pub chat_id: Option<Id>,
    pub title: String,
    pub snippet: String,
    pub at: Option<Time>,
}

// A method name paired with its statically declared parameter shape. Runtime
// gateways can deserialize a request by first reading `method` and then using
// the matching `Params` variant.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[serde(rename = "session.resume")]
    SessionResume,
    Ping,
    Bootstrap,
    #[serde(rename = "device.register")]
    DeviceRegister,
    #[serde(rename = "chat.list")]
    ChatList,
    #[serde(rename = "chat.get")]
    ChatGet,
    #[serde(rename = "chat.history")]
    ChatHistory,
    #[serde(rename = "chat.thread")]
    ChatThread,
    #[serde(rename = "chat.send")]
    ChatSend,
    #[serde(rename = "chat.mark_read")]
    ChatMarkRead,
    #[serde(rename = "chat.react")]
    ChatReact,
    #[serde(rename = "chat.set_pinned")]
    ChatSetPinned,
    #[serde(rename = "chat.set_muted")]
    ChatSetMuted,
    #[serde(rename = "bot.list")]
    BotList,
    #[serde(rename = "bot.get")]
    BotGet,
    #[serde(rename = "bot.create")]
    BotCreate,
    #[serde(rename = "bot.update")]
    BotUpdate,
    #[serde(rename = "bot.duplicate")]
    BotDuplicate,
    #[serde(rename = "bot.delete")]
    BotDelete,
    #[serde(rename = "bot.templates")]
    BotTemplates,
    #[serde(rename = "bot.create_from_template")]
    BotCreateFromTemplate,
    #[serde(rename = "project.list")]
    ProjectList,
    #[serde(rename = "project.get")]
    ProjectGet,
    #[serde(rename = "project.create")]
    ProjectCreate,
    #[serde(rename = "project.update")]
    ProjectUpdate,
    #[serde(rename = "project.add_member")]
    ProjectAddMember,
    #[serde(rename = "project.remove_member")]
    ProjectRemoveMember,
    #[serde(rename = "project.confirm_done")]
    ProjectConfirmDone,
    #[serde(rename = "project.request_changes")]
    ProjectRequestChanges,
    #[serde(rename = "project.archive")]
    ProjectArchive,
    #[serde(rename = "project.reopen")]
    ProjectReopen,
    #[serde(rename = "assignment.list")]
    AssignmentList,
    #[serde(rename = "assignment.get")]
    AssignmentGet,
    #[serde(rename = "assignment.stop")]
    AssignmentStop,
    #[serde(rename = "trace.history")]
    TraceHistory,
    #[serde(rename = "trace.subscribe")]
    TraceSubscribe,
    #[serde(rename = "trace.unsubscribe")]
    TraceUnsubscribe,
    #[serde(rename = "approval.list")]
    ApprovalList,
    #[serde(rename = "approval.decide")]
    ApprovalDecide,
    #[serde(rename = "question.answer")]
    QuestionAnswer,
    #[serde(rename = "loop.resolve")]
    LoopResolve,
    #[serde(rename = "takeover.start")]
    TakeoverStart,
    #[serde(rename = "takeover.release")]
    TakeoverRelease,
    #[serde(rename = "workbench.get")]
    WorkbenchGet,
    #[serde(rename = "skill.list")]
    SkillList,
    #[serde(rename = "skill.get")]
    SkillGet,
    #[serde(rename = "skill.create")]
    SkillCreate,
    #[serde(rename = "skill.update")]
    SkillUpdate,
    #[serde(rename = "skill.delete")]
    SkillDelete,
    #[serde(rename = "skill.set_enabled")]
    SkillSetEnabled,
    #[serde(rename = "skill.publish")]
    SkillPublish,
    #[serde(rename = "skill.import")]
    SkillImport,
    #[serde(rename = "routine.list")]
    RoutineList,
    #[serde(rename = "routine.create")]
    RoutineCreate,
    #[serde(rename = "routine.update")]
    RoutineUpdate,
    #[serde(rename = "routine.delete")]
    RoutineDelete,
    #[serde(rename = "routine.set_enabled")]
    RoutineSetEnabled,
    #[serde(rename = "routine.test_run")]
    RoutineTestRun,
    #[serde(rename = "routine.runs")]
    RoutineRuns,
    #[serde(rename = "provider.list")]
    ProviderList,
    #[serde(rename = "provider.create")]
    ProviderCreate,
    #[serde(rename = "provider.update")]
    ProviderUpdate,
    #[serde(rename = "provider.delete")]
    ProviderDelete,
    #[serde(rename = "provider.test")]
    ProviderTest,
    #[serde(rename = "model.refresh")]
    ModelRefresh,
    #[serde(rename = "model.upsert")]
    ModelUpsert,
    #[serde(rename = "model.delete")]
    ModelDelete,
    #[serde(rename = "settings.get")]
    SettingsGet,
    #[serde(rename = "settings.update")]
    SettingsUpdate,
    #[serde(rename = "usage.summary")]
    UsageSummary,
    #[serde(rename = "usage.heatmap")]
    UsageHeatmap,
    #[serde(rename = "usage.timeseries")]
    UsageTimeseries,
    #[serde(rename = "usage.breakdown")]
    UsageBreakdown,
    Search,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum MethodParams {
    SessionResume(SessionResumeParams),
    Ping(EmptyParams),
    Bootstrap(EmptyParams),
    DeviceRegister(DeviceRegisterParams),
    ChatList(IncludeArchivedParams),
    ChatGet(ChatIdParams),
    ChatHistory(ChatHistoryParams),
    ChatThread(ChatThreadParams),
    ChatSend(ChatSendParams),
    ChatMarkRead(ChatMarkReadParams),
    ChatReact(ChatReactParams),
    ChatSetPinned(ChatPinnedParams),
    ChatSetMuted(ChatMutedParams),
    BotList(BotListParams),
    BotGet(BotIdParams),
    BotCreate(BotCreateParams),
    BotUpdate(BotUpdateParams),
    BotDuplicate(BotDuplicateParams),
    BotDelete(BotIdParams),
    BotTemplates(EmptyParams),
    BotCreateFromTemplate(CreateFromTemplateParams),
    ProjectList(ProjectListParams),
    ProjectGet(ProjectIdParams),
    ProjectCreate(ProjectCreateParams),
    ProjectUpdate(ProjectUpdateParams),
    ProjectAddMember(ProjectMemberParams),
    ProjectRemoveMember(ProjectRemoveMemberParams),
    ProjectConfirmDone(ProjectConfirmDoneParams),
    ProjectRequestChanges(ProjectRequestChangesParams),
    ProjectArchive(ProjectIdParams),
    ProjectReopen(ProjectIdParams),
    AssignmentList(AssignmentListParams),
    AssignmentGet(AssignmentIdParams),
    AssignmentStop(AssignmentIdParams),
    TraceHistory(TraceHistoryParams),
    TraceSubscribe(TraceSubscribeParams),
    TraceUnsubscribe(TraceUnsubscribeParams),
    ApprovalList(ApprovalListParams),
    ApprovalDecide(ApprovalDecideParams),
    QuestionAnswer(QuestionAnswerParams),
    LoopResolve(LoopResolveParams),
    TakeoverStart(TakeoverStartParams),
    TakeoverRelease(TakeoverReleaseParams),
    WorkbenchGet(EmptyParams),
    SkillList(EmptyParams),
    SkillGet(SkillNameParams),
    SkillCreate(SkillCreateParams),
    SkillUpdate(SkillCreateParams),
    SkillDelete(SkillNameParams),
    SkillSetEnabled(SkillSetEnabledParams),
    SkillPublish(SkillNameParams),
    SkillImport(SkillImportParams),
    RoutineList(RoutineListParams),
    RoutineCreate(RoutineCreateParams),
    RoutineUpdate(RoutineUpdateParams),
    RoutineDelete(RoutineIdParams),
    RoutineSetEnabled(RoutineEnabledParams),
    RoutineTestRun(RoutineIdParams),
    RoutineRuns(RoutineIdParams),
    ProviderList(EmptyParams),
    ProviderCreate(ProviderCreateParams),
    ProviderUpdate(ProviderUpdateParams),
    ProviderDelete(ProviderIdParams),
    ProviderTest(ProviderIdParams),
    ModelRefresh(ProviderIdParams),
    ModelUpsert(ModelUpsertParams),
    ModelDelete(ModelDeleteParams),
    SettingsGet(EmptyParams),
    SettingsUpdate(SettingsUpdateParams),
    UsageSummary(UsagePeriodParams),
    UsageHeatmap(UsageHeatmapParams),
    UsageTimeseries(UsageTimeseriesParams),
    UsageBreakdown(UsageBreakdownParams),
    Search(SearchParams),
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum MethodResult {
    SessionResume(SessionResumeResult),
    Ping(PingResult),
    Bootstrap(BootstrapResult),
    DeviceRegister(DeviceRegisterResult),
    ChatList(ChatListResult),
    ChatGet(ChatGetResult),
    ChatHistory(ChatHistoryResult),
    ChatThread(ChatThreadResult),
    ChatSend(ChatSendResult),
    ChatMarkRead(EmptyResult),
    ChatReact(ChatReactResult),
    ChatSetPinned(ChatPinnedResult),
    ChatSetMuted(ChatMutedResult),
    BotList(BotListResult),
    BotGet(BotGetResult),
    BotCreate(BotCreateResult),
    BotUpdate(BotUpdateResult),
    BotDuplicate(BotDuplicateResult),
    BotDelete(EmptyResult),
    BotTemplates(BotTemplatesResult),
    BotCreateFromTemplate(CreateFromTemplateResult),
    ProjectList(ProjectListResult),
    ProjectGet(ProjectGetResult),
    ProjectCreate(ProjectCreateResult),
    ProjectUpdate(ProjectUpdateResult),
    ProjectAddMember(ProjectResult),
    ProjectRemoveMember(ProjectResult),
    ProjectConfirmDone(ProjectResult),
    ProjectRequestChanges(ProjectRequestChangesResult),
    ProjectArchive(ProjectResult),
    ProjectReopen(ProjectResult),
    AssignmentList(AssignmentListResult),
    AssignmentGet(AssignmentGetResult),
    AssignmentStop(AssignmentResult),
    TraceHistory(TraceHistoryResult),
    TraceSubscribe(TraceSubscribeResult),
    TraceUnsubscribe(EmptyResult),
    ApprovalList(ApprovalListResult),
    ApprovalDecide(ApprovalResult),
    QuestionAnswer(QuestionResult),
    LoopResolve(EmptyResult),
    TakeoverStart(EmptyResult),
    TakeoverRelease(EmptyResult),
    WorkbenchGet(WorkbenchResult),
    SkillList(SkillListResult),
    SkillGet(SkillGetResult),
    SkillCreate(SkillResult),
    SkillUpdate(SkillResult),
    SkillDelete(EmptyResult),
    SkillSetEnabled(SkillResult),
    SkillPublish(SkillResult),
    SkillImport(SkillImportResult),
    RoutineList(RoutineListResult),
    RoutineCreate(RoutineResult),
    RoutineUpdate(RoutineResult),
    RoutineDelete(EmptyResult),
    RoutineSetEnabled(RoutineResult),
    RoutineTestRun(RoutineRunResult),
    RoutineRuns(RoutineRunsResult),
    ProviderList(ProviderListResult),
    ProviderCreate(ProviderResult),
    ProviderUpdate(ProviderResult),
    ProviderDelete(EmptyResult),
    ProviderTest(ProviderTestResult),
    ModelRefresh(ModelRefreshResult),
    ModelUpsert(ModelResult),
    ModelDelete(EmptyResult),
    SettingsGet(SettingsResult),
    SettingsUpdate(SettingsResult),
    UsageSummary(UsageSummaryResult),
    UsageHeatmap(HeatmapResult),
    UsageTimeseries(UsageTimeseriesResult),
    UsageBreakdown(UsageBreakdownResult),
    Search(SearchResult),
}

impl MethodParams {
    pub fn decode(method: &Method, value: serde_json::Value) -> Result<Self, String> {
        macro_rules! p {
            ($variant:ident, $ty:ty) => {
                decode_value::<$ty>(value).map(Self::$variant)
            };
        }
        match method {
            Method::SessionResume => p!(SessionResume, SessionResumeParams),
            Method::Ping => p!(Ping, EmptyParams),
            Method::Bootstrap => p!(Bootstrap, EmptyParams),
            Method::DeviceRegister => p!(DeviceRegister, DeviceRegisterParams),
            Method::ChatList => p!(ChatList, IncludeArchivedParams),
            Method::ChatGet => p!(ChatGet, ChatIdParams),
            Method::ChatHistory => p!(ChatHistory, ChatHistoryParams),
            Method::ChatThread => p!(ChatThread, ChatThreadParams),
            Method::ChatSend => p!(ChatSend, ChatSendParams),
            Method::ChatMarkRead => p!(ChatMarkRead, ChatMarkReadParams),
            Method::ChatReact => p!(ChatReact, ChatReactParams),
            Method::ChatSetPinned => p!(ChatSetPinned, ChatPinnedParams),
            Method::ChatSetMuted => p!(ChatSetMuted, ChatMutedParams),
            Method::BotList => p!(BotList, BotListParams),
            Method::BotGet => p!(BotGet, BotIdParams),
            Method::BotCreate => p!(BotCreate, BotCreateParams),
            Method::BotUpdate => p!(BotUpdate, BotUpdateParams),
            Method::BotDuplicate => p!(BotDuplicate, BotDuplicateParams),
            Method::BotDelete => p!(BotDelete, BotIdParams),
            Method::BotTemplates => p!(BotTemplates, EmptyParams),
            Method::BotCreateFromTemplate => p!(BotCreateFromTemplate, CreateFromTemplateParams),
            Method::ProjectList => p!(ProjectList, ProjectListParams),
            Method::ProjectGet => p!(ProjectGet, ProjectIdParams),
            Method::ProjectCreate => p!(ProjectCreate, ProjectCreateParams),
            Method::ProjectUpdate => p!(ProjectUpdate, ProjectUpdateParams),
            Method::ProjectAddMember => p!(ProjectAddMember, ProjectMemberParams),
            Method::ProjectRemoveMember => p!(ProjectRemoveMember, ProjectRemoveMemberParams),
            Method::ProjectConfirmDone => p!(ProjectConfirmDone, ProjectConfirmDoneParams),
            Method::ProjectRequestChanges => p!(ProjectRequestChanges, ProjectRequestChangesParams),
            Method::ProjectArchive => p!(ProjectArchive, ProjectIdParams),
            Method::ProjectReopen => p!(ProjectReopen, ProjectIdParams),
            Method::AssignmentList => p!(AssignmentList, AssignmentListParams),
            Method::AssignmentGet => p!(AssignmentGet, AssignmentIdParams),
            Method::AssignmentStop => p!(AssignmentStop, AssignmentIdParams),
            Method::TraceHistory => p!(TraceHistory, TraceHistoryParams),
            Method::TraceSubscribe => p!(TraceSubscribe, TraceSubscribeParams),
            Method::TraceUnsubscribe => p!(TraceUnsubscribe, TraceUnsubscribeParams),
            Method::ApprovalList => p!(ApprovalList, ApprovalListParams),
            Method::ApprovalDecide => p!(ApprovalDecide, ApprovalDecideParams),
            Method::QuestionAnswer => p!(QuestionAnswer, QuestionAnswerParams),
            Method::LoopResolve => p!(LoopResolve, LoopResolveParams),
            Method::TakeoverStart => p!(TakeoverStart, TakeoverStartParams),
            Method::TakeoverRelease => p!(TakeoverRelease, TakeoverReleaseParams),
            Method::WorkbenchGet => p!(WorkbenchGet, EmptyParams),
            Method::SkillList => p!(SkillList, EmptyParams),
            Method::SkillGet => p!(SkillGet, SkillNameParams),
            Method::SkillCreate => p!(SkillCreate, SkillCreateParams),
            Method::SkillUpdate => p!(SkillUpdate, SkillCreateParams),
            Method::SkillDelete => p!(SkillDelete, SkillNameParams),
            Method::SkillSetEnabled => p!(SkillSetEnabled, SkillSetEnabledParams),
            Method::SkillPublish => p!(SkillPublish, SkillNameParams),
            Method::SkillImport => p!(SkillImport, SkillImportParams),
            Method::RoutineList => p!(RoutineList, RoutineListParams),
            Method::RoutineCreate => p!(RoutineCreate, RoutineCreateParams),
            Method::RoutineUpdate => p!(RoutineUpdate, RoutineUpdateParams),
            Method::RoutineDelete => p!(RoutineDelete, RoutineIdParams),
            Method::RoutineSetEnabled => p!(RoutineSetEnabled, RoutineEnabledParams),
            Method::RoutineTestRun => p!(RoutineTestRun, RoutineIdParams),
            Method::RoutineRuns => p!(RoutineRuns, RoutineIdParams),
            Method::ProviderList => p!(ProviderList, EmptyParams),
            Method::ProviderCreate => p!(ProviderCreate, ProviderCreateParams),
            Method::ProviderUpdate => p!(ProviderUpdate, ProviderUpdateParams),
            Method::ProviderDelete => p!(ProviderDelete, ProviderIdParams),
            Method::ProviderTest => p!(ProviderTest, ProviderIdParams),
            Method::ModelRefresh => p!(ModelRefresh, ProviderIdParams),
            Method::ModelUpsert => p!(ModelUpsert, ModelUpsertParams),
            Method::ModelDelete => p!(ModelDelete, ModelDeleteParams),
            Method::SettingsGet => p!(SettingsGet, EmptyParams),
            Method::SettingsUpdate => p!(SettingsUpdate, SettingsUpdateParams),
            Method::UsageSummary => p!(UsageSummary, UsagePeriodParams),
            Method::UsageHeatmap => p!(UsageHeatmap, UsageHeatmapParams),
            Method::UsageTimeseries => p!(UsageTimeseries, UsageTimeseriesParams),
            Method::UsageBreakdown => p!(UsageBreakdown, UsageBreakdownParams),
            Method::Search => p!(Search, SearchParams),
        }
    }
}

impl MethodResult {
    pub fn decode(method: &Method, value: serde_json::Value) -> Result<Self, String> {
        macro_rules! r {
            ($variant:ident, $ty:ty) => {
                decode_value::<$ty>(value).map(Self::$variant)
            };
        }
        match method {
            Method::SessionResume => r!(SessionResume, SessionResumeResult),
            Method::Ping => r!(Ping, PingResult),
            Method::Bootstrap => r!(Bootstrap, BootstrapResult),
            Method::DeviceRegister => r!(DeviceRegister, DeviceRegisterResult),
            Method::ChatList => r!(ChatList, ChatListResult),
            Method::ChatGet => r!(ChatGet, ChatGetResult),
            Method::ChatHistory => r!(ChatHistory, ChatHistoryResult),
            Method::ChatThread => r!(ChatThread, ChatThreadResult),
            Method::ChatSend => r!(ChatSend, ChatSendResult),
            Method::ChatMarkRead => r!(ChatMarkRead, EmptyResult),
            Method::ChatReact => r!(ChatReact, ChatReactResult),
            Method::ChatSetPinned => r!(ChatSetPinned, ChatPinnedResult),
            Method::ChatSetMuted => r!(ChatSetMuted, ChatMutedResult),
            Method::BotList => r!(BotList, BotListResult),
            Method::BotGet => r!(BotGet, BotGetResult),
            Method::BotCreate => r!(BotCreate, BotCreateResult),
            Method::BotUpdate => r!(BotUpdate, BotUpdateResult),
            Method::BotDuplicate => r!(BotDuplicate, BotDuplicateResult),
            Method::BotDelete => r!(BotDelete, EmptyResult),
            Method::BotTemplates => r!(BotTemplates, BotTemplatesResult),
            Method::BotCreateFromTemplate => r!(BotCreateFromTemplate, CreateFromTemplateResult),
            Method::ProjectList => r!(ProjectList, ProjectListResult),
            Method::ProjectGet => r!(ProjectGet, ProjectGetResult),
            Method::ProjectCreate => r!(ProjectCreate, ProjectCreateResult),
            Method::ProjectUpdate => r!(ProjectUpdate, ProjectUpdateResult),
            Method::ProjectAddMember => r!(ProjectAddMember, ProjectResult),
            Method::ProjectRemoveMember => r!(ProjectRemoveMember, ProjectResult),
            Method::ProjectConfirmDone => r!(ProjectConfirmDone, ProjectResult),
            Method::ProjectRequestChanges => r!(ProjectRequestChanges, ProjectRequestChangesResult),
            Method::ProjectArchive => r!(ProjectArchive, ProjectResult),
            Method::ProjectReopen => r!(ProjectReopen, ProjectResult),
            Method::AssignmentList => r!(AssignmentList, AssignmentListResult),
            Method::AssignmentGet => r!(AssignmentGet, AssignmentGetResult),
            Method::AssignmentStop => r!(AssignmentStop, AssignmentResult),
            Method::TraceHistory => r!(TraceHistory, TraceHistoryResult),
            Method::TraceSubscribe => r!(TraceSubscribe, TraceSubscribeResult),
            Method::TraceUnsubscribe => r!(TraceUnsubscribe, EmptyResult),
            Method::ApprovalList => r!(ApprovalList, ApprovalListResult),
            Method::ApprovalDecide => r!(ApprovalDecide, ApprovalResult),
            Method::QuestionAnswer => r!(QuestionAnswer, QuestionResult),
            Method::LoopResolve => r!(LoopResolve, EmptyResult),
            Method::TakeoverStart => r!(TakeoverStart, EmptyResult),
            Method::TakeoverRelease => r!(TakeoverRelease, EmptyResult),
            Method::WorkbenchGet => r!(WorkbenchGet, WorkbenchResult),
            Method::SkillList => r!(SkillList, SkillListResult),
            Method::SkillGet => r!(SkillGet, SkillGetResult),
            Method::SkillCreate => r!(SkillCreate, SkillResult),
            Method::SkillUpdate => r!(SkillUpdate, SkillResult),
            Method::SkillDelete => r!(SkillDelete, EmptyResult),
            Method::SkillSetEnabled => r!(SkillSetEnabled, SkillResult),
            Method::SkillPublish => r!(SkillPublish, SkillResult),
            Method::SkillImport => r!(SkillImport, SkillImportResult),
            Method::RoutineList => r!(RoutineList, RoutineListResult),
            Method::RoutineCreate => r!(RoutineCreate, RoutineResult),
            Method::RoutineUpdate => r!(RoutineUpdate, RoutineResult),
            Method::RoutineDelete => r!(RoutineDelete, EmptyResult),
            Method::RoutineSetEnabled => r!(RoutineSetEnabled, RoutineResult),
            Method::RoutineTestRun => r!(RoutineTestRun, RoutineRunResult),
            Method::RoutineRuns => r!(RoutineRuns, RoutineRunsResult),
            Method::ProviderList => r!(ProviderList, ProviderListResult),
            Method::ProviderCreate => r!(ProviderCreate, ProviderResult),
            Method::ProviderUpdate => r!(ProviderUpdate, ProviderResult),
            Method::ProviderDelete => r!(ProviderDelete, EmptyResult),
            Method::ProviderTest => r!(ProviderTest, ProviderTestResult),
            Method::ModelRefresh => r!(ModelRefresh, ModelRefreshResult),
            Method::ModelUpsert => r!(ModelUpsert, ModelResult),
            Method::ModelDelete => r!(ModelDelete, EmptyResult),
            Method::SettingsGet => r!(SettingsGet, SettingsResult),
            Method::SettingsUpdate => r!(SettingsUpdate, SettingsResult),
            Method::UsageSummary => r!(UsageSummary, UsageSummaryResult),
            Method::UsageHeatmap => r!(UsageHeatmap, HeatmapResult),
            Method::UsageTimeseries => r!(UsageTimeseries, UsageTimeseriesResult),
            Method::UsageBreakdown => r!(UsageBreakdown, UsageBreakdownResult),
            Method::Search => r!(Search, SearchResult),
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
pub struct RpcRequest {
    pub v: u32,
    pub kind: RequestKind,
    pub id: String,
    pub method: Method,
    pub params: MethodParams,
}

impl<'de> Deserialize<'de> for RpcRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawRpcRequest {
            v: u32,
            kind: RequestKind,
            id: String,
            method: Method,
            params: serde_json::Value,
        }
        let raw = RawRpcRequest::deserialize(deserializer)?;
        let params = MethodParams::decode(&raw.method, raw.params).map_err(de::Error::custom)?;
        Ok(Self {
            v: raw.v,
            kind: raw.kind,
            id: raw.id,
            method: raw.method,
            params,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    Req,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    SetupRequired,
    VersionUnsupported,
    InvalidParams,
    NotFound,
    Forbidden,
    Conflict,
    RateLimited,
    Busy,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RpcResponse {
    pub v: u32,
    pub kind: ResponseKind,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// A response has no method discriminator on the wire. Keep this as raw
    /// JSON so an unknown/new result can never lose fields; callers that know
    /// the request method can use `MethodResult::decode`.
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResponseKind {
    Res,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
pub struct EventFrame {
    pub v: u32,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub event: EventName,
    pub data: EventData,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Evt,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IdData {
    pub id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventName {
    #[serde(rename = "chat.created")]
    ChatCreated,
    #[serde(rename = "chat.updated")]
    ChatUpdated,
    #[serde(rename = "chat.deleted")]
    ChatDeleted,
    #[serde(rename = "read.updated")]
    ReadUpdated,
    #[serde(rename = "message.created")]
    MessageCreated,
    #[serde(rename = "message.updated")]
    MessageUpdated,
    #[serde(rename = "message.deleted")]
    MessageDeleted,
    #[serde(rename = "bot.created")]
    BotCreated,
    #[serde(rename = "bot.updated")]
    BotUpdated,
    #[serde(rename = "bot.deleted")]
    BotDeleted,
    #[serde(rename = "project.created")]
    ProjectCreated,
    #[serde(rename = "project.updated")]
    ProjectUpdated,
    #[serde(rename = "announcement.updated")]
    AnnouncementUpdated,
    #[serde(rename = "assignment.created")]
    AssignmentCreated,
    #[serde(rename = "assignment.updated")]
    AssignmentUpdated,
    #[serde(rename = "artifact.registered")]
    ArtifactRegistered,
    #[serde(rename = "approval.requested")]
    ApprovalRequested,
    #[serde(rename = "approval.resolved")]
    ApprovalResolved,
    #[serde(rename = "question.asked")]
    QuestionAsked,
    #[serde(rename = "question.answered")]
    QuestionAnswered,
    #[serde(rename = "skill.updated")]
    SkillUpdated,
    #[serde(rename = "skill.deleted")]
    SkillDeleted,
    #[serde(rename = "routine.updated")]
    RoutineUpdated,
    #[serde(rename = "routine.deleted")]
    RoutineDeleted,
    #[serde(rename = "routine.run")]
    RoutineRun,
    #[serde(rename = "provider.updated")]
    ProviderUpdated,
    #[serde(rename = "provider.deleted")]
    ProviderDeleted,
    #[serde(rename = "settings.updated")]
    SettingsUpdated,
    Hello,
    #[serde(rename = "sync.done")]
    SyncDone,
    #[serde(rename = "message.delta")]
    MessageDelta,
    Typing,
    #[serde(rename = "bot.status")]
    BotStatus,
    #[serde(rename = "usage.tick")]
    UsageTick,
    #[serde(rename = "host.status")]
    HostStatus,
    #[serde(rename = "trace.item")]
    TraceItem,
    #[serde(rename = "trace.delta")]
    TraceDelta,
    #[serde(rename = "trace.tool_output")]
    TraceToolOutput,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum EventData {
    Chat {
        chat: Chat,
    },
    ReadUpdated {
        chat_id: Id,
        last_read_seq: u64,
    },
    Message {
        message: Message,
    },
    MessageDelta(MessageDelta),
    MessageDeleted {
        chat_id: Id,
        message_id: Id,
    },
    Bot {
        bot: Bot,
    },
    Project {
        project: Project,
    },
    Announcement {
        announcement: Announcement,
    },
    Assignment {
        assignment: Assignment,
    },
    Artifact {
        artifact: Artifact,
    },
    Approval {
        approval: Approval,
    },
    Question {
        question: Question,
    },
    Skill {
        skill: Skill,
    },
    SkillDeleted {
        name: String,
    },
    Routine {
        routine: Routine,
    },
    RoutineDeleted {
        routine_id: Id,
    },
    RoutineRun {
        run: RoutineRun,
    },
    Provider {
        provider: Provider,
        models: Vec<Model>,
    },
    ProviderDeleted {
        provider_id: Id,
    },
    Settings {
        settings: Settings,
    },
    Hello(Hello),
    SyncDone(SeqEvent),
    Typing(TypingEvent),
    BotStatus(BotStatusEvent),
    UsageTick(UsageTick),
    HostStatus(HostStatus),
    TraceItem {
        stream: String,
        item: TraceItem,
    },
    TraceDelta {
        stream: String,
        request_id: Id,
        channel: TraceChannel,
        call_id: Option<Id>,
        text: String,
    },
    TraceToolOutput {
        stream: String,
        call_id: Id,
        chunk: String,
    },
    BotDeleted {
        bot_id: Id,
    },
    ChatDeleted {
        chat_id: Id,
    },
}

impl EventData {
    /// Decode event data using the event name from the envelope. The payload
    /// itself intentionally has no discriminator in the wire protocol.
    pub fn decode(event: &EventName, value: serde_json::Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct ChatPayload {
            chat: Chat,
        }
        #[derive(Deserialize)]
        struct MessagePayload {
            message: Message,
        }
        #[derive(Deserialize)]
        struct BotPayload {
            bot: Bot,
        }
        #[derive(Deserialize)]
        struct ProjectPayload {
            project: Project,
        }
        #[derive(Deserialize)]
        struct AnnouncementPayload {
            announcement: Announcement,
        }
        #[derive(Deserialize)]
        struct AssignmentPayload {
            assignment: Assignment,
        }
        #[derive(Deserialize)]
        struct ArtifactPayload {
            artifact: Artifact,
        }
        #[derive(Deserialize)]
        struct ApprovalPayload {
            approval: Approval,
        }
        #[derive(Deserialize)]
        struct QuestionPayload {
            question: Question,
        }
        #[derive(Deserialize)]
        struct SkillPayload {
            skill: Skill,
        }
        #[derive(Deserialize)]
        struct RoutinePayload {
            routine: Routine,
        }
        #[derive(Deserialize)]
        struct RoutineRunPayload {
            run: RoutineRun,
        }
        #[derive(Deserialize)]
        struct ProviderPayload {
            provider: Provider,
            models: Vec<Model>,
        }
        #[derive(Deserialize)]
        struct SettingsPayload {
            settings: Settings,
        }
        #[derive(Deserialize)]
        struct ReadPayload {
            chat_id: Id,
            last_read_seq: u64,
        }
        #[derive(Deserialize)]
        struct MessageDeletedPayload {
            chat_id: Id,
            message_id: Id,
        }
        #[derive(Deserialize)]
        struct BotDeletedPayload {
            bot_id: Id,
        }
        #[derive(Deserialize)]
        struct ChatDeletedPayload {
            chat_id: Id,
        }
        #[derive(Deserialize)]
        struct SkillDeletedPayload {
            name: String,
        }
        #[derive(Deserialize)]
        struct RoutineDeletedPayload {
            routine_id: Id,
        }
        #[derive(Deserialize)]
        struct ProviderDeletedPayload {
            provider_id: Id,
        }
        #[derive(Deserialize)]
        struct TracePayload {
            stream: String,
            item: TraceItem,
        }
        #[derive(Deserialize)]
        struct TraceDeltaPayload {
            stream: String,
            request_id: Id,
            channel: TraceChannel,
            call_id: Option<Id>,
            text: String,
        }
        #[derive(Deserialize)]
        struct TraceOutputPayload {
            stream: String,
            call_id: Id,
            chunk: String,
        }
        macro_rules! val {
            ($ty:ty) => {
                decode_value::<$ty>(value)
            };
        }
        match event {
            EventName::ChatCreated | EventName::ChatUpdated => {
                val!(ChatPayload).map(|x| Self::Chat { chat: x.chat })
            }
            EventName::ChatDeleted => {
                val!(ChatDeletedPayload).map(|x| Self::ChatDeleted { chat_id: x.chat_id })
            }
            EventName::ReadUpdated => val!(ReadPayload).map(|x| Self::ReadUpdated {
                chat_id: x.chat_id,
                last_read_seq: x.last_read_seq,
            }),
            EventName::MessageCreated | EventName::MessageUpdated => {
                val!(MessagePayload).map(|x| Self::Message { message: x.message })
            }
            EventName::MessageDeleted => {
                val!(MessageDeletedPayload).map(|x| Self::MessageDeleted {
                    chat_id: x.chat_id,
                    message_id: x.message_id,
                })
            }
            EventName::BotCreated | EventName::BotUpdated => {
                val!(BotPayload).map(|x| Self::Bot { bot: x.bot })
            }
            EventName::BotDeleted => {
                val!(BotDeletedPayload).map(|x| Self::BotDeleted { bot_id: x.bot_id })
            }
            EventName::ProjectCreated | EventName::ProjectUpdated => {
                val!(ProjectPayload).map(|x| Self::Project { project: x.project })
            }
            EventName::AnnouncementUpdated => {
                val!(AnnouncementPayload).map(|x| Self::Announcement {
                    announcement: x.announcement,
                })
            }
            EventName::AssignmentCreated | EventName::AssignmentUpdated => val!(AssignmentPayload)
                .map(|x| Self::Assignment {
                    assignment: x.assignment,
                }),
            EventName::ArtifactRegistered => val!(ArtifactPayload).map(|x| Self::Artifact {
                artifact: x.artifact,
            }),
            EventName::ApprovalRequested | EventName::ApprovalResolved => val!(ApprovalPayload)
                .map(|x| Self::Approval {
                    approval: x.approval,
                }),
            EventName::QuestionAsked | EventName::QuestionAnswered => {
                val!(QuestionPayload).map(|x| Self::Question {
                    question: x.question,
                })
            }
            EventName::SkillUpdated => val!(SkillPayload).map(|x| Self::Skill { skill: x.skill }),
            EventName::SkillDeleted => {
                val!(SkillDeletedPayload).map(|x| Self::SkillDeleted { name: x.name })
            }
            EventName::RoutineUpdated => {
                val!(RoutinePayload).map(|x| Self::Routine { routine: x.routine })
            }
            EventName::RoutineDeleted => {
                val!(RoutineDeletedPayload).map(|x| Self::RoutineDeleted {
                    routine_id: x.routine_id,
                })
            }
            EventName::RoutineRun => {
                val!(RoutineRunPayload).map(|x| Self::RoutineRun { run: x.run })
            }
            EventName::ProviderUpdated => val!(ProviderPayload).map(|x| Self::Provider {
                provider: x.provider,
                models: x.models,
            }),
            EventName::ProviderDeleted => {
                val!(ProviderDeletedPayload).map(|x| Self::ProviderDeleted {
                    provider_id: x.provider_id,
                })
            }
            EventName::SettingsUpdated => val!(SettingsPayload).map(|x| Self::Settings {
                settings: x.settings,
            }),
            EventName::Hello => val!(Hello).map(Self::Hello),
            EventName::SyncDone => val!(SeqEvent).map(Self::SyncDone),
            EventName::MessageDelta => val!(MessageDelta).map(Self::MessageDelta),
            EventName::Typing => val!(TypingEvent).map(Self::Typing),
            EventName::BotStatus => val!(BotStatusEvent).map(Self::BotStatus),
            EventName::UsageTick => val!(UsageTick).map(Self::UsageTick),
            EventName::HostStatus => val!(HostStatus).map(Self::HostStatus),
            EventName::TraceItem => val!(TracePayload).map(|x| Self::TraceItem {
                stream: x.stream,
                item: x.item,
            }),
            EventName::TraceDelta => val!(TraceDeltaPayload).map(|x| Self::TraceDelta {
                stream: x.stream,
                request_id: x.request_id,
                channel: x.channel,
                call_id: x.call_id,
                text: x.text,
            }),
            EventName::TraceToolOutput => val!(TraceOutputPayload).map(|x| Self::TraceToolOutput {
                stream: x.stream,
                call_id: x.call_id,
                chunk: x.chunk,
            }),
        }
    }
}

impl<'de> Deserialize<'de> for EventFrame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawEventFrame {
            v: u32,
            kind: EventKind,
            seq: Option<u64>,
            event: EventName,
            data: serde_json::Value,
        }
        let raw = RawEventFrame::deserialize(deserializer)?;
        let data = EventData::decode(&raw.event, raw.data).map_err(de::Error::custom)?;
        Ok(Self {
            v: raw.v,
            kind: raw.kind,
            seq: raw.seq,
            event: raw.event,
            data,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScreenState {
    pub bot_id: Id,
    pub driver: ScreenDriver,
    pub tabs: Vec<ScreenTab>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ScreenDriver {
    Bot,
    User,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScreenTab {
    pub tab_id: String,
    pub title: String,
    pub url: String,
    pub assignment_id: Option<Id>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScreenFrameHeader {
    pub seq: u64,
    pub tab_id: String,
    pub w: u32,
    pub h: u32,
    pub ts: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScreenClientFrame {
    Ack { seq: u64 },
    SwitchTab { tab_id: String },
    Input { event: ScreenInput },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScreenInput {
    Mouse {
        action: MouseAction,
        x: f64,
        y: f64,
        button: MouseButton,
        click_count: u32,
    },
    Wheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Key {
        action: KeyAction,
        key: String,
        code: String,
        text: Option<String>,
        modifiers: Vec<String>,
    },
    Touch {
        action: TouchAction,
        points: Vec<TouchPoint>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Move,
    Down,
    Up,
    Click,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Down,
    Up,
    Press,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TouchAction {
    Start,
    Move,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TouchPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceDelta {
    pub request_id: Id,
    pub channel: TraceChannel,
    pub call_id: Option<Id>,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TraceChannel {
    Text,
    Thinking,
    ToolArgs,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraceToolOutput {
    pub call_id: Id,
    pub chunk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MessageDelta {
    pub chat_id: Id,
    pub message_id: Id,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TypingEvent {
    pub chat_id: Id,
    pub bot_id: Id,
    pub on: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BotStatusEvent {
    pub bot_id: Id,
    pub status: BotStatus,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UsageTick {
    pub assignment_id: Id,
    pub usage: UsageTotals,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HostStatus {
    pub running: u32,
    pub queued: u32,
    pub global_limit: u32,
    pub subagents_running: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SeqEvent {
    pub seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enums_use_wire_names() {
        assert_eq!(
            serde_json::to_string(&Sender::Bot {
                bot_id: "bot_1".into()
            })
            .unwrap(),
            r#"{"kind":"bot","bot_id":"bot_1"}"#
        );
        assert_eq!(
            serde_json::to_string(&Block::TaskCard {
                assignment_id: "asg_1".into()
            })
            .unwrap(),
            r#"{"type":"task_card","assignment_id":"asg_1"}"#
        );
        assert_eq!(
            serde_json::to_string(&ApiKind::OpenaiCompletions).unwrap(),
            r#""openai-completions""#
        );
    }
    #[test]
    fn unknown_fields_are_ignored() {
        let value: Bot = serde_json::from_str(r#"{"id":"bot_1","name":"x","label":"","description":"","avatar":{"kind":"bean","color":1},"is_main":false,"model":null,"max_parallel":1,"tools":{"files":true,"bash":true,"browser":false,"subagent":false,"web":false,"mcp":false},"browser_mode":"headless","pinned":false,"hidden":false,"notifications":true,"dm_chat_id":"chat_1","created_at":"t","updated_at":"t","status":{"summary":"idle","active":0,"queued":0,"waiting":0},"future":true}"#).unwrap();
        assert_eq!(value.id, "bot_1");
    }

    #[test]
    fn rpc_request_dispatches_by_method_without_empty_variant_swallowing_fields() {
        let ping: RpcRequest = serde_json::from_value(serde_json::json!({
            "v": 1, "kind": "req", "id": "r1", "method": "ping", "params": {}
        }))
        .unwrap();
        assert!(matches!(ping.params, MethodParams::Ping(_)));

        let list: RpcRequest = serde_json::from_value(serde_json::json!({
            "v": 1, "kind": "req", "id": "r2", "method": "chat.list", "params": {"include_archived": true}
        })).unwrap();
        assert!(matches!(
            list.params,
            MethodParams::ChatList(IncludeArchivedParams {
                include_archived: Some(true)
            })
        ));
        let encoded = serde_json::to_value(list).unwrap();
        assert_eq!(encoded["params"]["include_archived"], true);

        let bot_get: RpcRequest = serde_json::from_value(serde_json::json!({
            "v": 1, "kind": "req", "id": "r3", "method": "bot.get", "params": {"bot_id": "bot_1"}
        }))
        .unwrap();
        assert!(
            matches!(bot_get.params, MethodParams::BotGet(BotIdParams { bot_id, .. }) if bot_id == "bot_1")
        );
    }

    #[test]
    fn patch_preserves_omitted_vs_explicit_null() {
        let omitted: BotPatch = serde_json::from_value(serde_json::json!({"name":"编程"})).unwrap();
        assert_eq!(
            serde_json::to_value(&omitted).unwrap(),
            serde_json::json!({"name":"编程"})
        );
        let cleared: BotPatch = serde_json::from_value(serde_json::json!({"model":null})).unwrap();
        assert_eq!(cleared.model, Patch::Null);
        assert_eq!(
            serde_json::to_value(&cleared).unwrap(),
            serde_json::json!({"model":null})
        );

        let deadline: ProjectPatch =
            serde_json::from_value(serde_json::json!({"deadline":null})).unwrap();
        assert_eq!(deadline.deadline, Patch::Null);
        assert_eq!(
            serde_json::to_value(&deadline).unwrap(),
            serde_json::json!({"deadline":null})
        );
    }

    #[test]
    fn event_and_trace_discriminators_reject_wrong_payload_shapes() {
        let wrong_event = serde_json::json!({"v":1,"kind":"evt","event":"chat.deleted","data":{"message_id":"msg_1"}});
        assert!(serde_json::from_value::<EventFrame>(wrong_event).is_err());
        let wrong_trace = serde_json::json!({"assignment_id":null,"chat_id":"chat_1","run_id":"run_1","aseq":1,"at":"2026-10-09T00:00:00Z","type":"run.start","data":{"reason":"x","before_tokens":1,"after_tokens":0}});
        assert!(serde_json::from_value::<TraceItem>(wrong_trace).is_err());

        let unknown_result: RpcResponse = serde_json::from_value(serde_json::json!({"v":1,"kind":"res","id":"r1","ok":true,"result":{"future":{"kept":true}},"error":null})).unwrap();
        assert_eq!(
            unknown_result.result.unwrap(),
            serde_json::json!({"future":{"kept":true}})
        );
    }
}
