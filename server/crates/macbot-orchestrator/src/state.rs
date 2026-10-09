use crate::model::*;
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::de::Error as DeError;
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

/// A durable, idempotent signal that a worker assignment needs attention from
/// the main Bot.  The gateway turns this into the canonical system message and
/// event; keeping the value here lets the scheduler poll without inventing a
/// second public protocol type.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttentionNotice {
    pub message: Message,
    pub code: String,
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
    question_created_at: HashMap<Id, String>,
    question_scopes: HashMap<Id, QuestionScope>,
    routines: HashMap<Id, Routine>,
    routine_runs: HashMap<Id, Vec<RoutineRun>>,
    approval_rules: Vec<ApprovalRuleRecord>,
    idempotent_messages: HashMap<String, Id>,
    loop_states: HashMap<Id, String>,
    loop_pauses: HashMap<Id, LoopPause>,
    pending_loop_dispatches: Vec<PendingLoopDispatch>,
    attention_notices: HashSet<String>,
    highlights: HashMap<Id, Vec<Highlight>>,
    templates: Vec<Template>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LoopPause {
    root_message_id: Id,
    hops: usize,
    state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingLoopDispatch {
    root_message_id: Id,
    project_id: Option<Id>,
    origin_chat_id: Id,
    bot_id: Id,
    title: String,
    instruction: String,
    from: Id,
    parent_assignment_id: Option<Id>,
    priority: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct QuestionScope {
    bot_id: Id,
    chat_id: Id,
}

fn new_id() -> Id {
    Uuid::now_v7().to_string()
}
fn now() -> String {
    Utc::now().to_rfc3339()
}
fn timestamp_nanos(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| value.timestamp_nanos_opt())
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

fn private_question_scope(chat_id: &str) -> Id {
    let component = chat_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("dm_{component}")
}

fn same_bot_id(left: &str, right: &str) -> bool {
    left == right || (matches!(left, "main" | "bot_main") && matches!(right, "main" | "bot_main"))
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
                question_created_at: HashMap::new(),
                question_scopes: HashMap::new(),
                routines: HashMap::new(),
                routine_runs: HashMap::new(),
                approval_rules: Vec::new(),
                idempotent_messages: HashMap::new(),
                loop_states: HashMap::new(),
                loop_pauses: HashMap::new(),
                pending_loop_dispatches: Vec::new(),
                attention_notices: HashSet::new(),
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

    pub fn configure(&self, settings: OrchestratorSettings) -> Result<()> {
        let mut inner = self.lock()?;
        inner.settings = settings;
        inner.pump_queue();
        Ok(())
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
            "question_created_at": i.question_created_at.clone(),
            "question_scopes": i.question_scopes.clone(),
            "routines": i.routines.clone(),
            "routine_runs": i.routine_runs.clone(),
            "approval_rules": i.approval_rules.clone(),
            "idempotent_messages": i.idempotent_messages.clone(),
            "loop_states": i.loop_states.clone(),
            "loop_pauses": i.loop_pauses.clone(),
            "pending_loop_dispatches": i.pending_loop_dispatches.clone(),
            "attention_notices": i.attention_notices.clone(),
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
        for project in i.projects.values_mut() {
            ensure_main_project_member(project);
        }
        restore_map(&mut i.assignments, value.get("assignments"))?;
        restore_map(&mut i.messages, value.get("messages"))?;
        restore_map(&mut i.artifacts, value.get("artifacts"))?;
        restore_map(&mut i.approvals, value.get("approvals"))?;
        restore_map(&mut i.questions, value.get("questions"))?;
        restore_map(&mut i.question_created_at, value.get("question_created_at"))?;
        restore_map(&mut i.question_scopes, value.get("question_scopes"))?;
        restore_map(&mut i.routines, value.get("routines"))?;
        restore_map(&mut i.routine_runs, value.get("routine_runs"))?;
        if let Some(v) = value.get("approval_rules") {
            i.approval_rules = serde_json::from_value(v.clone())
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        }
        restore_map(&mut i.idempotent_messages, value.get("idempotent_messages"))?;
        restore_map(&mut i.loop_states, value.get("loop_states"))?;
        restore_map(&mut i.loop_pauses, value.get("loop_pauses"))?;
        if let Some(v) = value.get("pending_loop_dispatches") {
            i.pending_loop_dispatches = serde_json::from_value(v.clone())
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        }
        if let Some(v) = value.get("attention_notices") {
            i.attention_notices = serde_json::from_value(v.clone())
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        }
        restore_map(&mut i.highlights, value.get("highlights"))?;
        migrate_legacy_main_chat_ids(&mut i);
        restore_legacy_question_message_links(&mut i);
        restore_legacy_decision_wait_messages(&mut i);
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

    /// Resolve the stable read-only Bot-to-Bot DM used by
    /// `send_msg(to:{bot})`. This is deliberately separate from a Bot's
    /// user-facing `dm_chat_id`.
    pub fn bot_dm_route(&self, from_bot_id: &str, to_bot_id: &str) -> Result<BotDmRoute> {
        if from_bot_id == to_bot_id {
            return Err(OrchestratorError::Invalid(
                "a Bot cannot send a Bot DM to itself".into(),
            ));
        }
        let inner = self.lock()?;
        let from = inner.bot(from_bot_id)?;
        let to = inner.bot(to_bot_id)?;
        let mut members = vec![
            (from.id.clone(), from.name.clone()),
            (to.id.clone(), to.name.clone()),
        ];
        members.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(BotDmRoute {
            chat_id: format!("bot_dm_{}_{}", members[0].0, members[1].0),
            kind: "bot_dm".into(),
            title: format!("{} ↔ {}", members[0].1, members[1].1),
            member_bot_ids: members.into_iter().map(|(id, _)| id).collect(),
            read_only: true,
        })
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

    /// Poll worker assignments for durable attention signals.  Call this after
    /// a terminal assignment update and from the regular scheduler tick.
    pub fn poll_project_attention(&self, at: DateTime<Utc>) -> Result<Vec<AttentionNotice>> {
        let mut inner = self.lock()?;
        inner.poll_project_attention(at)
    }

    /// Mark a project ready for user review without synthesizing a worker
    /// completion message.  The gateway owns the wire-level review card.
    pub fn mark_project_review(&self, project_id: &str) -> Result<Project> {
        let mut i = self.lock()?;
        i.mark_project_review(project_id)
    }

    pub fn create_task_stopped_message(&self, assignment_id: &str) -> Result<Message> {
        let mut i = self.lock()?;
        i.create_task_stopped_message(assignment_id)
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

    /// Reconcile a durable decision message after restoring an older snapshot.
    ///
    /// This deliberately accepts the canonical message identity and routing
    /// fields from the gateway instead of inferring anything from the message
    /// text.  It is used for jobs which already persisted a decision wait but
    /// whose Question record was not persisted by an older server.
    pub fn reconcile_waiting_decision(
        &self,
        message_id: &str,
        assignment_id: Option<&str>,
        bot_id: &str,
        chat_id: &str,
    ) -> Result<bool> {
        let mut i = self.lock()?;
        i.reconcile_waiting_decision(message_id, assignment_id, bot_id, chat_id)
    }

    /// Finish a Bot-to-Bot decision handoff without executing the continuation.
    /// Runtime calls this after the child Bot has produced its answer; the
    /// method only resolves the parent's linked question (when present) and
    /// makes that parent runnable again.
    pub fn answer_decision_for_child(
        &self,
        parent_assignment_id: &str,
        text: String,
    ) -> Result<Option<Question>> {
        let mut i = self.lock()?;
        i.answer_decision_for_child(parent_assignment_id, text)
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotDmRoute {
    pub chat_id: Id,
    pub kind: String,
    pub title: String,
    pub member_bot_ids: Vec<Id>,
    pub read_only: bool,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ApprovalRuleRecord {
    id: Id,
    kind: String,
    text: String,
    created_at: String,
}

fn ensure_main_project_member(project: &mut Project) {
    if !project.members.iter().any(|member| member.bot_id == "main") {
        project.members.insert(
            0,
            ProjectMember {
                bot_id: "main".into(),
                role_note: String::new(),
                joined_at: project.created_at.clone(),
            },
        );
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

fn restore_legacy_question_message_links(inner: &mut Inner) {
    let links = inner
        .messages
        .values()
        .filter(|message| {
            message.question_id.is_none()
                && message.intent.as_deref() == Some("decision")
                && !message.options.is_empty()
        })
        .filter_map(|message| {
            let private_scope = message.assignment_id.is_none();
            let assignment_id = message
                .assignment_id
                .clone()
                .unwrap_or_else(|| private_question_scope(&message.chat_id));
            let mut candidates = inner
                .questions
                .values()
                .filter(|question| {
                    question.state == "pending"
                        && question.assignment_id.as_str() == assignment_id.as_str()
                        && question.chat_id == message.chat_id
                        && (!private_scope || question.bot_id == message.sender)
                        && question.text == message.text
                        && question.options == message.options
                })
                .collect::<Vec<_>>();
            let question = if candidates.len() == 1 {
                candidates.pop()?
            } else {
                let message_at = timestamp_nanos(&message.created_at)?;
                candidates.retain(|question| inner.question_created_at.contains_key(&question.id));
                candidates.sort_by_key(|question| {
                    inner
                        .question_created_at
                        .get(&question.id)
                        .and_then(|created_at| timestamp_nanos(created_at))
                        .map(|at| (at - message_at).abs())
                        .unwrap_or(i64::MAX)
                });
                let first = candidates.first()?;
                let first_distance = inner
                    .question_created_at
                    .get(&first.id)
                    .and_then(|created_at| timestamp_nanos(created_at))?
                    .saturating_sub(message_at)
                    .abs();
                let second_distance = candidates
                    .get(1)
                    .and_then(|question| {
                        inner
                            .question_created_at
                            .get(&question.id)
                            .and_then(|created_at| timestamp_nanos(created_at))
                    })
                    .map(|at| at.saturating_sub(message_at).abs());
                if second_distance.is_some_and(|distance| distance == first_distance) {
                    return None;
                }
                first
            };
            Some((message.id.clone(), question.id.clone()))
        })
        .collect::<Vec<_>>();
    for (message_id, question_id) in links {
        if let Some(message) = inner.messages.get_mut(&message_id) {
            message.question_id = Some(question_id);
        }
    }
}

fn restore_legacy_decision_wait_messages(inner: &mut Inner) {
    let updates = inner
        .assignments
        .values()
        .filter_map(|assignment| {
            let wait = assignment.wait.as_ref()?;
            if wait.reason != "decision"
                || wait.message_id.is_some()
                || !matches!(assignment.status.as_str(), "waiting_user" | "waiting_bot")
            {
                return None;
            }
            let mut messages = inner
                .messages
                .values()
                .filter(|message| {
                    message.assignment_id.as_deref() == Some(assignment.id.as_str())
                        && message.intent.as_deref() == Some("decision")
                })
                .collect::<Vec<_>>();
            messages.sort_by_key(|message| timestamp_nanos(&message.created_at).unwrap_or(0));
            let latest = messages.pop()?;
            let latest_at = timestamp_nanos(&latest.created_at)?;
            if messages
                .last()
                .and_then(|message| timestamp_nanos(&message.created_at))
                .is_some_and(|created_at| created_at == latest_at)
            {
                return None;
            }
            Some((assignment.id.clone(), latest.id.clone()))
        })
        .collect::<Vec<_>>();
    for (assignment_id, message_id) in updates {
        if let Some(assignment) = inner.assignments.get_mut(&assignment_id) {
            if let Some(wait) = assignment.wait.as_mut() {
                if wait.reason == "decision" && wait.message_id.is_none() {
                    wait.message_id = Some(message_id);
                }
            }
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

#[derive(Clone, Debug, Serialize)]
pub enum MentionInput {
    Bot {
        bot_id: Id,
        #[serde(default)]
        instruction: Option<String>,
    },
    Main(String),
    User(String),
}

impl<'de> Deserialize<'de> for MentionInput {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(id) if id == "main" || id == "bot_main" => Ok(Self::Main(id)),
            Value::String(id) if id == "user" => Ok(Self::User(id)),
            Value::String(id) if !id.trim().is_empty() => Ok(Self::Bot {
                bot_id: id,
                instruction: None,
            }),
            Value::String(_) => Err(DeError::custom("mention id must not be empty")),
            Value::Object(mut object) => {
                let bot_id = object
                    .remove("bot")
                    .or_else(|| object.remove("bot_id"))
                    .ok_or_else(|| DeError::custom("Bot mention requires bot or bot_id"))?;
                let Value::String(bot_id) = bot_id else {
                    return Err(DeError::custom("Bot mention id must be a string"));
                };
                if bot_id.trim().is_empty() {
                    return Err(DeError::custom("Bot mention id must not be empty"));
                }
                let instruction = match object.remove("instruction") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(instruction)) => Some(instruction),
                    Some(_) => return Err(DeError::custom("mention instruction must be a string")),
                };
                Ok(Self::Bot {
                    bot_id,
                    instruction,
                })
            }
            _ => Err(DeError::custom("mention must be a string or Bot object")),
        }
    }
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
    pub assignment_id: Option<Id>,
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
            "bot.duplicate" => {
                let target_bot_id = match p.get("target_bot_id") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(id)) if !id.trim().is_empty() => Some(id.clone()),
                    Some(Value::String(_)) => {
                        return Err(OrchestratorError::Invalid(
                            "target_bot_id must not be empty".into(),
                        ));
                    }
                    Some(_) => {
                        return Err(OrchestratorError::Invalid(
                            "target_bot_id must be a string or null".into(),
                        ));
                    }
                };
                Self::json(self.duplicate_bot(
                    str_param(&p, "bot_id")?,
                    str_param(&p, "name")?,
                    target_bot_id,
                )?)
            }
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
                let status = p.get("status").and_then(Value::as_array);
                let projects = self
                    .projects
                    .values()
                    .filter(|project| {
                        status.is_none_or(|values| {
                            values
                                .iter()
                                .any(|value| value.as_str() == Some(&project.status))
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(json!({ "projects": projects }))
            }
            "project.get" => {
                let id = str_param(&p, "project_id")?;
                let project = self.project(&id)?.clone();
                Ok(json!({ "project": project, "announcement": self.announcement(&id)? }))
            }
            "project.create" => Self::json(self.create_project(&p)?),
            "project.update" => Ok(json!({ "project": self.update_project(
                str_param(&p, "project_id")?,
                p.get("patch").cloned().unwrap_or(Value::Null),
            )? })),
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
            "project.confirm_done" => Ok(json!({
                "project": self.confirm_project_done(str_param(&p, "project_id")?)?
            })),
            "project.request_changes" => Ok(json!({
                "message": self.request_project_changes(
                    str_param(&p, "project_id")?,
                    str_param(&p, "text")?,
                )?
            })),
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
            "assignment.create" | "assign" => {
                let mut request: AssignmentRequest = serde_json::from_value(p)
                    .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
                request.origin_chat_id = match request.project_id.as_deref() {
                    Some(project_id) => self.project(project_id)?.chat_id.clone(),
                    None => "chat_main".into(),
                };
                Self::json(self.create_assignment(request)?)
            }
            "delegate" => {
                let bot_id = str_param(&p, "bot_id")?;
                let instruction = str_param(&p, "instruction")?;
                let project_id = p
                    .get("project_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let origin_chat_id = match project_id.as_deref() {
                    Some(project_id) => self.project(project_id)?.chat_id.clone(),
                    None => "chat_main".into(),
                };
                Self::json(
                    self.create_assignment(AssignmentRequest {
                        project_id,
                        origin_chat_id,
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
                let project_id = p.get("project_id").and_then(Value::as_str);
                let bot_id = p.get("bot_id").and_then(Value::as_str);
                let statuses = p.get("status").and_then(Value::as_array);
                let mut items = self
                    .assignments
                    .values()
                    .filter(|assignment| {
                        project_id.is_none_or(|id| assignment.project_id.as_deref() == Some(id))
                            && bot_id.is_none_or(|id| assignment.bot_id == id)
                            && statuses.is_none_or(|values| {
                                values
                                    .iter()
                                    .any(|value| value.as_str() == Some(assignment.status.as_str()))
                            })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                let start = p
                    .get("cursor")
                    .and_then(Value::as_str)
                    .and_then(|cursor| items.iter().position(|item| item.id == cursor))
                    .map_or(0, |index| index + 1);
                let limit = p
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(50)
                    .clamp(1, 50) as usize;
                let mut page = items
                    .into_iter()
                    .skip(start)
                    .take(limit + 1)
                    .collect::<Vec<_>>();
                let next_cursor = (page.len() > limit).then(|| page[limit - 1].id.clone());
                page.truncate(limit);
                Self::json(json!({ "items": page, "next_cursor": next_cursor }))
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
            "send_msg" => {
                let assignment_id = p.get("assignment_id").and_then(Value::as_str);
                let root = assignment_id.and_then(|id| {
                    self.assignments
                        .get(id)
                        .and_then(|assignment| assignment.root_message_id.clone())
                });
                let message = self.send_msg(
                    serde_json::from_value(p)
                        .map_err(|e| OrchestratorError::Invalid(e.to_string()))?,
                )?;
                let mut value = serde_json::to_value(message)
                    .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
                if let Some(root) = root {
                    if let Some(pause) = self.loop_pauses.remove(&root) {
                        let object = value
                            .as_object_mut()
                            .expect("serialized Message is an object");
                        object
                            .entry("blocks")
                            .or_insert_with(|| json!([]))
                            .as_array_mut()
                            .expect("Message blocks is an array")
                            .push(json!({
                                "type": "loop_paused",
                                "root_message_id": pause.root_message_id,
                                "hops": pause.hops,
                                "state": pause.state,
                            }));
                    }
                }
                Ok(value)
            }
            "approval.list" => {
                let states = p.get("state").and_then(Value::as_array);
                let approvals = self
                    .approvals
                    .values()
                    .filter(|approval| {
                        states.is_none_or(|values| {
                            values
                                .iter()
                                .any(|value| value.as_str() == Some(approval.state.as_str()))
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(json!({ "approvals": approvals }))
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
                self.loop_states.insert(root.clone(), action.clone());
                if action == "continue" {
                    let mut remaining = Vec::new();
                    let pending = std::mem::take(&mut self.pending_loop_dispatches);
                    for dispatch in pending {
                        if dispatch.root_message_id != root {
                            remaining.push(dispatch);
                            continue;
                        }
                        self.create_assignment(AssignmentRequest {
                            project_id: dispatch.project_id,
                            origin_chat_id: dispatch.origin_chat_id,
                            bot_id: dispatch.bot_id,
                            title: dispatch.title,
                            instruction: dispatch.instruction,
                            from: dispatch.from,
                            trigger_message_id: None,
                            parent_assignment_id: dispatch.parent_assignment_id,
                            priority: dispatch.priority,
                            root_message_id: Some(dispatch.root_message_id),
                            // A user continuation grants the next hop while
                            // preserving the root for deduplication/audit.
                            loop_hops: 0,
                        })?;
                    }
                    self.pending_loop_dispatches = remaining;
                } else {
                    self.pending_loop_dispatches
                        .retain(|dispatch| dispatch.root_message_id != root);
                    self.loop_pauses.remove(&root);
                }
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
                if !self.routines.contains_key(&id) {
                    return Err(OrchestratorError::NotFound(format!("routine {id}")));
                }
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
                .clamp(1, 8) as usize,
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

    fn duplicate_bot(
        &mut self,
        source_id: String,
        name: String,
        target_bot_id: Option<String>,
    ) -> Result<Value> {
        if self.bots.values().any(|bot| bot.name == name) {
            return Err(OrchestratorError::Conflict(format!(
                "bot name {name} already exists"
            )));
        }
        let source = self.bot(&source_id)?.clone();
        let id = target_bot_id.unwrap_or_else(new_id);
        if id == "main" || self.bots.contains_key(&id) {
            return Err(OrchestratorError::Conflict(format!(
                "bot id {id} already exists"
            )));
        }
        let timestamp = now();
        let bot = Bot {
            id: id.clone(),
            name,
            label: source.label,
            description: source.description,
            avatar: source.avatar,
            model: source.model,
            max_parallel: source.max_parallel,
            tools: source.tools,
            browser_mode: source.browser_mode,
            dm_chat_id: format!("dm_{id}"),
            pinned: source.pinned,
            hidden: source.hidden,
            notifications: source.notifications,
            is_main: false,
            created_at: timestamp.clone(),
            updated_at: timestamp.clone(),
        };
        let routines = self
            .routines
            .values()
            .filter(|routine| routine.bot_id == source_id)
            .cloned()
            .map(|mut routine| {
                routine.id = new_id();
                routine.bot_id = id.clone();
                routine.last_run = None;
                routine.created_at = timestamp.clone();
                routine.updated_at = timestamp.clone();
                routine.next_run_at = if routine.enabled {
                    Some(
                        next_routine_at(&routine.schedules, &routine.timezone, Utc::now())?
                            .to_rfc3339(),
                    )
                } else {
                    None
                };
                Ok::<Routine, OrchestratorError>(routine)
            })
            .collect::<Result<Vec<_>>>()?;
        self.bots.insert(id.clone(), bot.clone());
        for routine in routines {
            self.routines.insert(routine.id.clone(), routine);
        }
        Ok(json!({"bot":bot,"dm_chat":dm_chat_for_bot(&bot)}))
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
        if let Some(value) = patch.get("model") {
            bot.model = match value {
                Value::Null => None,
                Value::String(model) => Some(model.clone()),
                _ => {
                    return Err(OrchestratorError::Invalid(
                        "model must be a string or null".into(),
                    ));
                }
            };
        }
        if let Some(value) = patch.get("tools") {
            bot.tools = serde_json::from_value(value.clone())
                .map_err(|error| OrchestratorError::Invalid(error.to_string()))?;
        }
        if let Some(x) = patch.get("browser_mode").and_then(Value::as_str) {
            bot.browser_mode = x.into();
        }
        if let Some(x) = patch.get("max_parallel").and_then(Value::as_u64) {
            bot.max_parallel = x.clamp(1, 8) as usize;
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
                    "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
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
        pm.push(ProjectMember {
            bot_id: "main".into(),
            role_note: String::new(),
            joined_at: now(),
        });
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
        ensure_main_project_member(p);
        if p.members.iter().any(|m| m.bot_id == bot_id) {
            return Ok(());
        }
        if p.members
            .iter()
            .filter(|member| member.bot_id != "main")
            .count()
            >= 6
        {
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
        if bot_id == "main" {
            return Err(OrchestratorError::Forbidden(
                "main Bot cannot be removed from a project".into(),
            ));
        }
        let ids = self
            .assignments
            .values()
            .filter(|a| {
                a.project_id.as_deref() == Some(project_id)
                    && a.bot_id == bot_id
                    && matches!(
                        a.status.as_str(),
                        "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
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

    fn confirm_project_done(&mut self, id: String) -> Result<Project> {
        let current = self.project(&id)?.clone();
        if !matches!(current.status.as_str(), "active" | "review") {
            return Err(OrchestratorError::Conflict(
                "project must be active or in review".into(),
            ));
        }
        let ids = self
            .assignments
            .values()
            .filter(|assignment| {
                assignment.project_id.as_deref() == Some(id.as_str())
                    && matches!(
                        assignment.status.as_str(),
                        "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
                    )
            })
            .map(|assignment| assignment.id.clone())
            .collect::<Vec<_>>();
        for assignment_id in ids {
            self.finish_assignment(&assignment_id, "cancelled")?;
        }
        let timestamp = now();
        let project = self.project_mut(&id)?;
        project.status = "done".into();
        project.done_at = Some(timestamp);
        project.updated_at = now();
        Ok(project.clone())
    }

    fn mark_project_review(&mut self, id: &str) -> Result<Project> {
        let current = self.project(id)?.clone();
        if matches!(current.status.as_str(), "done" | "archived") {
            return Err(OrchestratorError::Conflict(
                "cannot move a completed or archived project to review".into(),
            ));
        }
        if current.status == "review" {
            return Ok(current);
        }
        let project = self.project_mut(id)?;
        project.status = "review".into();
        project.updated_at = now();
        Ok(project.clone())
    }

    fn update_project(&mut self, id: String, patch: Value) -> Result<Project> {
        let mut project = self
            .projects
            .get(&id)
            .cloned()
            .ok_or_else(|| OrchestratorError::NotFound(format!("project {id}")))?;
        if let Some(name) = patch.get("name").and_then(Value::as_str) {
            project.name = name.into();
            project.slug = slugify(name);
            project.home_path = format!("~/MacBot/projects/{}/", project.slug);
        }
        if let Some(goal) = patch.get("goal").and_then(Value::as_str) {
            project.goal = goal.into();
        }
        if let Some(flow) = patch.get("flow") {
            project.flow = serde_json::from_value(flow.clone())
                .map_err(|e| OrchestratorError::Invalid(e.to_string()))?;
        }
        if let Some(deadline) = patch.get("deadline") {
            project.deadline = deadline.as_str().map(str::to_owned);
        }
        project.updated_at = now();
        self.projects.insert(id, project.clone());
        Ok(project)
    }

    fn request_project_changes(&mut self, id: String, text: String) -> Result<Message> {
        let chat_id = self.project(&id)?.chat_id.clone();
        let timestamp = now();
        let message = Message {
            id: new_id(),
            chat_id,
            sender: "user".into(),
            created_at: timestamp.clone(),
            text: text.clone(),
            intent: None,
            assignment_id: None,
            mentions: vec![Mention::Main],
            artifacts: Vec::new(),
            options: Vec::new(),
            question_id: None,
            delivery: Vec::new(),
            fallback_text: text.clone(),
        };
        self.messages.insert(message.id.clone(), message.clone());
        let project = self.project_mut(&id)?;
        project.status = "active".into();
        project.updated_at = timestamp.clone();
        self.highlights
            .entry(id.clone())
            .or_default()
            .push(Highlight {
                text: text.clone(),
                at: timestamp,
            });
        self.wake_main_for_message(Some(&id), &message, None, &message.id, 0, "处理项目修改")?;
        Ok(message)
    }

    fn wake_main_for_message(
        &mut self,
        project_id: Option<&str>,
        message: &Message,
        parent_assignment_id: Option<Id>,
        root_message_id: &str,
        parent_hops: usize,
        title: &str,
    ) -> Result<()> {
        let Some(project_id) = project_id else {
            return Ok(());
        };
        let already_active = self.assignments.values().any(|assignment| {
            assignment.bot_id == "main"
                && assignment.project_id.as_deref() == Some(project_id)
                && matches!(
                    assignment.status.as_str(),
                    "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
                )
        });
        if already_active {
            return Ok(());
        }
        let mut instruction = format!("{title}：{}", message.text);
        if !message.artifacts.is_empty() {
            let artifacts = message
                .artifacts
                .iter()
                .map(|artifact| format!("{} ({})", artifact.title, artifact.path_or_url))
                .collect::<Vec<_>>()
                .join(", ");
            instruction.push_str(&format!("\n产物：{artifacts}"));
        }
        if parent_hops >= self.settings.loop_hops {
            self.loop_states
                .insert(root_message_id.to_owned(), "paused".into());
            self.loop_pauses
                .entry(root_message_id.to_owned())
                .or_insert(LoopPause {
                    root_message_id: root_message_id.into(),
                    hops: parent_hops,
                    state: "paused".into(),
                });
            self.pending_loop_dispatches.push(PendingLoopDispatch {
                root_message_id: root_message_id.into(),
                project_id: Some(project_id.into()),
                origin_chat_id: message.chat_id.clone(),
                bot_id: "main".into(),
                title: title.into(),
                instruction,
                from: message.sender.clone(),
                parent_assignment_id,
                priority: 2,
            });
            return Ok(());
        }
        self.create_assignment(AssignmentRequest {
            project_id: Some(project_id.into()),
            origin_chat_id: message.chat_id.clone(),
            bot_id: "main".into(),
            title: title.into(),
            instruction,
            from: if message.sender == "system" {
                "main".into()
            } else {
                message.sender.clone()
            },
            trigger_message_id: Some(message.id.clone()),
            parent_assignment_id,
            priority: 2,
            root_message_id: Some(root_message_id.into()),
            loop_hops: parent_hops + 1,
        })?;
        Ok(())
    }

    fn poll_project_attention(&mut self, at: DateTime<Utc>) -> Result<Vec<AttentionNotice>> {
        let candidates = self
            .assignments
            .values()
            .filter(|assignment| {
                assignment.bot_id != "main"
                    && assignment.project_id.is_some()
                    && matches!(
                        assignment.status.as_str(),
                        "blocked" | "failed" | "done" | "working"
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut notices = Vec::new();
        for assignment in candidates {
            let (code, reason) = match assignment.status.as_str() {
                "blocked" => (Some("info"), "blocked".to_string()),
                "failed" => (Some("task_no_report"), "failed".to_string()),
                "done" if assignment.result_message_id.is_none() => {
                    (Some("task_no_report"), "done".to_string())
                }
                "working" => {
                    let Some(started_at) = assignment.started_at.as_deref().and_then(|value| {
                        DateTime::parse_from_rfc3339(value)
                            .ok()
                            .map(|parsed| parsed.with_timezone(&Utc))
                    }) else {
                        continue;
                    };
                    let latest_progress = self
                        .messages
                        .values()
                        .filter(|message| {
                            message.assignment_id.as_deref() == Some(assignment.id.as_str())
                                && matches!(
                                    message.intent.as_deref(),
                                    Some("progress") | Some("ack")
                                )
                        })
                        .filter_map(|message| {
                            DateTime::parse_from_rfc3339(&message.created_at)
                                .ok()
                                .map(|parsed| parsed.with_timezone(&Utc))
                        })
                        .filter(|created| *created >= started_at && *created <= at)
                        .max()
                        .unwrap_or(started_at);
                    if at.signed_duration_since(latest_progress) < Duration::hours(2) {
                        continue;
                    }
                    (
                        Some("task_no_report"),
                        format!("stale:{}", latest_progress.to_rfc3339()),
                    )
                }
                _ => (None, String::new()),
            };
            let Some(code) = code else {
                continue;
            };
            let marker = format!("{}:{code}:{reason}", assignment.id);
            if !self.attention_notices.insert(marker) {
                continue;
            }
            let project_id = assignment.project_id.clone().ok_or_else(|| {
                OrchestratorError::Invalid("attention assignment has no project".into())
            })?;
            let text = match code {
                "info" => format!("Bot {} 的任务已阻塞，主 Bot 请跟进", assignment.bot_id),
                _ if assignment.status == "failed" => {
                    format!("Bot {} 的任务失败，主 Bot 请跟进", assignment.bot_id)
                }
                _ if assignment.status == "done" => {
                    format!("Bot {} 的任务结束但没有提交完成报告", assignment.bot_id)
                }
                _ => format!("Bot {} 的任务超过 2 小时没有进展", assignment.bot_id),
            };
            let created_at = at.to_rfc3339();
            let message_id = format!("task_attention:{code}:{}", assignment.id);
            let message = self
                .messages
                .entry(message_id.clone())
                .or_insert_with(|| Message {
                    id: message_id.clone(),
                    chat_id: assignment.origin_chat_id.clone(),
                    sender: "system".into(),
                    created_at: created_at.clone(),
                    text: text.clone(),
                    intent: None,
                    assignment_id: Some(assignment.id.clone()),
                    mentions: Vec::new(),
                    artifacts: Vec::new(),
                    options: Vec::new(),
                    question_id: None,
                    delivery: Vec::new(),
                    fallback_text: text.clone(),
                })
                .clone();
            self.wake_main_for_message(
                Some(&project_id),
                &message,
                Some(assignment.id.clone()),
                assignment
                    .root_message_id
                    .as_deref()
                    .unwrap_or(message.id.as_str()),
                assignment.loop_hops,
                "主 Bot 跟进任务",
            )?;
            notices.push(AttentionNotice {
                message,
                code: code.into(),
            });
        }
        Ok(notices)
    }

    fn clear_attention_markers(&mut self, assignment_id: &str) {
        let prefix = format!("{assignment_id}:");
        self.attention_notices
            .retain(|marker| !marker.starts_with(&prefix));
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
                            "queued"
                                | "working"
                                | "waiting_user"
                                | "waiting_bot"
                                | "blocked"
                                | "done"
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
            .filter(|a| a.status == "working" && a.bot_id != "main")
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
        let (status, reason) = if request.bot_id != "main" && active_bot >= bot_limit {
            ("queued", Some("bot_parallel_limit"))
        } else if request.bot_id != "main" && active_global >= self.settings.global_limit {
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
                .filter(|a| a.status == "working" && a.bot_id != "main")
                .count();
            let next = self
                .assignments
                .values()
                .filter(|a| a.status == "queued")
                .filter(|a| a.bot_id == "main" || global < self.settings.global_limit)
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
                    (a.bot_id == "main"
                        || n < self
                            .bots
                            .get(&a.bot_id)
                            .map(|b| b.max_parallel)
                            .unwrap_or(self.settings.bot_default_limit))
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
        if req.intent == "decision" {
            let mut has_recipient = false;
            for mention in &req.mentions {
                match mention {
                    MentionInput::Bot { bot_id, .. } => {
                        if same_bot_id(&req.bot_id, bot_id) {
                            return Err(OrchestratorError::Invalid(
                                "decision cannot mention the sending Bot itself".into(),
                            ));
                        }
                        if bot_id != "main" && !self.bots.contains_key(bot_id) {
                            return Err(OrchestratorError::NotFound(format!(
                                "mentioned bot {bot_id}"
                            )));
                        }
                        has_recipient = true;
                    }
                    MentionInput::Main(_) | MentionInput::User(_) => {
                        has_recipient = true;
                    }
                }
            }
            // A decision with options is an explicit user question.  Without
            // options it still needs a concrete recipient; otherwise the
            // assignment would enter waiting_bot with no possible wake-up.
            if !has_recipient && req.options.is_empty() {
                return Err(OrchestratorError::Invalid(
                    "decision requires a Bot/user recipient or options".into(),
                ));
            }
            if req.assignment_id.is_none() && !req.options.is_empty() {
                let bot = self.bot(&req.bot_id)?;
                if bot.dm_chat_id != req.chat_id {
                    return Err(OrchestratorError::Forbidden(
                        "a decision without an assignment must target the Bot direct chat".into(),
                    ));
                }
            }
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
            question_id: None,
            delivery: Vec::new(),
            fallback_text: req.text.clone(),
        };
        if let Some(aid) = &req.assignment_id {
            let a = self.assignment_mut(aid)?;
            a.result_message_id =
                matches!(req.intent.as_str(), "done" | "blocked").then_some(msg_id.clone());
            match req.intent.as_str() {
                "decision" => {
                    let has_user_recipient = req
                        .mentions
                        .iter()
                        .any(|m| matches!(m, MentionInput::User(_)));
                    let has_bot_recipient = req.mentions.iter().any(|mention| {
                        matches!(mention, MentionInput::Bot { .. } | MentionInput::Main(_))
                    });
                    a.status = if has_user_recipient || !has_bot_recipient {
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
        let mut question_id = None;
        if req.intent == "decision" && !req.options.is_empty() {
            let question = self.create_question(QuestionRequest {
                bot_id: req.bot_id.clone(),
                assignment_id: req.assignment_id.clone(),
                chat_id: req.chat_id.clone(),
                text: req.text.clone(),
                options: req.options.clone(),
                allow_free_text: true,
            })?;
            question_id = Some(question.id);
        }
        msg.question_id = question_id;
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
        if req.intent == "blocked" {
            // A blocked worker must wake coordination immediately, even when
            // the model omitted an explicit @Main mention.  The periodic
            // attention poll may later persist the durable system notice.
            self.wake_main_for_message(
                project_id.as_deref(),
                &msg,
                req.assignment_id.clone(),
                &root,
                parent_hops,
                "跟进阻塞任务",
            )?;
        }
        let mut seen = HashSet::new();
        for mention in msg.mentions.clone() {
            match mention {
                Mention::Main => {
                    if seen.insert("main".into()) {
                        self.wake_main_for_message(
                            project_id.as_deref(),
                            &msg,
                            req.assignment_id.clone(),
                            &root,
                            parent_hops,
                            "主 Bot 汇总",
                        )?;
                    }
                }
                Mention::Bot {
                    bot_id,
                    instruction,
                } => {
                    if !seen.insert(bot_id.clone()) {
                        continue;
                    }
                    if bot_id == "main" {
                        self.wake_main_for_message(
                            project_id.as_deref(),
                            &msg,
                            req.assignment_id.clone(),
                            &root,
                            parent_hops,
                            "主 Bot 汇总",
                        )?;
                        continue;
                    }
                    if parent_hops >= self.settings.loop_hops {
                        self.loop_states.insert(root.clone(), "paused".into());
                        self.loop_pauses.entry(root.clone()).or_insert(LoopPause {
                            root_message_id: root.clone(),
                            hops: parent_hops,
                            state: "paused".into(),
                        });
                        self.pending_loop_dispatches.push(PendingLoopDispatch {
                            root_message_id: root.clone(),
                            project_id: project_id.clone(),
                            origin_chat_id: req.chat_id.clone(),
                            bot_id,
                            title: format!("交接：{}", req.intent),
                            instruction: instruction.unwrap_or(req.text.clone()),
                            from: req.bot_id.clone(),
                            parent_assignment_id: req.assignment_id.clone(),
                            priority: 2,
                        });
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
                _ => {}
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
        if status == "cancelled" {
            self.create_task_stopped_message(id)?;
        }
        self.pump_queue();
        Ok(out)
    }

    fn create_task_stopped_message(&mut self, assignment_id: &str) -> Result<Message> {
        let message_id = format!("task_stopped:{assignment_id}");
        if let Some(message) = self.messages.get(&message_id) {
            return Ok(message.clone());
        }
        let chat_id = self.assignment(assignment_id)?.origin_chat_id.clone();
        let text = "任务已停止".to_string();
        let message = Message {
            id: message_id.clone(),
            chat_id,
            sender: "system".into(),
            created_at: now(),
            text: text.clone(),
            intent: Some("task_stopped".into()),
            assignment_id: Some(assignment_id.into()),
            mentions: Vec::new(),
            artifacts: Vec::new(),
            options: Vec::new(),
            question_id: None,
            delivery: Vec::new(),
            fallback_text: text,
        };
        self.messages.insert(message_id, message.clone());
        Ok(message)
    }

    fn queue_steer(&mut self, req: SteerRequest) -> Result<SteerDelivery> {
        let message_id = req.message_id.unwrap_or_else(new_id);
        let existing = self
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
        let ts = now();
        let (assignment_id, state, steer) = if let Some(id) = existing {
            let steer = Steer {
                message_id: message_id.clone(),
                text: req.text.clone(),
                at: ts.clone(),
                applied_at: None,
            };
            let resumed_from_blocked = self
                .assignments
                .get(&id)
                .is_some_and(|assignment| assignment.status == "blocked");
            let a = self.assignment_mut(&id)?;
            if matches!(
                a.status.as_str(),
                "waiting_user" | "waiting_bot" | "blocked"
            ) {
                a.status = "working".into();
                a.wait = None;
            }
            a.steers.push(steer.clone());
            if resumed_from_blocked {
                self.clear_attention_markers(&id);
            }
            (Some(id), "queued", steer)
        } else {
            let assignment = self.create_assignment(AssignmentRequest {
                project_id: req.project_id.clone(),
                origin_chat_id: req.chat_id.clone(),
                bot_id: req.bot_id.clone(),
                title: "插话".into(),
                instruction: req.text.clone(),
                from: "user".into(),
                trigger_message_id: Some(message_id.clone()),
                parent_assignment_id: None,
                priority: 2,
                root_message_id: Some(message_id.clone()),
                loop_hops: 0,
            })?;
            let steer = Steer {
                message_id: message_id.clone(),
                text: req.text.clone(),
                at: ts.clone(),
                applied_at: Some(ts.clone()),
            };
            self.assignment_mut(&assignment.id)?
                .steers
                .push(steer.clone());
            (Some(assignment.id), "delivered", steer)
        };
        let _ = steer;
        if state == "delivered" {
            self.pump_queue();
        }
        Ok(SteerDelivery {
            message_id,
            bot_id: req.bot_id,
            assignment_id,
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
        if let Some(assignment_id) = &r.assignment_id {
            if !self.assignments.contains_key(assignment_id) {
                return Err(OrchestratorError::NotFound(format!(
                    "assignment {assignment_id}"
                )));
            }
        }
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
        if state == "always_allowed" {
            self.approval_rules.push(ApprovalRuleRecord {
                id: new_id(),
                kind: "auto_allow".into(),
                text: out.summary.clone(),
                created_at: now(),
            });
        }
        if let Some(x) = &out.assignment_id {
            if let Some(asn) = self.assignments.get_mut(x) {
                asn.wait = None;
                if state == "denied" {
                    asn.status = "failed".into();
                    asn.finished_at = Some(now());
                } else {
                    asn.status = "working".into();
                }
            }
        }
        Ok(out)
    }
    fn create_question(&mut self, r: QuestionRequest) -> Result<Question> {
        let (assignment_id, scope_only) = match r.assignment_id {
            Some(assignment_id) => {
                if !self.assignments.contains_key(&assignment_id) {
                    return Err(OrchestratorError::NotFound(format!(
                        "assignment {}",
                        assignment_id
                    )));
                }
                (assignment_id, false)
            }
            None => {
                let bot = self
                    .bots
                    .get(&r.bot_id)
                    .ok_or_else(|| OrchestratorError::NotFound(format!("bot {}", r.bot_id)))?;
                if bot.dm_chat_id != r.chat_id {
                    return Err(OrchestratorError::Forbidden(
                        "a question without an assignment must target the bot direct chat".into(),
                    ));
                }
                (private_question_scope(&r.chat_id), true)
            }
        };
        let id = new_id();
        let q = Question {
            id: id.clone(),
            bot_id: r.bot_id,
            assignment_id: assignment_id.clone(),
            chat_id: r.chat_id,
            text: r.text,
            options: r.options,
            allow_free_text: r.allow_free_text,
            state: "pending".into(),
            answer: None,
        };
        if scope_only {
            self.question_scopes.insert(
                id.clone(),
                QuestionScope {
                    bot_id: q.bot_id.clone(),
                    chat_id: q.chat_id.clone(),
                },
            );
        } else {
            let a = self.assignment_mut(&assignment_id)?;
            let preserve_decision_wait =
                matches!(a.status.as_str(), "waiting_user" | "waiting_bot")
                    && a.wait
                        .as_ref()
                        .is_some_and(|wait| wait.reason == "decision");
            if !preserve_decision_wait {
                a.status = "waiting_user".into();
                a.wait = Some(WaitState {
                    reason: "decision".into(),
                    message_id: None,
                });
            }
        }
        self.question_created_at.insert(id.clone(), now());
        self.questions.insert(id, q.clone());
        Ok(q)
    }

    fn reconcile_waiting_decision(
        &mut self,
        message_id: &str,
        assignment_id: Option<&str>,
        bot_id: &str,
        chat_id: &str,
    ) -> Result<bool> {
        let message = self
            .messages
            .get(message_id)
            .ok_or_else(|| OrchestratorError::NotFound(format!("message {message_id}")))?
            .clone();
        if message.chat_id != chat_id
            || !same_bot_id(&message.sender, bot_id)
            || message.intent.as_deref() != Some("decision")
            || message.assignment_id.as_deref() != assignment_id
        {
            return Err(OrchestratorError::Conflict(
                "decision message routing does not match canonical identity".into(),
            ));
        }

        let mut assignment = None;
        if let Some(assignment_id) = assignment_id {
            let current = self.assignments.get(assignment_id).ok_or_else(|| {
                OrchestratorError::NotFound(format!("assignment {assignment_id}"))
            })?;
            if !matches!(
                current.status.as_str(),
                "working" | "waiting_user" | "waiting_bot"
            ) {
                return Err(OrchestratorError::Conflict(
                    "cannot reconcile a queued or terminal assignment".into(),
                ));
            }
            if current.origin_chat_id != chat_id {
                return Err(OrchestratorError::Conflict(
                    "decision assignment chat does not match canonical message".into(),
                ));
            }
            if !same_bot_id(&current.bot_id, bot_id) {
                return Err(OrchestratorError::Conflict(
                    "decision assignment Bot does not match canonical message".into(),
                ));
            }
            if current
                .wait
                .as_ref()
                .and_then(|wait| wait.message_id.as_deref())
                .is_some_and(|id| id != message_id)
            {
                return Err(OrchestratorError::Conflict(
                    "assignment already waits on another message".into(),
                ));
            }
            assignment = Some(current.clone());
        } else {
            let bot = self
                .bots
                .get(bot_id)
                .or_else(|| {
                    (bot_id == "bot_main")
                        .then(|| self.bots.get("main"))
                        .flatten()
                })
                .ok_or_else(|| OrchestratorError::NotFound(format!("bot {bot_id}")))?;
            if bot.dm_chat_id != chat_id {
                return Err(OrchestratorError::Forbidden(
                    "a private decision must target the Bot direct chat".into(),
                ));
            }
        }

        let options = message.options.clone();
        let question_scope = assignment_id
            .map(str::to_owned)
            .unwrap_or_else(|| private_question_scope(chat_id));
        let (existing_question_id, ambiguous_question) = if options.is_empty() {
            (None, false)
        } else if let Some(question_id) = message
            .question_id
            .as_deref()
            .filter(|id| self.questions.contains_key(*id))
        {
            (Some(question_id.to_owned()), false)
        } else {
            let stable = format!("decision:{message_id}");
            if self.questions.contains_key(&stable) {
                (Some(stable), false)
            } else {
                let matches = self
                    .questions
                    .values()
                    .filter(|question| {
                        question.assignment_id == question_scope
                            && same_bot_id(&question.bot_id, bot_id)
                            && question.chat_id == chat_id
                            && question.text == message.text
                            && question.options == options
                    })
                    .map(|question| question.id.clone())
                    .take(2)
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [] => (None, false),
                    [question_id] => (Some(question_id.clone()), false),
                    _ => (None, true),
                }
            }
        };
        if ambiguous_question {
            return Ok(false);
        }
        let answered_question = if let Some(question_id) = existing_question_id.as_deref() {
            let question = self.questions.get(question_id).ok_or_else(|| {
                OrchestratorError::Conflict(
                    "decision Question disappeared during reconciliation".into(),
                )
            })?;
            if !same_bot_id(&question.bot_id, bot_id)
                || question.assignment_id != question_scope
                || question.chat_id != chat_id
                || question.text != message.text
                || question.options != options
            {
                return Err(OrchestratorError::Conflict(
                    "decision Question does not match canonical message".into(),
                ));
            }
            if !matches!(question.state.as_str(), "pending" | "answered") {
                return Ok(false);
            }
            question.state == "answered"
        } else {
            false
        };
        let mut changed = false;
        if let Some(assignment) = assignment.as_ref() {
            let wait = WaitState {
                reason: "decision".into(),
                message_id: Some(message_id.into()),
            };
            let target_status = if answered_question {
                "working".into()
            } else if options.is_empty() {
                if matches!(assignment.status.as_str(), "waiting_user" | "waiting_bot") {
                    assignment.status.clone()
                } else {
                    "waiting_bot".into()
                }
            } else if matches!(assignment.status.as_str(), "waiting_user" | "waiting_bot") {
                assignment.status.clone()
            } else if message
                .mentions
                .iter()
                .any(|mention| matches!(mention, Mention::Bot { .. } | Mention::Main))
            {
                "waiting_bot".into()
            } else {
                "waiting_user".into()
            };
            let current = self.assignments.get_mut(&assignment.id).expect("validated");
            if current.status != target_status || current.wait.as_ref() != Some(&wait) {
                current.status = target_status;
                current.wait = Some(wait);
                changed = true;
            }
        }

        if options.is_empty() {
            return Ok(changed);
        }

        let question_id = existing_question_id.unwrap_or_else(|| format!("decision:{message_id}"));
        if !self.questions.contains_key(&question_id) {
            self.questions.insert(
                question_id.clone(),
                Question {
                    id: question_id.clone(),
                    bot_id: bot_id.into(),
                    assignment_id: question_scope.clone(),
                    chat_id: chat_id.into(),
                    text: message.text.clone(),
                    options,
                    allow_free_text: true,
                    state: "pending".into(),
                    answer: None,
                },
            );
            self.question_created_at
                .insert(question_id.clone(), message.created_at.clone());
            if assignment_id.is_none() {
                self.question_scopes.insert(
                    question_id.clone(),
                    QuestionScope {
                        bot_id: bot_id.into(),
                        chat_id: chat_id.into(),
                    },
                );
            }
            changed = true;
        }
        if message.question_id.as_deref() != Some(question_id.as_str()) {
            if let Some(message) = self.messages.get_mut(message_id) {
                message.question_id = Some(question_id);
            }
            changed = true;
        }
        Ok(changed)
    }

    fn answer_decision_for_child(
        &mut self,
        parent_assignment_id: &str,
        text: String,
    ) -> Result<Option<Question>> {
        let Some(parent) = self.assignments.get(parent_assignment_id) else {
            return Err(OrchestratorError::NotFound(format!(
                "assignment {parent_assignment_id}"
            )));
        };
        if parent.status != "waiting_bot"
            || parent
                .wait
                .as_ref()
                .is_none_or(|wait| wait.reason != "decision")
        {
            return Ok(None);
        }
        let question_id = parent
            .wait
            .as_ref()
            .and_then(|wait| wait.message_id.as_deref())
            .and_then(|message_id| self.messages.get(message_id))
            .and_then(|message| message.question_id.as_deref())
            .map(str::to_owned);
        let mut answered = None;
        if let Some(question_id) = question_id {
            if let Some(question) = self.questions.get_mut(&question_id) {
                if question.assignment_id == parent_assignment_id {
                    if question.state == "pending" {
                        question.state = "answered".into();
                        question.answer = Some(QuestionAnswer {
                            option_index: None,
                            text: Some(text),
                            at: now(),
                        });
                    }
                    answered = Some(question.clone());
                }
            }
        }
        let parent = self.assignment_mut(parent_assignment_id)?;
        parent.status = "working".into();
        parent.wait = None;
        Ok(answered)
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
        if text.is_some() && !q.allow_free_text {
            return Err(OrchestratorError::Invalid(
                "free-text answers are not allowed".into(),
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
        if !self.question_scopes.contains_key(&out.id) {
            if let Some(a) = self.assignments.get_mut(&out.assignment_id) {
                a.status = "working".into();
                a.wait = None;
            }
        }
        Ok(out)
    }

    fn workbench(&self) -> Value {
        let running = self
            .assignments
            .values()
            .filter(|a| a.status == "working" && a.bot_id != "main")
            .count();
        let mut waiting = Vec::new();
        for project in self
            .projects
            .values()
            .filter(|project| project.status == "review")
        {
            waiting.push(json!({
                "kind": "review",
                "project_id": project.id,
                "since": project.updated_at,
            }));
        }
        for approval in self
            .approvals
            .values()
            .filter(|approval| approval.state == "pending")
        {
            waiting.push(json!({"kind": "approval", "approval": approval}));
        }
        for question in self
            .questions
            .values()
            .filter(|question| question.state == "pending")
        {
            waiting.push(json!({"kind": "question", "question": question}));
        }
        for assignment in self.assignments.values().filter(|assignment| {
            matches!(assignment.status.as_str(), "waiting_user" | "waiting_bot")
                && assignment
                    .wait
                    .as_ref()
                    .is_some_and(|wait| wait.reason == "takeover")
        }) {
            waiting.push(json!({
                "kind": "takeover",
                "bot_id": assignment.bot_id,
                "assignment_id": assignment.id,
                "reason": assignment.wait.as_ref().map(|wait| wait.reason.clone()).unwrap_or_default(),
            }));
        }
        let active_status = |status: &str| {
            matches!(
                status,
                "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
            )
        };
        let bots = self
            .bots
            .values()
            .map(|b| {
                let assignments = self
                    .assignments
                    .values()
                    .filter(|a| a.bot_id == b.id && active_status(&a.status))
                    .cloned()
                    .collect::<Vec<_>>();
                let active = assignments.iter().filter(|a| a.status == "working").count();
                json!({
                    "bot_id": b.id,
                    "active": active,
                    "max_parallel": b.max_parallel,
                    "assignments": assignments,
                })
            })
            .collect::<Vec<_>>();
        let today = Utc::now().date_naive().to_string();
        let done_today = self
            .assignments
            .values()
            .filter(|a| {
                a.status == "done"
                    && a.finished_at
                        .as_deref()
                        .is_some_and(|finished| finished.starts_with(&today))
            })
            .cloned()
            .collect::<Vec<_>>();
        json!({"running":running,"global_limit":self.settings.global_limit,"subagents_running":self.assignments.values().map(|a|a.subagents_active).sum::<usize>(),"waiting":waiting,"bots":bots,"done_today":done_today})
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
        if let Some(project_id) = p.get("project_id").and_then(Value::as_str) {
            self.project(project_id)?;
        }
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
            let project_id = x.as_str().map(str::to_owned);
            if let Some(project_id) = &project_id {
                self.project(project_id)?;
            }
            updated.project_id = project_id;
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
    fn private_question_scope_does_not_create_or_resume_assignment() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc("bot.create", json!({"name":"私聊问题 Bot"})))
            .unwrap();
        let bot_id = created["bot"]["id"].as_str().unwrap().to_owned();
        let chat_id = created["bot"]["dm_chat_id"].as_str().unwrap().to_owned();

        let result = rt
            .block_on(o.rpc(
                "question.ask",
                json!({
                    "bot_id": bot_id,
                    "assignment_id": null,
                    "chat_id": chat_id,
                    "text": "是否继续？",
                    "options": ["继续", "停止"],
                    "allow_free_text": false
                }),
            ))
            .unwrap();
        let question = &result;
        let question_id = question["id"].as_str().unwrap().to_owned();
        assert_eq!(question["assignment_id"], format!("dm_{chat_id}"));
        assert_eq!(
            o.snapshot().unwrap()["assignments"]
                .as_object()
                .unwrap()
                .len(),
            0
        );

        let answered = rt
            .block_on(o.rpc(
                "question.answer",
                json!({"question_id": question_id, "option_index": 0}),
            ))
            .unwrap();
        assert_eq!(answered["question"]["state"], "answered");
        assert_eq!(
            o.snapshot().unwrap()["assignments"]
                .as_object()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn private_question_scope_rejects_non_dm_chat() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc("bot.create", json!({"name":"私聊校验 Bot"})))
            .unwrap();
        let bot_id = created["bot"]["id"].as_str().unwrap();
        let error = rt
            .block_on(o.rpc(
                "question.ask",
                json!({
                    "bot_id": bot_id,
                    "assignment_id": null,
                    "chat_id": "project_wrong",
                    "text": "不应创建",
                    "options": [],
                    "allow_free_text": true
                }),
            ))
            .unwrap_err();
        assert!(matches!(error, OrchestratorError::Forbidden(_)));
        assert!(o.snapshot().unwrap()["questions"]
            .as_object()
            .unwrap()
            .is_empty());
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
                assignment_id: Some(assignment.id.clone()),
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
    fn workbench_includes_standalone_pending_approval() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "工作台审批");
        let approval = o
            .create_approval(ApprovalRequest {
                bot_id,
                assignment_id: None,
                chat_id: "chat_main".into(),
                tool: "browser.act".into(),
                risk: "exec".into(),
                summary: "需要确认".into(),
                detail: "独立审批".into(),
            })
            .unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(o.rpc("workbench.get", json!({}))).unwrap();
        let waiting = result["waiting"].as_array().unwrap();
        assert!(waiting.iter().any(|item| {
            item["kind"] == "approval"
                && item["approval"]["id"] == approval.id
                && item["approval"]["assignment_id"].is_null()
        }));
        assert!(waiting.iter().all(|item| {
            matches!(
                item["kind"].as_str(),
                Some("review" | "approval" | "question" | "takeover")
            )
        }));
        rt.block_on(o.rpc(
            "approval.decide",
            json!({"approval_id":approval.id,"decision":"allow_once"}),
        ))
        .unwrap();
        let pending = rt
            .block_on(o.rpc("approval.list", json!({"state":["pending"]})))
            .unwrap();
        assert!(pending["approvals"].as_array().unwrap().is_empty());
        let resolved = rt
            .block_on(o.rpc("approval.list", json!({"state":["allowed_once"]})))
            .unwrap();
        assert_eq!(resolved["approvals"].as_array().unwrap().len(), 1);
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
    fn rpc_assignment_routes_project_work_to_project_chat_and_standalone_to_main_dm() {
        let o = Orchestrator::default();
        let worker = bot(&o, "路由 worker");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"新项目群","goal":"路由","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let project_chat = project["chat"]["id"].as_str().unwrap();

        let assigned = rt
            .block_on(o.rpc(
                "assign",
                json!({
                    "project_id": project_id,
                    "origin_chat_id": "old-main-dm-or-group",
                    "bot_id": worker,
                    "title": "项目任务",
                    "instruction": "在新项目群执行",
                    "from": "main",
                    "trigger_message_id": null,
                    "parent_assignment_id": null,
                    "priority": 1,
                    "root_message_id": null,
                    "loop_hops": 0
                }),
            ))
            .unwrap();
        assert_eq!(assigned["origin_chat_id"], project_chat);

        let delegated = rt
            .block_on(o.rpc(
                "delegate",
                json!({
                    "origin_chat_id": "old-project-group",
                    "bot_id": worker,
                    "instruction": "无项目的小事"
                }),
            ))
            .unwrap();
        assert_eq!(delegated["origin_chat_id"], "chat_main");
    }

    #[test]
    fn worker_done_to_main_wakes_main_with_project_context_and_artifacts() {
        let o = Orchestrator::default();
        let worker = bot(&o, "汇报 worker");
        let project = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"自动汇总","goal":"验证主 Bot 唤醒","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let source = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: chat_id.clone(),
                bot_id: worker.clone(),
                title: "实现任务".into(),
                instruction: "执行并汇报".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let report = o
            .send_msg(SendMessageRequest {
                bot_id: worker,
                chat_id: chat_id.clone(),
                assignment_id: Some(source.id.clone()),
                run_id: None,
                call_id: None,
                text: "实现完成，请主 Bot 汇总".into(),
                intent: "done".into(),
                mentions: vec![MentionInput::Bot {
                    bot_id: "main".into(),
                    instruction: Some("汇总真实产物并请求验收".into()),
                }],
                artifacts: vec![ArtifactRef {
                    title: "实现报告".into(),
                    path_or_url: "code/report.md".into(),
                }],
                options: vec![],
            })
            .unwrap();
        let snapshot = o.snapshot().unwrap();
        assert_eq!(snapshot["projects"][&project_id]["status"], "active");
        let main = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .find(|assignment| {
                assignment["bot_id"] == "main"
                    && assignment["project_id"] == project_id
                    && assignment["trigger_message_id"] == report.id
            })
            .expect("main coordination assignment");
        assert_eq!(main["origin_chat_id"], chat_id);
        assert_eq!(main["parent_assignment_id"], source.id);
        assert!(main["instruction"].as_str().unwrap().contains("report.md"));
    }

    #[test]
    fn project_changes_wakes_main_with_user_message_trigger() {
        let o = Orchestrator::default();
        let worker = bot(&o, "修改 worker");
        let project = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"验收修改","goal":"验证修改唤醒","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        o.mark_project_review(&project_id).unwrap();
        let text = "请把登录错误提示改成中文";
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.request_changes",
                json!({"project_id":project_id,"text":text}),
            ))
            .unwrap();
        let message = &result["message"];
        let snapshot = o.snapshot().unwrap();
        assert_eq!(snapshot["projects"][&project_id]["status"], "active");
        let main = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .find(|assignment| {
                assignment["bot_id"] == "main"
                    && assignment["project_id"] == project_id
                    && assignment["trigger_message_id"] == message["id"]
            })
            .expect("main assignment for project changes");
        assert_eq!(main["origin_chat_id"], chat_id);
        assert_eq!(main["from"], "user");
        assert!(main["instruction"].as_str().unwrap().contains(text));
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
    fn bot_dm_handoff_keeps_source_project_and_target_dm_scope() {
        let o = Orchestrator::default();
        let from = bot(&o, "私信发起者");
        let to = bot(&o, "私信接收者");
        let project = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"私信交接","goal":"验证 Bot DM","member_bot_ids":[from,to]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let source_chat = project["chat"]["id"].as_str().unwrap().to_owned();
        let source = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: source_chat.clone(),
                bot_id: from.clone(),
                title: "源任务".into(),
                instruction: "准备私信交接".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let route = o.bot_dm_route(&from, &to).unwrap();
        assert_eq!(route.kind, "bot_dm");
        assert!(route.read_only);
        assert_eq!(route.member_bot_ids.len(), 2);
        assert!(route.chat_id.starts_with("bot_dm_"));
        assert_eq!(o.bot_dm_route(&to, &from).unwrap(), route);
        assert!(o.bot_dm_route(&from, &from).is_err());
        let private = o
            .send_msg(SendMessageRequest {
                bot_id: from,
                chat_id: route.chat_id.clone(),
                assignment_id: Some(source.id.clone()),
                run_id: None,
                call_id: None,
                text: "请在私信里接手验证".into(),
                intent: "progress".into(),
                mentions: vec![MentionInput::Bot {
                    bot_id: to.clone(),
                    instruction: Some("接手私信验证".into()),
                }],
                artifacts: vec![],
                options: vec![],
            })
            .unwrap();
        assert_eq!(private.chat_id, route.chat_id);
        let snapshot = o.snapshot().unwrap();
        let target = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .find(|assignment| {
                assignment["bot_id"] == to
                    && assignment["origin_chat_id"] == route.chat_id
                    && assignment["project_id"] == project_id
            })
            .expect("Bot DM handoff assignment");
        assert_eq!(
            snapshot["assignments"][&source.id]["origin_chat_id"],
            source_chat
        );
        assert_eq!(target["parent_assignment_id"], source.id);
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
        let message = o
            .send_msg(SendMessageRequest {
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
        let snapshot = o.snapshot().unwrap();
        let question_id = message.question_id.clone().expect("decision question");
        assert_eq!(
            snapshot["assignments"][&a.id]["wait"]["message_id"],
            message.id
        );
        assert_eq!(snapshot["questions"][&question_id]["assignment_id"], a.id);
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
    fn decision_without_recipient_or_options_rejects_without_mutating_state() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "无收件人");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "决策".into(),
                instruction: "等待选择".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let before = o.snapshot().unwrap();
        let error = o
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "没有目标".into(),
                intent: "decision".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec![],
            })
            .expect_err("a decision without a recipient must not wait forever");
        assert!(error.to_string().contains("recipient or options"));
        assert_eq!(o.snapshot().unwrap(), before);
    }

    #[test]
    fn decision_self_mention_rejects_main_alias_without_mutating_state() {
        let o = Orchestrator::default();
        let before = o.snapshot().unwrap();
        let error = o
            .send_msg(SendMessageRequest {
                bot_id: "main".into(),
                chat_id: "chat_main".into(),
                assignment_id: None,
                run_id: None,
                call_id: None,
                text: "不要自循环".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::Bot {
                    bot_id: "bot_main".into(),
                    instruction: None,
                }],
                artifacts: vec![],
                options: vec![],
            })
            .expect_err("main aliases must not self-mention");
        assert!(error.to_string().contains("sending Bot itself"));
        assert_eq!(o.snapshot().unwrap(), before);
    }

    #[test]
    fn json_string_mentions_route_user_and_bot_without_main_alias() {
        let o = Orchestrator::default();
        let source = bot(&o, "JSON 发起方");
        let target = bot(&o, "JSON 目标");
        let first = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: source.clone(),
                title: "用户决策".into(),
                instruction: "等待用户".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let user_message = rt
            .block_on(o.rpc(
                "send_msg",
                json!({
                    "bot_id":source,
                    "chat_id":"chat",
                    "assignment_id":first.id.clone(),
                    "text":"请用户选择",
                    "intent":"decision",
                    "mentions":["user"]
                }),
            ))
            .unwrap();
        assert_eq!(user_message["mentions"][0]["kind"], "user");
        assert_eq!(
            o.snapshot().unwrap()["assignments"][&first.id]["status"],
            "waiting_user"
        );

        let second = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: user_message["sender"].as_str().unwrap().into(),
                title: "Bot 决策".into(),
                instruction: "等待 Bot".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let bot_message = rt
            .block_on(o.rpc(
                "send_msg",
                json!({
                    "bot_id":user_message["sender"],
                    "chat_id":"chat",
                    "assignment_id":second.id.clone(),
                    "text":"请目标 Bot 回答",
                    "intent":"decision",
                    "mentions":[target.clone()]
                }),
            ))
            .unwrap();
        assert_eq!(bot_message["mentions"][0]["kind"], "bot");
        assert_eq!(
            o.snapshot().unwrap()["assignments"][&second.id]["status"],
            "waiting_bot"
        );
        assert!(o.snapshot().unwrap()["assignments"]
            .as_object()
            .unwrap()
            .values()
            .any(|assignment| {
                assignment["parent_assignment_id"] == second.id && assignment["bot_id"] == target
            }));
    }

    #[test]
    fn json_string_self_mention_is_rejected_before_state_change() {
        let o = Orchestrator::default();
        let source = bot(&o, "JSON 自提及");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: source.clone(),
                title: "自提及".into(),
                instruction: "拒绝".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let before = o.snapshot().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let error = rt
            .block_on(o.rpc(
                "send_msg",
                json!({
                    "bot_id":source.clone(),
                    "chat_id":"chat",
                    "assignment_id":assignment.id.clone(),
                    "text":"不能自提及",
                    "intent":"decision",
                    "mentions":[source]
                }),
            ))
            .expect_err("JSON self mention must be rejected");
        assert!(error.to_string().contains("sending Bot itself"));
        assert_eq!(o.snapshot().unwrap(), before);
    }

    #[test]
    fn private_decision_options_create_question_without_assignment() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "私聊提问");
        let chat_id = o.snapshot().unwrap()["bots"][&bot_id]["dm_chat_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message = o
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: chat_id.clone(),
                assignment_id: None,
                run_id: None,
                call_id: None,
                text: "选择登录方式".into(),
                intent: "decision".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec!["邮箱".into(), "手机号".into()],
            })
            .unwrap();
        let question_id = message.question_id.expect("private question");
        let snapshot = o.snapshot().unwrap();
        assert_eq!(snapshot["questions"][&question_id]["chat_id"], chat_id);
        assert_eq!(snapshot["questions"][&question_id]["state"], "pending");
    }

    #[test]
    fn restore_relinks_legacy_private_decision_message() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "私聊恢复");
        let chat_id = o.snapshot().unwrap()["bots"][&bot_id]["dm_chat_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message = o
            .send_msg(SendMessageRequest {
                bot_id,
                chat_id,
                assignment_id: None,
                run_id: None,
                call_id: None,
                text: "私聊旧问题".into(),
                intent: "decision".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec!["继续".into(), "停止".into()],
            })
            .unwrap();
        let question_id = message.question_id.clone().unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert_eq!(
            restored.snapshot().unwrap()["messages"][&message.id]["question_id"],
            question_id
        );
    }

    #[test]
    fn decision_options_to_bot_waits_for_bot_and_resolves_question() {
        let o = Orchestrator::default();
        let source = bot(&o, "提问方");
        let target = bot(&o, "回答方");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: source.clone(),
                title: "需要回答".into(),
                instruction: "请询问回答方".into(),
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
                bot_id: source,
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "请回答选择".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::Bot {
                    bot_id: target.clone(),
                    instruction: Some("回答这个问题".into()),
                }],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        let snapshot = o.snapshot().unwrap();
        assert_eq!(
            snapshot["assignments"][&assignment.id]["status"],
            "waiting_bot"
        );
        let question_id = message.question_id.clone().expect("Bot decision question");
        assert_eq!(snapshot["questions"].as_object().unwrap().len(), 1);
        assert_eq!(snapshot["questions"][&question_id]["state"], "pending");
        let child_id = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .find(|item| item["parent_assignment_id"] == assignment.id && item["bot_id"] == target)
            .expect("Bot decision child")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!child_id.is_empty());
        let answered = o
            .answer_decision_for_child(&assignment.id, "回答 A".into())
            .unwrap()
            .expect("linked decision question");
        assert_eq!(answered.id, question_id);
        assert_eq!(answered.state, "answered");
        assert_eq!(answered.answer.unwrap().text.as_deref(), Some("回答 A"));
        assert_eq!(
            o.snapshot().unwrap()["assignments"][&assignment.id]["status"],
            "working"
        );
    }

    #[test]
    fn restore_relinks_legacy_decision_message_to_question() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "恢复提问");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "恢复".into(),
                instruction: "恢复问题".into(),
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
                bot_id,
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "恢复选项".into(),
                intent: "decision".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        let question_id = message.question_id.clone().unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        snapshot["assignments"][&assignment.id]["wait"]["message_id"] = Value::Null;
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert_eq!(
            restored.snapshot().unwrap()["messages"][&message.id]["question_id"],
            question_id
        );
        assert_eq!(
            restored.snapshot().unwrap()["assignments"][&assignment.id]["wait"]["message_id"],
            message.id
        );
    }

    #[test]
    fn reconcile_legacy_working_decision_is_idempotent() {
        let source = Orchestrator::default();
        let bot_id = bot(&source, "迁移决策");
        let assignment = source
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "迁移".into(),
                instruction: "恢复旧等待".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = source
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "请确认登录方式".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::User("user".into())],
                artifacts: vec![],
                options: vec!["邮箱".into(), "手机".into()],
            })
            .unwrap();
        let mut snapshot = source.snapshot().unwrap();
        snapshot["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        snapshot["questions"] = json!({});
        snapshot["question_created_at"] = json!({});
        snapshot["assignments"][&assignment.id]["status"] = json!("working");
        snapshot["assignments"][&assignment.id]["wait"] = Value::Null;

        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert!(restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat",)
            .unwrap());
        let after = restored.snapshot().unwrap();
        let question_id = format!("decision:{}", message.id);
        assert_eq!(after["messages"][&message.id]["question_id"], question_id);
        assert_eq!(
            after["questions"][&question_id]["options"],
            json!(["邮箱", "手机"])
        );
        assert_eq!(
            after["assignments"][&assignment.id]["status"],
            "waiting_user"
        );
        assert_eq!(
            after["assignments"][&assignment.id]["wait"]["message_id"],
            message.id
        );
        assert!(!restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat",)
            .unwrap());
        assert_eq!(restored.snapshot().unwrap(), after);
    }

    #[test]
    fn reconcile_answered_question_relinks_without_reopening_or_duplicating() {
        let source = Orchestrator::default();
        let bot_id = bot(&source, "已回答迁移");
        let assignment = source
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "已回答".into(),
                instruction: "恢复已回答问题".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = source
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "选择登录方式".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::User("user".into())],
                artifacts: vec![],
                options: vec!["邮箱".into(), "手机".into()],
            })
            .unwrap();
        let question_id = message.question_id.clone().unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(source.rpc(
                "question.answer",
                json!({"question_id":question_id,"option_index":0}),
            ))
            .unwrap();
        let answered_snapshot = source.snapshot().unwrap();
        let answered_question = answered_snapshot["questions"][&question_id].clone();
        let mut legacy = answered_snapshot;
        legacy["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        legacy["assignments"][&assignment.id]["status"] = json!("working");
        legacy["assignments"][&assignment.id]["wait"] = Value::Null;

        let restored = Orchestrator::default();
        restored.restore(legacy).unwrap();
        assert!(restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat")
            .unwrap());
        let after = restored.snapshot().unwrap();
        assert_eq!(after["messages"][&message.id]["question_id"], question_id);
        assert_eq!(after["questions"].as_object().unwrap().len(), 1);
        assert_eq!(after["questions"][&question_id], answered_question);
        assert_eq!(after["assignments"][&assignment.id]["status"], "working");
        assert_eq!(
            after["assignments"][&assignment.id]["wait"]["message_id"],
            message.id
        );
        assert!(!restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat")
            .unwrap());
        assert_eq!(restored.snapshot().unwrap(), after);
    }

    #[test]
    fn reconcile_decision_rejects_terminal_and_does_not_infer_without_options() {
        let source = Orchestrator::default();
        let bot_id = bot(&source, "迁移校验");
        let assignment = source
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "迁移".into(),
                instruction: "拒绝终态".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = source
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "旧决定".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::User("user".into())],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        let mut terminal = source.snapshot().unwrap();
        terminal["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        terminal["questions"] = json!({});
        terminal["question_created_at"] = json!({});
        terminal["assignments"][&assignment.id]["status"] = json!("done");
        terminal["assignments"][&assignment.id]["wait"] = Value::Null;
        let restored = Orchestrator::default();
        restored.restore(terminal).unwrap();
        assert!(restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat")
            .is_err());
        assert!(restored.snapshot().unwrap()["questions"]
            .as_object()
            .unwrap()
            .is_empty());

        let mut no_options = source.snapshot().unwrap();
        no_options["messages"][&message.id]["options"] = json!([]);
        no_options["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        no_options["questions"] = json!({});
        no_options["question_created_at"] = json!({});
        no_options["assignments"][&assignment.id]["status"] = json!("working");
        no_options["assignments"][&assignment.id]["wait"] = Value::Null;
        let restored = Orchestrator::default();
        restored.restore(no_options).unwrap();
        assert!(restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat")
            .unwrap());
        let after = restored.snapshot().unwrap();
        assert!(after["questions"].as_object().unwrap().is_empty());
        assert_eq!(
            after["assignments"][&assignment.id]["status"],
            "waiting_bot"
        );
        assert_eq!(
            after["assignments"][&assignment.id]["wait"]["message_id"],
            message.id
        );
    }

    #[test]
    fn reconcile_bot_decision_waits_for_bot_and_skips_ambiguous_match() {
        let source = Orchestrator::default();
        let bot_id = bot(&source, "提问 Bot");
        let target_id = bot(&source, "回答 Bot");
        let assignment = source
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "Bot 决策".into(),
                instruction: "恢复".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = source
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "请回答".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::Bot {
                    bot_id: target_id,
                    instruction: None,
                }],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        let mut snapshot = source.snapshot().unwrap();
        snapshot["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        snapshot["questions"] = json!({});
        snapshot["question_created_at"] = json!({});
        snapshot["assignments"][&assignment.id]["status"] = json!("working");
        snapshot["assignments"][&assignment.id]["wait"] = Value::Null;
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert!(restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat")
            .unwrap());
        assert_eq!(
            restored.snapshot().unwrap()["assignments"][&assignment.id]["status"],
            "waiting_bot"
        );

        let source = Orchestrator::default();
        let bot_id = bot(&source, "歧义决策");
        let assignment = source
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "歧义".into(),
                instruction: "不猜".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = source
            .send_msg(SendMessageRequest {
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "重复选项".into(),
                intent: "decision".into(),
                mentions: vec![MentionInput::User("user".into())],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        source
            .create_question(QuestionRequest {
                bot_id: bot_id.clone(),
                assignment_id: Some(assignment.id.clone()),
                chat_id: "chat".into(),
                text: message.text.clone(),
                options: message.options.clone(),
                allow_free_text: true,
            })
            .unwrap();
        let mut snapshot = source.snapshot().unwrap();
        snapshot["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        snapshot["question_created_at"] = json!({});
        snapshot["assignments"][&assignment.id]["status"] = json!("working");
        snapshot["assignments"][&assignment.id]["wait"] = Value::Null;
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert!(!restored
            .reconcile_waiting_decision(&message.id, Some(&assignment.id), &bot_id, "chat")
            .unwrap());
        assert_eq!(
            restored.snapshot().unwrap()["assignments"][&assignment.id]["wait"],
            Value::Null
        );
    }

    #[test]
    fn restore_legacy_question_link_uses_creation_time_for_duplicate_text() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "重复问题");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "恢复".into(),
                instruction: "恢复问题".into(),
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
                bot_id: bot_id.clone(),
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "重复选项".into(),
                intent: "decision".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        let first_question_id = message.question_id.clone().unwrap();
        let _second_question = o
            .create_question(QuestionRequest {
                bot_id,
                assignment_id: Some(assignment.id),
                chat_id: "chat".into(),
                text: "重复选项".into(),
                options: vec!["A".into(), "B".into()],
                allow_free_text: true,
            })
            .unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["messages"][&message.id]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert_eq!(
            restored.snapshot().unwrap()["messages"][&message.id]["question_id"],
            first_question_id
        );
    }

    #[test]
    fn restore_does_not_guess_ambiguous_decision_wait_message() {
        let o = Orchestrator::default();
        let bot_id = bot(&o, "歧义恢复");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: bot_id.clone(),
                title: "恢复".into(),
                instruction: "恢复问题".into(),
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
                bot_id,
                chat_id: "chat".into(),
                assignment_id: Some(assignment.id.clone()),
                run_id: None,
                call_id: None,
                text: "同一决策".into(),
                intent: "decision".into(),
                mentions: vec![],
                artifacts: vec![],
                options: vec!["A".into(), "B".into()],
            })
            .unwrap();
        let mut snapshot = o.snapshot().unwrap();
        let duplicate_id = "legacy-duplicate-message";
        let mut duplicate = snapshot["messages"][&message.id].clone();
        duplicate["id"] = json!(duplicate_id);
        duplicate["created_at"] = json!(snapshot["messages"][&message.id]["created_at"]);
        snapshot["messages"]
            .as_object_mut()
            .unwrap()
            .insert(duplicate_id.into(), duplicate);
        snapshot["assignments"][&assignment.id]["wait"]["message_id"] = Value::Null;
        let restored = Orchestrator::default();
        restored.restore(snapshot).unwrap();
        assert!(
            restored.snapshot().unwrap()["assignments"][&assignment.id]["wait"]["message_id"]
                .is_null()
        );
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
    fn loop_limit_returns_pause_block_and_resume_dispatches_once() {
        let o = Orchestrator::new(OrchestratorSettings {
            loop_hops: 1,
            ..Default::default()
        });
        let from = bot(&o, "A");
        let to = bot(&o, "B");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat".into(),
                bot_id: from.clone(),
                title: "A→B→A".into(),
                instruction: "循环起点".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: Some("root-loop".into()),
                loop_hops: 1,
            })
            .unwrap();
        let response = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "send_msg",
                json!({
                    "bot_id": from,
                    "chat_id": "chat",
                    "assignment_id": assignment.id,
                    "text": "继续交接",
                    "intent": "done",
                    "mentions": [{"kind":"bot","bot_id":to,"instruction":"下一跳"}]
                }),
            ))
            .unwrap();
        let block = response["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "loop_paused")
            .expect("loop pause block");
        assert_eq!(block["root_message_id"], "root-loop");
        assert_eq!(block["hops"], 1);
        assert_eq!(block["state"], "paused");
        let snapshot = o.snapshot().unwrap();
        assert_eq!(
            snapshot["pending_loop_dispatches"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(snapshot["loop_states"]["root-loop"], "paused");

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "loop.resolve",
                json!({"root_message_id":"root-loop","action":"continue"}),
            ))
            .unwrap();
        let assignments = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc("assignment.list", json!({})))
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .clone();
        let child = assignments
            .iter()
            .find(|item| item["parent_assignment_id"] == assignment.id)
            .expect("continued loop assignment");
        assert_eq!(child["loop_hops"], 0);
        assert_eq!(
            o.snapshot().unwrap()["pending_loop_dispatches"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
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
    fn bot_duplicate_copies_profile_and_routines_without_history() {
        let o = Orchestrator::default();
        let worker = bot(&o, "原 Bot");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(o.rpc(
            "bot.update",
            json!({"bot_id":worker,"patch":{"description":"资料","model":"provider/model","max_parallel":4}}),
        ))
        .unwrap();
        let routine = rt
            .block_on(o.rpc(
                "routine.create",
                json!({"bot_id":worker,"name":"巡检","instructions":"检查","schedules":[{"cron":"*/5 * * * *","label":"five"}]}),
            ))
            .unwrap();
        let routine_id = routine["routine"]["id"].as_str().unwrap();
        rt.block_on(o.rpc("routine.test_run", json!({"routine_id":routine_id})))
            .unwrap();
        let duplicated = rt
            .block_on(o.rpc(
                "bot.duplicate",
                json!({"bot_id":worker,"name":"复制 Bot","target_bot_id":"bot-copy-target"}),
            ))
            .unwrap();
        assert_eq!(duplicated["bot"]["id"], "bot-copy-target");
        assert_eq!(duplicated["bot"]["dm_chat_id"], "dm_bot-copy-target");
        assert_eq!(duplicated["bot"]["description"], "资料");
        assert_eq!(duplicated["bot"]["model"], "provider/model");
        assert_eq!(duplicated["bot"]["max_parallel"], 4);
        let snapshot = o.snapshot().unwrap();
        let copied = snapshot["routines"]
            .as_object()
            .unwrap()
            .values()
            .filter(|routine| routine["bot_id"] == "bot-copy-target")
            .collect::<Vec<_>>();
        assert_eq!(copied.len(), 1);
        assert!(copied[0]["last_run"].is_null());
        assert!(snapshot["routine_runs"]
            .as_object()
            .unwrap()
            .get(copied[0]["id"].as_str().unwrap())
            .is_none());
        assert!(rt
            .block_on(o.rpc(
                "bot.duplicate",
                json!({"bot_id":worker,"name":"重复目标","target_bot_id":"bot-copy-target"}),
            ))
            .is_err());
    }

    #[test]
    fn cancellation_emits_system_message_and_confirm_done_stops_active_work() {
        let o = Orchestrator::default();
        let worker = bot(&o, "取消测试");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"取消项目","goal":"验证","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: project["chat"]["id"].as_str().unwrap().into(),
                bot_id: worker,
                title: "待取消".into(),
                instruction: "x".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let done = rt
            .block_on(o.rpc("project.confirm_done", json!({"project_id":project_id})))
            .unwrap();
        assert_eq!(done["project"]["status"], "done");
        let snapshot = o.snapshot().unwrap();
        assert_eq!(
            snapshot["assignments"][&assignment.id]["status"],
            "cancelled"
        );
        let message_id = format!("task_stopped:{}", assignment.id);
        assert_eq!(snapshot["messages"][&message_id]["sender"], "system");
        assert_eq!(snapshot["messages"][&message_id]["intent"], "task_stopped");
        assert_eq!(
            snapshot["messages"][&message_id]["assignment_id"],
            assignment.id
        );
        assert!(rt
            .block_on(o.rpc("project.confirm_done", json!({"project_id":project_id})))
            .is_err());
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
        assert_eq!(assignment["assignment"]["project_id"], project_id);

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

    #[test]
    fn bot_update_model_null_restores_default_selection() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let created = rt
            .block_on(o.rpc(
                "bot.create",
                json!({"name":"模型覆盖","model":"provider/model-a"}),
            ))
            .unwrap();
        let bot_id = created["bot"]["id"].as_str().unwrap();
        let updated = rt
            .block_on(o.rpc(
                "bot.update",
                json!({"bot_id":bot_id,"patch":{"model":null}}),
            ))
            .unwrap();
        assert!(updated["bot"]["model"].is_null());
    }

    #[test]
    fn main_assignment_ignores_zero_bot_limit_and_projects_include_main() {
        let o = Orchestrator::default();
        let main = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat_main".into(),
                bot_id: "main".into(),
                title: "主任务".into(),
                instruction: "汇总".into(),
                from: "user".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        assert_eq!(main.status, "working");

        let worker = bot(&o, "项目成员");
        let project = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(o.rpc(
                "project.create",
                json!({"name":"主成员","goal":"测试","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let members = project["project"]["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["bot_id"], "main");
    }

    #[test]
    fn main_coordination_does_not_consume_worker_global_slot() {
        let o = Orchestrator::new(OrchestratorSettings {
            global_limit: 1,
            bot_default_limit: 1,
            subagent_per_run: 4,
            subagent_global: 12,
            loop_hops: 8,
        });
        let worker = bot(&o, "全局限额 worker");
        let worker_running = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: format!("dm_{worker}"),
                bot_id: worker.clone(),
                title: "占用 worker 槽位".into(),
                instruction: "执行中".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        assert_eq!(worker_running.status, "working");
        let worker_queued = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: format!("dm_{worker}"),
                bot_id: worker.clone(),
                title: "等待 worker 槽位".into(),
                instruction: "排队".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        assert_eq!(worker_queued.status, "queued");
        let main = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat_main".into(),
                bot_id: "main".into(),
                title: "协调".into(),
                instruction: "汇总 worker 进度".into(),
                from: "user".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 2,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        assert_eq!(main.status, "working");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let workbench = rt.block_on(o.rpc("workbench.get", json!({}))).unwrap();
        assert_eq!(workbench["running"], 1);

        o.finish_assignment(&main.id, "done").unwrap();
        let queued_status = |id: &str| {
            rt.block_on(o.rpc("assignment.get", json!({"assignment_id":id})))
                .unwrap()["assignment"]["status"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(queued_status(&worker_queued.id), "queued");
        o.finish_assignment(&worker_running.id, "done").unwrap();
        assert_eq!(queued_status(&worker_queued.id), "working");
    }

    #[test]
    fn project_update_duplicate_and_completion_operations_are_persisted() {
        let o = Orchestrator::default();
        let worker = bot(&o, "可复制");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"旧名称","goal":"旧目标","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let updated = rt
            .block_on(o.rpc(
                "project.update",
                json!({"project_id":project_id,"patch":{"name":"新名称","goal":"新目标"}}),
            ))
            .unwrap();
        assert_eq!(updated["project"]["name"], "新名称");

        rt.block_on(o.rpc(
            "routine.create",
            json!({"bot_id":worker,"project_id":project_id,"name":"原定时","instructions":"巡检","schedules":[{"cron":"0 * * * *","label":"hourly"}]}),
        ))
        .unwrap();

        let duplicated = rt
            .block_on(o.rpc(
                "bot.duplicate",
                json!({"bot_id":worker,"name":"复制 Bot","target_bot_id":"bot-copy-target"}),
            ))
            .unwrap();
        assert_eq!(duplicated["bot"]["id"], "bot-copy-target");
        assert_eq!(duplicated["bot"]["dm_chat_id"], duplicated["dm_chat"]["id"]);
        let duplicate_id = duplicated["bot"]["id"].as_str().unwrap();
        let snapshot = o.snapshot().unwrap();
        let routines = snapshot["routines"]
            .as_object()
            .unwrap()
            .values()
            .filter(|routine| routine["bot_id"] == duplicate_id)
            .collect::<Vec<_>>();
        assert_eq!(routines.len(), 1);
        assert!(routines[0]["last_run"].is_null());
        assert!(rt
            .block_on(o.rpc(
                "bot.duplicate",
                json!({"bot_id":worker,"name":"重复目标","target_bot_id":"bot-copy-target"}),
            ))
            .is_err());

        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.into()),
                origin_chat_id: project["chat"]["id"].as_str().unwrap().into(),
                bot_id: worker,
                title: "待取消".into(),
                instruction: "x".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let message = rt
            .block_on(o.rpc(
                "project.request_changes",
                json!({"project_id":project_id,"text":"请补测试"}),
            ))
            .unwrap();
        assert_eq!(message["message"]["mentions"][0]["kind"], "main");
        let done = rt
            .block_on(o.rpc("project.confirm_done", json!({"project_id":project_id})))
            .unwrap();
        assert_eq!(done["project"]["status"], "done");
        let stopped = rt
            .block_on(o.rpc("assignment.get", json!({"assignment_id":assignment.id})))
            .unwrap();
        assert_eq!(stopped["assignment"]["status"], "cancelled");
    }

    #[test]
    fn project_review_transition_is_idempotent_and_terminal_projects_are_protected() {
        let o = Orchestrator::default();
        let worker = bot(&o, "待验收");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"待验收项目","goal":"x","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();

        let review = o.mark_project_review(project_id).unwrap();
        assert_eq!(review.status, "review");
        let repeated = o.mark_project_review(project_id).unwrap();
        assert_eq!(repeated, review);

        rt.block_on(o.rpc("project.confirm_done", json!({"project_id":project_id})))
            .unwrap();
        let done_error = o.mark_project_review(project_id).unwrap_err();
        assert!(matches!(done_error, OrchestratorError::Conflict(_)));

        let archived = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"已归档项目","goal":"x","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let archived_id = archived["project"]["id"].as_str().unwrap();
        rt.block_on(o.rpc("project.archive", json!({"project_id":archived_id})))
            .unwrap();
        let archived_error = o.mark_project_review(archived_id).unwrap_err();
        assert!(matches!(archived_error, OrchestratorError::Conflict(_)));
    }

    #[test]
    fn filtered_lists_steer_idle_bot_and_validate_routine_project() {
        let o = Orchestrator::default();
        let worker = bot(&o, "筛选");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"筛选项目","goal":"x","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let chat_id = project["chat"]["id"].as_str().unwrap();
        let steer = o
            .queue_steer(SteerRequest {
                bot_id: worker.clone(),
                project_id: Some(project_id.into()),
                chat_id: chat_id.into(),
                text: "开始".into(),
                message_id: None,
            })
            .unwrap();
        assert_eq!(steer.state, "delivered");
        assert!(steer.assignment_id.is_some());
        let filtered = rt
            .block_on(o.rpc(
                "assignment.list",
                json!({"project_id":project_id,"status":["working"],"limit":1}),
            ))
            .unwrap();
        assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
        let invalid = rt.block_on(o.rpc(
            "routine.create",
            json!({"bot_id":worker,"project_id":"missing","name":"坏","instructions":"x","schedules":[{"cron":"0 * * * *","label":"hourly"}]}),
        ));
        assert!(invalid.is_err());
    }

    #[test]
    fn blocked_assignments_block_bot_and_member_removal_and_unknown_routine_is_not_found() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let worker = bot(&o, "阻塞任务");
        let standalone = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "chat_main".into(),
                bot_id: worker.clone(),
                title: "阻塞任务".into(),
                instruction: "等待处理".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.finish_assignment(&standalone.id, "blocked").unwrap();
        assert!(rt
            .block_on(o.rpc("bot.delete", json!({"bot_id":worker})))
            .is_err());

        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"阻塞项目","goal":"验证","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let project_assignment = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: project["chat"]["id"].as_str().unwrap().into(),
                bot_id: worker,
                title: "项目阻塞任务".into(),
                instruction: "等待项目处理".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.finish_assignment(&project_assignment.id, "blocked")
            .unwrap();
        rt.block_on(o.rpc(
            "project.remove_member",
            json!({"project_id":project_id,"bot_id":project_assignment.bot_id}),
        ))
        .unwrap();
        let stopped = rt
            .block_on(o.rpc(
                "assignment.get",
                json!({"assignment_id":project_assignment.id}),
            ))
            .unwrap();
        assert_eq!(stopped["assignment"]["status"], "cancelled");
        assert!(rt
            .block_on(o.rpc("routine.runs", json!({"routine_id":"missing"})))
            .is_err());
    }

    #[test]
    fn approval_and_question_waiting_states_validate_inputs() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let worker = bot(&o, "审批");
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "dm".into(),
                bot_id: worker.clone(),
                title: "审批任务".into(),
                instruction: "x".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let private_approval = o
            .create_approval(ApprovalRequest {
                bot_id: worker.clone(),
                assignment_id: None,
                chat_id: "dm".into(),
                tool: "browser".into(),
                risk: "external".into(),
                summary: "打开网页".into(),
                detail: "需要用户确认".into(),
            })
            .unwrap();
        let workbench = rt.block_on(o.rpc("workbench.get", json!({}))).unwrap();
        let waiting = workbench["waiting"].as_array().unwrap();
        assert!(waiting.iter().any(|item| {
            item["kind"] == "approval"
                && item["approval"]["id"] == private_approval.id
                && item["approval"]["assignment_id"].is_null()
        }));
        assert!(waiting.iter().all(|item| {
            matches!(
                item["kind"].as_str(),
                Some("review" | "approval" | "question" | "takeover")
            )
        }));
        assert!(workbench["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["bot_id"] == "main"));
        assert!(o
            .create_approval(ApprovalRequest {
                bot_id: worker.clone(),
                assignment_id: Some("missing".into()),
                chat_id: "dm".into(),
                tool: "bash".into(),
                risk: "exec".into(),
                summary: "x".into(),
                detail: "x".into(),
            })
            .is_err());
        let approval = o
            .create_approval(ApprovalRequest {
                bot_id: worker.clone(),
                assignment_id: Some(assignment.id.clone()),
                chat_id: "dm".into(),
                tool: "bash".into(),
                risk: "exec".into(),
                summary: "执行".into(),
                detail: "echo x".into(),
            })
            .unwrap();
        let denied = rt
            .block_on(o.rpc(
                "approval.decide",
                json!({"approval_id":approval.id,"decision":"deny"}),
            ))
            .unwrap();
        assert_eq!(denied["approval"]["state"], "denied");
        let state = rt
            .block_on(o.rpc("assignment.get", json!({"assignment_id":assignment.id})))
            .unwrap();
        assert_eq!(state["assignment"]["status"], "failed");

        let second = o
            .create_assignment(AssignmentRequest {
                project_id: None,
                origin_chat_id: "dm".into(),
                bot_id: state["assignment"]["bot_id"].as_str().unwrap().into(),
                title: "规则任务".into(),
                instruction: "y".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        let always = o
            .create_approval(ApprovalRequest {
                bot_id: second.bot_id.clone(),
                assignment_id: Some(second.id.clone()),
                chat_id: "dm".into(),
                tool: "bash".into(),
                risk: "exec".into(),
                summary: "允许规则".into(),
                detail: "echo y".into(),
            })
            .unwrap();
        rt.block_on(o.rpc(
            "approval.decide",
            json!({"approval_id":always.id,"decision":"always_allow"}),
        ))
        .unwrap();
        assert_eq!(
            o.snapshot().unwrap()["approval_rules"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let question = o
            .create_question(QuestionRequest {
                bot_id: worker,
                assignment_id: Some(state["assignment"]["id"].as_str().unwrap().into()),
                chat_id: "dm".into(),
                text: "选项".into(),
                options: vec!["A".into()],
                allow_free_text: false,
            })
            .unwrap();
        let free = rt.block_on(o.rpc(
            "question.answer",
            json!({"question_id":question.id,"text":"自由回答"}),
        ));
        assert!(free.is_err());
    }

    #[test]
    fn finish_routine_run_updates_snapshot_and_is_idempotent() {
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
    fn project_attention_polls_terminal_and_stale_workers_idempotently() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let worker = bot(&o, "关注 worker");
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"关注项目","goal":"测试","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let make_assignment = |title: &str| {
            o.create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: chat_id.clone(),
                bot_id: worker.clone(),
                title: title.into(),
                instruction: title.into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap()
        };

        let failed = make_assignment("失败");
        o.finish_assignment(&failed.id, "failed").unwrap();
        let blocked = make_assignment("阻塞");
        o.finish_assignment(&blocked.id, "blocked").unwrap();
        let done_without_report = make_assignment("无报告完成");
        o.finish_assignment(&done_without_report.id, "done")
            .unwrap();
        let stale = make_assignment("超时");
        let mut snapshot = o.snapshot().unwrap();
        let old_started = (Utc::now() - Duration::hours(3)).to_rfc3339();
        snapshot["assignments"][&stale.id]["started_at"] = json!(old_started);
        o.restore(snapshot).unwrap();

        let at = Utc::now();
        let notices = o.poll_project_attention(at).unwrap();
        assert_eq!(notices.len(), 4);
        assert!(notices.iter().any(|notice| notice.code == "info"));
        assert_eq!(
            notices
                .iter()
                .filter(|notice| notice.code == "task_no_report")
                .count(),
            3
        );
        for notice in &notices {
            assert!(notice.message.id.starts_with("task_attention:"));
            assert_eq!(notice.message.intent, None);
            assert_eq!(notice.message.created_at, at.to_rfc3339());
        }
        assert!(o.poll_project_attention(at).unwrap().is_empty());

        let persisted = o.snapshot().unwrap();
        assert_eq!(persisted["attention_notices"].as_array().unwrap().len(), 4);
        let restored = Orchestrator::default();
        restored.restore(persisted).unwrap();
        assert!(restored.poll_project_attention(at).unwrap().is_empty());

        let cancelled = restored
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id),
                origin_chat_id: chat_id,
                bot_id: worker,
                title: "取消".into(),
                instruction: "取消".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        restored
            .finish_assignment(&cancelled.id, "cancelled")
            .unwrap();
        assert!(restored.poll_project_attention(at).unwrap().is_empty());
    }

    #[test]
    fn stale_attention_uses_latest_ack_or_progress_and_blocked_wakes_main() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let worker = bot(&o, "及时 worker");
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"及时项目","goal":"测试","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: chat_id.clone(),
                bot_id: worker.clone(),
                title: "有进展".into(),
                instruction: "有进展".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.send_msg(SendMessageRequest {
            bot_id: worker.clone(),
            chat_id: chat_id.clone(),
            assignment_id: Some(assignment.id.clone()),
            run_id: None,
            call_id: None,
            text: "已确认进展".into(),
            intent: "ack".into(),
            mentions: vec![],
            artifacts: vec![],
            options: vec![],
        })
        .unwrap();
        let mut snapshot = o.snapshot().unwrap();
        snapshot["assignments"][&assignment.id]["started_at"] =
            json!((Utc::now() - Duration::hours(3)).to_rfc3339());
        o.restore(snapshot).unwrap();
        assert!(o.poll_project_attention(Utc::now()).unwrap().is_empty());

        let blocked = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: chat_id.clone(),
                bot_id: worker.clone(),
                title: "无显式提及的阻塞".into(),
                instruction: "等待用户".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.send_msg(SendMessageRequest {
            bot_id: worker,
            chat_id,
            assignment_id: Some(blocked.id.clone()),
            run_id: None,
            call_id: None,
            text: "无法继续".into(),
            intent: "blocked".into(),
            mentions: vec![],
            artifacts: vec![],
            options: vec![],
        })
        .unwrap();
        let state = o.snapshot().unwrap();
        assert!(state["assignments"]
            .as_object()
            .unwrap()
            .values()
            .any(|value| {
                value["bot_id"] == "main"
                    && value["project_id"] == project_id
                    && matches!(value["status"].as_str(), Some("working" | "queued"))
            }));
    }

    #[test]
    fn blocked_attention_is_deduped_per_episode_and_reopens_after_resume() {
        let o = Orchestrator::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let worker = bot(&o, "需要再次提醒");
        let project = rt
            .block_on(o.rpc(
                "project.create",
                json!({"name":"阻塞重开","goal":"测试","member_bot_ids":[worker]}),
            ))
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let assignment = o
            .create_assignment(AssignmentRequest {
                project_id: Some(project_id.clone()),
                origin_chat_id: chat_id.clone(),
                bot_id: worker.clone(),
                title: "可恢复阻塞".into(),
                instruction: "等待外部输入".into(),
                from: "main".into(),
                trigger_message_id: None,
                parent_assignment_id: None,
                priority: 1,
                root_message_id: None,
                loop_hops: 0,
            })
            .unwrap();
        o.finish_assignment(&assignment.id, "blocked").unwrap();

        assert_eq!(o.poll_project_attention(Utc::now()).unwrap().len(), 1);
        assert!(o.poll_project_attention(Utc::now()).unwrap().is_empty());

        let resumed = o
            .queue_steer(SteerRequest {
                bot_id: worker.clone(),
                project_id: Some(project_id.clone()),
                chat_id: chat_id.clone(),
                text: "继续处理".into(),
                message_id: None,
            })
            .unwrap();
        assert_eq!(
            resumed.assignment_id.as_deref(),
            Some(assignment.id.as_str())
        );
        let snapshot = o.snapshot().unwrap();
        assert_eq!(snapshot["assignments"][&assignment.id]["status"], "working");
        assert!(o.poll_project_attention(Utc::now()).unwrap().is_empty());

        o.send_msg(SendMessageRequest {
            bot_id: worker,
            chat_id,
            assignment_id: Some(assignment.id.clone()),
            run_id: None,
            call_id: None,
            text: "再次阻塞".into(),
            intent: "blocked".into(),
            mentions: vec![],
            artifacts: vec![],
            options: vec![],
        })
        .unwrap();
        assert_eq!(o.poll_project_attention(Utc::now()).unwrap().len(), 1);
        assert!(o.poll_project_attention(Utc::now()).unwrap().is_empty());
    }
}
