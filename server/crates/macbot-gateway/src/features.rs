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
    Completion, HttpProvider, ModelProvider, ModelRequest, ProviderConfig, SecretStore, TokenUsage,
};
use macbot_skills::{BotSkillSettingsSnapshot, Skill, SkillDraftFrom, SkillError, SkillRegistry};
use macbot_store::{Event, Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use thiserror::Error;
use uuid::Uuid;

const IDEMPOTENCY_FILE: &str = "data/features/idempotency.json";
const SKILLS_FILE: &str = "data/features/skills.json";
const CONVERSATIONS_FILE: &str = "data/features/conversations.json";
const MAINTENANCE_FILE: &str = "data/features/maintenance.json";
const PROFILE_ONBOARDING_FILE: &str = "data/features/profile_onboarding.json";

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
    #[error("user memory version conflict")]
    UserMemoryConflict(Value),
    #[error("user profile version conflict")]
    UserProfileConflict(Value),
    #[error("memory entry version conflict")]
    MemoryMutationConflict(Value),
    #[error("memory quota is full")]
    MemoryToolQuotaFull(Value),
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
            Self::Skill(SkillError::Conflict(_))
            | Self::Conflict(_)
            | Self::UserMemoryConflict(_)
            | Self::UserProfileConflict(_)
            | Self::MemoryMutationConflict(_)
            | Self::Memory(MemoryError::UserConflict(_))
            | Self::Memory(MemoryError::ProfileConflict(_)) => "conflict",
            Self::Skill(SkillError::BuiltinReadOnly) => "forbidden",
            Self::Memory(MemoryError::QuotaExceeded { .. }) => "quota_exceeded",
            Self::Memory(MemoryError::UserQuotaFull { .. }) => "quota_full",
            Self::MemoryToolQuotaFull(_) => "quota_full",
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

/// Result of one maintenance scheduler pass.  Each event is already durable;
/// an error from a later target must not hide events committed by earlier
/// targets in the same pass.
pub struct MaintenanceTickReport {
    pub committed: Vec<MemoryEntry>,
    pub events: Vec<Event>,
    pub error: Option<FeatureError>,
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
pub use macbot_memory::MemoryAccess;

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

    pub(crate) fn bot_id(&self) -> Option<&str> {
        match self {
            Self::MainBot { bot_id } | Self::Bot { bot_id } => Some(bot_id),
            Self::System => None,
        }
    }

    pub(crate) fn is_main(&self) -> bool {
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
    profile_onboarding: Arc<Mutex<ProfileOnboardingState>>,
    project_finalize_lock: Arc<Mutex<()>>,
}

/// The one feature state handle shared by gateway RPCs, a model run and
/// background maintenance.  Keeping this alias public makes the ownership
/// boundary explicit for the runtime without exposing the internal locks.
pub type SharedFeatureService = Arc<FeatureService>;

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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ProfileOnboardingState {
    initial_prompted: bool,
    reminder_sent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileOnboardingPrompt {
    Initialize,
    Reminder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileOnboardingStatus {
    pub available: bool,
    pub prompt: Option<ProfileOnboardingPrompt>,
}

pub struct UserProfileOnboardingInput {
    pub run_id: String,
    pub bot_id: String,
    pub expected_version: u64,
    pub name: Option<String>,
    pub language: Option<String>,
    pub identity: Option<String>,
    pub skip: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MaintenanceUsage {
    day: String,
    calls: u32,
    last_run: chrono::DateTime<chrono::Utc>,
}

impl FeatureService {
    /// Open one shared feature service for a gateway process.  Runtime code
    /// should clone the returned `Arc` into every run instead of reopening the
    /// store, which would create independent staged-memory and skill indexes.
    pub fn open_shared(
        home: impl Into<PathBuf>,
        extra_dirs: impl IntoIterator<Item = PathBuf>,
    ) -> FeatureResult<SharedFeatureService> {
        Ok(Arc::new(Self::open(home, extra_dirs)?))
    }

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
        if let Some(saved_skills) = shared_store.read_snapshot::<Vec<Skill>>(SKILLS_FILE)? {
            registry.restore_metadata(&saved_skills);
        }
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
        let profile_onboarding = shared_store
            .read_snapshot::<ProfileOnboardingState>(PROFILE_ONBOARDING_FILE)?
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
            profile_onboarding: Arc::new(Mutex::new(profile_onboarding)),
            project_finalize_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Initialize the local user's profile and claim the one-time onboarding
    /// prompt/reminder for an authenticated main-Bot private-chat request.
    /// The caller must establish that the request is a direct user chat.
    pub fn profile_onboarding_status(
        &self,
        host_timezone: &str,
    ) -> FeatureResult<ProfileOnboardingStatus> {
        let profile = self
            .shared_memory
            .user_profile_get_or_init(&self.memory_path, host_timezone)?;
        if profile.onboarded {
            return Ok(ProfileOnboardingStatus {
                available: false,
                prompt: None,
            });
        }

        let mut state = self
            .profile_onboarding
            .lock()
            .map_err(|_| FeatureError::Invalid("profile onboarding lock poisoned".into()))?;
        let mut candidate = state.clone();
        let prompt = if !candidate.initial_prompted {
            candidate.initial_prompted = true;
            Some(ProfileOnboardingPrompt::Initialize)
        } else if !candidate.reminder_sent {
            candidate.reminder_sent = true;
            Some(ProfileOnboardingPrompt::Reminder)
        } else {
            None
        };
        if prompt.is_some() {
            self.shared_store
                .write_snapshot(PROFILE_ONBOARDING_FILE, &candidate)?;
            *state = candidate;
        }
        Ok(ProfileOnboardingStatus {
            available: true,
            prompt,
        })
    }

    pub fn profile_needs_first_connection_greeting(
        &self,
        host_timezone: &str,
    ) -> FeatureResult<bool> {
        let profile = self
            .shared_memory
            .user_profile_get_or_init(&self.memory_path, host_timezone)?;
        Ok(!profile.onboarded
            && profile.name.is_none()
            && profile.language.is_none()
            && profile.identity.is_none())
    }

    pub fn user_profile_language(&self, host_timezone: &str) -> FeatureResult<Option<String>> {
        let profile = self
            .shared_memory
            .user_profile_get_or_init(&self.memory_path, host_timezone)?;
        Ok(profile.language)
    }

    /// Apply the model's completion or explicit skip from an eligible
    /// onboarding run. Ownership and actor are fixed by the gateway runtime.
    pub fn set_user_profile_onboarding(
        &self,
        input: UserProfileOnboardingInput,
    ) -> FeatureResult<FeatureResponse> {
        if !input.skip
            && input.name.is_none()
            && input.language.is_none()
            && input.identity.is_none()
        {
            return Err(FeatureError::Invalid(
                "provide at least one profile field or set skip=true".into(),
            ));
        }
        let params = json!({
            "client_request_id":format!("profile-onboarding:{}", input.run_id),
            "expected_version":input.expected_version,
            "name":input.name.clone(),
            "language":input.language.clone(),
            "identity":input.identity.clone(),
            "skip":input.skip,
        });
        self.transact_user_memory("memory.profile.onboard", &params, move |store| {
            let profile =
                store.apply_user_profile_onboarding(macbot_memory::UserProfileUpdate {
                    // Existing profile state retains its Host timezone. This is
                    // only a fallback for a malformed or legacy missing state.
                    timezone: "UTC".into(),
                    expected_version: input.expected_version,
                    name: if input.skip {
                        None
                    } else {
                        input.name.map(Some)
                    },
                    language: if input.skip {
                        None
                    } else {
                        input.language.map(Some)
                    },
                    identity: if input.skip {
                        None
                    } else {
                        input.identity.map(Some)
                    },
                    updated_by: macbot_memory::MemorySender::Bot {
                        bot_id: input.bot_id,
                    },
                })?;
            let profile = protocol_user_profile(&profile);
            let result = json!({"profile":profile});
            Ok((
                result.clone(),
                "memory.profile.updated".into(),
                json!({"profile":profile}),
            ))
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

    /// Apply the runtime-owned feature configuration to this shared handle.
    /// The caller may pass either setting independently; skill roots are
    /// rescanned before the provider is installed so a failed scan cannot
    /// leave a partially configured runtime.
    pub fn configure_runtime(
        &self,
        extra_dirs: impl IntoIterator<Item = PathBuf>,
        provider: Option<Arc<dyn AsyncMaintenanceProvider>>,
    ) -> FeatureResult<()> {
        self.set_skill_extra_dirs(extra_dirs)?;
        self.set_maintenance_provider(provider)
    }

    /// Poll skill roots after settings or filesystem changes. The index only
    /// reads frontmatter; full bodies remain on-demand.
    pub fn refresh_skills(&self) -> FeatureResult<Option<Vec<Skill>>> {
        let refreshed = self
            .skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .rescan_if_changed()?;
        if refreshed.is_some() {
            self.persist_skills()?;
        }
        Ok(refreshed)
    }

    pub fn set_skill_extra_dirs(
        &self,
        extra_dirs: impl IntoIterator<Item = PathBuf>,
    ) -> FeatureResult<Vec<Skill>> {
        let skills = self
            .skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .set_extra_dirs(extra_dirs)?;
        self.persist_skills()?;
        Ok(skills)
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
        self.create_skill_draft_with_origin(name, content, None)
    }

    pub fn create_skill_draft_with_origin(
        &self,
        name: &str,
        content: &str,
        draft_from: Option<SkillDraftFrom>,
    ) -> FeatureResult<Skill> {
        let skill = self
            .skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .create_draft_with_origin(name, content, draft_from)?;
        self.persist_skills()?;
        Ok(skill)
    }

    /// Record a model-side skill load and persist the rolling invocation
    /// counter. Full text is still loaded only by `SkillRegistry` on demand.
    pub fn record_skill_invocation(&self, name: &str, bot_id: Option<&str>) -> FeatureResult<()> {
        self.skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .record_invocation(name, bot_id)?;
        self.persist_skills()
    }

    /// Copy source Bot skill disablement settings for `bot.duplicate`.
    /// The Bot RPC owner remains responsible for copying Bot metadata and
    /// emitting its event; this hook persists only shared skill metadata.
    pub fn duplicate_bot_skill_settings(
        &self,
        source_bot_id: &str,
        target_bot_id: &str,
    ) -> FeatureResult<()> {
        self.prepare_duplicate_bot_skill_settings(source_bot_id, target_bot_id)
            .map(|_| ())
    }

    /// Apply duplicate skill settings and return a rollback snapshot. The
    /// snapshot is intentionally limited to per-Bot disablement metadata, so
    /// invocation counters and all memory/history remain untouched.
    pub fn prepare_duplicate_bot_skill_settings(
        &self,
        source_bot_id: &str,
        target_bot_id: &str,
    ) -> FeatureResult<BotSkillSettingsSnapshot> {
        let mut registry = self
            .skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?;
        let snapshot = registry.snapshot_bot_settings(target_bot_id)?;
        if let Err(error) = registry.copy_bot_settings(source_bot_id, target_bot_id) {
            let _ = registry.restore_bot_settings(&snapshot);
            return Err(error.into());
        }
        drop(registry);
        if let Err(error) = self.persist_skills() {
            let restore_result = self
                .skill_registry
                .write()
                .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))
                .and_then(|mut registry| {
                    registry.restore_bot_settings(&snapshot)?;
                    Ok(())
                });
            let _ = self.persist_skills();
            if let Err(restore_error) = restore_result {
                return Err(FeatureError::Invalid(format!(
                    "skill copy failed ({error}); rollback failed ({restore_error})"
                )));
            }
            return Err(error);
        }
        Ok(snapshot)
    }

    /// Restore a snapshot returned by `prepare_duplicate_bot_skill_settings`
    /// when the owning Bot transaction fails after skill metadata was saved.
    pub fn restore_duplicate_bot_skill_settings(
        &self,
        snapshot: &BotSkillSettingsSnapshot,
    ) -> FeatureResult<()> {
        self.skill_registry
            .write()
            .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
            .restore_bot_settings(snapshot)?;
        self.persist_skills()
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
        if method.starts_with("memory.") && record.signature != signature(method, params) {
            return Err(FeatureError::Conflict(format!(
                "client_request_id {key} was reused with different request content"
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
        self.commit_memory_run_with_events(run_id)
            .map(|(entries, _)| entries)
    }

    /// Commit staged memory and return the already durable public events so
    /// the gateway can fan them out without appending duplicate records.
    pub fn commit_memory_run_with_events(
        &self,
        run_id: &str,
    ) -> FeatureResult<(Vec<MemoryEntry>, Vec<Event>)> {
        let entries = self
            .shared_memory
            .commit_run_persisted(run_id, &self.memory_path)?;
        let mut events = Vec::with_capacity(entries.len());
        for entry in &entries {
            events.push(
                self.shared_store
                    .append_event("memory.updated", json!({"entry":entry}))?,
            );
        }
        Ok((entries, events))
    }

    pub fn rollback_memory_run(&self, run_id: &str) -> FeatureResult<bool> {
        Ok(self.shared_memory.rollback_run(run_id)?)
    }

    /// Persist the final project summary as an idempotent project-memory
    /// transaction. The Bot's worklog has its own quota and lifecycle; a
    /// project confirmation must not consume that quota or fail because it is
    /// full. `bot_id` remains part of the internal call shape and source
    /// metadata for compatibility with older callers, but is not the memory
    /// target scope.
    pub fn finalize_project_summary(
        &self,
        project_id: &str,
        bot_id: &str,
        summary: &str,
    ) -> FeatureResult<Vec<MemoryEntry>> {
        let project_id = project_id.trim();
        let bot_id = bot_id.trim();
        let summary = summary.trim();
        if project_id.is_empty() || bot_id.is_empty() || summary.is_empty() {
            return Err(FeatureError::Invalid(
                "project_id, bot_id and summary are required".into(),
            ));
        }
        let _guard = self
            .project_finalize_lock
            .lock()
            .map_err(|_| FeatureError::Invalid("project finalize lock poisoned".into()))?;
        let project_entry_id = format!("project-summary:{project_id}");
        let worklog_entry_id = format!("project-summary-worklog:{project_id}:main");
        let existing = self.shared_memory.entries()?;
        let existing_project = existing.iter().find(|entry| entry.id == project_entry_id);
        let existing_worklog = existing.iter().find(|entry| entry.id == worklog_entry_id);
        let worklog_can_be_saved = existing
            .iter()
            .filter(|entry| {
                entry.target == MemoryTarget::bot("main") && entry.kind == MemoryKind::BotWorklog
            })
            .map(|entry| entry.content.chars().count())
            .sum::<usize>()
            .saturating_sub(
                existing_worklog
                    .as_ref()
                    .map(|entry| entry.content.chars().count())
                    .unwrap_or_default(),
            )
            .saturating_add(summary.chars().count())
            <= macbot_memory::BOT_WORKLOG_LIMIT;
        if existing_project
            .as_ref()
            .is_some_and(|project| project.content == summary)
            && (!worklog_can_be_saved
                || existing_worklog
                    .as_ref()
                    .is_some_and(|worklog| worklog.content == summary))
        {
            let mut entries = vec![existing_project.expect("checked above").clone()];
            if worklog_can_be_saved {
                entries.push(existing_worklog.expect("checked above").clone());
            }
            return Ok(entries);
        }

        let run_id = format!("project-summary-run:{project_id}");
        self.begin_memory_run(&run_id)?;
        let source = MemorySource {
            bot_id: Some(bot_id.to_owned()),
            run_id: Some(run_id.clone()),
            session_id: None,
        };
        let request = MemoryRequest {
            target: MemoryTarget::project(project_id),
            action: if existing_project.is_some() {
                MemoryAction::Replace
            } else {
                MemoryAction::Add
            },
            content: summary.to_owned(),
            id: Some(project_entry_id),
            kind: Some(MemoryKind::Project),
            source,
        };
        if let Err(error) = self.shared_memory.stage(&run_id, request) {
            let _ = self.rollback_memory_run(&run_id);
            return Err(error.into());
        }
        if worklog_can_be_saved {
            let worklog = MemoryRequest {
                target: MemoryTarget::bot("main"),
                action: if existing_worklog.is_some() {
                    MemoryAction::Replace
                } else {
                    MemoryAction::Add
                },
                content: summary.to_owned(),
                id: Some(worklog_entry_id),
                kind: Some(MemoryKind::BotWorklog),
                source: MemorySource {
                    bot_id: Some("main".into()),
                    run_id: Some(run_id.clone()),
                    session_id: None,
                },
            };
            if let Err(error) = self.shared_memory.stage(&run_id, worklog) {
                let _ = self.rollback_memory_run(&run_id);
                return Err(error.into());
            }
        }
        match self.commit_memory_run(&run_id) {
            Ok(entries) => Ok(entries),
            Err(error) => {
                let _ = self.rollback_memory_run(&run_id);
                Err(error)
            }
        }
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
        self.complete_run_with_worklog_events(run_id, target, worklog, source)
            .map(|(entries, _)| entries)
    }

    pub fn complete_run_with_worklog_events(
        &self,
        run_id: &str,
        target: Option<MemoryTarget>,
        worklog: Option<&str>,
        source: MemorySource,
    ) -> FeatureResult<(Vec<MemoryEntry>, Vec<Event>)> {
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
        self.commit_memory_run_with_events(run_id)
    }

    /// Stage a compaction summary in the active model run. It is committed
    /// together with ordinary memory writes only after the run reaches done.
    pub fn stage_context_summary(
        &self,
        actor: &MemoryActor,
        access: &MemoryAccess,
        run_id: &str,
        target: MemoryTarget,
        summary: &str,
        source: MemorySource,
    ) -> FeatureResult<()> {
        if summary.trim().is_empty() {
            return Ok(());
        }
        self.stage_memory_with_access(
            actor,
            access,
            run_id,
            MemoryRequest {
                target,
                action: MemoryAction::Add,
                content: summary.to_owned(),
                id: None,
                kind: Some(MemoryKind::BotWorklog),
                source,
            },
        )
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
        let run_id = params
            .get("run_id")
            .and_then(Value::as_str)
            .unwrap_or("legacy-run");
        // Source is provenance, not model input.  Keep the optional JSON
        // field for wire compatibility, but ignore all caller-supplied
        // provenance and assign only authenticated identity/run ownership.
        let source = MemorySource {
            bot_id: actor.bot_id().map(str::to_string),
            run_id: Some(run_id.to_string()),
            session_id: None,
        };
        let request = MemoryRequest {
            target,
            action: action.clone(),
            content: params
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            id: if matches!(action, MemoryAction::Add) {
                None
            } else {
                params.get("id").and_then(Value::as_str).map(str::to_string)
            },
            kind: params
                .get("kind")
                .cloned()
                .map(|value| {
                    serde_json::from_value(value).map_err(|e| FeatureError::Invalid(e.to_string()))
                })
                .transpose()?,
            source,
        };
        if request.target.scope == macbot_memory::MemoryScope::User {
            return Err(FeatureError::Invalid(
                "user preferences are written only through the authenticated user RPC or curator"
                    .into(),
            ));
        }
        authorize_memory(actor, access, &request.target, &request.action)?;
        let expected_version = params
            .get("expected_version")
            .or_else(|| params.get("version"))
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    FeatureError::Invalid("expected_version must be a non-negative integer".into())
                })
            })
            .transpose()?;
        if matches!(request.action, MemoryAction::Replace | MemoryAction::Remove)
            && expected_version.is_none()
        {
            return Err(FeatureError::Invalid(
                "replace/remove require expected_version".into(),
            ));
        }
        let result =
            self.shared_memory
                .mutate_persisted(&self.memory_path, request.clone(), expected_version)
                .map_err(|error| match error {
                    MemoryError::VersionConflict { current, version } => {
                        FeatureError::MemoryMutationConflict(json!({
                            "current": {
                                "entry_id": current.id,
                                "version": version,
                                "content": current.content,
                            }
                        }))
                    }
                    MemoryError::QuotaExceeded { .. } => {
                        let entries = self.shared_memory.entries().unwrap_or_default()
                        .into_iter()
                        .filter(|entry| entry.target == request.target)
                        .map(|entry| json!({
                            "entry_id":entry.id,
                            "version":self.shared_memory.entry_version(&entry.id).ok().flatten().unwrap_or(1),
                            "content":entry.content,
                        }))
                        .collect::<Vec<_>>();
                        FeatureError::MemoryToolQuotaFull(json!({"entries":entries}))
                    }
                    other => FeatureError::Memory(other),
                })?;
        let entry_id = result
            .entry
            .as_ref()
            .map(|entry| entry.id.clone())
            .or_else(|| request.id.clone())
            .unwrap_or_default();
        self.shared_store.append_event(
            "memory.updated",
            json!({"entry":result.entry,"entry_id":entry_id,"version":result.version,"revision":result.revision}),
        )?;
        Ok(json!({
            "ok":true,
            "entry_id":entry_id,
            "version":result.version,
            "revision":result.revision,
            "remaining_chars":result.remaining_chars,
        }))
    }

    /// Execute the client protocol's user-memory and profile RPCs.  The
    /// authenticated server currently has one local user, so ownership is
    /// fixed here and never read from request parameters.
    pub fn user_memory_rpc(
        &self,
        method: &str,
        params: Value,
        timezone: &str,
    ) -> FeatureResult<FeatureResponse> {
        const USER_ID: &str = "local-user";
        if let Some(replay) = self.replay_idempotent(method, &params)? {
            return Ok(replay);
        }
        let target = MemoryTarget::user(USER_ID);
        match method {
            "memory.user.list" => {
                let entries = self
                    .shared_memory
                    .user_records(&target)?
                    .iter()
                    .map(protocol_user_entry)
                    .collect::<Vec<_>>();
                let quota = protocol_user_quota(self.shared_memory.user_quota(&target)?);
                Ok(FeatureResponse {
                    result: json!({"entries":entries,"quota":quota}),
                    events: Vec::new(),
                })
            }
            "memory.user.create" => {
                let request: macbot_protocol::MemoryUserCreateParams =
                    serde_json::from_value(params.clone())
                        .map_err(|error| FeatureError::Invalid(error.to_string()))?;
                self.transact_user_memory(method, &params, move |store| {
                    let record = store.apply_user_create(
                        target,
                        request.content,
                        macbot_memory::MemorySender::User,
                        None,
                        None,
                    )?;
                    let quota = protocol_user_quota(store.user_quota(&MemoryTarget::user(USER_ID)));
                    let entry = protocol_user_entry(&record);
                    let result = json!({"entry":entry,"quota":quota});
                    Ok((
                        result.clone(),
                        "memory.user.updated".into(),
                        json!({"entry":entry,"quota":quota}),
                    ))
                })
            }
            "memory.user.update" => {
                let request: macbot_protocol::MemoryUserUpdateParams =
                    serde_json::from_value(params.clone())
                        .map_err(|error| FeatureError::Invalid(error.to_string()))?;
                self.transact_user_memory(method, &params, move |store| {
                    let record = store.apply_user_update(
                        &target,
                        &request.entry_id,
                        request.content,
                        request.expected_version,
                        macbot_memory::MemorySender::User,
                    )?;
                    let quota = protocol_user_quota(store.user_quota(&target));
                    let entry = protocol_user_entry(&record);
                    let result = json!({"entry":entry,"quota":quota});
                    Ok((
                        result.clone(),
                        "memory.user.updated".into(),
                        json!({"entry":entry,"quota":quota}),
                    ))
                })
            }
            "memory.user.delete" => {
                let request: macbot_protocol::MemoryUserDeleteParams =
                    serde_json::from_value(params.clone())
                        .map_err(|error| FeatureError::Invalid(error.to_string()))?;
                self.transact_user_memory(method, &params, move |store| {
                    store.apply_user_delete(
                        &target,
                        &request.entry_id,
                        request.expected_version,
                    )?;
                    let quota = protocol_user_quota(store.user_quota(&target));
                    let result = json!({"quota":quota});
                    Ok((
                        result.clone(),
                        "memory.user.deleted".into(),
                        json!({"entry_id":request.entry_id,"quota":quota}),
                    ))
                })
            }
            "memory.profile.get" => {
                let profile = self
                    .shared_memory
                    .user_profile_get_or_init(&self.memory_path, timezone)?;
                Ok(FeatureResponse {
                    result: json!({"profile":protocol_user_profile(&profile)}),
                    events: Vec::new(),
                })
            }
            "memory.profile.update" => {
                let request: macbot_protocol::MemoryProfileUpdateParams =
                    serde_json::from_value(params.clone())
                        .map_err(|error| FeatureError::Invalid(error.to_string()))?;
                let timezone = timezone.to_owned();
                self.transact_user_memory(method, &params, move |store| {
                    let profile =
                        store.apply_user_profile_update(macbot_memory::UserProfileUpdate {
                            timezone,
                            expected_version: request.expected_version,
                            name: patch_to_option(request.name),
                            language: patch_to_option(request.language),
                            identity: patch_to_option(request.identity),
                            updated_by: macbot_memory::MemorySender::User,
                        })?;
                    let profile = protocol_user_profile(&profile);
                    let result = json!({"profile":profile});
                    Ok((
                        result.clone(),
                        "memory.profile.updated".into(),
                        json!({"profile":profile}),
                    ))
                })
            }
            _ => Err(FeatureError::Invalid(format!(
                "unknown user-memory method: {method}"
            ))),
        }
    }

    /// Internal curator path. The target is fixed to this host's sole user;
    /// provenance comes from the triggering user message, never a model
    /// supplied owner id.
    pub fn curate_user_memory_add(
        &self,
        content: String,
        chat_id: &str,
        message_id: &str,
    ) -> FeatureResult<FeatureResponse> {
        let target = MemoryTarget::user("local-user");
        let record = self.shared_memory.user_create_persisted(
            &self.memory_path,
            target.clone(),
            content,
            macbot_memory::MemorySender::System,
            Some(chat_id.to_owned()),
            Some(message_id.to_owned()),
        )?;
        let entry = protocol_user_entry(&record);
        let quota = protocol_user_quota(self.shared_memory.user_quota(&target)?);
        let result = json!({"entry":entry,"quota":quota});
        self.finish_user_memory_mutation(
            "memory.user.curator.add",
            &json!({}),
            &result,
            "memory.user.updated",
            json!({"entry":entry,"quota":quota}),
        )
    }

    pub fn curate_user_memory_update(
        &self,
        entry_id: &str,
        content: String,
        expected_version: u64,
    ) -> FeatureResult<FeatureResponse> {
        let target = MemoryTarget::user("local-user");
        let record = self
            .shared_memory
            .user_update_persisted(
                &self.memory_path,
                &target,
                entry_id,
                content,
                expected_version,
                macbot_memory::MemorySender::System,
            )
            .map_err(|error| match error {
                MemoryError::UserConflict(current) => FeatureError::UserMemoryConflict(
                    json!({"current":protocol_user_entry(&current)}),
                ),
                other => FeatureError::Memory(other),
            })?;
        let entry = protocol_user_entry(&record);
        let quota = protocol_user_quota(self.shared_memory.user_quota(&target)?);
        let result = json!({"entry":entry,"quota":quota});
        self.finish_user_memory_mutation(
            "memory.user.curator.update",
            &json!({}),
            &result,
            "memory.user.updated",
            json!({"entry":entry,"quota":quota}),
        )
    }

    pub fn curate_user_profile_update(
        &self,
        timezone: &str,
        expected_version: u64,
        name: Option<Option<String>>,
        language: Option<Option<String>>,
        identity: Option<Option<String>>,
    ) -> FeatureResult<FeatureResponse> {
        let profile = self
            .shared_memory
            .user_profile_update_persisted(
                &self.memory_path,
                macbot_memory::UserProfileUpdate {
                    timezone: timezone.to_owned(),
                    expected_version,
                    name,
                    language,
                    identity,
                    updated_by: macbot_memory::MemorySender::System,
                },
            )
            .map_err(|error| match error {
                MemoryError::ProfileConflict(current) => FeatureError::UserProfileConflict(
                    json!({"current":protocol_user_profile(&current)}),
                ),
                other => FeatureError::Memory(other),
            })?;
        let profile = protocol_user_profile(&profile);
        let result = json!({"profile":profile});
        self.finish_user_memory_mutation(
            "memory.profile.curator.update",
            &json!({}),
            &result,
            "memory.profile.updated",
            json!({"profile":profile}),
        )
    }

    fn transact_user_memory<F>(
        &self,
        method: &str,
        params: &Value,
        operation: F,
    ) -> FeatureResult<FeatureResponse>
    where
        F: FnOnce(&mut macbot_memory::MemoryStore) -> Result<(Value, String, Value), MemoryError>,
    {
        let transaction = self.persist_user_memory_transaction(method, params, operation)?;
        self.publish_user_memory_transaction(method, params, transaction)
    }

    fn persist_user_memory_transaction<F>(
        &self,
        method: &str,
        params: &Value,
        operation: F,
    ) -> FeatureResult<macbot_memory::MemoryTransactionResult>
    where
        F: FnOnce(&mut macbot_memory::MemoryStore) -> Result<(Value, String, Value), MemoryError>,
    {
        let request_id = params.get("client_request_id").and_then(Value::as_str);
        let signature = signature(method, params);
        let transaction = self
            .shared_memory
            .transact_user_persisted(&self.memory_path, request_id, &signature, operation)
            .map_err(|error| match error {
                MemoryError::UserConflict(current) => FeatureError::UserMemoryConflict(
                    json!({"current":protocol_user_entry(&current)}),
                ),
                MemoryError::ProfileConflict(current) => FeatureError::UserProfileConflict(
                    json!({"current":protocol_user_profile(&current)}),
                ),
                MemoryError::IdempotencyConflict(key) => FeatureError::Conflict(format!(
                    "client_request_id {key} was reused with different request content"
                )),
                other => FeatureError::Memory(other),
            })?;
        Ok(transaction)
    }

    fn publish_user_memory_transaction(
        &self,
        method: &str,
        params: &Value,
        transaction: macbot_memory::MemoryTransactionResult,
    ) -> FeatureResult<FeatureResponse> {
        let request_id = params.get("client_request_id").and_then(Value::as_str);
        let receipt = transaction.receipt;
        let event = if let Some(request_id) = request_id {
            self.shared_store.append_event_once(
                &format!("memory-user-rpc:{request_id}"),
                receipt.event_name.clone(),
                receipt.event_data.clone(),
            )?
        } else {
            Some(
                self.shared_store
                    .append_event(receipt.event_name.clone(), receipt.event_data.clone())?,
            )
        };
        if let Err(error) = self.save_idempotency(method, params, &receipt.result) {
            // The memory snapshot already contains the authoritative receipt;
            // failure to refresh this secondary index must not turn a
            // committed mutation into an error after its event was appended.
            tracing::warn!(%error, method, "memory RPC idempotency index refresh failed");
        }
        Ok(FeatureResponse {
            result: receipt.result,
            events: event.into_iter().collect(),
        })
    }

    fn finish_user_memory_mutation(
        &self,
        method: &str,
        params: &Value,
        result: &Value,
        event_name: &str,
        event_data: Value,
    ) -> FeatureResult<FeatureResponse> {
        let event = self.shared_store.append_event(event_name, event_data)?;
        self.save_idempotency(method, params, result)?;
        Ok(FeatureResponse {
            result: result.clone(),
            events: vec![event],
        })
    }

    pub fn memory_search(
        &self,
        query: &str,
        target: Option<&MemoryTarget>,
    ) -> FeatureResult<Vec<MemoryEntry>> {
        Ok(self.shared_memory.search(query, target)?)
    }

    pub fn memory_search_for_access(
        &self,
        actor: &MemoryActor,
        access: &MemoryAccess,
        query: &str,
        target: Option<&MemoryTarget>,
    ) -> FeatureResult<Vec<MemoryEntry>> {
        if let Some(target) = target {
            authorize_memory(actor, access, target, &MemoryAction::Add)?;
            return self.memory_search(query, Some(target));
        }
        if actor.is_main() {
            return self.memory_search(query, None);
        }
        let mut targets = Vec::new();
        if let Some(user_id) = &access.user_id {
            targets.push(MemoryTarget::user(user_id.clone()));
        }
        if let Some(bot_id) = actor.bot_id() {
            targets.push(MemoryTarget::bot(bot_id));
            if let Some(project_id) = &access.project_id {
                if access.is_project_member(bot_id, project_id) {
                    targets.push(MemoryTarget::project(project_id.clone()));
                }
            }
        }
        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for target in targets {
            for entry in self.memory_search(query, Some(&target))? {
                if seen.insert(entry.id.clone()) {
                    results.push(entry);
                }
            }
        }
        Ok(results)
    }

    pub fn add_session_message(&self, message: SessionMessage) -> FeatureResult<()> {
        self.add_session_messages(vec![message])
    }

    pub fn add_session_messages(&self, messages: Vec<SessionMessage>) -> FeatureResult<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let mut conversations = self
            .conversations
            .write()
            .map_err(|_| FeatureError::Invalid("conversation lock poisoned".into()))?;
        for message in messages {
            conversations.add_message(message);
        }
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

    pub fn project_find_for_access(
        &self,
        actor: &MemoryActor,
        access: &MemoryAccess,
        query: &str,
    ) -> FeatureResult<Vec<ProjectRecord>> {
        let projects = self.project_find(query)?;
        if actor.is_main() {
            return Ok(projects);
        }
        let Some(project_id) = access.project_id.as_deref() else {
            return Ok(Vec::new());
        };
        let Some(bot_id) = actor.bot_id() else {
            return Ok(Vec::new());
        };
        if !access.is_project_member(bot_id, project_id) {
            return Ok(Vec::new());
        }
        Ok(projects
            .into_iter()
            .filter(|project| project.id == project_id)
            .collect())
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
        Ok(self
            .maintain_worklog_with_events(target)
            .await?
            .map(|(entry, _)| entry))
    }

    /// Run idle maintenance and return the durable events produced by the
    /// commit.  The gateway publishes these events after this method returns;
    /// the feature service never appends them a second time.
    pub async fn maintain_worklog_with_events(
        &self,
        target: &MemoryTarget,
    ) -> FeatureResult<Option<(MemoryEntry, Vec<Event>)>> {
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
        // Extraction is an additive maintenance pass. A provider that
        // returns malformed extraction JSON must not discard a valid
        // worklog summary; retain the summary and skip only those drafts.
        let drafts = match provider.extract(&transcript).await {
            Ok(drafts) => drafts,
            Err(error) => {
                tracing::warn!(%error, bot_id = %target.owner_id, "maintenance extraction skipped");
                Vec::new()
            }
        };
        // The unique run id prevents collision with an in-flight model run.
        let run_id = format!("maintenance-{}", uuid::Uuid::now_v7());
        let maintenance_source = MemorySource {
            bot_id: Some(target.owner_id.clone()),
            run_id: Some(run_id.clone()),
            session_id: Some("maintenance".into()),
        };
        let commit = (|| -> FeatureResult<(Vec<MemoryEntry>, Vec<Event>)> {
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
                    source: maintenance_source.clone(),
                },
            )?;
            for draft in drafts {
                // Maintenance reads only this Bot's worklog. Keep extracted
                // memories within that same private scope so a model cannot
                // turn private Bot history into user/project data.
                if draft.target != *target {
                    return Err(FeatureError::Invalid(
                        "maintenance drafts must target the source Bot".into(),
                    ));
                }
                let mut source = draft.source;
                // Extraction output is untrusted model data.  Keep only its
                // content/kind; all provenance is assigned by maintenance.
                source.bot_id = Some(target.owner_id.clone());
                source.run_id = Some(run_id.clone());
                source.session_id = Some("maintenance".into());
                self.shared_memory.stage(
                    &run_id,
                    MemoryRequest {
                        target: draft.target,
                        action: MemoryAction::Add,
                        content: draft.content,
                        id: None,
                        kind: Some(draft.kind),
                        source,
                    },
                )?;
            }
            self.commit_memory_run_with_events(&run_id)
        })();
        let (entries, events) = match commit {
            Ok(result) => result,
            Err(error) => {
                let _ = self.rollback_memory_run(&run_id);
                self.release_maintenance(&target.owner_id)?;
                return Err(error);
            }
        };
        Ok(entries
            .into_iter()
            .find(|entry| entry.target == *target)
            .map(|entry| (entry, events)))
    }

    /// Persist a compacted context segment through the same atomic run path
    /// as model memory writes. This makes compact summaries recoverable after
    /// restart without exposing them as a client-facing protocol event.
    pub fn persist_context_summary(
        &self,
        target: MemoryTarget,
        summary: &str,
        source: MemorySource,
    ) -> FeatureResult<Option<MemoryEntry>> {
        if target.scope != macbot_memory::MemoryScope::Bot || summary.trim().is_empty() {
            return Ok(None);
        }
        let run_id = format!("compact-{}", uuid::Uuid::now_v7());
        self.begin_memory_run(&run_id)?;
        self.shared_memory.stage(
            &run_id,
            MemoryRequest {
                target: target.clone(),
                action: MemoryAction::Add,
                content: summary.to_owned(),
                id: None,
                kind: Some(MemoryKind::BotWorklog),
                source,
            },
        )?;
        match self.commit_memory_run(&run_id) {
            Ok(entries) => Ok(entries.into_iter().find(|entry| entry.target == target)),
            Err(error) => {
                let _ = self.rollback_memory_run(&run_id);
                Err(error)
            }
        }
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
        Ok(self
            .run_maintenance_if_due_with_events(target)
            .await?
            .map(|(entry, _)| entry))
    }

    async fn run_maintenance_if_due_with_events(
        &self,
        target: &MemoryTarget,
    ) -> FeatureResult<Option<(MemoryEntry, Vec<Event>)>> {
        if self.maintenance_due(target, chrono::Utc::now())? {
            self.maintain_worklog_with_events(target).await
        } else {
            Ok(None)
        }
    }

    /// Scheduler entrypoint: call this from the gateway's idle/day tick. The
    /// policy and durable per-Bot counters make repeated ticks cheap and safe.
    pub async fn maintenance_tick(
        &self,
        targets: &[MemoryTarget],
    ) -> FeatureResult<Vec<MemoryEntry>> {
        let mut committed = Vec::new();
        for target in targets {
            if let Some(entry) = self.run_maintenance_if_due(target).await? {
                committed.push(entry);
            }
        }
        Ok(committed)
    }

    /// Scheduler entrypoint variant that preserves the already durable event
    /// records for the gateway live fan-out and resume buffer.
    pub async fn maintenance_tick_with_events(
        &self,
        targets: &[MemoryTarget],
    ) -> MaintenanceTickReport {
        let mut committed = Vec::new();
        let mut events = Vec::new();
        let mut error = None;
        for target in targets {
            match self.run_maintenance_if_due_with_events(target).await {
                Ok(Some((entry, entry_events))) => {
                    committed.push(entry);
                    events.extend(entry_events);
                }
                Ok(None) => {}
                Err(target_error) => {
                    if error.is_none() {
                        error = Some(target_error);
                    }
                }
            }
        }
        MaintenanceTickReport {
            committed,
            events,
            error,
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
        Ok(Some(provider.compact(&text).await?))
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
        if kinds
            .iter()
            .any(|kind| !SEARCH_KINDS.contains(&kind.as_str()))
        {
            return Err(FeatureError::Invalid("unsupported search kind".into()));
        }
        let filter = kinds.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut hits = BTreeMap::new();
        let data_root = self.shared_store.root().join("data");
        collect_search_files(&data_root, &data_root, &filter, &mut hits)?;
        let current_ids = current_orchestrator_ids(&self.shared_store.root().join("data"));
        let query = query.to_lowercase();
        Ok(hits
            .into_values()
            .filter(|record| search_record_is_live(record, current_ids.as_ref()))
            .filter(|record| record.search_text.to_lowercase().contains(&query))
            .map(|record| record.hit)
            .take(limit)
            .collect())
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
    usage_sink: Option<Arc<dyn MaintenanceUsageSink>>,
    usage_context: Option<MaintenanceUsageContext>,
}

#[derive(Clone, Debug, Default)]
pub struct MaintenanceUsageContext {
    pub request_id: String,
    pub bot_id: String,
    pub project_id: Option<String>,
    pub chat_id: String,
    pub run_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub routine: bool,
}

#[async_trait]
pub trait MaintenanceUsageSink: Send + Sync {
    async fn record(
        &self,
        phase: &str,
        context: &MaintenanceUsageContext,
        usage: &TokenUsage,
    ) -> Result<(), String>;
}

/// Production can provide a cheap/local fallback model for idle maintenance
/// and compaction. A primary provider failure is retried once through the
/// fallback; successful primary calls are never duplicated.
pub struct FallbackMaintenanceProvider {
    primary: Arc<dyn AsyncMaintenanceProvider>,
    fallback: Arc<dyn AsyncMaintenanceProvider>,
}

impl FallbackMaintenanceProvider {
    pub fn new(
        primary: Arc<dyn AsyncMaintenanceProvider>,
        fallback: Arc<dyn AsyncMaintenanceProvider>,
    ) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl AsyncMaintenanceProvider for FallbackMaintenanceProvider {
    async fn summarize(&self, text: &str) -> Result<String, MemoryError> {
        match self.primary.summarize(text).await {
            Ok(value) => Ok(value),
            Err(_) => self.fallback.summarize(text).await,
        }
    }

    async fn compact(&self, text: &str) -> Result<String, MemoryError> {
        match self.primary.compact(text).await {
            Ok(value) => Ok(value),
            Err(_) => self.fallback.compact(text).await,
        }
    }

    async fn extract(&self, text: &str) -> Result<Vec<macbot_memory::MemoryDraft>, MemoryError> {
        match self.primary.extract(text).await {
            Ok(value) => Ok(value),
            Err(_) => self.fallback.extract(text).await,
        }
    }
}

impl ModelMaintenanceAdapter {
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            usage_sink: None,
            usage_context: None,
        }
    }

    /// Attach a gateway-owned UsageLedger adapter. The feature crate keeps
    /// this as a trait so it does not take a dependency on the dashboard
    /// implementation; protocol usage phase keys are `memory` and `compact`.
    pub fn with_usage_sink(mut self, sink: Arc<dyn MaintenanceUsageSink>) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    pub fn with_usage_context(mut self, context: MaintenanceUsageContext) -> Self {
        self.usage_context = Some(context);
        self
    }

    pub fn from_http(
        config: ProviderConfig,
        secrets: Arc<dyn SecretStore>,
        model: impl Into<String>,
    ) -> Self {
        Self::new(Arc::new(HttpProvider::new(config, secrets)), model)
    }

    async fn complete(
        &self,
        phase: &str,
        system: &str,
        text: &str,
    ) -> Result<Completion, FeatureError> {
        let completion = self
            .provider
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
            .map_err(|error| FeatureError::Provider(error.to_string()))?;
        if let (Some(sink), Some(context)) = (&self.usage_sink, &self.usage_context) {
            // A single configured adapter serves both idle summarization and
            // context compaction. Give every provider call its own durable
            // request id so UsageLedger does not deduplicate later calls.
            let mut context = context.clone();
            context.request_id = format!("{}:{}", context.request_id, Uuid::now_v7());
            sink.record(phase, &context, &completion.usage)
                .await
                .map_err(FeatureError::Provider)?;
        }
        Ok(completion)
    }
}

#[async_trait]
impl AsyncMaintenanceProvider for ModelMaintenanceAdapter {
    async fn summarize(&self, text: &str) -> Result<String, MemoryError> {
        self.complete(
            "memory",
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
                "memory",
                "Extract only durable memories as a JSON array with target, kind, and content fields. Return [] when none.",
                text,
            )
            .await
            .map_err(|error| MemoryError::Provider(error.to_string()))?;
        serde_json::from_str(&completion.text)
            .map_err(|error| MemoryError::Provider(format!("maintenance JSON: {error}")))
    }

    async fn compact(&self, text: &str) -> Result<String, MemoryError> {
        self.complete(
            "compact",
            "Compact the context segment while preserving decisions, constraints, and unresolved items.",
            text,
        )
        .await
        .map(|completion| completion.text)
        .map_err(|error| MemoryError::Provider(error.to_string()))
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

const SEARCH_KINDS: [&str; 5] = ["message", "chat", "bot", "artifact", "routine"];

struct SearchRecord {
    hit: SearchHit,
    search_text: String,
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

fn protocol_memory_sender(sender: &macbot_memory::MemorySender) -> macbot_protocol::Sender {
    match sender {
        macbot_memory::MemorySender::User => macbot_protocol::Sender::User,
        macbot_memory::MemorySender::System => macbot_protocol::Sender::System,
        macbot_memory::MemorySender::Bot { bot_id } => macbot_protocol::Sender::Bot {
            bot_id: bot_id.clone(),
        },
    }
}

fn protocol_user_entry(record: &macbot_memory::UserMemoryRecord) -> macbot_protocol::MemoryEntry {
    macbot_protocol::MemoryEntry {
        id: record.id.clone(),
        content: record.content.clone(),
        version: record.version,
        source: macbot_protocol::MemorySource {
            by: protocol_memory_sender(&record.source.by),
            chat_id: record.source.chat_id.clone(),
            message_id: record.source.message_id.clone(),
        },
        updated_by: protocol_memory_sender(&record.updated_by),
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn protocol_user_profile(profile: &macbot_memory::UserProfile) -> macbot_protocol::UserProfile {
    macbot_protocol::UserProfile {
        name: profile.name.clone(),
        language: profile.language.clone(),
        identity: profile.identity.clone(),
        timezone: profile.timezone.clone(),
        onboarded: profile.onboarded,
        version: profile.version,
        updated_by: protocol_memory_sender(&profile.updated_by),
        updated_at: profile.updated_at.to_rfc3339(),
    }
}

fn protocol_user_quota((used_chars, limit_chars): (usize, usize)) -> macbot_protocol::MemoryQuota {
    macbot_protocol::MemoryQuota {
        used_chars: used_chars as u64,
        limit_chars: limit_chars as u64,
    }
}

fn patch_to_option<T>(patch: macbot_protocol::Patch<T>) -> Option<Option<T>> {
    match patch {
        macbot_protocol::Patch::Unset => None,
        macbot_protocol::Patch::Null => Some(None),
        macbot_protocol::Patch::Value(value) => Some(Some(value)),
    }
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
    if matches!(actor, MemoryActor::System) {
        return Ok(());
    }
    if target.scope == macbot_memory::MemoryScope::User {
        return Err(FeatureError::Invalid(
            "only the user or curator may write user preferences".into(),
        ));
    }
    if actor.is_main() {
        return Ok(());
    }
    match target.scope {
        macbot_memory::MemoryScope::Bot if actor.bot_id() == Some(target.owner_id.as_str()) => {}
        macbot_memory::MemoryScope::User
            if access.user_id.as_deref() == Some(target.owner_id.as_str()) => {}
        macbot_memory::MemoryScope::Project
            if access.is_project_member(
                actor.bot_id().unwrap_or_default(),
                target.owner_id.as_str(),
            ) => {}
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
    data_root: &Path,
    root: &Path,
    kinds: &HashSet<&str>,
    hits: &mut BTreeMap<(String, String), SearchRecord>,
) -> FeatureResult<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_search_files(data_root, &path, kinds, hits)?;
            continue;
        }
        if !is_canonical_search_source(data_root, &path) {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            for line in fs::read_to_string(&path)?.lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    collect_search_value(&value, &path, kinds, hits);
                }
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            if let Ok(value) = serde_json::from_slice::<Value>(&fs::read(&path)?) {
                collect_search_value(&value, &path, kinds, hits);
            }
        }
    }
    Ok(())
}

fn is_canonical_search_source(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let Some(top_level) = relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
    else {
        return false;
    };
    match top_level {
        "chats" => matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("messages.jsonl" | "metadata.json")
        ),
        "orchestrator" => matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("state.json" | "history.json")
        ),
        "messages" | "artifacts" | "routines" | "bots" => true,
        _ => false,
    }
}

fn collect_search_value(
    value: &Value,
    path: &Path,
    kinds: &HashSet<&str>,
    hits: &mut BTreeMap<(String, String), SearchRecord>,
) {
    collect_search_value_inner(value, path, kinds, hits, false);
}

fn collect_search_value_inner(
    value: &Value,
    path: &Path,
    kinds: &HashSet<&str>,
    hits: &mut BTreeMap<(String, String), SearchRecord>,
    inherited_deleted: bool,
) {
    if let Some(object) = value.as_object() {
        let deleted = inherited_deleted
            || object.get("deleted").and_then(Value::as_bool) == Some(true)
            || object.get("type").and_then(Value::as_str) == Some("message.deleted");
        if deleted {
            return;
        }
        let kind = search_kind(object);
        let serialized = serde_json::to_string(value).unwrap_or_default();
        let is_envelope = object.contains_key("type") && object.contains_key("data");
        let has_nested = object
            .values()
            .any(|child| child.is_object() || child.is_array());
        let has_search_fields = [
            "id",
            "message_id",
            "artifact_id",
            "routine_id",
            "bot_id",
            "title",
            "name",
            "fallback_text",
            "description",
            "content",
            "text",
            "path_or_url",
            "max_parallel",
            "schedules",
        ]
        .iter()
        .any(|field| object.contains_key(*field));
        if !is_envelope
            && SEARCH_KINDS.contains(&kind.as_str())
            && (!has_nested || has_search_fields)
            && (kinds.is_empty() || kinds.contains(kind.as_str()))
        {
            let id = object
                .get("id")
                .or_else(|| object.get("message_id"))
                .or_else(|| object.get("artifact_id"))
                .or_else(|| object.get("routine_id"))
                .or_else(|| object.get("bot_id"))
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
                .get("updated_at")
                .or_else(|| object.get("at"))
                .or_else(|| object.get("created_at"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let hit = SearchHit {
                kind: kind.clone(),
                id: id.to_string(),
                chat_id: object
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                title,
                snippet: readable_snippet(object, &kind, &serialized),
                at,
            };
            let key = (hit.kind.clone(), hit.id.clone());
            let replace = hits
                .get(&key)
                .is_none_or(|previous| search_hit_is_newer(&hit, &previous.hit));
            if replace {
                hits.insert(
                    key,
                    SearchRecord {
                        hit,
                        search_text: serialized,
                    },
                );
            }
        }
        for child in object.values() {
            collect_search_value_inner(child, path, kinds, hits, deleted);
        }
    } else if let Some(items) = value.as_array() {
        for child in items {
            collect_search_value_inner(child, path, kinds, hits, inherited_deleted);
        }
    }
}

fn current_orchestrator_ids(data_root: &Path) -> Option<HashMap<&'static str, HashSet<String>>> {
    let path = data_root.join("orchestrator/state.json");
    let value = serde_json::from_slice::<Value>(&fs::read(path).ok()?).ok()?;
    let mut ids = HashMap::new();
    for (kind, field) in [
        ("bot", "bots"),
        ("routine", "routines"),
        ("artifact", "artifacts"),
    ] {
        if let Some(value) = value.get(field) {
            let mut field_ids = HashSet::new();
            collect_state_ids(value, &mut field_ids);
            ids.insert(kind, field_ids);
        }
    }
    Some(ids)
}

fn collect_state_ids(value: &Value, ids: &mut HashSet<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_state_ids(item, ids)),
        Value::Object(object) => {
            if let Some(id) = ["id", "bot_id", "routine_id", "artifact_id"]
                .iter()
                .find_map(|field| object.get(*field).and_then(Value::as_str))
            {
                ids.insert(id.to_string());
            } else {
                for (key, child) in object {
                    if child.is_object() || child.is_array() {
                        ids.insert(key.clone());
                        collect_state_ids(child, ids);
                    }
                }
            }
        }
        _ => {}
    }
}

fn search_record_is_live(
    record: &SearchRecord,
    current_ids: Option<&HashMap<&'static str, HashSet<String>>>,
) -> bool {
    let Some(current_ids) = current_ids else {
        return true;
    };
    current_ids
        .get(record.hit.kind.as_str())
        .is_none_or(|ids| ids.contains(&record.hit.id))
}

fn search_hit_is_newer(candidate: &SearchHit, previous: &SearchHit) -> bool {
    match (candidate.at.as_deref(), previous.at.as_deref()) {
        (Some(candidate), Some(previous)) => candidate >= previous,
        (Some(_), None) | (None, None) => true,
        (None, Some(_)) => false,
    }
}

fn readable_snippet(
    object: &serde_json::Map<String, Value>,
    kind: &str,
    serialized: &str,
) -> String {
    let fields: &[&str] = match kind {
        "bot" => &["name", "label", "description", "model", "status"],
        "chat" => &["title", "name", "last_message", "description"],
        "message" => &["fallback_text", "content", "text"],
        "artifact" => &["title", "description", "path_or_url"],
        "routine" => &["name", "instructions", "description"],
        _ => &["title", "name", "description", "content", "text"],
    };
    let mut parts = fields
        .iter()
        .filter_map(|field| object.get(*field))
        .filter_map(|value| match value {
            Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        parts.push(serialized.to_string());
    }
    parts.join(" · ").chars().take(240).collect()
}

fn search_kind(object: &serde_json::Map<String, Value>) -> String {
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
    // Only explicit entity fields may classify a record. Path names are not
    // entity metadata, and inferring from them can expose internal records as
    // user-facing search hits.
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
    fn user_memory_rpc_persists_cas_events_and_profile_without_owner_input() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let created = service.user_memory_rpc(
            "memory.user.create",
            json!({"content":"中文回复","owner_id":"attacker","client_request_id":"user-add-1"}),
            "Asia/Shanghai",
        ).unwrap();
        assert_eq!(created.events[0].event, "memory.user.updated");
        let entry = &created.result["entry"];
        assert_eq!(entry["source"]["by"]["kind"], "user");
        assert_eq!(entry["source"]["chat_id"], Value::Null);
        assert_eq!(entry["version"], 1);
        let entry_id = entry["id"].as_str().unwrap().to_owned();

        let stale = service
            .user_memory_rpc(
                "memory.user.update",
                json!({"entry_id":entry_id,"content":"旧版本","expected_version":0}),
                "Asia/Shanghai",
            )
            .unwrap_err();
        assert!(
            matches!(stale, FeatureError::UserMemoryConflict(details) if details["current"]["version"] == 1)
        );
        let updated = service
            .user_memory_rpc(
                "memory.user.update",
                json!({"entry_id":entry_id,"content":"更新后的偏好","expected_version":1}),
                "Asia/Shanghai",
            )
            .unwrap();
        assert_eq!(updated.result["entry"]["version"], 2);
        assert_eq!(updated.result["entry"]["source"]["by"]["kind"], "user");
        assert_eq!(updated.result["entry"]["updated_by"]["kind"], "user");

        let listed = service
            .user_memory_rpc(
                "memory.user.list",
                json!({"owner_id":"another-user"}),
                "Asia/Shanghai",
            )
            .unwrap();
        assert_eq!(listed.result["entries"].as_array().unwrap().len(), 1);
        let profile = service
            .user_memory_rpc("memory.profile.get", json!({}), "Asia/Tokyo")
            .unwrap();
        assert_eq!(profile.result["profile"]["timezone"], "Asia/Tokyo");
        let updated_profile = service
            .user_memory_rpc(
                "memory.profile.update",
                json!({"name":"Ari","timezone":"Pacific/Auckland","expected_version":1}),
                "UTC",
            )
            .unwrap();
        assert_eq!(updated_profile.result["profile"]["name"], "Ari");
        assert_eq!(updated_profile.result["profile"]["timezone"], "Asia/Tokyo");
        assert_eq!(updated_profile.events[0].event, "memory.profile.updated");
        let patched_profile = service
            .user_memory_rpc(
                "memory.profile.update",
                json!({"identity":"human","expected_version":2}),
                "UTC",
            )
            .unwrap();
        assert_eq!(patched_profile.result["profile"]["name"], "Ari");
        assert_eq!(patched_profile.result["profile"]["identity"], "human");
        let cleared_profile = service
            .user_memory_rpc(
                "memory.profile.update",
                json!({"name":null,"expected_version":3}),
                "UTC",
            )
            .unwrap();
        assert_eq!(cleared_profile.result["profile"]["name"], Value::Null);
        assert_eq!(cleared_profile.result["profile"]["identity"], "human");

        drop(service);
        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        let listed = restored
            .user_memory_rpc("memory.user.list", json!({}), "UTC")
            .unwrap();
        assert_eq!(listed.result["entries"][0]["content"], "更新后的偏好");
        assert_eq!(listed.result["entries"][0]["version"], 2);
        assert_eq!(
            restored
                .user_memory_rpc("memory.profile.get", json!({}), "UTC")
                .unwrap()
                .result["profile"]["name"],
            Value::Null
        );
        let deleted = restored
            .user_memory_rpc(
                "memory.user.delete",
                json!({"entry_id":entry_id,"expected_version":2}),
                "UTC",
            )
            .unwrap();
        assert_eq!(deleted.events[0].event, "memory.user.deleted");
        assert_eq!(deleted.result["quota"]["used_chars"], 0);

        let curated = restored
            .curate_user_memory_add(
                "Avoid weekend notifications".into(),
                "chat-origin",
                "msg-origin",
            )
            .unwrap();
        assert_eq!(curated.result["entry"]["source"]["by"]["kind"], "system");
        assert_eq!(curated.result["entry"]["source"]["chat_id"], "chat-origin");
        assert_eq!(
            curated.result["entry"]["source"]["message_id"],
            "msg-origin"
        );
        let curator_profile = restored
            .curate_user_profile_update("UTC", 4, None, Some(Some("zh-CN".into())), None)
            .unwrap();
        assert_eq!(
            curator_profile.result["profile"]["updated_by"]["kind"],
            "system"
        );

        let _ = restored
            .shared_memory
            .add(MemoryRequest {
                target: MemoryTarget::bot("bot-private"),
                action: MemoryAction::Add,
                content: "private experience".into(),
                id: None,
                kind: Some(MemoryKind::BotExperience),
                source: MemorySource::default(),
            })
            .unwrap();
        let only_user_entries = restored
            .user_memory_rpc("memory.user.list", json!({}), "UTC")
            .unwrap();
        assert_eq!(
            only_user_entries.result["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let bot_write = restored.memory_rpc_with_access(
            &MemoryActor::main("bot-main"), &MemoryAccess::user("local-user"),
            json!({"scope":"user","user_id":"local-user","run_id":"bot-run","content":"must not write user preference"}),
        );
        assert!(matches!(bot_write, Err(FeatureError::Invalid(_))));
    }

    #[test]
    fn profile_onboarding_completes_or_skips_with_host_timezone_bot_actor_and_cas() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let initial = service.profile_onboarding_status("Asia/Tokyo").unwrap();
        assert!(initial.available);
        assert_eq!(initial.prompt, Some(ProfileOnboardingPrompt::Initialize));

        let completed = service
            .set_user_profile_onboarding(UserProfileOnboardingInput {
                run_id: "run-onboard".into(),
                bot_id: "main-bot".into(),
                expected_version: 1,
                name: Some("Ari".into()),
                language: Some("zh-CN".into()),
                identity: Some("product designer".into()),
                skip: false,
            })
            .unwrap();
        let profile = &completed.result["profile"];
        assert_eq!(profile["name"], "Ari");
        assert_eq!(profile["language"], "zh-CN");
        assert_eq!(profile["identity"], "product designer");
        assert_eq!(profile["timezone"], "Asia/Tokyo");
        assert_eq!(profile["onboarded"], true);
        assert_eq!(profile["updated_by"]["kind"], "bot");
        assert_eq!(profile["updated_by"]["bot_id"], "main-bot");
        assert_eq!(completed.events[0].event, "memory.profile.updated");
        assert!(!service.profile_onboarding_status("UTC").unwrap().available);

        let stale = service.set_user_profile_onboarding(UserProfileOnboardingInput {
            run_id: "run-stale".into(),
            bot_id: "main-bot".into(),
            expected_version: 1,
            name: Some("Other".into()),
            language: None,
            identity: None,
            skip: false,
        });
        assert!(matches!(stale, Err(FeatureError::UserProfileConflict(_))));

        drop(service);
        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        let restored_profile = restored
            .user_memory_rpc("memory.profile.get", json!({}), "UTC")
            .unwrap();
        assert_eq!(restored_profile.result["profile"]["timezone"], "Asia/Tokyo");
        assert_eq!(restored_profile.result["profile"]["onboarded"], true);
    }

    #[test]
    fn profile_onboarding_skip_and_single_reminder_survive_restart() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let initial = service.profile_onboarding_status("Asia/Shanghai").unwrap();
        assert_eq!(initial.prompt, Some(ProfileOnboardingPrompt::Initialize));
        let reminder = service.profile_onboarding_status("UTC").unwrap();
        assert_eq!(reminder.prompt, Some(ProfileOnboardingPrompt::Reminder));
        drop(service);

        let restored = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let after_restart = restored.profile_onboarding_status("UTC").unwrap();
        assert!(after_restart.available);
        assert_eq!(after_restart.prompt, None);
        let skipped = restored
            .set_user_profile_onboarding(UserProfileOnboardingInput {
                run_id: "run-skip".into(),
                bot_id: "main-bot".into(),
                expected_version: 1,
                name: None,
                language: None,
                identity: None,
                skip: true,
            })
            .unwrap();
        assert_eq!(skipped.result["profile"]["onboarded"], true);
        assert_eq!(skipped.result["profile"]["updated_by"]["kind"], "bot");
        assert_eq!(skipped.result["profile"]["timezone"], "Asia/Shanghai");
        assert!(!restored.profile_onboarding_status("UTC").unwrap().available);
    }

    #[test]
    fn user_memory_mutations_recover_atomic_receipts_after_snapshot_before_event() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let target = MemoryTarget::user("local-user");

        let create_params = json!({"content":"remember tea","client_request_id":"tx-create"});
        let prepared_create = service
            .persist_user_memory_transaction("memory.user.create", &create_params, |store| {
                let record = store.apply_user_create(
                    target.clone(),
                    "remember tea".into(),
                    macbot_memory::MemorySender::User,
                    None,
                    None,
                )?;
                let entry = protocol_user_entry(&record);
                let quota = protocol_user_quota(store.user_quota(&target));
                let result = json!({"entry":entry,"quota":quota});
                Ok((result.clone(), "memory.user.updated".into(), result))
            })
            .unwrap();
        assert!(service.shared_store.events_since(0).unwrap().is_empty());
        drop(service);

        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let create_retry = service
            .user_memory_rpc("memory.user.create", create_params.clone(), "UTC")
            .unwrap();
        assert_eq!(create_retry.result, prepared_create.receipt.result);
        assert_eq!(create_retry.events.len(), 1);
        let create_duplicate = service
            .user_memory_rpc("memory.user.create", create_params.clone(), "UTC")
            .unwrap();
        assert_eq!(create_duplicate.result, prepared_create.receipt.result);
        assert!(create_duplicate.events.is_empty());
        assert_eq!(
            service.shared_memory.user_records(&target).unwrap().len(),
            1
        );
        let bad_reuse = service
            .user_memory_rpc(
                "memory.user.create",
                json!({"content":"different","client_request_id":"tx-create"}),
                "UTC",
            )
            .unwrap_err();
        assert!(matches!(bad_reuse, FeatureError::Conflict(_)));

        let entry_id = prepared_create.receipt.result["entry"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let update_params = json!({
            "entry_id":entry_id,
            "content":"remember green tea",
            "expected_version":1,
            "client_request_id":"tx-update"
        });
        let prepared_update = service
            .persist_user_memory_transaction("memory.user.update", &update_params, |store| {
                let record = store.apply_user_update(
                    &target,
                    &entry_id,
                    "remember green tea".into(),
                    1,
                    macbot_memory::MemorySender::User,
                )?;
                let entry = protocol_user_entry(&record);
                let quota = protocol_user_quota(store.user_quota(&target));
                let result = json!({"entry":entry,"quota":quota});
                Ok((result.clone(), "memory.user.updated".into(), result))
            })
            .unwrap();
        drop(service);
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let update_retry = service
            .user_memory_rpc("memory.user.update", update_params.clone(), "UTC")
            .unwrap();
        assert_eq!(update_retry.result, prepared_update.receipt.result);
        assert_eq!(update_retry.events.len(), 1);
        assert!(service
            .user_memory_rpc("memory.user.update", update_params.clone(), "UTC")
            .unwrap()
            .events
            .is_empty());
        assert_eq!(
            service.shared_memory.user_records(&target).unwrap()[0].version,
            2
        );

        let delete_params = json!({
            "entry_id":entry_id,
            "expected_version":2,
            "client_request_id":"tx-delete"
        });
        let prepared_delete = service
            .persist_user_memory_transaction("memory.user.delete", &delete_params, |store| {
                store.apply_user_delete(&target, &entry_id, 2)?;
                let quota = protocol_user_quota(store.user_quota(&target));
                let result = json!({"quota":quota});
                Ok((
                    result.clone(),
                    "memory.user.deleted".into(),
                    json!({"entry_id":entry_id,"quota":quota}),
                ))
            })
            .unwrap();
        drop(service);
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let delete_retry = service
            .user_memory_rpc("memory.user.delete", delete_params.clone(), "UTC")
            .unwrap();
        assert_eq!(delete_retry.result, prepared_delete.receipt.result);
        assert_eq!(delete_retry.events.len(), 1);
        assert!(service
            .user_memory_rpc("memory.user.delete", delete_params.clone(), "UTC")
            .unwrap()
            .events
            .is_empty());
        assert!(service
            .shared_memory
            .user_records(&target)
            .unwrap()
            .is_empty());

        let _ = service
            .user_memory_rpc("memory.profile.get", json!({}), "UTC")
            .unwrap();
        let profile_params = json!({
            "name":"Ari",
            "expected_version":1,
            "client_request_id":"tx-profile"
        });
        let prepared_profile = service
            .persist_user_memory_transaction("memory.profile.update", &profile_params, |store| {
                let profile =
                    store.apply_user_profile_update(macbot_memory::UserProfileUpdate {
                        timezone: "UTC".into(),
                        expected_version: 1,
                        name: Some(Some("Ari".into())),
                        language: None,
                        identity: None,
                        updated_by: macbot_memory::MemorySender::User,
                    })?;
                let profile = protocol_user_profile(&profile);
                let result = json!({"profile":profile});
                Ok((result.clone(), "memory.profile.updated".into(), result))
            })
            .unwrap();
        drop(service);
        let service = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        let profile_retry = service
            .user_memory_rpc("memory.profile.update", profile_params.clone(), "UTC")
            .unwrap();
        assert_eq!(profile_retry.result, prepared_profile.receipt.result);
        assert_eq!(profile_retry.events.len(), 1);
        assert!(service
            .user_memory_rpc("memory.profile.update", profile_params, "UTC")
            .unwrap()
            .events
            .is_empty());
        assert_eq!(service.shared_store.events_since(0).unwrap().len(), 4);
    }

    #[test]
    fn user_memory_rpc_reports_quota_full_at_the_user_limit() {
        let service = service();
        let content = "x".repeat(500);
        for _ in 0..3 {
            service
                .user_memory_rpc("memory.user.create", json!({"content":content}), "UTC")
                .unwrap();
        }
        let error = service
            .user_memory_rpc("memory.user.create", json!({"content":"x"}), "UTC")
            .unwrap_err();
        assert_eq!(error.code(), "quota_full");
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
    fn feature_skill_roots_follow_settings_and_filesystem_changes() {
        let service = service();
        let extra = service.home.join("extra-skills");
        std::fs::create_dir_all(extra.join("external")).unwrap();
        std::fs::write(extra.join("external/SKILL.md"), skill("external")).unwrap();
        let skills = service.set_skill_extra_dirs(vec![extra.clone()]).unwrap();
        assert!(skills.iter().any(|item| item.name == "external"));
        std::fs::create_dir_all(extra.join("second")).unwrap();
        std::fs::write(extra.join("second/SKILL.md"), skill("second")).unwrap();
        assert!(service.refresh_skills().unwrap().is_some());
        assert!(service.skill_registry.read().unwrap().get("second").is_ok());
    }

    #[test]
    fn skill_metadata_survives_feature_service_restart() {
        let home = tempfile::tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        service
            .skill_rpc(
                "skill.create",
                json!({"name":"persistent","content":skill("persistent")}),
            )
            .unwrap();
        service
            .skill_registry
            .write()
            .unwrap()
            .set_enabled("persistent", false, Some("bot-a"))
            .unwrap();
        service
            .record_skill_invocation("persistent", Some("bot-a"))
            .unwrap();
        service.persist_skills().unwrap();
        drop(service);

        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        let skill = restored
            .skill_registry
            .read()
            .unwrap()
            .get("persistent")
            .unwrap()
            .skill;
        assert!(!restored
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("persistent", Some("bot-a"))
            .unwrap());
        assert_eq!(skill.invocations_7d.total, 1);
    }

    #[test]
    fn skill_update_preserves_enablement_after_persist_and_restart() {
        let home = tempfile::tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        service
            .skill_rpc(
                "skill.create",
                json!({"name":"editable","content":skill("editable")}),
            )
            .unwrap();
        service
            .skill_rpc(
                "skill.set_enabled",
                json!({"name":"editable","enabled":false}),
            )
            .unwrap();
        service
            .skill_rpc(
                "skill.set_enabled",
                json!({"name":"editable","enabled":false,"bot_id":"bot-a"}),
            )
            .unwrap();
        let updated = service
            .skill_rpc(
                "skill.update",
                json!({
                    "name":"editable",
                    "content":"---\nname: editable\ndescription: updated\n---\n# updated\n"
                }),
            )
            .unwrap();
        assert_eq!(updated.result["skill"]["enabled"], false);
        assert_eq!(
            updated.result["skill"]["disabled_bot_ids"],
            json!(["bot-a"])
        );
        drop(service);

        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        let skill = restored
            .skill_registry
            .read()
            .unwrap()
            .get("editable")
            .unwrap()
            .skill;
        assert!(!skill.enabled);
        assert_eq!(skill.disabled_bot_ids, vec!["bot-a"]);
        assert!(!restored
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("editable", None)
            .unwrap());
        assert!(!restored
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("editable", Some("bot-a"))
            .unwrap());
    }

    #[test]
    fn duplicate_bot_skill_settings_persist_without_copying_invocations() {
        let service = service();
        service
            .skill_rpc(
                "skill.create",
                json!({"name":"duplicate-skill","content":skill("duplicate-skill")}),
            )
            .unwrap();
        service
            .skill_registry
            .write()
            .unwrap()
            .set_enabled("duplicate-skill", false, Some("source-bot"))
            .unwrap();
        service
            .record_skill_invocation("duplicate-skill", Some("source-bot"))
            .unwrap();
        service
            .duplicate_bot_skill_settings("source-bot", "target-bot")
            .unwrap();
        let skill = service
            .skill_registry
            .read()
            .unwrap()
            .get("duplicate-skill")
            .unwrap()
            .skill;
        assert!(skill.disabled_bot_ids.iter().any(|id| id == "target-bot"));
        assert_eq!(skill.invocations_7d.total, 1);
    }

    #[test]
    fn duplicate_bot_skill_settings_can_be_rolled_back() {
        let service = service();
        service
            .skill_rpc(
                "skill.create",
                json!({"name":"rollback-skill","content":skill("rollback-skill")}),
            )
            .unwrap();
        service
            .skill_registry
            .write()
            .unwrap()
            .set_enabled("rollback-skill", false, Some("source-bot"))
            .unwrap();
        let snapshot = service
            .prepare_duplicate_bot_skill_settings("source-bot", "target-bot")
            .unwrap();
        assert!(!service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("rollback-skill", Some("target-bot"))
            .unwrap());
        service
            .restore_duplicate_bot_skill_settings(&snapshot)
            .unwrap();
        assert!(service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("rollback-skill", Some("target-bot"))
            .unwrap());
        let home = service.home.clone();
        drop(service);
        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        assert!(restored
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("rollback-skill", Some("target-bot"))
            .unwrap());
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

    #[test]
    fn project_summary_is_idempotent_and_survives_restart() {
        let home = tempfile::tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let first = service
            .finalize_project_summary("project-a", "bot-a", "最终项目结论")
            .unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(service.shared_memory.entries().unwrap().len(), 2);
        let retry = service
            .finalize_project_summary("project-a", "bot-a", "最终项目结论")
            .unwrap();
        assert_eq!(retry, first);
        assert_eq!(service.shared_memory.entries().unwrap().len(), 2);
        let updated = service
            .finalize_project_summary("project-a", "bot-a", "第二次项目结论")
            .unwrap();
        assert_eq!(updated.len(), 2);
        assert_eq!(service.shared_memory.entries().unwrap().len(), 2);
        let retry_updated = service
            .finalize_project_summary("project-a", "bot-a", "第二次项目结论")
            .unwrap();
        assert_eq!(retry_updated, updated);
        drop(service);

        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        let entries = restored.shared_memory.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| {
            entry.target == MemoryTarget::project("project-a")
                && entry.kind == MemoryKind::Project
                && entry.content == "第二次项目结论"
        }));
        assert!(!entries
            .iter()
            .any(|entry| entry.target == MemoryTarget::bot("bot-a")));
        assert!(entries.iter().any(|entry| {
            entry.id == "project-summary-worklog:project-a:main"
                && entry.target == MemoryTarget::bot("main")
                && entry.content == "第二次项目结论"
        }));
    }

    #[test]
    fn main_project_summary_writes_worklog_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let first = service
            .finalize_project_summary("project-main", "main", "项目完成，产物 artifacts/report.md")
            .unwrap();
        assert_eq!(first.len(), 2);
        let entries = service.shared_memory.entries().unwrap();
        let summary = entries
            .iter()
            .find(|entry| entry.id == "project-summary:project-main")
            .unwrap();
        let worklog = entries
            .iter()
            .find(|entry| entry.id == "project-summary-worklog:project-main:main")
            .unwrap();
        assert_eq!(summary.target, MemoryTarget::project("project-main"));
        assert_eq!(worklog.target, MemoryTarget::bot("main"));
        assert_eq!(worklog.kind, MemoryKind::BotWorklog);
        assert!(worklog.content.contains("artifacts/report.md"));
        assert_eq!(
            service
                .finalize_project_summary("project-main", "main", &summary.content)
                .unwrap(),
            first
        );
        drop(service);

        let restored = FeatureService::open(home, Vec::<PathBuf>::new()).unwrap();
        assert!(restored
            .shared_memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.id == "project-summary-worklog:project-main:main"));
    }

    #[test]
    fn project_summary_ignores_full_bot_worklog_quota() {
        let service = service();
        service
            .shared_memory
            .add(MemoryRequest {
                target: MemoryTarget::bot("main"),
                action: MemoryAction::Add,
                content: "x".repeat(macbot_memory::BOT_WORKLOG_LIMIT),
                id: Some("full-main-worklog".into()),
                kind: Some(MemoryKind::BotWorklog),
                source: MemorySource::default(),
            })
            .unwrap();

        let entries = service
            .finalize_project_summary("project-full-bot", "main", "项目完成总结")
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].target, MemoryTarget::project("project-full-bot"));
        assert!(service
            .shared_memory
            .entries()
            .unwrap()
            .iter()
            .any(|entry| entry.id == "full-main-worklog"));
        let retry = service
            .finalize_project_summary("project-full-bot", "main", "项目完成总结")
            .unwrap();
        assert_eq!(retry, entries);
        assert_eq!(service.shared_memory.entries().unwrap().len(), 2);
    }

    #[test]
    fn project_summary_retry_keeps_legacy_worklog_entries() {
        let service = service();
        service
            .shared_memory
            .add(MemoryRequest {
                target: MemoryTarget::bot("bot-a"),
                action: MemoryAction::Add,
                content: "legacy project summary worklog".into(),
                id: Some("project-summary-worklog:project-legacy:bot-a".into()),
                kind: Some(MemoryKind::BotWorklog),
                source: MemorySource::default(),
            })
            .unwrap();

        let first = service
            .finalize_project_summary("project-legacy", "bot-a", "新的项目总结")
            .unwrap();
        let retry = service
            .finalize_project_summary("project-legacy", "bot-a", "新的项目总结")
            .unwrap();
        assert_eq!(first, retry);
        let entries = service.shared_memory.entries().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|entry| {
            entry.id == "project-summary-worklog:project-legacy:bot-a"
                && entry.content == "legacy project summary worklog"
        }));
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
                assistant_content: None,
                thinking: String::new(),
                tool_calls: Vec::new(),
                usage: TokenUsage::default(),
                stop_reason: "stop".into(),
            })
        }
    }

    struct FailingMaintenance;
    #[async_trait]
    impl AsyncMaintenanceProvider for FailingMaintenance {
        async fn summarize(&self, _: &str) -> Result<String, MemoryError> {
            Err(MemoryError::Provider("primary unavailable".into()))
        }
    }

    struct FailAfterFirst(Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait]
    impl AsyncMaintenanceProvider for FailAfterFirst {
        async fn summarize(&self, _: &str) -> Result<String, MemoryError> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok("summarized work".into())
            } else {
                Err(MemoryError::Provider("maintenance target failed".into()))
            }
        }
    }

    struct RecordingUsage(Arc<std::sync::Mutex<Vec<String>>>);
    #[async_trait]
    impl MaintenanceUsageSink for RecordingUsage {
        async fn record(
            &self,
            phase: &str,
            _: &MaintenanceUsageContext,
            _: &TokenUsage,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push(phase.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn maintenance_has_fallback_and_usage_phase_hooks() {
        let fallback = FallbackMaintenanceProvider::new(
            Arc::new(FailingMaintenance),
            Arc::new(ModelMaintenanceAdapter::new(
                Arc::new(FakeMaintenance),
                "fallback",
            )),
        );
        assert_eq!(fallback.summarize("work").await.unwrap(), "summarized work");

        let phases = Arc::new(std::sync::Mutex::new(Vec::new()));
        let adapter = ModelMaintenanceAdapter::new(Arc::new(FakeMaintenance), "mock")
            .with_usage_sink(Arc::new(RecordingUsage(Arc::clone(&phases))))
            .with_usage_context(MaintenanceUsageContext {
                request_id: "maintenance-1".into(),
                bot_id: "bot-a".into(),
                chat_id: "chat-a".into(),
                run_id: "run-a".into(),
                provider_id: "mock".into(),
                model_id: "mock".into(),
                ..Default::default()
            });
        assert_eq!(adapter.summarize("work").await.unwrap(), "summarized work");
        assert_eq!(adapter.compact("context").await.unwrap(), "summarized work");
        assert_eq!(&*phases.lock().unwrap(), &["memory", "compact"]);
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
        let (result, events) = service
            .maintain_worklog_with_events(&MemoryTarget::bot("bot-a"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.content, "summarized work");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "memory.updated");
        assert_eq!(
            service
                .shared_store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| event.event == "memory.updated")
                .count(),
            1
        );
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

    #[tokio::test]
    async fn maintenance_tick_keeps_prior_events_when_a_later_target_fails() {
        let service = service();
        for bot in ["bot-a", "bot-b"] {
            service
                .shared_memory
                .append_worklog(MemoryTarget::bot(bot), "first", MemorySource::default())
                .unwrap();
            service
                .shared_memory
                .append_worklog(MemoryTarget::bot(bot), "second", MemorySource::default())
                .unwrap();
        }
        service
            .set_maintenance_provider(Some(Arc::new(FailAfterFirst(Arc::new(
                std::sync::atomic::AtomicUsize::new(0),
            )))))
            .unwrap();

        let report = service
            .maintenance_tick_with_events(&[MemoryTarget::bot("bot-a"), MemoryTarget::bot("bot-b")])
            .await;

        assert_eq!(report.committed.len(), 1);
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].event, "memory.updated");
        assert!(report.error.is_some());
        assert_eq!(
            service
                .shared_store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| event.event == "memory.updated")
                .count(),
            1
        );
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
            .is_err());
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
        assert_eq!(service.shared_memory.entries().unwrap().len(), 1);
    }

    #[test]
    fn memory_rpc_overwrites_untrusted_provenance() {
        let service = service();
        let actor = MemoryActor::bot("bot-a");
        let access = MemoryAccess::user("user-a");
        let result = service
            .memory_rpc_with_access(
                &actor,
                &access,
                json!({
                    "scope": "bot",
                    "bot_id": "bot-a",
                    "action": "add",
                    "kind": "bot_experience",
                    "content": "source sentinel",
                    "run_id": "actual-run",
                    "source": {
                        "bot_id": "other-bot",
                        "run_id": "forged-run",
                        "session_id": "forged-session"
                    }
                }),
            )
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["version"], 1);
        let entries = service.shared_memory.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source.bot_id.as_deref(), Some("bot-a"));
        assert_eq!(entries[0].source.run_id.as_deref(), Some("actual-run"));
        assert_eq!(entries[0].source.session_id, None);
    }

    #[test]
    fn compact_summary_is_staged_until_run_success() {
        let service = service();
        let actor = MemoryActor::bot("bot-a");
        let access = MemoryAccess::user("u");
        service.begin_memory_run("compact-fail").unwrap();
        service
            .stage_context_summary(
                &actor,
                &access,
                "compact-fail",
                MemoryTarget::bot("bot-a"),
                "compacted context",
                MemorySource {
                    bot_id: Some("bot-a".into()),
                    run_id: Some("compact-fail".into()),
                    session_id: Some("chat".into()),
                },
            )
            .unwrap();
        assert!(service
            .memory_search("compacted", Some(&MemoryTarget::bot("bot-a")))
            .unwrap()
            .is_empty());
        service.rollback_memory_run("compact-fail").unwrap();
        assert!(service
            .memory_search("compacted", Some(&MemoryTarget::bot("bot-a")))
            .unwrap()
            .is_empty());

        service.begin_memory_run("compact-ok").unwrap();
        service
            .stage_context_summary(
                &actor,
                &access,
                "compact-ok",
                MemoryTarget::bot("bot-a"),
                "compacted context",
                MemorySource::default(),
            )
            .unwrap();
        service.commit_memory_run("compact-ok").unwrap();
        assert_eq!(
            service
                .memory_search("compacted", Some(&MemoryTarget::bot("bot-a")))
                .unwrap()
                .len(),
            1
        );
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
        assert!(!kinds.contains("object"));
        assert!(service
            .search_rpc(json!({"query":"needle","kinds":["object"]}))
            .is_err());
        service.shared_store.write_snapshot(
            "data/jobs/internal.json",
            &json!({"id":"job-internal","checkpoint":{"messages":[{"role":"system","content":"internal-prompt-only"}]}}),
        ).unwrap();
        assert!(service
            .search_rpc(json!({"query":"internal-prompt-only"}))
            .unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty());
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
    fn search_indexes_chat_messages_without_path_classifying_internal_records() {
        let service = service();
        let marker = "search-memory-regression-marker";
        service
            .shared_store
            .append_jsonl(
                "data/chats/chat_main/messages.jsonl",
                &json!({
                    "id": "chat-message-1",
                    "chat_id": "chat_main",
                    "fallback_text": marker,
                    "created_at": "2026-01-01T00:00:00Z"
                }),
            )
            .unwrap();
        service
            .shared_store
            .append_jsonl(
                "data/traces/chat_main.jsonl",
                &json!({
                    "type": "trace.item",
                    "data": {
                        "details": {
                            "entries": [
                                {
                                    "id": "memory-entry-1",
                                    "kind": "user_preference",
                                    "content": marker
                                },
                                {
                                    "id": "internal-message-1",
                                    "fallback_text": marker
                                }
                            ]
                        }
                    }
                }),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/run_requests/run_chat_internal.json",
                &json!({"messages":[{"role":"user","content":marker}]}),
            )
            .unwrap();

        let result = service
            .search_rpc(json!({"query": marker, "limit": 20}))
            .unwrap();
        let hits: Vec<SearchHit> = serde_json::from_value(result["results"].clone()).unwrap();
        assert_eq!(
            hits.iter().filter(|hit| hit.id == "chat-message-1").count(),
            1
        );
        assert!(hits.iter().any(|hit| hit.id == "chat-message-1"
            && hit.kind == "message"
            && hit.chat_id.as_deref() == Some("chat_main")));
        assert!(!hits.iter().any(|hit| hit.id == "memory-entry-1"));
        assert!(!hits.iter().any(|hit| hit.id == "internal-message-1"));
        assert!(!hits.iter().any(|hit| hit.id == "run_chat_internal"));

        let filtered = service
            .search_rpc(json!({"query": marker, "kinds":["message"], "limit": 20}))
            .unwrap();
        let message_hits: Vec<SearchHit> =
            serde_json::from_value(filtered["results"].clone()).unwrap();
        assert_eq!(message_hits.len(), 1);
        assert_eq!(message_hits[0].id, "chat-message-1");
        assert!(service
            .search_rpc(json!({"query": marker, "kinds":["chat"], "limit": 20}))
            .unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn bot_search_upserts_state_objects_and_survives_restart() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/bots/history.json",
                &json!([{
                    "id":"bot-1",
                    "name":"集成复现旧名称",
                    "description":"旧描述",
                    "max_parallel":1,
                    "updated_at":"2026-01-01T00:00:00Z"
                }]),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/orchestrator/state.json",
                &json!({"bots":[{
                    "id":"bot-1",
                    "name":"集成复现新名称",
                    "description":"当前描述",
                    "max_parallel":2,
                    "updated_at":"2026-01-02T00:00:00Z"
                }]}),
            )
            .unwrap();
        let result = service
            .search_rpc(json!({"query":"集成复现","kinds":["bot"]}))
            .unwrap();
        let hits: Vec<SearchHit> = serde_json::from_value(result["results"].clone()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "bot-1");
        assert!(hits[0].snippet.contains("集成复现新名称"));
        assert!(hits[0].snippet.contains("当前描述"));
        assert!(!hits[0].snippet.starts_with('{'));
        drop(service);

        let restored = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let result = restored
            .search_rpc(json!({"query":"集成复现","kinds":["bot"]}))
            .unwrap();
        let hits: Vec<SearchHit> = serde_json::from_value(result["results"].clone()).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("当前描述"));
        let stale = restored
            .search_rpc(json!({"query":"集成复现旧名称","kinds":["bot"]}))
            .unwrap();
        let stale_hits: Vec<SearchHit> = serde_json::from_value(stale["results"].clone()).unwrap();
        assert!(stale_hits.is_empty());

        restored
            .shared_store
            .write_snapshot(
                "data/orchestrator/state.json",
                &json!({"bots":[{
                    "id":"bot-1",
                    "name":"集成复现已重命名",
                    "description":"更新后的描述",
                    "max_parallel":3,
                    "updated_at":"2026-01-03T00:00:00Z"
                }]}),
            )
            .unwrap();
        let result = restored
            .search_rpc(json!({"query":"集成复现已重命名","kinds":["bot"]}))
            .unwrap();
        let hits: Vec<SearchHit> = serde_json::from_value(result["results"].clone()).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("更新后的描述"));
    }

    #[test]
    fn search_filters_deleted_objects_against_live_state_and_keeps_no_state_fixtures() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/orchestrator/state.json",
                &json!({
                    "bots": {"bot-live": {"id":"bot-live","name":"保留机器人","max_parallel":1}},
                    "routines": {"routine-live": {"routine_id":"routine-live","name":"保留例行任务","schedules":[]}},
                    "artifacts": {"artifact-live": {"artifact_id":"artifact-live","title":"保留产物","path_or_url":"/tmp/live"}}
                }),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/orchestrator/history.json",
                &json!({
                    "bot": {"id":"bot-gone","name":"gone bot needle","max_parallel":1},
                    "routine": {"routine_id":"routine-gone","name":"gone routine needle","schedules":[]},
                    "artifact": {"artifact_id":"artifact-gone","title":"gone artifact needle","path_or_url":"/tmp/gone"}
                }),
            )
            .unwrap();
        service
            .shared_store
            .write_snapshot(
                "data/events/messages.json",
                &json!({"type":"message.deleted","data":{"message_id":"message-gone","fallback_text":"gone message needle"}}),
            )
            .unwrap();
        drop(service);

        let restored = FeatureService::open(home.clone(), Vec::<PathBuf>::new()).unwrap();
        let live = restored
            .search_rpc(json!({"query":"保留","kinds":["bot","routine","artifact"],"limit":10}))
            .unwrap();
        let live_hits: Vec<SearchHit> = serde_json::from_value(live["results"].clone()).unwrap();
        assert_eq!(live_hits.len(), 3);
        for query in ["gone bot", "gone routine", "gone artifact", "gone message"] {
            let result = restored
                .search_rpc(json!({"query":query,"limit":10}))
                .unwrap();
            let hits: Vec<SearchHit> = serde_json::from_value(result["results"].clone()).unwrap();
            assert!(
                hits.is_empty(),
                "deleted search result for {query}: {hits:?}"
            );
        }

        fs::remove_file(home.join("data/orchestrator/state.json")).unwrap();
        let fixture = restored
            .search_rpc(json!({"query":"gone bot","kinds":["bot"],"limit":10}))
            .unwrap();
        let fixture_hits: Vec<SearchHit> =
            serde_json::from_value(fixture["results"].clone()).unwrap();
        assert_eq!(fixture_hits.len(), 1);
        assert_eq!(fixture_hits[0].id, "bot-gone");
    }

    #[test]
    fn conversation_batch_preserves_order_and_persists_all_messages() {
        let service = service();
        let messages: Vec<_> = (0..120)
            .map(|index| SessionMessage {
                id: format!("message-{index}"),
                session_id: "session".into(),
                chat_id: "chat".into(),
                project_id: Some("project".into()),
                role: "user".into(),
                content: format!("content-{index}"),
                at: Utc::now(),
            })
            .collect();
        service.add_session_messages(messages.clone()).unwrap();
        service.add_session_messages(Vec::new()).unwrap();
        let persisted: ConversationIndex = service
            .shared_store
            .read_snapshot(CONVERSATIONS_FILE)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.chat_history("chat", 200), messages);
        assert_eq!(service.chat_history("chat", 200).unwrap(), messages[20..]);
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
