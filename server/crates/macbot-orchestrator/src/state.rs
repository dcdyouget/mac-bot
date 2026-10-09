use crate::model::*;
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorSettings {
    pub global_limit: usize,
    pub bot_default_limit: usize,
    pub subagent_per_run: usize,
    pub subagent_global: usize,
    pub loop_hops: usize,
}

impl Default for OrchestratorSettings {
    fn default() -> Self {
        Self {
            global_limit: 8,
            bot_default_limit: 3,
            subagent_per_run: 4,
            subagent_global: 12,
            loop_hops: 8,
        }
    }
}

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("invalid parameters: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("method not found: {0}")]
    MethodNotFound(String),
    #[error("internal state lock poisoned")]
    Poisoned,
}

type Result<T> = std::result::Result<T, OrchestratorError>;

#[derive(Clone)]
pub struct Orchestrator {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    settings: OrchestratorSettings,
    bots: HashMap<Id, Bot>,
    projects: HashMap<Id, Project>,
    assignments: HashMap<Id, Assignment>,
    messages: HashMap<Id, Message>,
    artifacts: HashMap<Id, Artifact>,
    approvals: HashMap<Id, Approval>,
    questions: HashMap<Id, Question>,
    routines: HashMap<Id, Routine>,
    routine_runs: HashMap<Id, Vec<RoutineRun>>,
    idempotent_messages: HashMap<String, Id>,
    loop_states: HashMap<Id, String>,
    highlights: HashMap<Id, Vec<Highlight>>,
    templates: Vec<Template>,
}

fn new_id() -> Id {
    Uuid::now_v7().to_string()
}
fn now() -> String {
    Utc::now().to_rfc3339()
}
fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed
    }
}

impl Orchestrator {
    pub fn new(settings: OrchestratorSettings) -> Self {
        let ts = now();
        let main = Bot {
            id: "main".to_string(),
            name: "总管".to_string(),
            label: "主 Bot".to_string(),
            description: "负责协调团队、派发任务和提醒验收".to_string(),
            avatar: None,
            model: None,
            max_parallel: 0,
            tools: ToolToggles::default(),
            browser_mode: "headless".into(),
            dm_chat_id: "chat_main".into(),
            pinned: true,
            hidden: false,
            notifications: true,
            is_main: true,
            created_at: ts.clone(),
            updated_at: ts,
        };
        let templates = vec![
            Template {
                id: "product-code-test".into(),
                name: "产品 + 编码 + 测试".into(),
                description: "从需求到实现和验收的完整团队".into(),
                bots: vec![
                    TemplateBot {
                        name: "产品".into(),
                        label: "产品经理".into(),
                        description: "梳理需求、编写 PRD 和验收标准".into(),
                        avatar: None,
                    },
                    TemplateBot {
                        name: "编码".into(),
                        label: "工程师".into(),
                        description: "实现功能、编写测试和部署".into(),
                        avatar: None,
                    },
                    TemplateBot {
                        name: "测试".into(),
                        label: "测试工程师".into(),
                        description: "验证功能并报告问题".into(),
                        avatar: None,
                    },
                ],
            },
            Template {
                id: "research-writing".into(),
                name: "调研 + 写作".into(),
                description: "先调研再整理成稿".into(),
                bots: vec![
                    TemplateBot {
                        name: "调研".into(),
                        label: "研究员".into(),
                        description: "收集资料和事实".into(),
                        avatar: None,
                    },
                    TemplateBot {
                        name: "写作".into(),
                        label: "写作者".into(),
                        description: "组织内容并完成交付".into(),
                        avatar: None,
                    },
                ],
            },
        ];
        Self {
            inner: Arc::new(Mutex::new(Inner {
                settings,
                bots: HashMap::from([(main.id.clone(), main)]),
                projects: HashMap::new(),
                assignments: HashMap::new(),
                messages: HashMap::new(),
                artifacts: HashMap::new(),
                approvals: HashMap::new(),
                questions: HashMap::new(),
                routines: HashMap::new(),
                routine_runs: HashMap::new(),
                idempotent_messages: HashMap::new(),
                loop_states: HashMap::new(),
                highlights: HashMap::new(),
                templates,
            })),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>> {
        self.inner.lock().map_err(|_| OrchestratorError::Poisoned)
    }

    /// JSON RPC adapter used by gateway and by the mock server.
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let mut i = self.lock()?;
        i.rpc(method, params)
    }

    pub fn settings(&self) -> Result<OrchestratorSettings> {
        Ok(self.lock()?.settings.clone())
    }

    pub fn snapshot(&self) -> Result<Value> {
        let i = self.lock()?;
        Ok(json!({
            "settings": i.settings.clone(),
            "bots": i.bots.clone(),
            "projects": i.projects.clone(),
            "assignments": i.assignments.clone(),
            "messages": i.messages.clone(),
            "artifacts": i.artifacts.clone(),
            "approvals": i.approvals.clone(),
            "questions": i.questions.clone(),
            "routines": i.routines.clone(),
            "routine_runs": i.routine_runs.clone(),
            "idempotent_messages": i.idempotent_messages.clone(),
            "loop_states": i.loop_states.clone(),
            "highlights": i.highlights.clone(),
        }))
    }

    pub fn restore(&self, value: Value) -> Result<()> {
        let mut i = self.lock()?;
        if let Some(v) = value.get("settings") {
            i.settings = serde_json::from_value(v.clone())
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        }
        restore_map(&mut i.bots, value.get("bots"))?;
        for bot in i.bots.values_mut() {
            normalize_bot_dm_chat_id(bot);
        }
        restore_map(&mut i.projects, value.get("projects"))?;
        restore_map(&mut i.assignments, value.get("assignments"))?;
        restore_map(&mut i.messages, value.get("messages"))?;
        restore_map(&mut i.artifacts, value.get("artifacts"))?;
        restore_map(&mut i.approvals, value.get("approvals"))?;
        restore_map(&mut i.questions, value.get("questions"))?;
        restore_map(&mut i.routines, value.get("routines"))?;
        restore_map(&mut i.routine_runs, value.get("routine_runs"))?;
        restore_map(&mut i.idempotent_messages, value.get("idempotent_messages"))?;
        restore_map(&mut i.loop_states, value.get("loop_states"))?;
        restore_map(&mut i.highlights, value.get("highlights"))?;
        migrate_legacy_main_chat_ids(&mut i);
        migrate_legacy_routine_chat_ids(&mut i)?;
        Ok(())
    }

    pub fn with_defaults() -> Self {
        Self::new(OrchestratorSettings::default())
    }
    pub fn bots(&self) -> Result<Vec<Bot>> {
        Ok(self
            .lock()?
            .bots
            .values()
            .filter(|b| !b.hidden)
            .cloned()
            .collect())
    }

    pub fn create_assignment(&self, request: AssignmentRequest) -> Result<Assignment> {
        let mut i = self.lock()?;
        i.create_assignment(request)
    }

    pub fn send_msg(&self, request: SendMessageRequest) -> Result<Message> {
        let mut i = self.lock()?;
        i.send_msg(request)
    }

    pub fn queue_steer(&self, request: SteerRequest) -> Result<SteerDelivery> {
        let mut i = self.lock()?;
        i.queue_steer(request)
    }

    pub fn mark_steer_delivered(&self, message_id: &str) -> Result<SteerDelivery> {
        let mut i = self.lock()?;
        i.mark_steer(message_id, "delivered")
    }

    pub fn mark_steer_read(&self, message_id: &str) -> Result<SteerDelivery> {
        let mut i = self.lock()?;
        i.mark_steer(message_id, "read")
    }

    pub fn finish_assignment(&self, assignment_id: &str, status: &str) -> Result<Assignment> {
        let mut i = self.lock()?;
        i.finish_assignment(assignment_id, status)
    }

    pub fn update_assignment_usage(
        &self,
        assignment_id: &str,
        usage: UsageTotals,
    ) -> Result<Assignment> {
        let mut i = self.lock()?;
        let assignment = i.assignment_mut(assignment_id)?;
        assignment.usage = usage;
        Ok(assignment.clone())
    }

    pub fn start_subagent(&self, request: SubagentRequest) -> Result<SubagentHandle> {
        let mut i = self.lock()?;
        i.start_subagent(request)
    }

    pub fn finish_subagent(&self, assignment_id: &str, subagent_id: &str) -> Result<()> {
        let mut i = self.lock()?;
        i.finish_subagent(assignment_id, subagent_id)
    }

    pub fn create_approval(&self, request: ApprovalRequest) -> Result<Approval> {
        let mut i = self.lock()?;
        i.create_approval(request)
    }

    pub fn create_question(&self, request: QuestionRequest) -> Result<Question> {
        let mut i = self.lock()?;
        i.create_question(request)
    }

    pub fn tick_routines(&self, at: DateTime<Utc>) -> Result<Vec<RoutineRun>> {
        let mut i = self.lock()?;
        i.tick_routines(at)
    }

    pub fn finish_routine_run(
        &self,
        id: &str,
        status: &str,
        error: Option<String>,
    ) -> Result<Option<RoutineRun>> {
        let mut i = self.lock()?;
        i.finish_routine_run(id, status, error)
    }
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::with_defaults()
    }
}

