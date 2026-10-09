use serde::{Deserialize, Serialize};

pub type Id = String;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolToggles {
    pub files: bool,
    pub bash: bool,
    pub browser: bool,
    pub subagent: bool,
    pub web: bool,
    pub mcp: bool,
}

impl Default for ToolToggles {
    fn default() -> Self {
        Self {
            files: true,
            bash: true,
            browser: true,
            subagent: true,
            web: true,
            mcp: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bot {
    pub id: Id,
    pub name: String,
    pub label: String,
    pub description: String,
    pub avatar: Option<String>,
    pub model: Option<String>,
    pub max_parallel: usize,
    #[serde(default)]
    pub tools: ToolToggles,
    #[serde(default = "default_browser_mode")]
    pub browser_mode: String,
    #[serde(default)]
    pub dm_chat_id: Id,
    pub pinned: bool,
    pub hidden: bool,
    pub notifications: bool,
    pub is_main: bool,
    pub created_at: String,
    pub updated_at: String,
}

fn default_browser_mode() -> String {
    "headless".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMember {
    pub bot_id: Id,
    pub role_note: String,
    pub joined_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: Id,
    pub chat_id: Id,
    pub name: String,
    pub slug: String,
    pub goal: String,
    pub flow: Vec<String>,
    pub deadline: Option<String>,
    pub home_path: String,
    pub status: String,
    pub lead_bot_id: Id,
    pub members: Vec<ProjectMember>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub done_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    pub id: Id,
    pub project_id: Option<Id>,
    pub bot_id: Id,
    pub assignment_id: Id,
    pub title: String,
    pub path_or_url: String,
    pub kind: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Steer {
    pub message_id: Id,
    pub text: String,
    pub at: String,
    pub applied_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    pub id: Id,
    pub project_id: Option<Id>,
    pub origin_chat_id: Id,
    pub bot_id: Id,
    pub title: String,
    pub instruction: String,
    pub from: String,
    pub trigger_message_id: Option<Id>,
    pub parent_assignment_id: Option<Id>,
    pub status: String,
    pub queue_reason: Option<String>,
    pub wait: Option<WaitState>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub usage: UsageTotals,
    pub subagents_active: usize,
    pub steers: Vec<Steer>,
    pub result_message_id: Option<Id>,
    pub model: Option<String>,
    pub priority: u8,
    pub root_message_id: Option<Id>,
    pub loop_hops: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitState {
    pub reason: String,
    pub message_id: Option<Id>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: Id,
    pub chat_id: Id,
    pub sender: String,
    pub created_at: String,
    pub text: String,
    pub intent: Option<String>,
    pub assignment_id: Option<Id>,
    pub mentions: Vec<Mention>,
    pub artifacts: Vec<ArtifactRef>,
    pub options: Vec<String>,
    /// Internal linkage for a decision question.  The gateway turns this
    /// into the protocol `question` block and must not expose this field on
    /// the wire Message shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_id: Option<Id>,
    pub delivery: Vec<Delivery>,
    pub fallback_text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mention {
    Bot {
        bot_id: Id,
        instruction: Option<String>,
    },
    Main,
    User,
    Everyone,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    pub title: String,
    pub path_or_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Delivery {
    pub bot_id: Id,
    pub assignment_id: Option<Id>,
    pub state: String,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approval {
    pub id: Id,
    pub bot_id: Id,
    pub assignment_id: Option<Id>,
    pub chat_id: Id,
    pub tool: String,
    pub risk: String,
    pub summary: String,
    pub detail: String,
    pub state: String,
    pub created_at: String,
    pub decided_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    pub id: Id,
    pub bot_id: Id,
    pub assignment_id: Id,
    pub chat_id: Id,
    pub text: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
    pub state: String,
    pub answer: Option<QuestionAnswer>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionAnswer {
    pub option_index: Option<usize>,
    pub text: Option<String>,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Routine {
    pub id: Id,
    pub bot_id: Id,
    pub project_id: Option<Id>,
    pub name: String,
    pub instructions: String,
    pub schedules: Vec<Schedule>,
    pub timezone: String,
    pub enabled: bool,
    pub next_run_at: Option<String>,
    pub last_run: Option<RoutineRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Schedule {
    pub cron: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutineRun {
    pub id: Id,
    pub routine_id: Id,
    pub assignment_id: Option<Id>,
    pub trigger: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub bots: Vec<TemplateBot>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateBot {
    pub name: String,
    pub label: String,
    pub description: String,
    pub avatar: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Announcement {
    pub project_id: Id,
    pub members: Vec<AnnouncementMember>,
    pub artifacts: Vec<Artifact>,
    pub highlights: Vec<Highlight>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnnouncementMember {
    pub bot_id: Id,
    pub role_note: String,
    pub state: String,
    pub current_assignment_id: Option<Id>,
    pub since: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Highlight {
    pub text: String,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteerDelivery {
    pub message_id: Id,
    pub bot_id: Id,
    pub assignment_id: Option<Id>,
    pub state: String,
    pub at: String,
}
