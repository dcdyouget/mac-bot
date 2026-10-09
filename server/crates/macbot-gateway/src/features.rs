//! Durable S3 feature facade for skills, memory and search.
//!
//! The gateway backend owns transport and live event fan-out.  This module
//! owns the feature state and returns events only after `Store::append_event`
//! has synced them.  The caller can therefore publish the returned events
//! without ever exposing an event which was not durable.

use async_trait::async_trait;
use macbot_memory::{
    default_l0_platform_rules, AsyncMaintenanceProvider, ContextPackage, ContextRequest,
    ConversationIndex, MemoryAction, MemoryEntry, MemoryError, MemoryKind, MemoryRequest,
    MemorySource, MemoryTarget, ProjectRecord, SessionMessage, SharedMemoryStore,
};
use macbot_providers::{
    Completion, HttpProvider, ModelProvider, ModelRequest, ProviderConfig, SecretStore,
};
use macbot_skills::{Skill, SkillError, SkillRegistry};
use macbot_store::{Event, Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use thiserror::Error;

const IDEMPOTENCY_FILE: &str = "data/features/idempotency.json";
const SKILLS_FILE: &str = "data/features/skills.json";
const CONVERSATIONS_FILE: &str = "data/features/conversations.json";
const MAINTENANCE_FILE: &str = "data/features/maintenance.json";

#[derive(Debug, Error)]
pub enum FeatureError {
    #[error("skill: {0}")]
    Skill(#[from] SkillError),
    #[error("memory: {0}")]
    Memory(#[from] MemoryError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("invalid feature request: {0}")]
    Invalid(String),
    #[error("feature request conflict: {0}")]
    Conflict(String),
    #[error("provider: {0}")]
    Provider(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}

pub type FeatureResult<T> = Result<T, FeatureError>;

impl FeatureError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Skill(SkillError::NotFound(_)) | Self::Memory(MemoryError::NotFound(_)) => {
                "not_found"
            }
            Self::Skill(SkillError::Conflict(_)) | Self::Conflict(_) => "conflict",
            Self::Skill(SkillError::BuiltinReadOnly) => "forbidden",
            Self::Memory(MemoryError::QuotaExceeded { .. }) => "quota_exceeded",
            Self::Invalid(_) | Self::Skill(_) | Self::Memory(_) => "invalid_params",
            Self::Store(_) | Self::Io(_) | Self::Provider(_) => "internal",
        }
    }
}

/// A response includes events which are already durable in `shared_store`.
/// Gateway code should publish them after this method returns.
#[derive(Clone, Debug)]
pub struct FeatureResponse {
    pub result: Value,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdempotencyRecord {
    key: String,
    method: String,
    signature: String,
    result: Value,
}

#[derive(Clone, Debug)]
pub enum MemoryActor {
    MainBot { bot_id: String },
    Bot { bot_id: String },
    System,
}

/// Authorization context supplied by the gateway after it has authenticated
/// the user and resolved group membership. User memory is shared by all Bots
/// for that user; project memory is visible only to project members; Bot
/// memory remains private to its owner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryAccess {
    pub user_id: Option<String>,
    pub project_id: Option<String>,
    pub project_member_bot_ids: Vec<String>,
}

impl MemoryAccess {
    pub fn user(user_id: impl Into<String>) -> Self {
        Self {
            user_id: Some(user_id.into()),
            ..Self::default()
        }
    }

    pub fn group(project_id: impl Into<String>, member_bot_ids: Vec<String>) -> Self {
        Self {
            project_id: Some(project_id.into()),
            project_member_bot_ids: member_bot_ids,
            ..Self::default()
        }
    }
}

impl MemoryActor {
    pub fn bot(bot_id: impl Into<String>) -> Self {
        Self::Bot {
            bot_id: bot_id.into(),
        }
    }

    pub fn main(bot_id: impl Into<String>) -> Self {
        Self::MainBot {
            bot_id: bot_id.into(),
        }
    }

    fn bot_id(&self) -> Option<&str> {
        match self {
            Self::MainBot { bot_id } | Self::Bot { bot_id } => Some(bot_id),
            Self::System => None,
        }
    }

    fn is_main(&self) -> bool {
        matches!(self, Self::MainBot { .. } | Self::System)
    }
}

/// The service is intentionally cloneable. The locks are shared with all
/// gateway handlers, so quota checks and skill mutations remain serialized.
#[derive(Clone)]
pub struct FeatureService {
    pub shared_store: Store,
    pub skill_registry: Arc<RwLock<SkillRegistry>>,
    pub shared_memory: SharedMemoryStore,
    pub home: PathBuf,
    memory_path: PathBuf,
    conversations: Arc<RwLock<ConversationIndex>>,
    idempotency: Arc<RwLock<BTreeMap<String, IdempotencyRecord>>>,
    maintenance: Arc<RwLock<Option<Arc<dyn AsyncMaintenanceProvider>>>>,
    maintenance_policy: Arc<RwLock<MaintenancePolicy>>,
    maintenance_state: Arc<RwLock<MaintenanceState>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenancePolicy {
    pub daily_max_calls: u32,
    pub min_interval_seconds: i64,
    pub max_input_chars: usize,
}

impl Default for MaintenancePolicy {
    fn default() -> Self {
        Self {
            daily_max_calls: 1,
            min_interval_seconds: 24 * 60 * 60,
            max_input_chars: macbot_memory::BOT_WORKLOG_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MaintenanceState {
    usage: BTreeMap<String, MaintenanceUsage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MaintenanceUsage {
    day: String,
    calls: u32,
    last_run: chrono::DateTime<chrono::Utc>,
}

impl FeatureService {
    pub fn open(
        home: impl Into<PathBuf>,
        extra_dirs: impl IntoIterator<Item = PathBuf>,
    ) -> FeatureResult<Self> {
        let home = home.into();
        let store = Store::open(&home)?;
        Self::with_store(store, home, extra_dirs)
    }

    pub fn with_store(
        shared_store: Store,
        home: impl Into<PathBuf>,
        extra_dirs: impl IntoIterator<Item = PathBuf>,
    ) -> FeatureResult<Self> {
        let home = home.into();
        let memory_path = home.join("data/memory/state.json");
        if let Some(parent) = memory_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let memory = SharedMemoryStore::new(macbot_memory::MemoryStore::load_from(&memory_path)?);
        let mut registry = SkillRegistry::new(&home, extra_dirs);
        registry.rescan()?;
        let records = shared_store
            .read_snapshot::<Vec<IdempotencyRecord>>(IDEMPOTENCY_FILE)?
            .unwrap_or_default();
        let idempotency = records
            .into_iter()
            .map(|record| (record.key.clone(), record))
            .collect();
        let conversations = shared_store
            .read_snapshot::<ConversationIndex>(CONVERSATIONS_FILE)?
            .unwrap_or_default();
        let maintenance_state = shared_store
            .read_snapshot::<MaintenanceState>(MAINTENANCE_FILE)?
            .unwrap_or_default();
        Ok(Self {
            shared_store,
            skill_registry: Arc::new(RwLock::new(registry)),
            shared_memory: memory,
            home,
            memory_path,
            conversations: Arc::new(RwLock::new(conversations)),
            idempotency: Arc::new(RwLock::new(idempotency)),
            maintenance: Arc::new(RwLock::new(None)),
            maintenance_policy: Arc::new(RwLock::new(MaintenancePolicy::default())),
            maintenance_state: Arc::new(RwLock::new(maintenance_state)),
        })
    }

    pub fn set_maintenance_policy(&self, policy: MaintenancePolicy) -> FeatureResult<()> {
        if policy.daily_max_calls == 0
            || policy.min_interval_seconds < 0
            || policy.max_input_chars == 0
        {
            return Err(FeatureError::Invalid(
                "maintenance policy must have positive limits".into(),
            ));
        }
        *self
            .maintenance_policy
            .write()
            .map_err(|_| FeatureError::Invalid("maintenance policy lock poisoned".into()))? =
            policy;
        Ok(())
    }

    pub fn set_maintenance_provider(
        &self,
        provider: Option<Arc<dyn AsyncMaintenanceProvider>>,
    ) -> FeatureResult<()> {
        *self
            .maintenance
            .write()
            .map_err(|_| FeatureError::Invalid("maintenance lock poisoned".into()))? = provider;
        Ok(())
    }

    /// Dispatch the protocol's complete skill management surface.
    pub fn skill_rpc(&self, method: &str, params: Value) -> FeatureResult<FeatureResponse> {
        if !matches!(
            method,
            "skill.list"
                | "skill.get"
                | "skill.create"
                | "skill.update"
                | "skill.delete"
                | "skill.set_enabled"
                | "skill.publish"
                | "skill.import"
        ) {
            return Err(FeatureError::Invalid(format!(
                "unknown skill method: {method}"
            )));
        }
        if let Some(replay) = self.replay_idempotent(method, &params)? {
            return Ok(replay);
        }
        match method {
            "skill.list" => {
                let skills = self
                    .skill_registry
                    .read()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .list();
                Ok(FeatureResponse {
                    result: json!({"skills": skills}),
                    events: Vec::new(),
                })
            }
            "skill.get" => {
                let name = required_string(&params, "name")?;
                let detail = self
                    .skill_registry
                    .read()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .get(name)?;
                Ok(FeatureResponse {
                    result: json!({"skill": detail}),
                    events: Vec::new(),
                })
            }
            "skill.create" => {
                let name = required_string(&params, "name")?;
                let content = required_string(&params, "content")?;
                let skill = self
                    .skill_registry
                    .write()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .create(name, content)?;
                self.finish_skill_mutation(
                    method,
                    &params,
                    json!({"skill":skill}),
                    vec![("skill.updated", json!({"skill":skill}))],
                )
            }
            "skill.update" => {
                let name = required_string(&params, "name")?;
                let content = required_string(&params, "content")?;
                let skill = self
                    .skill_registry
                    .write()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .update(name, content)?;
                self.finish_skill_mutation(
                    method,
                    &params,
                    json!({"skill":skill}),
                    vec![("skill.updated", json!({"skill":skill}))],
                )
            }
            "skill.delete" => {
                let name = required_string(&params, "name")?.to_string();
                self.skill_registry
                    .write()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .delete(&name)?;
                self.finish_skill_mutation(
                    method,
                    &params,
                    json!({}),
                    vec![("skill.deleted", json!({"name":name}))],
                )
            }
            "skill.set_enabled" => {
                let name = required_string(&params, "name")?;
                let enabled = params
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| FeatureError::Invalid("enabled is required".into()))?;
                let bot_id = params.get("bot_id").and_then(Value::as_str);
                let skill = self
                    .skill_registry
                    .write()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .set_enabled(name, enabled, bot_id)?;
                self.finish_skill_mutation(
                    method,
                    &params,
                    json!({"skill":skill}),
                    vec![("skill.updated", json!({"skill":skill}))],
                )
            }
            "skill.publish" => {
                let name = required_string(&params, "name")?;
                let skill = self
                    .skill_registry
                    .write()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                    .publish(name)?;
                self.finish_skill_mutation(
                    method,
                    &params,
                    json!({"skill":skill}),
                    vec![("skill.updated", json!({"skill":skill}))],
                )
            }
            "skill.import" => {
                let source = params
                    .get("source")
                    .ok_or_else(|| FeatureError::Invalid("source is required".into()))?;
                let mut registry = self
                    .skill_registry
                    .write()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?;
                let skills = match source.get("kind").and_then(Value::as_str) {
                    Some("path") => registry.import_path(required_string(source, "path")?)?,
                    Some("git") => registry.import_git(
                        required_string(source, "url")?,
                        source.get("subdir").and_then(Value::as_str),
                    )?,
                    Some("upload") => {
                        let id = required_string(source, "upload_id")?;
                        let path = upload_path(&self.home, id)?;
                        registry.import_zip(&fs::read(path)?)?
                    }
                    Some(kind) => {
                        return Err(FeatureError::Invalid(format!(
                            "unsupported import source: {kind}"
                        )))
                    }
                    None => return Err(FeatureError::Invalid("source.kind is required".into())),
                };
                drop(registry);
                let events = skills
                    .iter()
                    .map(|skill| ("skill.updated", json!({"skill":skill})))
                    .collect();
                self.finish_skill_mutation(method, &params, json!({"skills":skills}), events)
            }
            _ => unreachable!(),
        }
    }

    pub fn create_skill_draft(&self, name: &str, content: &str) -> FeatureResult<Skill> {
        let skill = self
            .skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .create_draft(name, content)?;
        self.persist_skills()?;
        Ok(skill)
    }

    fn finish_skill_mutation(
        &self,
        method: &str,
        params: &Value,
        result: Value,
        event_specs: Vec<(&str, Value)>,
    ) -> FeatureResult<FeatureResponse> {
        self.persist_skills()?;
        let mut events = Vec::with_capacity(event_specs.len());
        for (name, data) in event_specs {
            events.push(self.shared_store.append_event(name, data)?);
        }
        self.save_idempotency(method, params, &result)?;
        Ok(FeatureResponse { result, events })
    }

    fn persist_skills(&self) -> FeatureResult<()> {
        let skills = self
            .skill_registry
            .read()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .list();
        self.shared_store.write_snapshot(SKILLS_FILE, &skills)?;
        Ok(())
    }

    fn replay_idempotent(
        &self,
        method: &str,
        params: &Value,
    ) -> FeatureResult<Option<FeatureResponse>> {
        let Some(key) = params.get("client_request_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        let records = self
            .idempotency
            .read()
            .map_err(|_| FeatureError::Invalid("idempotency lock poisoned".into()))?;
        let Some(record) = records.get(key) else {
            return Ok(None);
        };
        if record.method != method {
            return Err(FeatureError::Conflict(format!(
                "client_request_id {key} was used for {}",
                record.method
            )));
        }
        Ok(Some(FeatureResponse {
            result: record.result.clone(),
            events: Vec::new(),
        }))
    }

    fn save_idempotency(&self, method: &str, params: &Value, result: &Value) -> FeatureResult<()> {
        let Some(key) = params.get("client_request_id").and_then(Value::as_str) else {
            return Ok(());
        };
        let record = IdempotencyRecord {
            key: key.to_string(),
            method: method.to_string(),
            signature: signature(method, params),
            result: result.clone(),
        };
        let mut records = self
            .idempotency
            .write()
            .map_err(|_| FeatureError::Invalid("idempotency lock poisoned".into()))?;
        records.insert(key.to_string(), record);
        self.shared_store.write_snapshot(
            IDEMPOTENCY_FILE,
            &records.values().cloned().collect::<Vec<_>>(),
        )?;
        Ok(())
    }

    pub fn begin_memory_run(&self, run_id: &str) -> FeatureResult<()> {
        self.shared_memory.begin_run(run_id)?;
        Ok(())
    }

    pub fn stage_memory(
        &self,
        actor: &MemoryActor,
        run_id: &str,
        request: MemoryRequest,
    ) -> FeatureResult<()> {
        self.stage_memory_with_access(actor, &MemoryAccess::default(), run_id, request)
    }

    pub fn stage_memory_with_access(
        &self,
        actor: &MemoryActor,
        access: &MemoryAccess,
        run_id: &str,
        request: MemoryRequest,
    ) -> FeatureResult<()> {
        authorize_memory(actor, access, &request.target, &request.action)?;
        self.shared_memory.stage(run_id, request)?;
        Ok(())
    }

    pub fn commit_memory_run(&self, run_id: &str) -> FeatureResult<Vec<MemoryEntry>> {
        let entries = self
            .shared_memory
            .commit_run_persisted(run_id, &self.memory_path)?;
        for entry in &entries {
            self.shared_store
                .append_event("memory.updated", json!({"entry":entry}))?;
        }
        Ok(entries)
    }

    pub fn rollback_memory_run(&self, run_id: &str) -> FeatureResult<bool> {
        Ok(self.shared_memory.rollback_run(run_id)?)
    }

    /// Complete a successful execution and append its durable Bot worklog in
    /// the same staged commit. Callers can omit the worklog when a run had no
    /// durable outcome.
    pub fn complete_run_with_worklog(
        &self,
        run_id: &str,
        target: Option<MemoryTarget>,
        worklog: Option<&str>,
        source: MemorySource,
    ) -> FeatureResult<Vec<MemoryEntry>> {
        if let (Some(target), Some(worklog)) = (target, worklog) {
            self.shared_memory.stage(
                run_id,
                MemoryRequest {
                    target,
                    action: MemoryAction::Add,
                    content: worklog.to_string(),
                    id: None,
                    kind: Some(MemoryKind::BotWorklog),
                    source,
                },
            )?;
        }
        self.commit_memory_run(run_id)
    }

    pub fn memory_rpc(&self, actor: &MemoryActor, params: Value) -> FeatureResult<Value> {
        self.memory_rpc_with_access(actor, &MemoryAccess::default(), params)
    }

    pub fn memory_rpc_with_access(
        &self,
        actor: &MemoryActor,
        access: &MemoryAccess,
        params: Value,
    ) -> FeatureResult<Value> {
        let target = memory_target(&params)?;
        let action = params
            .get("action")
            .cloned()
            .map(|value| {
                serde_json::from_value(value).map_err(|e| FeatureError::Invalid(e.to_string()))
            })
            .transpose()?
            .unwrap_or(MemoryAction::Add);
        let mut source = serde_json::from_value::<MemorySource>(
            params.get("source").cloned().unwrap_or_else(|| json!({})),
        )
        .map_err(|e| FeatureError::Invalid(e.to_string()))?;
        let run_id = params
            .get("run_id")
            .and_then(Value::as_str)
            .ok_or_else(|| FeatureError::Invalid("memory writes require run_id".into()))?;
        if source.bot_id.is_none() {
            source.bot_id = actor.bot_id().map(str::to_string);
        }
        if source.run_id.is_none() {
            source.run_id = Some(run_id.to_string());
        }
        let request = MemoryRequest {
            target,
            action,
            content: params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            id: params.get("id").and_then(Value::as_str).map(str::to_string),
            kind: params
                .get("kind")
                .cloned()
                .map(|value| {
                    serde_json::from_value(value).map_err(|e| FeatureError::Invalid(e.to_string()))
                })
                .transpose()?,
            source,
        };
        self.stage_memory_with_access(actor, access, run_id, request)?;
        Ok(json!({"staged":true,"run_id":run_id}))
    }

    pub fn memory_search(
        &self,
        query: &str,
        target: Option<&MemoryTarget>,
    ) -> FeatureResult<Vec<MemoryEntry>> {
        Ok(self.shared_memory.search(query, target)?)
    }

    pub fn add_session_message(&self, message: SessionMessage) -> FeatureResult<()> {
        let mut conversations = self
            .conversations
            .write()
            .map_err(|_| FeatureError::Invalid("conversation lock poisoned".into()))?;
        conversations.add_message(message);
        self.shared_store
            .write_snapshot(CONVERSATIONS_FILE, &*conversations)?;
        Ok(())
    }

    pub fn add_project_record(&self, project: ProjectRecord) -> FeatureResult<()> {
        let mut conversations = self
            .conversations
            .write()
            .map_err(|_| FeatureError::Invalid("conversation lock poisoned".into()))?;
        conversations.add_project(project);
        self.shared_store
            .write_snapshot(CONVERSATIONS_FILE, &*conversations)?;
        Ok(())
    }

    pub fn session_search(
        &self,
        session_id: &str,
        query: &str,
    ) -> FeatureResult<Vec<SessionMessage>> {
        Ok(self
            .conversations
            .read()
            .map_err(|_| FeatureError::Invalid("conversation lock poisoned".into()))?
            .session_search(session_id, query))
    }

    pub fn project_find(&self, query: &str) -> FeatureResult<Vec<ProjectRecord>> {
        Ok(self
            .conversations
            .read()
            .map_err(|_| FeatureError::Invalid("conversation lock poisoned".into()))?
            .project_find(query))
    }

    pub fn chat_history(&self, chat_id: &str, limit: usize) -> FeatureResult<Vec<SessionMessage>> {
        Ok(self
            .conversations
            .read()
            .map_err(|_| FeatureError::Invalid("conversation lock poisoned".into()))?
            .chat_history(chat_id, limit.min(100)))
    }

    pub fn context(&self, request: &ContextRequest) -> FeatureResult<ContextPackage> {
        Ok(self.shared_memory.assemble_context(request)?)
    }

    pub fn l0_platform_rules(&self) -> &'static str {
        default_l0_platform_rules()
    }

    pub async fn maintain_worklog(
        &self,
        target: &MemoryTarget,
    ) -> FeatureResult<Option<MemoryEntry>> {
        if target.scope != macbot_memory::MemoryScope::Bot {
            return Err(FeatureError::Invalid(
                "automatic maintenance only accepts private Bot worklogs".into(),
            ));
        }
        let provider = self
            .maintenance
            .read()
            .map_err(|_| FeatureError::Invalid("maintenance lock poisoned".into()))?
            .clone()
            .ok_or_else(|| {
                FeatureError::Provider("maintenance provider is not configured".into())
            })?;
        let rows = self
            .shared_memory
            .entries()?
            .into_iter()
            .filter(|entry| entry.target == *target && entry.kind == MemoryKind::BotWorklog)
            .collect::<Vec<_>>();
        if rows.len() < 2 {
            return Ok(None);
        }
        let now = chrono::Utc::now();
        if !self.reserve_maintenance(&target.owner_id, now)? {
            return Ok(None);
        }
        let max_input_chars = self
            .maintenance_policy
            .read()
            .map_err(|_| FeatureError::Invalid("maintenance policy lock poisoned".into()))?
            .max_input_chars;
        let transcript = rows
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let transcript: String = transcript.chars().take(max_input_chars).collect();
        let summary = match provider.summarize(&transcript).await {
            Ok(summary) => summary,
            Err(error) => {
                self.release_maintenance(&target.owner_id)?;
                return Err(error.into());
            }
        };
        // The unique run id prevents collision with an in-flight model run.
        let run_id = format!("maintenance-{}", uuid::Uuid::now_v7());
        let commit = (|| -> FeatureResult<Vec<MemoryEntry>> {
            self.begin_memory_run(&run_id)?;
            for row in rows {
                self.shared_memory.stage(
                    &run_id,
                    MemoryRequest {
                        target: row.target,
                        action: MemoryAction::Remove,
                        content: String::new(),
                        id: Some(row.id),
                        kind: Some(MemoryKind::BotWorklog),
                        source: MemorySource::default(),
                    },
                )?;
            }
            self.shared_memory.stage(
                &run_id,
                MemoryRequest {
                    target: target.clone(),
                    action: MemoryAction::Add,
                    content: summary,
                    id: None,
                    kind: Some(MemoryKind::BotWorklog),
                    source: MemorySource::default(),
                },
            )?;
            self.commit_memory_run(&run_id)
        })();
        let entries = match commit {
            Ok(entries) => entries,
            Err(error) => {
                let _ = self.rollback_memory_run(&run_id);
                self.release_maintenance(&target.owner_id)?;
                return Err(error);
            }
        };
        Ok(entries.into_iter().find(|entry| entry.target == *target))
    }

    /// Returns whether the scheduler should run private worklog maintenance.
    /// This is deliberately a hook rather than a background task: the gateway
    /// owns lifecycle and can call it from its daily maintenance tick.
    pub fn maintenance_due(
        &self,
        target: &MemoryTarget,
        now: chrono::DateTime<chrono::Utc>,
    ) -> FeatureResult<bool> {
        if target.scope != macbot_memory::MemoryScope::Bot {
            return Ok(false);
        }
        let state = self
            .maintenance_state
            .read()
            .map_err(|_| FeatureError::Invalid("maintenance state lock poisoned".into()))?;
        let Some(usage) = state.usage.get(&target.owner_id) else {
            return Ok(true);
        };
        let policy = self
            .maintenance_policy
            .read()
            .map_err(|_| FeatureError::Invalid("maintenance policy lock poisoned".into()))?;
        let today = now.date_naive().to_string();
        Ok(usage.day != today
            || (usage.calls < policy.daily_max_calls
                && now.signed_duration_since(usage.last_run).num_seconds()
                    >= policy.min_interval_seconds))
    }

    pub async fn run_maintenance_if_due(
        &self,
        target: &MemoryTarget,
    ) -> FeatureResult<Option<MemoryEntry>> {
        if self.maintenance_due(target, chrono::Utc::now())? {
            self.maintain_worklog(target).await
        } else {
            Ok(None)
        }
    }

    /// Ask the configured provider to compact only when context assembly
    /// crossed the 80% segment boundary. This path receives the current
    /// context explicitly; automatic maintenance never reads private chats.
    pub async fn compact_context(&self, request: &ContextRequest) -> FeatureResult<Option<String>> {
        let package = self.context(request)?;
        if package.segment_reason.as_deref() != Some("context_80_percent") {
            return Ok(None);
        }
        let provider = self
            .maintenance
            .read()
            .map_err(|_| FeatureError::Invalid("maintenance lock poisoned".into()))?
            .clone()
            .ok_or_else(|| {
                FeatureError::Provider("maintenance provider is not configured".into())
            })?;
        let text = package
            .layers
            .iter()
            .filter(|layer| layer.level >= 3)
            .map(|layer| layer.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Some(provider.summarize(&text).await?))
    }

    fn reserve_maintenance(
        &self,
        owner_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> FeatureResult<bool> {
        let policy = self
            .maintenance_policy
            .read()
            .map_err(|_| FeatureError::Invalid("maintenance policy lock poisoned".into()))?
            .clone();
        let mut state = self
            .maintenance_state
            .write()
            .map_err(|_| FeatureError::Invalid("maintenance state lock poisoned".into()))?;
        let day = now.date_naive().to_string();
        let usage = state
            .usage
            .entry(owner_id.to_string())
            .or_insert_with(|| MaintenanceUsage {
                day: day.clone(),
                calls: 0,
                last_run: now - chrono::Duration::seconds(policy.min_interval_seconds),
            });
        if usage.day != day {
            usage.day = day;
            usage.calls = 0;
        }
        if usage.calls >= policy.daily_max_calls
            || now.signed_duration_since(usage.last_run).num_seconds() < policy.min_interval_seconds
        {
            return Ok(false);
        }
        usage.calls += 1;
        usage.last_run = now;
        self.shared_store
            .write_snapshot(MAINTENANCE_FILE, &*state)?;
        Ok(true)
    }

    fn release_maintenance(&self, owner_id: &str) -> FeatureResult<()> {
        let mut state = self
            .maintenance_state
            .write()
            .map_err(|_| FeatureError::Invalid("maintenance state lock poisoned".into()))?;
        if let Some(usage) = state.usage.get_mut(owner_id) {
            usage.calls = usage.calls.saturating_sub(1);
        }
        self.shared_store
            .write_snapshot(MAINTENANCE_FILE, &*state)?;
        Ok(())
    }

    pub fn search(
        &self,
        query: &str,
        kinds: &[String],
        limit: usize,
    ) -> FeatureResult<Vec<SearchHit>> {
        let limit = limit.clamp(1, 100);
        let filter = kinds.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut hits = Vec::new();
        collect_search_files(
            &self.shared_store.root().join("data"),
            query,
            &filter,
            limit,
            &mut hits,
        )?;
        Ok(hits)
    }

    pub fn search_rpc(&self, params: Value) -> FeatureResult<Value> {
        let query = required_string(&params, "query")?;
        let kinds = params
            .get("kinds")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        Ok(json!({"results":self.search(query, &kinds, limit)?}))
    }
}

#[async_trait]
pub trait ExecutionSink: Send + Sync {
    async fn run_started(&self, run_id: &str) -> FeatureResult<()>;
    async fn run_succeeded(&self, run_id: &str) -> FeatureResult<Vec<MemoryEntry>>;
    async fn run_failed(&self, run_id: &str) -> FeatureResult<bool>;
}

#[async_trait]
impl ExecutionSink for FeatureService {
    async fn run_started(&self, run_id: &str) -> FeatureResult<()> {
        self.begin_memory_run(run_id)
    }

    async fn run_succeeded(&self, run_id: &str) -> FeatureResult<Vec<MemoryEntry>> {
        self.commit_memory_run(run_id)
    }

    async fn run_failed(&self, run_id: &str) -> FeatureResult<bool> {
        self.rollback_memory_run(run_id)
    }
}

pub trait ContextProvider: Send + Sync {
    fn context(&self, request: &ContextRequest) -> FeatureResult<ContextPackage>;
}

impl ContextProvider for FeatureService {
    fn context(&self, request: &ContextRequest) -> FeatureResult<ContextPackage> {
        FeatureService::context(self, request)
    }
}

/// Adapter used by idle maintenance and compaction. It deliberately depends
/// on `ModelProvider`, so tests can inject a deterministic provider and the
/// production gateway can inject `HttpProvider` without this crate owning keys.
pub struct ModelMaintenanceAdapter {
    provider: Arc<dyn ModelProvider>,
    model: String,
}

impl ModelMaintenanceAdapter {
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    pub fn from_http(
        config: ProviderConfig,
        secrets: Arc<dyn SecretStore>,
        model: impl Into<String>,
    ) -> Self {
        Self::new(Arc::new(HttpProvider::new(config, secrets)), model)
    }

    async fn complete(&self, system: &str, text: &str) -> Result<Completion, FeatureError> {
        self.provider
            .complete(ModelRequest {
                model: self.model.clone(),
                messages: vec![
                    json!({"role":"system","content":system}),
                    json!({"role":"user","content":text}),
                ],
                tools: Vec::new(),
                max_output: 4096,
                session_id: None,
            })
            .await
            .map_err(|error| FeatureError::Provider(error.to_string()))
    }
}

#[async_trait]
impl AsyncMaintenanceProvider for ModelMaintenanceAdapter {
    async fn summarize(&self, text: &str) -> Result<String, MemoryError> {
        self.complete(
            "Summarize the durable work log. Preserve decisions, facts, and unresolved items.",
            text,
        )
        .await
        .map(|completion| completion.text)
        .map_err(|error| MemoryError::Provider(error.to_string()))
    }

    async fn extract(&self, text: &str) -> Result<Vec<macbot_memory::MemoryDraft>, MemoryError> {
        let completion = self
            .complete(
                "Extract only durable memories as a JSON array with target, kind, and content fields. Return [] when none.",
                text,
            )
            .await
            .map_err(|error| MemoryError::Provider(error.to_string()))?;
        serde_json::from_str(&completion.text)
            .map_err(|error| MemoryError::Provider(format!("maintenance JSON: {error}")))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchHit {
    pub kind: String,
    pub id: String,
    pub chat_id: Option<String>,
    pub title: String,
    pub snippet: String,
    pub at: Option<String>,
}

fn required_string<'a>(value: &'a Value, key: &str) -> FeatureResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FeatureError::Invalid(format!("{key} is required")))
}

fn signature(method: &str, params: &Value) -> String {
    let mut params = params.clone();
    if let Some(object) = params.as_object_mut() {
        object.remove("client_request_id");
    }
    format!("{method}:{params}")
}

fn memory_target(params: &Value) -> FeatureResult<MemoryTarget> {
    let scope = params
        .get("scope")
        .and_then(Value::as_str)
        .ok_or_else(|| FeatureError::Invalid("scope is required".into()))?;
    let owner = match scope {
        "user" => params
            .get("user_id")
            .or_else(|| params.get("owner_id"))
            .and_then(Value::as_str)
            .unwrap_or("user"),
        "bot" => required_string(params, "bot_id")?,
        "project" => required_string(params, "project_id")?,
        _ => {
            return Err(FeatureError::Invalid(format!(
                "unknown memory scope: {scope}"
            )))
        }
    };
    Ok(match scope {
        "user" => MemoryTarget::user(owner),
        "bot" => MemoryTarget::bot(owner),
        "project" => MemoryTarget::project(owner),
        _ => unreachable!(),
    })
}

fn authorize_memory(
    actor: &MemoryActor,
    access: &MemoryAccess,
    target: &MemoryTarget,
    action: &MemoryAction,
) -> FeatureResult<()> {
    if matches!(action, MemoryAction::Remove | MemoryAction::Replace) && target.owner_id.is_empty()
    {
        return Err(FeatureError::Invalid("memory owner is required".into()));
    }
    if actor.is_main() {
        return Ok(());
    }
    match target.scope {
        macbot_memory::MemoryScope::Bot if actor.bot_id() == Some(target.owner_id.as_str()) => {}
        macbot_memory::MemoryScope::User
            if access.user_id.as_deref() == Some(target.owner_id.as_str()) => {}
        macbot_memory::MemoryScope::Project
            if access.project_id.as_deref() == Some(target.owner_id.as_str())
                && actor.bot_id().is_some_and(|bot| {
                    access.project_member_bot_ids.iter().any(|id| id == bot)
                }) => {}
        macbot_memory::MemoryScope::Bot => {
            return Err(FeatureError::Invalid(
                "a Bot may only write its own private memory".into(),
            ))
        }
        macbot_memory::MemoryScope::User => {
            return Err(FeatureError::Invalid(
                "user memory requires the authenticated user context".into(),
            ))
        }
        macbot_memory::MemoryScope::Project => {
            return Err(FeatureError::Invalid(
                "project memory requires group membership".into(),
            ))
        }
    }
    Ok(())
}

fn upload_path(home: &Path, id: &str) -> FeatureResult<PathBuf> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(FeatureError::Invalid("unsafe upload_id".into()));
    }
    let path = home.join("uploads").join(id);
    if !path.starts_with(home.join("uploads")) {
        return Err(FeatureError::Invalid("upload path escapes home".into()));
    }
    Ok(path)
}

fn collect_search_files(
    root: &Path,
    query: &str,
    kinds: &HashSet<&str>,
    limit: usize,
    hits: &mut Vec<SearchHit>,
) -> FeatureResult<()> {
    if hits.len() >= limit || !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if hits.len() >= limit {
            break;
        }
        if path.is_dir() {
            collect_search_files(&path, query, kinds, limit, hits)?;
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            for line in fs::read_to_string(&path)?.lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    collect_search_value(&value, &path, query, kinds, limit, hits);
                }
                if hits.len() >= limit {
                    break;
                }
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            if let Ok(value) = serde_json::from_slice::<Value>(&fs::read(&path)?) {
                collect_search_value(&value, &path, query, kinds, limit, hits);
            }
        }
    }
    Ok(())
}

fn collect_search_value(
    value: &Value,
    path: &Path,
    query: &str,
    kinds: &HashSet<&str>,
    limit: usize,
    hits: &mut Vec<SearchHit>,
) {
    if hits.len() >= limit {
        return;
    }
    if let Some(object) = value.as_object() {
        let kind = search_kind(object, path);
        let serialized = serde_json::to_string(value).unwrap_or_default();
        if serialized.to_lowercase().contains(&query.to_lowercase())
            && (kinds.is_empty() || kinds.contains(kind.as_str()))
        {
            let id = object
                .get("id")
                .or_else(|| object.get("message_id"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|x| x.to_str())
                        .unwrap_or("object")
                });
            let title = object
                .get("title")
                .or_else(|| object.get("name"))
                .or_else(|| object.get("fallback_text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let at = object
                .get("at")
                .or_else(|| object.get("created_at"))
                .or_else(|| object.get("updated_at"))
                .and_then(Value::as_str)
                .map(str::to_string);
            hits.push(SearchHit {
                kind,
                id: id.to_string(),
                chat_id: object
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                title,
                snippet: serialized.chars().take(240).collect(),
                at,
            });
        }
        for child in object.values() {
            collect_search_value(child, path, query, kinds, limit, hits);
        }
    } else if let Some(items) = value.as_array() {
        for child in items {
            collect_search_value(child, path, query, kinds, limit, hits);
        }
    }
}

fn search_kind(object: &serde_json::Map<String, Value>, path: &Path) -> String {
    if object.contains_key("fallback_text") || object.contains_key("blocks") {
        return "message".into();
    }
    if object.contains_key("member_bot_ids") || object.contains_key("last_message") {
        return "chat".into();
    }
    if object.contains_key("dm_chat_id") || object.contains_key("max_parallel") {
        return "bot".into();
    }
    if object.contains_key("path_or_url") || object.contains_key("artifact_id") {
        return "artifact".into();
    }
    if object.contains_key("schedules") || object.contains_key("routine_id") {
        return "routine".into();
    }
    let path = path.to_string_lossy();
    for kind in ["message", "chat", "bot", "artifact", "routine"] {
        if path.contains(kind) {
            return kind.into();
        }
    }
    "object".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use macbot_memory::MemoryScope;
    use macbot_providers::{ModelEvent, TokenUsage};
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    fn service() -> FeatureService {
        let home = tempdir().unwrap().keep();
        FeatureService::open(home, Vec::<PathBuf>::new()).unwrap()
    }

    fn skill(name: &str) -> String {
        format!("---\nname: {name}\ndescription: test\n---\n# test")
    }

    #[test]
    fn skill_mutations_persist_events_and_replay() {
        let service = service();
        let first = service
            .skill_rpc(
                "skill.create",
                json!({"name":"demo","content":skill("demo"),"client_request_id":"r1"}),
            )
            .unwrap();
        assert_eq!(first.events.len(), 1);
        let replay = service
            .skill_rpc(
                "skill.create",
                json!({"name":"demo","content":"changed","client_request_id":"r1"}),
            )
            .unwrap();
        assert_eq!(first.result, replay.result);
        assert!(replay.events.is_empty());
        assert_eq!(service.shared_store.events_since(0).unwrap().len(), 1);
    }

    #[test]
    fn memory_actor_permissions_and_context_hook() {
        let service = service();
        service.begin_memory_run("run-1").unwrap();
        service
            .stage_memory(
                &MemoryActor::bot("bot-a"),
                "run-1",
                MemoryRequest {
                    target: MemoryTarget::bot("bot-a"),
                    action: MemoryAction::Add,
                    content: "fact".into(),
                    id: None,
                    kind: Some(MemoryKind::BotExperience),
                    source: MemorySource::default(),
                },
            )
            .unwrap();
        assert!(service
            .stage_memory(
                &MemoryActor::bot("bot-a"),
                "run-1",
                MemoryRequest {
                    target: MemoryTarget::bot("bot-b"),
                    action: MemoryAction::Add,
                    content: "denied".into(),
                    id: None,
                    kind: Some(MemoryKind::BotExperience),
                    source: MemorySource::default(),
                },
            )
            .is_err());
        service.commit_memory_run("run-1").unwrap();
        let package = service
            .context(&ContextRequest {
                kind: macbot_memory::ConversationKind::Private,
                memory_targets: vec![MemoryTarget::bot("bot-a")],
                l0_platform_rules: "rules".into(),
                l1_bot_identity: "bot".into(),
                announcement: String::new(),
                task: "task".into(),
                trigger: String::new(),
                references: Vec::new(),
                recent_messages: Vec::new(),
                segment_summary: String::new(),
                recent_context: Vec::new(),
                run_events: Vec::new(),
                model_context_window: 4096,
                previous_snapshot: None,
                now: Utc::now(),
            })
            .unwrap();
        assert_eq!(package.layers[0].level, 0);
        assert!(package
            .snapshot
            .entries
            .iter()
            .any(|entry| { entry.target.scope == MemoryScope::Bot && entry.content == "fact" }));
    }

    struct FakeMaintenance;

    #[async_trait]
    impl ModelProvider for FakeMaintenance {
        async fn stream(
            &self,
            _request: ModelRequest,
            _events: mpsc::Sender<ModelEvent>,
        ) -> macbot_providers::Result<Completion> {
            Ok(Completion {
                text: "summarized work".into(),
                thinking: String::new(),
                tool_calls: Vec::new(),
                usage: TokenUsage::default(),
                stop_reason: "stop".into(),
            })
        }
    }

    #[tokio::test]
    async fn maintenance_adapter_uses_model_provider_and_commits_summary() {
        let service = service();
        service
            .shared_memory
            .append_worklog(MemoryTarget::bot("bot-a"), "first", MemorySource::default())
            .unwrap();
        service
            .shared_memory
            .append_worklog(
                MemoryTarget::bot("bot-a"),
                "second",
                MemorySource::default(),
            )
            .unwrap();
        service
            .set_maintenance_provider(Some(Arc::new(ModelMaintenanceAdapter::new(
                Arc::new(FakeMaintenance),
                "mock-model",
            ))))
            .unwrap();
        let result = service
            .maintain_worklog(&MemoryTarget::bot("bot-a"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.content, "summarized work");
        assert!(service
            .maintain_worklog(&MemoryTarget::bot("bot-a"))
            .await
            .unwrap()
            .is_none());
        assert!(service
            .maintain_worklog(&MemoryTarget::user("user-a"))
            .await
            .is_err());
    }

    #[test]
    fn memory_access_separates_user_group_and_private_scopes() {
        let service = service();
        service.begin_memory_run("access").unwrap();
        let actor = MemoryActor::bot("bot-a");
        let user = MemoryRequest {
            target: MemoryTarget::user("u"),
            action: MemoryAction::Add,
            content: "preference".into(),
            id: None,
            kind: Some(MemoryKind::UserPreference),
            source: MemorySource::default(),
        };
        assert!(service
            .stage_memory_with_access(&actor, &MemoryAccess::user("u"), "access", user)
            .is_ok());
        let project = MemoryRequest {
            target: MemoryTarget::project("p"),
            action: MemoryAction::Add,
            content: "group decision".into(),
            id: None,
            kind: Some(MemoryKind::Project),
            source: MemorySource::default(),
        };
        assert!(service
            .stage_memory_with_access(
                &actor,
                &MemoryAccess::group("p", vec!["bot-b".into()]),
                "access",
                project.clone()
            )
            .is_err());
        assert!(service
            .stage_memory_with_access(
                &actor,
                &MemoryAccess::group("p", vec!["bot-a".into()]),
                "access",
                project
            )
            .is_ok());
        service.commit_memory_run("access").unwrap();
        assert_eq!(service.shared_memory.entries().unwrap().len(), 2);
    }

    #[test]
    fn search_returns_typed_hits_for_all_durable_object_kinds() {
        let service = service();
        service
            .shared_store
            .write_snapshot(
                "data/messages/messages.json",
                &json!({"message_id":"m1","chat_id":"c1","fallback_text":"needle message","created_at":"2026-01-01T00:00:00Z"}),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/artifacts/artifact.json",
                &json!({"artifact_id":"a1","title":"needle artifact","path_or_url":"/tmp/a","created_at":"2026-01-01T00:00:00Z"}),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/routines/routine.json",
                &json!({"routine_id":"r1","name":"needle routine","schedules":[]}),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/objects/object.json",
                &json!({"id":"o1","title":"needle object"}),
            )
            .unwrap();
        let result = service
            .search_rpc(json!({"query":"needle","limit":10}))
            .unwrap();
        let hits: Vec<SearchHit> = serde_json::from_value(result["results"].clone()).unwrap();
        let kinds = hits
            .iter()
            .map(|hit| hit.kind.as_str())
            .collect::<HashSet<_>>();
        assert!(kinds.contains("message"));
        assert!(kinds.contains("artifact"));
        assert!(kinds.contains("routine"));
        assert!(kinds.contains("object"));
        assert!(hits
            .iter()
            .any(|hit| hit.id == "m1" && hit.chat_id.as_deref() == Some("c1")));
        let filtered = service
            .search_rpc(json!({"query":"needle","kinds":["artifact"],"limit":1}))
            .unwrap();
        let typed: Vec<SearchHit> = serde_json::from_value(filtered["results"].clone()).unwrap();
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].kind, "artifact");
    }

    #[test]
    fn conversation_index_is_durable() {
        let service = service();
        service
            .add_session_message(SessionMessage {
                id: "m".into(),
                session_id: "s".into(),
                chat_id: "c".into(),
                project_id: None,
                role: "user".into(),
                content: "durable".into(),
                at: Utc::now(),
            })
            .unwrap();
        let persisted: ConversationIndex = service
            .shared_store
            .read_snapshot(CONVERSATIONS_FILE)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.session_search("s", "durable").len(), 1);
    }
}