fn restore_map<T: for<'de> Deserialize<'de>>(
    target: &mut HashMap<String, T>,
    value: Option<&Value>,
) -> Result<()> {
    if let Some(value) = value {
        *target = serde_json::from_value(value.clone())
            .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssignmentRequest {
    pub project_id: Option<Id>,
    pub origin_chat_id: Id,
    pub bot_id: Id,
    pub title: String,
    pub instruction: String,
    #[serde(default = "default_sender")]
    pub from: String,
    pub trigger_message_id: Option<Id>,
    pub parent_assignment_id: Option<Id>,
    #[serde(default = "default_priority")]
    pub priority: u8,
    pub root_message_id: Option<Id>,
    #[serde(default)]
    pub loop_hops: usize,
}
fn default_sender() -> String {
    "main".to_string()
}
fn default_priority() -> u8 {
    1
}

fn normalize_bot_dm_chat_id(bot: &mut Bot) {
    if bot.is_main || bot.id == "main" {
        bot.dm_chat_id = "chat_main".into();
    } else if bot.dm_chat_id.trim().is_empty() {
        bot.dm_chat_id = format!("dm_{}", bot.id);
    }
}

fn migrate_legacy_main_chat_ids(inner: &mut Inner) {
    for assignment in inner.assignments.values_mut() {
        if assignment.origin_chat_id == "dm_main" {
            assignment.origin_chat_id = "chat_main".into();
        }
    }
    for message in inner.messages.values_mut() {
        if message.chat_id == "dm_main" {
            message.chat_id = "chat_main".into();
        }
    }
    for approval in inner.approvals.values_mut() {
        if approval.chat_id == "dm_main" {
            approval.chat_id = "chat_main".into();
        }
    }
    for question in inner.questions.values_mut() {
        if question.chat_id == "dm_main" {
            question.chat_id = "chat_main".into();
        }
    }
}

fn migrate_legacy_routine_chat_ids(inner: &mut Inner) -> Result<()> {
    let mut assignment_routines = HashMap::new();
    for (routine_id, runs) in &inner.routine_runs {
        for run in runs {
            if let Some(assignment_id) = &run.assignment_id {
                assignment_routines.insert(assignment_id.clone(), routine_id.clone());
            }
        }
    }
    let assignment_updates = inner
        .assignments
        .iter()
        .filter_map(|(assignment_id, assignment)| {
            let routine_id = assignment_routines.get(assignment_id)?;
            if !assignment.origin_chat_id.starts_with("routine:") {
                return None;
            }
            let routine = inner.routines.get(routine_id)?;
            let chat_id = inner
                .resolve_routine_delivery(&routine.bot_id, routine.project_id.as_deref())
                .ok()?;
            Some((assignment_id.clone(), chat_id))
        })
        .collect::<Vec<_>>();
    for (assignment_id, chat_id) in assignment_updates {
        if let Some(assignment) = inner.assignments.get_mut(&assignment_id) {
            assignment.origin_chat_id = chat_id;
        }
    }
    let message_updates = inner
        .messages
        .iter()
        .filter_map(|(message_id, message)| {
            if !message.chat_id.starts_with("routine:") {
                return None;
            }
            let routine_id = message
                .assignment_id
                .as_ref()
                .and_then(|assignment_id| assignment_routines.get(assignment_id))
                .map(String::as_str)
                .or_else(|| message.chat_id.strip_prefix("routine:"))?;
            let routine = inner.routines.get(routine_id)?;
            let chat_id = inner
                .resolve_routine_delivery(&routine.bot_id, routine.project_id.as_deref())
                .ok()?;
            Some((message_id.clone(), chat_id))
        })
        .collect::<Vec<_>>();
    for (message_id, chat_id) in message_updates {
        if let Some(message) = inner.messages.get_mut(&message_id) {
            message.chat_id = chat_id;
        }
    }
    Ok(())
}

fn dm_chat_for_bot(bot: &Bot) -> Value {
    json!({
        "id": bot.dm_chat_id,
        "kind": if bot.is_main { "main" } else { "direct" },
        "title": bot.name,
        "bot_id": bot.id,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub bot_id: Id,
    pub chat_id: Id,
    pub assignment_id: Option<Id>,
    pub run_id: Option<Id>,
    pub call_id: Option<Id>,
    pub text: String,
    pub intent: String,
    #[serde(default)]
    pub mentions: Vec<MentionInput>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MentionInput {
    Bot {
        bot_id: Id,
        #[serde(default)]
        instruction: Option<String>,
    },
    Main(String),
    User(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SteerRequest {
    pub bot_id: Id,
    pub project_id: Option<Id>,
    pub chat_id: Id,
    pub text: String,
    pub message_id: Option<Id>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub bot_id: Id,
    pub assignment_id: Option<Id>,
    pub chat_id: Id,
    pub tool: String,
    pub risk: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub bot_id: Id,
    pub assignment_id: Id,
    pub chat_id: Id,
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub allow_free_text: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentRequest {
    pub assignment_id: Id,
    pub task: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentHandle {
    pub id: Id,
    pub assignment_id: Id,
    pub task: String,
}

impl Inner {
    fn resolve_routine_delivery(&self, bot_id: &str, project_id: Option<&str>) -> Result<Id> {
        if let Some(project_id) = project_id {
            if let Some(project) = self.projects.get(project_id) {
                if project.status != "archived" {
                    return Ok(project.chat_id.clone());
                }
            }
        }
        Ok(self
            .bots
            .get(bot_id)
            .map(|bot| bot.dm_chat_id.clone())
            .filter(|chat_id| !chat_id.trim().is_empty())
            .unwrap_or_else(|| "chat_main".into()))
    }

    fn project_id_for_chat(&self, chat_id: &str) -> Option<Id> {
        self.projects
            .values()
            .find(|project| project.chat_id == chat_id)
            .map(|project| project.id.clone())
    }

    fn rpc(&mut self, method: &str, p: Value) -> Result<Value> {
        match method {
            "bot.list" => Self::json(
                json!({ "bots": self.bots.values().filter(|b| p.get("include_hidden").and_then(Value::as_bool).unwrap_or(false) || !b.hidden).cloned().collect::<Vec<_>>() }),
            ),
            "bot.get" => {
                let id = str_param(&p, "bot_id")?;
                Ok(json!({ "bot": self.bot(&id)? }))
            }
            "bot.create" => Self::json(self.create_bot(&p)?),
            "bot.update" => {
                let id = str_param(&p, "bot_id")?;
                Ok(
                    json!({ "bot": self.update_bot(&id, p.get("patch").cloned().unwrap_or(Value::Null))? }),
                )
            }
            "bot.delete" => {
                let id = str_param(&p, "bot_id")?;
                self.delete_bot(&id)?;
                Ok(json!({}))
            }
            "bot.templates" => Ok(json!({ "templates": self.templates })),
            "bot.create_from_template" => {
                let id = str_param(&p, "template_id")?;
                Self::json(self.create_from_template(&id)?)
            }
            "project.list" => {
                Ok(json!({ "projects": self.projects.values().cloned().collect::<Vec<_>>() }))
            }
            "project.get" => {
                let id = str_param(&p, "project_id")?;
                let project = self.project(&id)?.clone();
                Ok(json!({ "project": project, "announcement": self.announcement(&id)? }))
            }
            "project.create" => Self::json(self.create_project(&p)?),
            "project.add_member" => {
                let id = str_param(&p, "project_id")?;
                self.add_member(
                    &id,
                    str_param(&p, "bot_id")?,
                    p.get("role_note")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                )?;
                Ok(json!({ "project": self.project(&id)? }))
            }
            "project.remove_member" => {
                let id = str_param(&p, "project_id")?;
                self.remove_member(&id, str_param(&p, "bot_id")?)?;
                Ok(json!({ "project": self.project(&id)? }))
            }
            "project.confirm_done" => {
                let id = str_param(&p, "project_id")?;
                let project = self.project_mut(&id)?;
                project.status = "done".into();
                project.done_at = Some(now());
                project.updated_at = now();
                Ok(json!({ "project": project }))
            }
            "project.request_changes" => {
                let id = str_param(&p, "project_id")?;
                let project = self.project_mut(&id)?;
                project.status = "active".into();
                project.updated_at = now();
                Ok(json!({ "project": project, "text": str_param(&p,"text")? }))
            }
            "project.archive" => {
                let id = str_param(&p, "project_id")?;
                let project = self.project_mut(&id)?;
                project.status = "archived".into();
                project.updated_at = now();
                Ok(json!({ "project": project }))
            }
            "project.reopen" => {
                let id = str_param(&p, "project_id")?;
                let project = self.project_mut(&id)?;
                project.status = "active".into();
                project.done_at = None;
                project.updated_at = now();
                Ok(json!({ "project": project }))
            }
            "assignment.create" | "assign" => Self::json(self.create_assignment(
                serde_json::from_value(p).map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            )?),
            "delegate" => {
                let bot_id = str_param(&p, "bot_id")?;
                let instruction = str_param(&p, "instruction")?;
                Self::json(
                    self.create_assignment(AssignmentRequest {
                        project_id: p
                            .get("project_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        origin_chat_id: p
                            .get("origin_chat_id")
                            .and_then(Value::as_str)
                            .unwrap_or("main-dm")
                            .into(),
                        bot_id,
                        title: p
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("转交任务")
                            .into(),
                        instruction,
                        from: p
                            .get("from")
                            .and_then(Value::as_str)
                            .unwrap_or("main")
                            .into(),
                        trigger_message_id: None,
                        parent_assignment_id: p
                            .get("parent_assignment_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        priority: 2,
                        root_message_id: p
                            .get("root_message_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        loop_hops: p.get("loop_hops").and_then(Value::as_u64).unwrap_or(0) as usize,
                    })?,
                )
            }
            "assignment.list" => {
                let mut items = self.assignments.values().cloned().collect::<Vec<_>>();
                items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                Self::json(json!({ "items": items, "next_cursor": null }))
            }
            "assignment.get" => {
                let id = str_param(&p, "assignment_id")?;
                Ok(json!({ "assignment": self.assignment(&id)? }))
            }
            "assignment.stop" => {
                let id = str_param(&p, "assignment_id")?;
                Ok(json!({ "assignment": self.finish_assignment(&id, "cancelled")? }))
            }
            "assignment.steer" | "steer" => Self::json(self.queue_steer(
                serde_json::from_value(p).map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            )?),
            "send_msg" => Self::json(self.send_msg(
                serde_json::from_value(p).map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            )?),
            "approval.list" => {
                Ok(json!({ "approvals": self.approvals.values().cloned().collect::<Vec<_>>() }))
            }
            "approval.decide" => Ok(
                json!({ "approval": self.decide_approval(str_param(&p, "approval_id")?, str_param(&p, "decision")?)? }),
            ),
            "question.answer" => Ok(json!({ "question": self.answer_question(
                    str_param(&p, "question_id")?,
                    p.get("option_index")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize),
                    p.get("text").and_then(Value::as_str).map(str::to_owned),
                )? })),
            "loop.resolve" => {
                let root = str_param(&p, "root_message_id")?;
                let action = str_param(&p, "action")?;
                if !matches!(action.as_str(), "continue" | "end") {
                    return Err(OrchestratorError::Invalid(
                        "action must be continue or end".into(),
                    ));
                }
                self.loop_states.insert(root, action);
                self.pump_queue();
                Ok(json!({}))
            }
            "project.status" | "project_status" => {
                let id = str_param(&p, "project_id")?;
                Ok(
                    json!({ "project": self.project(&id)?, "announcement": self.announcement(&id)? }),
                )
            }
            "propose_bot" => Ok(json!({
                "proposal_id": new_id(),
                "name": str_param(&p, "name")?,
                "label": p.get("label").and_then(Value::as_str).unwrap_or(""),
                "description": p.get("description").and_then(Value::as_str).unwrap_or(""),
                "state": "pending_user"
            })),
            "trace.history" => Ok(
                json!({ "items": [], "first_aseq": null, "last_aseq": null, "has_more_before": false, "live": false }),
            ),
            "workbench.get" => Self::json(self.workbench()),
            "routine.list" => {
                let bot = p.get("bot_id").and_then(Value::as_str);
                Ok(
                    json!({ "routines": self.routines.values().filter(|r| bot.is_none_or(|id| id == r.bot_id)).cloned().collect::<Vec<_>>() }),
                )
            }
            "routine.create" => Ok(json!({ "routine": self.create_routine(&p)? })),
            "routine.update" => Ok(json!({ "routine": self.update_routine(
                str_param(&p, "routine_id")?,
                p.get("patch").cloned().unwrap_or(Value::Null),
            )? })),
            "routine.delete" => {
                let id = str_param(&p, "routine_id")?;
                self.routines
                    .remove(&id)
                    .ok_or_else(|| OrchestratorError::NotFound(id.clone()))?;
                self.routine_runs.remove(&id);
                Ok(json!({}))
            }
            "routine.set_enabled" => {
                let id = str_param(&p, "routine_id")?;
                let enabled = p
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| OrchestratorError::Invalid("enabled is required".into()))?;
                let r = self.routine_mut(&id)?;
                r.enabled = enabled;
                if enabled {
                    r.next_run_at =
                        Some(next_routine_at(&r.schedules, &r.timezone, Utc::now())?.to_rfc3339());
                }
                r.updated_at = now();
                Ok(json!({ "routine": r }))
            }
            "routine.test_run" => {
                let (run, dispatch) = self.test_routine(str_param(&p, "routine_id")?)?;
                Ok(json!({ "run": run, "dispatch": [dispatch] }))
            }
            "routine.runs" => {
                let id = str_param(&p, "routine_id")?;
                Ok(
                    json!({ "runs": self.routine_runs.get(&id).cloned().unwrap_or_default().into_iter().rev().take(20).collect::<Vec<_>>() }),
                )
            }
            "routine.execution" => {
                let id = p
                    .get("run_id")
                    .or_else(|| p.get("assignment_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| OrchestratorError::Invalid("run_id is required".into()))?;
                let status = str_param(&p, "status")?;
                Ok(json!({
                    "run": self.finish_routine_run(
                        id,
                        &status,
                        p.get("error").and_then(Value::as_str).map(str::to_owned),
                    )?
                }))
            }
            "approval.request" => Self::json(self.create_approval(
                serde_json::from_value(p).map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            )?),
            "question.ask" => Self::json(self.create_question(
                serde_json::from_value(p).map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            )?),
            "subagent.start" => Self::json(self.start_subagent(
                serde_json::from_value(p).map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            )?),
            "subagent.finish" => {
                let assignment_id = str_param(&p, "assignment_id")?;
                let subagent_id = str_param(&p, "subagent_id")?;
                self.finish_subagent(&assignment_id, &subagent_id)?;
                Ok(json!({}))
            }
            _ => Err(OrchestratorError::MethodNotFound(method.into())),
        }
    }

    fn json<T: Serialize>(v: T) -> Result<Value> {
        serde_json::to_value(v).map_err(|e| OrchestratorError::Invalid(e.to_string()))
    }
    fn bot(&self, id: &str) -> Result<&Bot> {
        self.bots
            .get(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("bot {id}")))
    }
    fn project(&self, id: &str) -> Result<&Project> {
        self.projects
            .get(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("project {id}")))
    }
    fn project_mut(&mut self, id: &str) -> Result<&mut Project> {
        self.projects
            .get_mut(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("project {id}")))
    }
    fn assignment(&self, id: &str) -> Result<&Assignment> {
        self.assignments
            .get(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("assignment {id}")))
    }
    fn routine_mut(&mut self, id: &str) -> Result<&mut Routine> {
        self.routines
            .get_mut(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("routine {id}")))
    }

    fn create_bot(&mut self, p: &Value) -> Result<Value> {
        let name = str_param(p, "name")?;
        if self.bots.values().any(|b| b.name == name) {
            return Err(OrchestratorError::Conflict(format!(
                "bot name {name} already exists"
            )));
        }
        let ts = now();
        let id = new_id();
        let dm_chat_id = format!("dm_{id}");
        let bot = Bot {
            id: id.clone(),
            name,
            label: p.get("label").and_then(Value::as_str).unwrap_or("").into(),
            description: p
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            avatar: p.get("avatar").and_then(Value::as_str).map(str::to_owned),
            model: p.get("model").and_then(Value::as_str).map(str::to_owned),
            max_parallel: p
                .get("max_parallel")
                .and_then(Value::as_u64)
                .unwrap_or(self.settings.bot_default_limit as u64)
                as usize,
            tools: p
                .get("tools")
                .cloned()
                .map(|value| {
                    serde_json::from_value(value)
                        .map_err(|error| OrchestratorError::Invalid(error.to_string()))
                })
                .transpose()?
                .unwrap_or_default(),
            browser_mode: p
                .get("browser_mode")
                .and_then(Value::as_str)
                .unwrap_or("headless")
                .into(),
            dm_chat_id,
            pinned: false,
            hidden: false,
            notifications: true,
            is_main: false,
            created_at: ts.clone(),
            updated_at: ts,
        };
        self.bots.insert(id.clone(), bot.clone());
        Ok(json!({ "bot": bot, "dm_chat": dm_chat_for_bot(&bot) }))
    }

    fn update_bot(&mut self, id: &str, patch: Value) -> Result<Bot> {
        let bot = self
            .bots
            .get_mut(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("bot {id}")))?;
        if let Some(x) = patch.get("name").and_then(Value::as_str) {
            bot.name = x.into();
        }
        if let Some(x) = patch.get("label").and_then(Value::as_str) {
            bot.label = x.into();
        }
        if let Some(x) = patch.get("description").and_then(Value::as_str) {
            bot.description = x.into();
        }
        if let Some(x) = patch.get("avatar").and_then(Value::as_str) {
            bot.avatar = Some(x.into());
        }
        if let Some(x) = patch.get("model").and_then(Value::as_str) {
            bot.model = Some(x.into());
        }
        if let Some(value) = patch.get("tools") {
            bot.tools = serde_json::from_value(value.clone())
                .map_err(|error| OrchestratorError::Invalid(error.to_string()))?;
        }
        if let Some(x) = patch.get("browser_mode").and_then(Value::as_str) {
            bot.browser_mode = x.into();
        }
        if let Some(x) = patch.get("max_parallel").and_then(Value::as_u64) {
            bot.max_parallel = (x as usize).max(1);
        }
        if let Some(x) = patch.get("pinned").and_then(Value::as_bool) {
            bot.pinned = x;
        }
        if let Some(x) = patch.get("hidden").and_then(Value::as_bool) {
            bot.hidden = x;
        }
        if let Some(x) = patch.get("notifications").and_then(Value::as_bool) {
            bot.notifications = x;
        }
        bot.updated_at = now();
        Ok(bot.clone())
    }

    fn delete_bot(&mut self, id: &str) -> Result<()> {
        if id == "main" {
            return Err(OrchestratorError::Forbidden(
                "main Bot cannot be deleted".into(),
            ));
        }
        if self.assignments.values().any(|a| {
            a.bot_id == id
                && matches!(
                    a.status.as_str(),
                    "queued" | "working" | "waiting_user" | "waiting_bot"
                )
        }) {
            return Err(OrchestratorError::Conflict(
                "Bot has active assignments".into(),
            ));
        }
        self.bots
            .remove(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("bot {id}")))?;
        Ok(())
    }

    fn create_from_template(&mut self, id: &str) -> Result<Value> {
        let t = self
            .templates
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| OrchestratorError::NotFound(format!("template {id}")))?;
        let mut bots = Vec::new();
        let mut chats = Vec::new();
        for b in t.bots {
            if let Some(existing) = self.bots.values().find(|x| x.name == b.name).cloned() {
                chats.push(dm_chat_for_bot(&existing));
                bots.push(existing);
                continue;
            }
            let p = json!({"name":b.name,"label":b.label,"description":b.description,"avatar":b.avatar});
            let v = self.create_bot(&p)?;
            bots.push(
                serde_json::from_value(
                    v.get("bot")
                        .cloned()
                        .ok_or_else(|| OrchestratorError::Invalid("template bot result".into()))?,
                )
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
            );
            chats.push(v.get("dm_chat").cloned().unwrap_or(Value::Null));
        }
        Ok(json!({"bots":bots,"dm_chats":chats}))
    }

    fn create_project(&mut self, p: &Value) -> Result<Value> {
        let name = str_param(p, "name")?;
        let members = p
            .get("member_bot_ids")
            .and_then(Value::as_array)
            .ok_or_else(|| OrchestratorError::Invalid("member_bot_ids is required".into()))?;
        if members.is_empty() || members.len() > 6 {
            return Err(OrchestratorError::Invalid(
                "a project needs 1-6 member bots".into(),
            ));
        }
        let mut pm = Vec::new();
        for id in members {
            let id = str_value(id)?;
            let _ = self.bot(&id)?;
            pm.push(ProjectMember {
                bot_id: id,
                role_note: "".into(),
                joined_at: now(),
            });
        }
        let id = new_id();
        let ts = now();
        let project = Project {
            id: id.clone(),
            chat_id: new_id(),
            name: name.clone(),
            slug: slugify(&name),
            goal: str_param(p, "goal")?,
            flow: p
                .get("flow")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            deadline: p.get("deadline").and_then(Value::as_str).map(str::to_owned),
            home_path: format!("~/MacBot/projects/{}/", slugify(&name)),
            status: "active".into(),
            lead_bot_id: "main".into(),
            members: pm,
            created_by: p
                .get("created_by")
                .and_then(Value::as_str)
                .unwrap_or("user")
                .into(),
            created_at: ts.clone(),
            updated_at: ts,
            done_at: None,
        };
        self.highlights.insert(id.clone(), Vec::new());
        self.projects.insert(id.clone(), project.clone());
        Ok(
            json!({"project":project,"chat":{"id":project.chat_id,"kind":"project","title":name,"project_id":id}}),
        )
    }

    fn add_member(&mut self, project_id: &str, bot_id: Id, role_note: String) -> Result<()> {
        let _ = self.bot(&bot_id)?;
        let p = self.project_mut(project_id);
        let p = p?;
        if p.members.iter().any(|m| m.bot_id == bot_id) {
            return Ok(());
        }
        if p.members.len() >= 7 {
            return Err(OrchestratorError::Invalid(
                "project has at most 6 workers plus main".into(),
            ));
        }
        p.members.push(ProjectMember {
            bot_id,
            role_note,
            joined_at: now(),
        });
        p.updated_at = now();
        Ok(())
    }
    fn remove_member(&mut self, project_id: &str, bot_id: Id) -> Result<()> {
        let ids = self
            .assignments
            .values()
            .filter(|a| {
                a.project_id.as_deref() == Some(project_id)
                    && a.bot_id == bot_id
                    && matches!(
                        a.status.as_str(),
                        "queued" | "working" | "waiting_user" | "waiting_bot"
                    )
            })
            .map(|a| a.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.finish_assignment(&id, "cancelled")?;
        }
        let p = self.project_mut(project_id)?;
        p.members.retain(|m| m.bot_id != bot_id);
        p.updated_at = now();
        Ok(())
    }

    fn announcement(&self, project_id: &str) -> Result<Announcement> {
        let p = self.project(project_id)?;
        let artifacts = self
            .artifacts
            .values()
            .filter(|a| a.project_id.as_deref() == Some(project_id))
            .cloned()
            .collect();
        let members = p
            .members
            .iter()
            .map(|m| {
                let active = self.assignments.values().find(|a| {
                    a.project_id.as_deref() == Some(project_id)
                        && a.bot_id == m.bot_id
                        && matches!(
                            a.status.as_str(),
                            "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
                        )
                });
                AnnouncementMember {
                    bot_id: m.bot_id.clone(),
                    role_note: m.role_note.clone(),
                    state: active
                        .map(|a| a.status.clone())
                        .unwrap_or_else(|| "idle".into()),
                    current_assignment_id: active.map(|a| a.id.clone()),
                    since: active.and_then(|a| a.started_at.clone()),
                }
            })
            .collect();
        Ok(Announcement {
            project_id: project_id.into(),
            members,
            artifacts,
            highlights: self
                .highlights
                .get(project_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .rev()
                .take(20)
                .collect(),
            updated_at: now(),
        })
    }

    fn create_assignment(&mut self, request: AssignmentRequest) -> Result<Assignment> {
        let _ = self.bot(&request.bot_id)?;
        let bot_model = self.bot(&request.bot_id)?.model.clone();
        if request.from != "routine" {
            if let Some(pid) = &request.project_id {
                let _ = self.project(pid)?;
            }
        }
        let active_global = self
            .assignments
            .values()
            .filter(|a| a.status == "working")
            .count();
        let active_bot = self
            .assignments
            .values()
            .filter(|a| a.bot_id == request.bot_id && a.status == "working")
            .count();
        let same_project = self.assignments.values().any(|a| {
            a.project_id == request.project_id
                && a.bot_id == request.bot_id
                && matches!(a.status.as_str(), "working" | "queued")
        });
        let bot_limit = self.bot(&request.bot_id)?.max_parallel;
        let (status, reason) = if active_bot >= bot_limit {
            ("queued", Some("bot_parallel_limit"))
        } else if active_global >= self.settings.global_limit {
            ("queued", Some("global_limit"))
        } else if same_project {
            ("queued", Some("serial_in_project"))
        } else {
            ("working", None)
        };
        let id = new_id();
        let ts = now();
        let assignment = Assignment {
            id: id.clone(),
            project_id: request.project_id,
            origin_chat_id: request.origin_chat_id,
            bot_id: request.bot_id,
            title: request.title,
            instruction: request.instruction,
            from: request.from,
            trigger_message_id: request.trigger_message_id,
            parent_assignment_id: request.parent_assignment_id,
            status: status.into(),
            queue_reason: reason.map(str::to_owned),
            wait: None,
            created_at: ts.clone(),
            started_at: (status == "working").then_some(ts),
            finished_at: None,
            usage: UsageTotals::default(),
            subagents_active: 0,
            steers: Vec::new(),
            result_message_id: None,
            model: bot_model,
            priority: request.priority,
            root_message_id: request.root_message_id,
            loop_hops: request.loop_hops,
        };
        self.assignments.insert(id, assignment.clone());
        Ok(assignment)
    }

    fn pump_queue(&mut self) {
        loop {
            let global = self
                .assignments
                .values()
                .filter(|a| a.status == "working")
                .count();
            if global >= self.settings.global_limit {
                break;
            }
            let next = self
                .assignments
                .values()
                .filter(|a| a.status == "queued")
                .filter(|a| {
                    let n = self
                        .assignments
                        .values()
                        .filter(|x| x.bot_id == a.bot_id && x.status == "working")
                        .count();
                    let serial = self.assignments.values().any(|x| {
                        x.id != a.id
                            && x.project_id == a.project_id
                            && x.bot_id == a.bot_id
                            && x.status == "working"
                    });
                    n < self
                        .bots
                        .get(&a.bot_id)
                        .map(|b| b.max_parallel)
                        .unwrap_or(self.settings.bot_default_limit)
                        && !serial
                })
                .max_by_key(|a| (a.priority, std::cmp::Reverse(a.created_at.clone())))
                .map(|a| a.id.clone());
            let Some(id) = next else {
                break;
            };
            if let Some(a) = self.assignments.get_mut(&id) {
                a.status = "working".into();
                a.queue_reason = None;
                a.started_at = Some(now());
            }
        }
    }

    fn send_msg(&mut self, req: SendMessageRequest) -> Result<Message> {
        if !matches!(
            req.intent.as_str(),
            "ack" | "progress" | "decision" | "done" | "blocked"
        ) {
            return Err(OrchestratorError::Invalid("unknown send_msg intent".into()));
        }
        if let (Some(run), Some(call)) = (&req.run_id, &req.call_id) {
            if let Some(id) = self.idempotent_messages.get(&format!("{run}:{call}")) {
                return Ok(self
                    .messages
                    .get(id)
                    .ok_or_else(|| OrchestratorError::NotFound(id.clone()))?
                    .clone());
            }
        }
        if req.intent == "progress"
            && req
                .assignment_id
                .as_ref()
                .map(|id| {
                    self.messages
                        .values()
                        .filter(|m| {
                            m.assignment_id.as_ref() == Some(id)
                                && m.intent.as_deref() == Some("progress")
                        })
                        .count()
                })
                .unwrap_or(0)
                >= 3
        {
            return Err(OrchestratorError::Conflict(
                "an assignment may report at most three progress messages".into(),
            ));
        }
        let msg_id = new_id();
        let project_id = req
            .assignment_id
            .as_ref()
            .and_then(|id| self.assignments.get(id).and_then(|a| a.project_id.clone()))
            .or_else(|| self.project_id_for_chat(&req.chat_id));
        let mentions = req
            .mentions
            .iter()
            .map(|m| match m {
                MentionInput::Bot {
                    bot_id,
                    instruction,
                } => Mention::Bot {
                    bot_id: bot_id.clone(),
                    instruction: instruction.clone(),
                },
                MentionInput::Main(_) => Mention::Main,
                MentionInput::User(_) => Mention::User,
            })
            .collect::<Vec<_>>();
        let mut msg = Message {
            id: msg_id.clone(),
            chat_id: req.chat_id.clone(),
            sender: req.bot_id.clone(),
            created_at: now(),
            text: req.text.clone(),
            intent: Some(req.intent.clone()),
            assignment_id: req.assignment_id.clone(),
            mentions,
            artifacts: req.artifacts.clone(),
            options: req.options.clone(),
            delivery: Vec::new(),
            fallback_text: req.text.clone(),
        };
        if let Some(aid) = &req.assignment_id {
            let a = self.assignment_mut(aid)?;
            a.result_message_id =
                matches!(req.intent.as_str(), "done" | "blocked").then_some(msg_id.clone());
            match req.intent.as_str() {
                "decision" => {
                    a.status = if req
                        .mentions
                        .iter()
                        .any(|m| matches!(m, MentionInput::User(_)))
                    {
                        "waiting_user"
                    } else {
                        "waiting_bot"
                    }
                    .into();
                    a.wait = Some(WaitState {
                        reason: "decision".into(),
                        message_id: Some(msg_id.clone()),
                    });
                }
                "done" => {
                    a.status = "done".into();
                    a.finished_at = Some(now());
                    a.wait = None;
                }
                "blocked" => {
                    a.status = "blocked".into();
                    a.wait = Some(WaitState {
                        reason: "blocked".into(),
                        message_id: Some(msg_id.clone()),
                    });
                }
                _ => {}
            }
        }
        if let Some(project_id) = &project_id {
            if req.intent == "done"
                && req
                    .mentions
                    .iter()
                    .any(|m| matches!(m, MentionInput::Main(_)))
            {
                if let Some(project) = self.projects.get_mut(project_id) {
                    project.status = "review".into();
                    project.updated_at = now();
                }
            }
            if matches!(req.intent.as_str(), "decision" | "blocked") {
                let highlights = self.highlights.entry(project_id.clone()).or_default();
                highlights.push(Highlight {
                    text: req.text.clone(),
                    at: now(),
                });
                if highlights.len() > 20 {
                    let excess = highlights.len() - 20;
                    highlights.drain(..excess);
                }
            }
        }
        if req.intent == "decision" && !req.options.is_empty() {
            if let Some(assignment_id) = &req.assignment_id {
                let _ = self.create_question(QuestionRequest {
                    bot_id: req.bot_id.clone(),
                    assignment_id: assignment_id.clone(),
                    chat_id: req.chat_id.clone(),
                    text: req.text.clone(),
                    options: req.options.clone(),
                    allow_free_text: true,
                })?;
            }
        }
        self.messages.insert(msg_id.clone(), msg.clone());
        if let (Some(run), Some(call)) = (req.run_id, req.call_id) {
            self.idempotent_messages
                .insert(format!("{run}:{call}"), msg_id.clone());
        }
        for art in req.artifacts {
            let aid = new_id();
            let ts = now();
            self.artifacts.insert(
                aid.clone(),
                Artifact {
                    id: aid,
                    project_id: req
                        .assignment_id
                        .as_ref()
                        .and_then(|id| self.assignments.get(id)?.project_id.clone())
                        .or_else(|| project_id.clone()),
                    bot_id: req.bot_id.clone(),
                    assignment_id: req.assignment_id.clone().unwrap_or_default(),
                    title: art.title,
                    path_or_url: art.path_or_url,
                    kind: "file".into(),
                    created_at: ts.clone(),
                    updated_at: ts,
                },
            );
        }
        let parent_hops = req
            .assignment_id
            .as_ref()
            .and_then(|id| self.assignments.get(id).map(|a| a.loop_hops))
            .unwrap_or(0);
        let root = req
            .assignment_id
            .as_ref()
            .and_then(|id| {
                self.assignments
                    .get(id)
                    .and_then(|a| a.root_message_id.clone())
            })
            .unwrap_or_else(|| msg_id.clone());
        let mut seen = HashSet::new();
        for mention in msg.mentions.clone() {
            if let Mention::Bot {
                bot_id,
                instruction,
            } = mention
            {
                if !seen.insert(bot_id.clone()) {
                    continue;
                }
                if bot_id == "main" {
                    continue;
                }
                if parent_hops >= self.settings.loop_hops {
                    self.loop_states.insert(root.clone(), "paused".into());
                    continue;
                }
                let _ = self.create_assignment(AssignmentRequest {
                    project_id: project_id.clone(),
                    origin_chat_id: req.chat_id.clone(),
                    bot_id,
                    title: format!("交接：{}", req.intent),
                    instruction: instruction.unwrap_or(req.text.clone()),
                    from: req.bot_id.clone(),
                    trigger_message_id: Some(msg_id.clone()),
                    parent_assignment_id: req.assignment_id.clone(),
                    priority: 2,
                    root_message_id: Some(root.clone()),
                    loop_hops: parent_hops + 1,
                })?;
            }
        }
        self.pump_queue();
        msg.delivery = Vec::new();
        Ok(msg)
    }

    fn assignment_mut(&mut self, id: &str) -> Result<&mut Assignment> {
        self.assignments
            .get_mut(id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("assignment {id}")))
    }

    fn start_subagent(&mut self, request: SubagentRequest) -> Result<SubagentHandle> {
        let total = self
            .assignments
            .values()
            .map(|a| a.subagents_active)
            .sum::<usize>();
        let active = self.assignment(&request.assignment_id)?.subagents_active;
        if active >= self.settings.subagent_per_run {
            return Err(OrchestratorError::Conflict(
                "subagent per-run limit reached".into(),
            ));
        }
        if total >= self.settings.subagent_global {
            return Err(OrchestratorError::Conflict(
                "global subagent limit reached".into(),
            ));
        }
        let id = new_id();
        self.assignment_mut(&request.assignment_id)?
            .subagents_active += 1;
        Ok(SubagentHandle {
            id,
            assignment_id: request.assignment_id,
            task: request.task,
        })
    }

    fn finish_subagent(&mut self, assignment_id: &str, _subagent_id: &str) -> Result<()> {
        let a = self.assignment_mut(assignment_id)?;
        if a.subagents_active == 0 {
            return Err(OrchestratorError::Conflict("no active subagent".into()));
        }
        a.subagents_active -= 1;
        Ok(())
    }
    fn finish_assignment(&mut self, id: &str, status: &str) -> Result<Assignment> {
        if !matches!(status, "done" | "failed" | "cancelled" | "blocked") {
            return Err(OrchestratorError::Invalid("invalid final status".into()));
        }
        let a = self.assignment_mut(id)?;
        a.status = status.into();
        a.finished_at = Some(now());
        a.wait = None;
        let out = a.clone();
        self.pump_queue();
        Ok(out)
    }

    fn queue_steer(&mut self, req: SteerRequest) -> Result<SteerDelivery> {
        let message_id = req.message_id.unwrap_or_else(new_id);
        let aid = self
            .assignments
            .values()
            .filter(|a| {
                a.bot_id == req.bot_id
                    && a.project_id == req.project_id
                    && matches!(
                        a.status.as_str(),
                        "working" | "waiting_user" | "waiting_bot" | "blocked"
                    )
            })
            .max_by_key(|a| a.created_at.clone())
            .map(|a| a.id.clone());
        let state = if aid.is_some() { "queued" } else { "delivered" };
        let ts = now();
        let steer = Steer {
            message_id: message_id.clone(),
            text: req.text.clone(),
            at: ts.clone(),
            applied_at: (state == "delivered").then_some(ts.clone()),
        };
        if let Some(id) = &aid {
            let a = self.assignment_mut(id)?;
            if a.status == "waiting_user" || a.status == "waiting_bot" {
                a.status = "working".into();
                a.wait = None;
            }
            a.steers.push(steer);
        }
        Ok(SteerDelivery {
            message_id,
            bot_id: req.bot_id,
            assignment_id: aid,
            state: state.into(),
            at: ts,
        })
    }
    fn mark_steer(&mut self, message_id: &str, state: &str) -> Result<SteerDelivery> {
        for a in self.assignments.values_mut() {
            if let Some(s) = a.steers.iter_mut().find(|s| s.message_id == message_id) {
                let ts = now();
                s.applied_at = (state != "queued").then_some(ts.clone());
                return Ok(SteerDelivery {
                    message_id: message_id.into(),
                    bot_id: a.bot_id.clone(),
                    assignment_id: Some(a.id.clone()),
                    state: state.into(),
                    at: ts,
                });
            }
        }
        Err(OrchestratorError::NotFound(format!("steer {message_id}")))
    }

    fn create_approval(&mut self, r: ApprovalRequest) -> Result<Approval> {
        let id = new_id();
        let a = Approval {
            id: id.clone(),
            bot_id: r.bot_id,
            assignment_id: r.assignment_id.clone(),
            chat_id: r.chat_id,
            tool: r.tool,
            risk: r.risk,
            summary: r.summary,
            detail: r.detail,
            state: "pending".into(),
            created_at: now(),
            decided_at: None,
        };
        if let Some(x) = r.assignment_id {
            if let Some(asn) = self.assignments.get_mut(&x) {
                asn.status = "waiting_user".into();
                asn.wait = Some(WaitState {
                    reason: "approval".into(),
                    message_id: None,
                });
            }
        }
        self.approvals.insert(id, a.clone());
        Ok(a)
    }
    fn decide_approval(&mut self, id: String, decision: String) -> Result<Approval> {
        let a = self
            .approvals
            .get_mut(&id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("approval {id}")))?;
        let state = match decision.as_str() {
            "allow_once" => "allowed_once",
            "always_allow" => "always_allowed",
            "deny" => "denied",
            _ => {
                return Err(OrchestratorError::Invalid(
                    "invalid approval decision".into(),
                ))
            }
        };
        a.state = state.into();
        a.decided_at = Some(now());
        let out = a.clone();
        if let Some(x) = &out.assignment_id {
            if let Some(asn) = self.assignments.get_mut(x) {
                asn.wait = None;
                if state != "denied" {
                    asn.status = "working".into();
                }
            }
        }
        Ok(out)
    }
    fn create_question(&mut self, r: QuestionRequest) -> Result<Question> {
        let id = new_id();
        let q = Question {
            id: id.clone(),
            bot_id: r.bot_id,
            assignment_id: r.assignment_id.clone(),
            chat_id: r.chat_id,
            text: r.text,
            options: r.options,
            allow_free_text: r.allow_free_text,
            state: "pending".into(),
            answer: None,
        };
        if let Some(a) = self.assignments.get_mut(&r.assignment_id) {
            a.status = "waiting_user".into();
            a.wait = Some(WaitState {
                reason: "decision".into(),
                message_id: None,
            });
        }
        self.questions.insert(id, q.clone());
        Ok(q)
    }
    fn answer_question(
        &mut self,
        id: String,
        option: Option<usize>,
        text: Option<String>,
    ) -> Result<Question> {
        let q = self
            .questions
            .get_mut(&id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("question {id}")))?;
        if q.state != "pending" {
            return Err(OrchestratorError::Conflict(
                "question already answered".into(),
            ));
        }
        if option.is_none() && text.is_none() {
            return Err(OrchestratorError::Invalid(
                "an option or text is required".into(),
            ));
        }
        if let Some(n) = option {
            if n >= q.options.len() {
                return Err(OrchestratorError::Invalid(
                    "option index out of range".into(),
                ));
            }
        }
        q.answer = Some(QuestionAnswer {
            option_index: option,
            text,
            at: now(),
        });
        q.state = "answered".into();
        let out = q.clone();
        if let Some(a) = self.assignments.get_mut(&out.assignment_id) {
            a.status = "working".into();
            a.wait = None;
        }
        Ok(out)
    }

    fn workbench(&self) -> Value {
        let running = self
            .assignments
            .values()
            .filter(|a| a.status == "working")
            .count();
        let waiting = self
            .assignments
            .values()
            .filter(|a| {
                matches!(
                    a.status.as_str(),
                    "waiting_user" | "waiting_bot" | "blocked"
                )
            })
            .map(|a| json!({"kind":a.status,"assignment_id":a.id,"bot_id":a.bot_id}))
            .collect::<Vec<_>>();
        let bots=self.bots.values().filter(|b|!b.is_main).map(|b|json!({"bot_id":b.id,"active":self.assignments.values().filter(|a|a.bot_id==b.id&&matches!(a.status.as_str(),"working"|"queued"|"waiting_user"|"waiting_bot")).count(),"max_parallel":b.max_parallel,"assignments":self.assignments.values().filter(|a|a.bot_id==b.id&&matches!(a.status.as_str(),"working"|"queued"|"waiting_user"|"waiting_bot")).cloned().collect::<Vec<_>>() })).collect::<Vec<_>>();
        json!({"running":running,"global_limit":self.settings.global_limit,"subagents_running":self.assignments.values().map(|a|a.subagents_active).sum::<usize>(),"waiting":waiting,"bots":bots,"done_today":self.assignments.values().filter(|a|a.status=="done").cloned().collect::<Vec<_>>()})
    }

    fn create_routine(&mut self, p: &Value) -> Result<Routine> {
        let bot = str_param(p, "bot_id")?;
        let _ = self.bot(&bot)?;
        let n = self.routines.values().filter(|r| r.bot_id == bot).count();
        if n >= 50 {
            return Err(OrchestratorError::Conflict(
                "a Bot may have at most 50 routines".into(),
            ));
        }
        let schedules: Vec<Schedule> = serde_json::from_value(
            p.get("schedules")
                .cloned()
                .ok_or_else(|| OrchestratorError::Invalid("schedules is required".into()))?,
        )
        .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        let timezone = p
            .get("timezone")
            .and_then(Value::as_str)
            .unwrap_or("Asia/Shanghai");
        parse_timezone(timezone)?;
        validate_schedules(&schedules, timezone)?;
        let next_run_at = next_routine_at(&schedules, timezone, Utc::now())?.to_rfc3339();
        let ts = now();
        let r = Routine {
            id: new_id(),
            bot_id: bot,
            project_id: p
                .get("project_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: str_param(p, "name")?,
            instructions: str_param(p, "instructions")?,
            schedules,
            timezone: timezone.into(),
            enabled: true,
            next_run_at: Some(next_run_at),
            last_run: None,
            created_at: ts.clone(),
            updated_at: ts,
        };
        self.routines.insert(r.id.clone(), r.clone());
        Ok(r)
    }
    fn update_routine(&mut self, id: String, patch: Value) -> Result<Routine> {
        let mut updated = self
            .routines
            .get(&id)
            .cloned()
            .ok_or_else(|| OrchestratorError::NotFound(format!("routine {id}")))?;
        if let Some(x) = patch.get("name").and_then(Value::as_str) {
            updated.name = x.into();
        }
        if let Some(x) = patch.get("instructions").and_then(Value::as_str) {
            updated.instructions = x.into();
        }
        if let Some(x) = patch.get("timezone").and_then(Value::as_str) {
            parse_timezone(x)?;
            updated.timezone = x.into();
        }
        if let Some(v) = patch.get("schedules") {
            let s: Vec<Schedule> = serde_json::from_value(v.clone())
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
            updated.schedules = s;
        }
        if let Some(x) = patch.get("project_id") {
            updated.project_id = x.as_str().map(str::to_owned);
        }
        validate_schedules(&updated.schedules, &updated.timezone)?;
        updated.next_run_at =
            Some(next_routine_at(&updated.schedules, &updated.timezone, Utc::now())?.to_rfc3339());
        updated.updated_at = now();
        self.routines.insert(id, updated.clone());
        Ok(updated)
    }
    fn test_routine(&mut self, id: String) -> Result<(RoutineRun, Value)> {
        let (bot_id, project_id, name, instructions, schedules, timezone) = self
            .routines
            .get(&id)
            .map(|r| {
                (
                    r.bot_id.clone(),
                    r.project_id.clone(),
                    r.name.clone(),
                    r.instructions.clone(),
                    r.schedules.clone(),
                    r.timezone.clone(),
                )
            })
            .ok_or_else(|| OrchestratorError::NotFound(format!("routine {id}")))?;
        let origin_chat_id = self.resolve_routine_delivery(&bot_id, project_id.as_deref())?;
        let assignment = self.create_assignment(AssignmentRequest {
            project_id,
            origin_chat_id,
            bot_id,
            title: name,
            instruction: instructions,
            from: "routine".into(),
            trigger_message_id: None,
            parent_assignment_id: None,
            priority: 0,
            root_message_id: None,
            loop_hops: 0,
        })?;
        let ts = now();
        let run = RoutineRun {
            id: new_id(),
            routine_id: id.clone(),
            assignment_id: Some(assignment.id.clone()),
            trigger: "test".into(),
            status: "running".into(),
            started_at: ts.clone(),
            finished_at: None,
            error: None,
        };
        let next_run_at = next_routine_at(&schedules, &timezone, Utc::now())?;
        let r = self
            .routines
            .get_mut(&id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("routine {id}")))?;
        r.last_run = Some(run.clone());
        r.next_run_at = Some(next_run_at.to_rfc3339());
        r.updated_at = ts.clone();
        self.routine_runs.entry(id).or_default().push(run.clone());
        Ok((run.clone(), routine_dispatch(&run, &assignment)))
    }

    fn tick_routines(&mut self, at: DateTime<Utc>) -> Result<Vec<RoutineRun>> {
        let due = self
            .routines
            .values()
            .filter(|r| {
                r.enabled
                    && r.next_run_at
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .is_some_and(|next| next.with_timezone(&Utc) <= at)
            })
            .map(|r| r.id.clone())
            .collect::<Vec<_>>();
        let mut runs = Vec::new();
        for id in due {
            let (bot_id, project_id, name, instructions) = {
                let r = self
                    .routines
                    .get(&id)
                    .ok_or_else(|| OrchestratorError::NotFound(format!("routine {id}")))?;
                (
                    r.bot_id.clone(),
                    r.project_id.clone(),
                    r.name.clone(),
                    r.instructions.clone(),
                )
            };
            let origin_chat_id = self.resolve_routine_delivery(&bot_id, project_id.as_deref())?;
            let assignment = self.create_assignment(AssignmentRequest {
                project_id,
                origin_chat_id,
                bot_id,
                title: name,
                instruction: instructions,
                from: "routine".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 0,
                root_message_id: None,
                loop_hops: 0,
            })?;
            let ts = at.to_rfc3339();
            let run = RoutineRun {
                id: new_id(),
                routine_id: id.clone(),
                assignment_id: Some(assignment.id),
                trigger: "schedule".into(),
                status: "running".into(),
                started_at: ts,
                finished_at: None,
                error: None,
            };
            self.routine_runs
                .entry(id.clone())
                .or_default()
                .push(run.clone());
            let next_run_at = {
                let r = self
                    .routines
                    .get(&id)
                    .ok_or_else(|| OrchestratorError::NotFound(format!("routine {id}")))?;
                next_routine_at(&r.schedules, &r.timezone, at)?.to_rfc3339()
            };
            if let Some(r) = self.routines.get_mut(&id) {
                r.last_run = Some(run.clone());
                r.next_run_at = Some(next_run_at);
                r.updated_at = at.to_rfc3339();
            }
            runs.push(run);
        }
        Ok(runs)
    }

    fn finish_routine_run(
        &mut self,
        id: &str,
        status: &str,
        error: Option<String>,
    ) -> Result<Option<RoutineRun>> {
        let Some((routine_id, index)) = self.routine_runs.iter().find_map(|(routine_id, runs)| {
            runs.iter()
                .position(|run| run.id == id || run.assignment_id.as_deref() == Some(id))
                .map(|index| (routine_id.clone(), index))
        }) else {
            return Ok(None);
        };
        let run = self
            .routine_runs
            .get_mut(&routine_id)
            .and_then(|runs| runs.get_mut(index))
            .ok_or_else(|| OrchestratorError::NotFound(format!("routine run {id}")))?;
        if run.status != "running" {
            return Ok(None);
        }
        run.status = status.into();
        run.finished_at = Some(now());
        run.error = error;
        let output = run.clone();
        if let Some(routine) = self.routines.get_mut(&routine_id) {
            routine.last_run = Some(output.clone());
        }
        Ok(Some(output))
    }
}

fn str_param(p: &Value, k: &str) -> Result<String> {
    p.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| OrchestratorError::Invalid(format!("{k} is required")))
}
fn str_value(v: &Value) -> Result<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| OrchestratorError::Invalid("expected string id".into()))
}
const MIN_ROUTINE_INTERVAL: i64 = 5 * 60;
const CRON_SEARCH_MINUTES: usize = 366 * 24 * 60 * 5;

#[derive(Clone, Debug)]
struct CronExpression {
    minute: Vec<bool>,
    hour: Vec<bool>,
    day_of_month: Vec<bool>,
    month: Vec<bool>,
    day_of_week: Vec<bool>,
}

impl CronExpression {
    fn parse(value: &str) -> Result<Self> {
        let fields = value.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(OrchestratorError::Invalid(format!(
                "cron must have five fields: {value}"
            )));
        }
        Ok(Self {
            minute: parse_cron_field(fields[0], 0, 59, false)?,
            hour: parse_cron_field(fields[1], 0, 23, false)?,
            day_of_month: parse_cron_field(fields[2], 1, 31, false)?,
            month: parse_cron_field(fields[3], 1, 12, false)?,
            day_of_week: parse_cron_field(fields[4], 0, 6, true)?,
        })
    }

    fn matches(&self, value: NaiveDateTime) -> bool {
        if !self.minute[value.minute() as usize]
            || !self.hour[value.hour() as usize]
            || !self.month[value.month() as usize]
        {
            return false;
        }
        let dom = self.day_of_month[value.day() as usize];
        let dow = self.day_of_week[value.weekday().num_days_from_sunday() as usize];
        let dom_restricted = self.day_of_month[1..].iter().any(|enabled| !enabled);
        let dow_restricted = self.day_of_week.iter().any(|enabled| !enabled);
        match (dom_restricted, dow_restricted) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }
}

fn parse_cron_field(value: &str, min: u32, max: u32, sunday_alias: bool) -> Result<Vec<bool>> {
    if value.is_empty() {
        return Err(OrchestratorError::Invalid(
            "cron field cannot be empty".into(),
        ));
    }
    let mut enabled = vec![false; (max + 1) as usize];
    for item in value.split(',') {
        let (range, step, has_step) = item
            .split_once('/')
            .map_or((item, 1, false), |(range, step)| {
                (range, step.parse::<u32>().unwrap_or(0), true)
            });
        if step == 0 {
            return Err(OrchestratorError::Invalid(format!(
                "invalid cron step in {value}"
            )));
        }
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            let start = start
                .parse::<u32>()
                .map_err(|_| OrchestratorError::Invalid(format!("invalid cron range {range}")))?;
            let end = end
                .parse::<u32>()
                .map_err(|_| OrchestratorError::Invalid(format!("invalid cron range {range}")))?;
            if start > end {
                return Err(OrchestratorError::Invalid(format!(
                    "invalid cron range {range}"
                )));
            }
            (start, end)
        } else {
            let point = range
                .parse::<u32>()
                .map_err(|_| OrchestratorError::Invalid(format!("invalid cron value {range}")))?;
            (point, if has_step { max } else { point })
        };
        if (start < min || end > max) && !(sunday_alias && min == 0 && start <= 7 && end == 7) {
            return Err(OrchestratorError::Invalid(format!(
                "cron value outside range {range}"
            )));
        }
        let mut point = start;
        while point <= end {
            let normalized = if sunday_alias && point == 7 { 0 } else { point };
            enabled[normalized as usize] = true;
            match point.checked_add(step) {
                Some(next) => point = next,
                None => break,
            }
        }
    }
    if enabled.iter().all(|value| !value) {
        return Err(OrchestratorError::Invalid(format!(
            "cron field has no values: {value}"
        )));
    }
    Ok(enabled)
}

fn parse_timezone(value: &str) -> Result<Tz> {
    let normalized = match value {
        "UTC" => "Etc/UTC",
        other => other,
    };
    normalized
        .parse::<Tz>()
        .map_err(|_| OrchestratorError::Invalid(format!("invalid timezone {value}")))
}

fn next_local_occurrence(
    expression: &CronExpression,
    after: NaiveDateTime,
) -> Option<NaiveDateTime> {
    let time = after.time();
    let mut candidate = after
        - Duration::seconds(time.second() as i64)
        - Duration::nanoseconds(time.nanosecond() as i64)
        + Duration::minutes(1);
    for _ in 0..CRON_SEARCH_MINUTES {
        if expression.matches(candidate) {
            return Some(candidate);
        }
        candidate += Duration::minutes(1);
    }
    None
}

fn next_schedule_occurrence(
    expression: &CronExpression,
    timezone: Tz,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let mut local_after = after.with_timezone(&timezone).naive_local();
    for _ in 0..CRON_SEARCH_MINUTES {
        let local = next_local_occurrence(expression, local_after)?;
        match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) => {
                let value = value.with_timezone(&Utc);
                if value > after {
                    return Some(value);
                }
            }
            LocalResult::Ambiguous(first, second) => {
                let mut values = [first.with_timezone(&Utc), second.with_timezone(&Utc)];
                values.sort();
                if let Some(value) = values.into_iter().find(|value| *value > after) {
                    return Some(value);
                }
            }
            LocalResult::None => {}
        }
        local_after = local;
    }
    None
}

fn next_routine_at(
    schedules: &[Schedule],
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let timezone = parse_timezone(timezone)?;
    let mut next = None;
    for schedule in schedules {
        let expression = CronExpression::parse(&schedule.cron)?;
        if let Some(candidate) = next_schedule_occurrence(&expression, timezone, after) {
            next = Some(next.map_or(candidate, |current: DateTime<Utc>| current.min(candidate)));
        }
    }
    next.ok_or_else(|| OrchestratorError::Invalid("cron has no future occurrence".into()))
}

fn validate_schedules(schedules: &[Schedule], timezone: &str) -> Result<()> {
    if schedules.is_empty() {
        return Err(OrchestratorError::Invalid(
            "at least one schedule is required".into(),
        ));
    }
    let timezone = parse_timezone(timezone)?;
    let expressions = schedules
        .iter()
        .map(|schedule| {
            if schedule.cron.trim().is_empty() {
                return Err(OrchestratorError::Invalid("cron cannot be empty".into()));
            }
            CronExpression::parse(&schedule.cron)
        })
        .collect::<Result<Vec<_>>>()?;
    let anchor = DateTime::<Utc>::from_timestamp(1_704_067_200, 0)
        .expect("fixed cron validation anchor is valid");
    let mut occurrences = Vec::new();
    for expression in &expressions {
        let mut cursor = anchor - Duration::minutes(1);
        for _ in 0..16 {
            let Some(next) = next_schedule_occurrence(expression, timezone, cursor) else {
                break;
            };
            occurrences.push(next);
            cursor = next;
        }
    }
    occurrences.sort();
    occurrences.dedup();
    for window in occurrences.windows(2) {
        if (window[1] - window[0]).num_seconds() < MIN_ROUTINE_INTERVAL {
            return Err(OrchestratorError::Invalid(
                "routine schedules must be at least five minutes apart".into(),
            ));
        }
    }
    Ok(())
}

fn routine_dispatch(run: &RoutineRun, assignment: &Assignment) -> Value {
    json!({
        "run_id": run.id,
        "assignment_id": assignment.id,
        "bot_id": assignment.bot_id,
        "chat_id": assignment.origin_chat_id,
        "project_id": assignment.project_id,
        "instruction": assignment.instruction,
        "model": assignment.model,
        "trigger": run.trigger,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(o: &Orchestrator, name: &str) -> Id {
        futures_create_bot(o, name)
    }
    fn futures_create_bot(o: &Orchestrator, name: &str) -> Id {
        let v = o.rpc("bot.create", json!({"name":name}));
        let v = tokio::runtime::Runtime::new().unwrap().block_on(v).unwrap();
        v["bot"]["id"].as_str().unwrap().into()
    }

    #[test]
    fn bot_create_and_template_keep_dm_chat_id_in_sync() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc("bot.create", json!({"name":"单独 Bot"})))
            .unwrap();
        assert_eq!(
            created["bot"]["dm_chat_id"], created["dm_chat"]["id"],
            "bot.create must return the same direct-chat id in both objects"
        );
        assert_eq!(created["dm_chat"]["kind"], "direct");

        let templated = rt
            .block_on(o.rpc(
                "bot.create_from_template",
                json!({"template_id":"product-code-test"}),
            ))
            .unwrap();
        let bots = templated["bots"].as_array().unwrap();
        let chats = templated["dm_chats"].as_array().unwrap();
        assert_eq!(bots.len(), chats.len());
        for (bot, chat) in bots.iter().zip(chats) {
            assert_eq!(bot["dm_chat_id"], chat["id"]);
            assert_eq!(chat["kind"], "direct");
        }

        let listed = rt.block_on(o.rpc("bot.list", json!({}))).unwrap();
        let main = listed["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bot| bot["id"] == "main")
            .unwrap();
        assert_eq!(main["dm_chat_id"], "chat_main");
    }

    #[test]
    fn restore_normalizes_legacy_dm_ids() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc("bot.create", json!({"name":"旧数据 Bot"})))
            .unwrap();
        let worker_id = created["bot"]["id"].as_str().unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["bots"]["main"]["dm_chat_id"] = json!("dm_main");
        snapshot["bots"][worker_id]["dm_chat_id"] = json!("");
        o.restore(snapshot).unwrap();

        let listed = rt.block_on(o.rpc("bot.list", json!({}))).unwrap();
        let bots = listed["bots"].as_array().unwrap();
        let main = bots.iter().find(|bot| bot["id"] == "main").unwrap();
        let worker = bots.iter().find(|bot| bot["id"] == worker_id).unwrap();
        assert_eq!(main["dm_chat_id"], "chat_main");
        assert_eq!(worker["dm_chat_id"], format!("dm_{worker_id}"));
    }

    #[test]
    fn restore_migrates_legacy_main_dm_history_without_changing_refs() {
        let o = Orchestrator::default();
        let worker_id = bot(&o, "历史测试");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "dm_main".into(),
                bot_id: worker_id.clone(),
                title: "旧私聊任务".into(),
                instruction: "继续处理".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = o
            .send_msg(SendMessageRequest {
                bot_id: "main".into(),
                chat_id: "dm_main".into(),
                assignment_id: None,
                run_id: None,
                call_id: None,
                text: "旧历史".into(),
                intent: "ack".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec![],
            })
            .unwrap();
        let approval = o
            .create_approval(ApprovalRequest {
                bot_id: worker_id.clone(),
                assignment_id: Some(assignment.id.clone()),
                chat_id: "dm_main".into(),
                tool: "bash".into(),
                risk: "high".into(),
                summary: "旧审批".into(),
                detail: "继续".into(),
            })
            .unwrap();
        let question = o
            .create_question(QuestionRequest {
                bot_id: worker_id,
                assignment_id: assignment.id.clone(),
                chat_id: "dm_main".into(),
                text: "旧问题".into(),
                options: vec!["继续".into()],
                allow_free_text: false,
            })
            .unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["bots"]["main"]["dm_chat_id"] = json!("dm_main");

        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        let restored_snapshot = restored.snapshot().unwrap();
        assert_eq!(
            restored_snapshot["messages"][&message.id]["chat_id"],
            "chat_main"
        );
        assert_eq!(
            restored_snapshot["assignments"][&assignment.id]["origin_chat_id"],
            "chat_main"
        );
        assert_eq!(
            restored_snapshot["approvals"][&approval.id]["chat_id"],
            "chat_main"
        );
        assert_eq!(
            restored_snapshot["questions"][&question.id]["chat_id"],
            "chat_main"
        );
        assert!(restored_snapshot["messages"].get(&message.id).is_some());
        assert!(restored_snapshot["assignments"]
            .get(&assignment.id)
            .is_some());
    }

    #[test]
    fn send_msg_is_idempotent_and_hands_off() {
        let o = Orchestrator::new(Default::default());
        let b = bot(&o, "编码");
        let p = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"登录","goal":"邮箱","member_bot_ids":[b]}),
            ))
            .unwrap();
        let pid = p["project"]["id"].as_str().unwrap().to_string();
        let a = o
            .create_assignment(AssignmentRequest {
                project_id: Some(pid),
                origin_chat_id: "chat".into(),
                bot_id: b.clone(),
                title: "实现".into(),
                instruction: "做事".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let req = SendMessageRequest {
            bot_id: b.clone(),
            chat_id: "chat".into(),
            assignment_id: Some(a.id.clone()),
            run_id: Some("r".into()),
            call_id: Some("c".into()),
            text: "好了".into(),
            intent: "done".into(),
            mentions: vec![MentionInput::Main("main".into())],
            artifacts: vec![],
            options: vec![],
        };
        let m1 = o.send_msg(req.clone()).unwrap();
        let m2 = o.send_msg(req).unwrap();
        assert_eq!(m1.id, m2.id);
        assert_eq!(o.finish_assignment(&a.id, "done").unwrap().status, "done");
    }

    #[test]
    fn steer_delivery_progresses_queued_delivered_read() {
        let o = Orchestrator::new(Default::default());
        let b = bot(&o, "编码");
        let a = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: b.clone(),
                title: "x".into(),
                instruction: "x".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let s = o
            .queue_steer(SteerRequest {
                bot_id: b,
                project_id: None,
                chat_id: "chat".into(),
                text: "调整".into(),
                message_id: None,
            })
            .unwrap();
        assert_eq!(s.assignment_id, Some(a.id));
        assert_eq!(s.state, "queued");
        assert_eq!(
            o.mark_steer_delivered(&s.message_id).unwrap().state,
            "delivered"
        );
        assert_eq!(o.mark_steer_read(&s.message_id).unwrap().state, "read");
    }

    #[test]
    fn routine_limits_and_timezone_are_validated() {
        let o = Orchestrator::new(Default::default());
        let b = bot(&o, "定时");
        let r=tokio::runtime::Runtime::new().unwrap().block_on(o.rpc("routine.create",json!({"bot_id":b,"name":"检查","instructions":"检查","schedules":[{"cron":"0 * * * *","label":"hourly"}],"timezone":"Asia/Shanghai"}))).unwrap();
        assert_eq!(r["routine"]["timezone"], "Asia/Shanghai");
        let bad=tokio::runtime::Runtime::new().unwrap().block_on(o.rpc("routine.create",json!({"bot_id":b,"name":"坏","instructions":"坏","schedules":[{"cron":"* * * * *","label":"x"}],"timezone":"No/Such"})));
        assert!(bad.is_err());
    }

    #[test]
    fn group_done_creates_handoff_assignment() {
        let o = Orchestrator::default();
        let from = bot(&o, "产品");
        let to = bot(&o, "编码");
        let project = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"登录","goal":"邮箱","member_bot_ids":[from,to]}),
            ))
            .unwrap();
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project["project"]["id"].as_str().unwrap().into()),
                origin_chat_id: "project-chat".into(),
                bot_id: from.clone(),
                title: "PRD".into(),
                instruction: "写 PRD".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.send_msg(SendMessageRequest {
            bot_id: from,
            chat_id: "project-chat".into(),
            assignment_id: Some(assignment.id),
            run_id: None,
            call_id: None,
            text: "完成".into(),
            intent: "done".into(),
            mentions: vec![MentionInput::Bot {
                bot_id: to,
                instruction: Some("按 PRD 实现".into()),
            }],
            artifacts: vec![],
            options: vec![],
        })
        .unwrap();
        let list = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc("assignment.list", json!({})))
            .unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn group_send_msg_without_assignment_infers_project_for_handoff_and_artifact() {
        let o = Orchestrator::default();
        let from = bot(&o, "产品");
        let to = bot(&o, "编码");
        let project = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"登录","goal":"邮箱","member_bot_ids":[from,to]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();

        o.send_msg(SendMessageRequest {
            bot_id: from,
            chat_id: chat_id.clone(),
            assignment_id: None,
            run_id: None,
            call_id: None,
            text: "请编码实现".into(),
            intent: "ack".into(),
            mentions: vec![MentionInput::Bot {
                bot_id: to,
                instruction: Some("实现登录功能".into()),
            }],
            artifacts: vec![ArtifactRef {
                title: "需求".into(),
                path_or_url: "/tmp/requirements.md".into(),
            }],
            options: vec![],
        })
        .unwrap();

        let snapshot = o.snapshot().unwrap();
        let assignments = snapshot["assignments"].as_object().unwrap();
        let assignment = assignments.values().next().unwrap();
        assert_eq!(assignment["project_id"], project_id);
        assert_eq!(assignment["origin_chat_id"], chat_id);
        let artifact = snapshot["artifacts"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        assert_eq!(artifact["project_id"], project_id);
    }

    #[test]
    fn decision_steer_resumes_waiting_assignment() {
        let o = Orchestrator::default();
        let b = bot(&o, "编码");
        let a = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: b.clone(),
                title: "决策".into(),
                instruction: "询问".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.send_msg(SendMessageRequest {
            bot_id: b.clone(),
            chat_id: "chat".into(),
            assignment_id: Some(a.id.clone()),
            run_id: None,
            call_id: None,
            text: "选一个".into(),
            intent: "decision".into(),
            mentions: vec![MentionInput::User("user".into())],
            artifacts: vec![],
            options: vec!["A".into(), "B".into()],
        })
        .unwrap();
        let waiting = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc("assignment.get", json!({"assignment_id":a.id})))
            .unwrap();
        assert_eq!(waiting["assignment"]["status"], "waiting_user");
        let steer = o
            .queue_steer(SteerRequest {
                bot_id: b,
                project_id: None,
                chat_id: "chat".into(),
                text: "选 A".into(),
                message_id: None,
            })
            .unwrap();
        assert_eq!(steer.state, "queued");
        let resumed = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc("assignment.get", json!({"assignment_id":a.id})))
            .unwrap();
        assert_eq!(resumed["assignment"]["status"], "working");
    }

    #[test]
    fn subagent_limit_loop_resolution_and_routine_tick() {
        let settings = OrchestratorSettings {
            subagent_per_run: 1,
            ..Default::default()
        };
        let o = Orchestrator::new(settings);
        let b = bot(&o, "执行");
        let a = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: b,
                title: "工作".into(),
                instruction: "工作".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: Some("root".into()),
                loop_hops: 8,
            })
            .unwrap();
        let child = o
            .start_subagent(SubagentRequest {
                assignment_id: a.id.clone(),
                task: "调研".into(),
            })
            .unwrap();
        assert!(o
            .start_subagent(SubagentRequest {
                assignment_id: a.id.clone(),
                task: "第二个".into()
            })
            .is_err());
        o.finish_subagent(&a.id, &child.id).unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "loop.resolve",
                json!({"root_message_id":"root","action":"continue"}),
            ))
            .unwrap();
        let bot_id = a.bot_id.clone();
        let routine = tokio::runtime::Runtime::new().unwrap().block_on(o.rpc("routine.create", json!({"bot_id":bot_id,"name":"轮询","instructions":"轮询","schedules":[{"cron":"*/5 * * * *","label":"every five minutes"}],"timezone":"America/New_York"}))).unwrap();
        assert_eq!(routine["routine"]["timezone"], "America/New_York");
        let due = DateTime::parse_from_rfc3339(routine["routine"]["next_run_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let runs = o.tick_routines(due + Duration::seconds(1)).unwrap();
        assert_eq!(runs.len(), 1);
        let next = routine["routine"]["next_run_at"].as_str().unwrap();
        assert!(!next.is_empty());
    }

    #[test]
    fn routine_cron_timezone_and_minimum_interval_are_enforced() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "定时测试");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let invalid_cron = rt.block_on(o.rpc(
            "routine.create",
            json!({"bot_id":bot_id,"name":"太快","instructions":"x","schedules":[{"cron":"*/2 * * * *","label":"too fast"}],"timezone":"Asia/Shanghai"}),
        ));
        assert!(invalid_cron.is_err());
        let invalid_shape = rt.block_on(o.rpc(
            "routine.create",
            json!({"bot_id":bot_id,"name":"坏表达式","instructions":"x","schedules":[{"cron":"every minute","label":"bad"}],"timezone":"Asia/Shanghai"}),
        ));
        assert!(invalid_shape.is_err());
        let invalid_zone = rt.block_on(o.rpc(
            "routine.create",
            json!({"bot_id":bot_id,"name":"坏时区","instructions":"x","schedules":[{"cron":"0 * * * *","label":"hourly"}],"timezone":"No/Such"}),
        ));
        assert!(invalid_zone.is_err());

        let created = rt
            .block_on(o.rpc(
                "routine.create",
                json!({"bot_id":bot_id,"name":"多时区","instructions":"巡检","schedules":[{"cron":"0 9 * * *","label":"morning"},{"cron":"30 8 * * *","label":"early"}],"timezone":"America/New_York"}),
            ))
            .unwrap();
        let next =
            DateTime::parse_from_rfc3339(created["routine"]["next_run_at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc);
        assert!(next > Utc::now() - Duration::minutes(1));
        let expression = CronExpression::parse("30 1 * * *").unwrap();
        let dst_anchor = DateTime::parse_from_rfc3339("2024-11-03T04:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let dst_next = next_schedule_occurrence(
            &expression,
            parse_timezone("America/New_York").unwrap(),
            dst_anchor,
        )
        .unwrap();
        assert_eq!(
            dst_next
                .with_timezone(&parse_timezone("America/New_York").unwrap())
                .hour(),
            1
        );
        let month_start = CronExpression::parse("0 9 1 * *").unwrap();
        let day_anchor = DateTime::parse_from_rfc3339("2024-02-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let day_next = next_schedule_occurrence(
            &month_start,
            parse_timezone("Asia/Shanghai").unwrap(),
            day_anchor,
        )
        .unwrap();
        let day_local = day_next.with_timezone(&parse_timezone("Asia/Shanghai").unwrap());
        assert_eq!(
            (day_local.month(), day_local.day(), day_local.hour()),
            (3, 1, 9)
        );
    }

    #[test]
    fn routine_test_run_creates_assignment_and_dispatch_descriptor() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "测试执行");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc(
                "routine.create",
                json!({"bot_id":bot_id,"name":"手动运行","instructions":"执行一次 fake","schedules":[{"cron":"*/5 * * * *","label":"five"}],"timezone":"Asia/Shanghai"}),
            ))
            .unwrap();
        let routine_id = created["routine"]["id"].as_str().unwrap();
        let result = rt
            .block_on(o.rpc("routine.test_run", json!({"routine_id":routine_id})))
            .unwrap();
        let run = &result["run"];
        let assignment_id = run["assignment_id"].as_str().unwrap();
        assert_eq!(result["dispatch"][0]["run_id"], run["id"]);
        assert_eq!(result["dispatch"][0]["assignment_id"], assignment_id);
        let assignment = rt
            .block_on(o.rpc("assignment.get", json!({"assignment_id":assignment_id})))
            .unwrap();
        assert_eq!(assignment["assignment"]["instruction"], "执行一次 fake");
        assert_eq!(assignment["assignment"]["status"], "working");
    }

    #[test]
    fn finish_routine_run_updates_run_and_last_run_idempotently() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "完成定时");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc(
                "routine.create",
                json!({"bot_id":bot_id,"name":"执行一次","instructions":"x","schedules":[{"cron":"*/5 * * * *","label":"five"}]}),
            ))
            .unwrap();
        let routine_id = created["routine"]["id"].as_str().unwrap();
        let started = rt
            .block_on(o.rpc("routine.test_run", json!({"routine_id":routine_id})))
            .unwrap();
        let run_id = started["run"]["id"].as_str().unwrap();
        let assignment_id = started["run"]["assignment_id"].as_str().unwrap();
        let finished = o.finish_routine_run(run_id, "done", None).unwrap().unwrap();
        assert_eq!(finished.status, "done");
        let snapshot = o.snapshot().unwrap();
        assert_eq!(snapshot["routines"][routine_id]["last_run"]["id"], run_id);
        assert_eq!(
            snapshot["routines"][routine_id]["last_run"]["status"],
            "done"
        );
        assert_eq!(
            snapshot["routine_runs"][routine_id][0]["assignment_id"],
            assignment_id
        );
        assert!(o
            .finish_routine_run(run_id, "failed", Some("late".into()))
            .unwrap()
            .is_none());
        assert!(o
            .finish_routine_run("non-routine-assignment", "done", None)
            .unwrap()
            .is_none());
    }
    #[test]
    fn routine_delivery_uses_real_chat_and_migrates_legacy_origin() {
        let o = Orchestrator::default();
        let worker = bot(&o, "定时路由");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"路由项目","goal":"x","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let project_chat = project["chat"]["id"].as_str().unwrap();
        let routine = rt
            .block_on(o.rpc(
                "routine.create",
                json!({"bot_id":worker,"project_id":project_id,"name":"群定时","instructions":"x","schedules":[{"cron":"*/5 * * * *","label":"five"}]}),
            ))
            .unwrap();
        let routine_id = routine["routine"]["id"].as_str().unwrap();
        let run = rt
            .block_on(o.rpc("routine.test_run", json!({"routine_id":routine_id})))
            .unwrap();
        let assignment_id = run["run"]["assignment_id"].as_str().unwrap();
        let assignment = rt
            .block_on(o.rpc("assignment.get", json!({"assignment_id":assignment_id})))
            .unwrap();
        assert_eq!(assignment["assignment"]["origin_chat_id"], project_chat);

        let dm_routine = rt
            .block_on(o.rpc(
                "routine.create",
                json!({"bot_id":worker,"name":"私聊定时","instructions":"x","schedules":[{"cron":"*/5 * * * *","label":"five"}]}),
            ))
            .unwrap();
        let dm_run = rt
            .block_on(o.rpc(
                "routine.test_run",
                json!({"routine_id":dm_routine["routine"]["id"]}),
            ))
            .unwrap();
        let dm_chat = o.snapshot().unwrap()["bots"][&worker]["dm_chat_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let dm_assignment = rt
            .block_on(o.rpc(
                "assignment.get",
                json!({"assignment_id":dm_run["run"]["assignment_id"]}),
            ))
            .unwrap();
        assert_eq!(dm_assignment["assignment"]["origin_chat_id"], dm_chat);

        o.send_msg(SendMessageRequest {
            bot_id: worker.clone(),
            chat_id: format!("routine:{routine_id}"),
            assignment_id: Some(assignment_id.into()),
            run_id: None,
            call_id: None,
            text: "旧消息".into(),
            intent: "ack".into(),
            mentions: vec![],
            artifacts: vec![],
            options: vec![],
        })
        .unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["assignments"][assignment_id]["origin_chat_id"] =
            json!(format!("routine:{routine_id}"));
        for message in snapshot["messages"].as_object_mut().unwrap().values_mut() {
            if message["assignment_id"].as_str() == Some(assignment_id) {
                message["chat_id"] = json!(format!("routine:{routine_id}"));
            }
        }
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        let restored_snapshot = restored.snapshot().unwrap();
        assert_eq!(
            restored_snapshot["assignments"][assignment_id]["origin_chat_id"],
            project_chat
        );
        assert!(restored_snapshot["messages"]
            .as_object()
            .unwrap()
            .values()
            .any(|message| {
                message["assignment_id"].as_str() == Some(assignment_id)
                    && message["chat_id"] == project_chat
            }));

        let mut deleted_project_snapshot = o.snapshot().unwrap();
        deleted_project_snapshot["projects"]
            .as_object_mut()
            .unwrap()
            .remove(project_id);
        let deleted_project = Orchestrator::default();
        deleted_project.restore(deleted_project_snapshot).unwrap();
        let deleted_run = rt
            .block_on(deleted_project.rpc("routine.test_run", json!({"routine_id":routine_id})))
            .unwrap();
        let deleted_assignment = rt
            .block_on(deleted_project.rpc(
                "assignment.get",
                json!({"assignment_id":deleted_run["run"]["assignment_id"]}),
            ))
            .unwrap();
        assert_eq!(deleted_assignment["assignment"]["origin_chat_id"], dm_chat);
        assert_eq!(deleted_assignment["assignment"]["project_id"], project_id);
    }
}
