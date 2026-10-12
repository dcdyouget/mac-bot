//! Server-owned memory, search and prompt-context primitives.
//!
//! The gateway owns authorization and chooses the target identifiers.  This
//! crate owns quotas, immediate persistence, stable snapshots and the small search index
//! used by memory/session/project tools.

use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};
use thiserror::Error;
use uuid::Uuid;

pub const USER_LIMIT: usize = 1_500;
pub const BOT_EXPERIENCE_LIMIT: usize = 2_000;
pub const BOT_WORKLOG_LIMIT: usize = 1_500;
pub const PROJECT_LIMIT: usize = 3_000;
pub const SNAPSHOT_MAX_AGE_HOURS: i64 = 24;
pub const GROUP_RECENT_MESSAGES: usize = 30;
pub const SEGMENT_RETENTION_DAYS: i64 = 7;

/// Stable L0 platform rules shared by every context. Keep this string fixed
/// across turns so prompt caching remains effective; dynamic Bot/project data
/// belongs in L1/L3.
pub const DEFAULT_L0_PLATFORM_RULES: &str = r#"Platform rules:
- Use tools only through their declared schemas. Return tool errors to the model so it can correct them.
- Bot-to-Bot communication uses send_msg. Every message must be self-contained; do not rely on private model context.
- Main-Bot-only actions (project creation, member assignment, and cross-Bot coordination) are unavailable to ordinary Bots.
- Before a write, exec, external request, browser payment, sudo, destructive rm, or git push, follow the approval policy.
- When corrected or when you learn a durable lesson, record Bot experience in Bot memory.
- When a group reaches a durable conclusion, record it in project memory.
- Do not write user preferences; the memory curator handles them.
- Store worklogs as a concise summary of what was done and the result, not as a full reply.
- A forced /skill-name prefix loads that skill before the remaining text is treated as the instruction."#;

pub fn default_l0_platform_rules() -> &'static str {
    DEFAULT_L0_PLATFORM_RULES
}

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("memory quota exceeded for {scope}: {used} + {requested} > {limit} characters")]
    QuotaExceeded {
        scope: String,
        used: usize,
        requested: usize,
        limit: usize,
    },
    #[error("user memory quota is full: {used} + {requested} > {limit} characters")]
    UserQuotaFull {
        used: usize,
        requested: usize,
        limit: usize,
    },
    #[error("memory entry not found: {0}")]
    NotFound(String),
    #[error("memory entry version conflict")]
    UserConflict(Box<UserMemoryRecord>),
    #[error("memory entry version conflict")]
    VersionConflict {
        current: Box<MemoryEntry>,
        version: u64,
    },
    #[error("memory profile version conflict")]
    ProfileConflict(Box<UserProfile>),
    #[error("idempotency key was reused with different request content: {0}")]
    IdempotencyConflict(String),
    #[error("invalid memory request: {0}")]
    Invalid(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("maintenance provider failed: {0}")]
    Provider(String),
    #[error("memory store lock poisoned")]
    LockPoisoned,
    #[error(
        "memory change log no longer covers revision {seen_revision}; earliest available revision is {earliest_revision}"
    )]
    ChangeLogGap {
        seen_revision: u64,
        earliest_revision: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    User,
    Bot,
    Project,
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::User => "user",
            Self::Bot => "bot",
            Self::Project => "project",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    UserPreference,
    UserProfile,
    BotExperience,
    BotWorklog,
    Project,
}

impl MemoryKind {
    fn default_for(scope: &MemoryScope) -> Self {
        match scope {
            MemoryScope::User => Self::UserPreference,
            MemoryScope::Bot => Self::BotExperience,
            MemoryScope::Project => Self::Project,
        }
    }
    fn valid_for(&self, scope: &MemoryScope) -> bool {
        matches!(
            (scope, self),
            (MemoryScope::User, Self::UserPreference)
                | (MemoryScope::User, Self::UserProfile)
                | (MemoryScope::Bot, Self::BotExperience | Self::BotWorklog)
                | (MemoryScope::Project, Self::Project)
        )
    }
    fn limit(&self) -> usize {
        match self {
            Self::UserPreference => USER_LIMIT,
            Self::UserProfile => 0,
            Self::BotExperience => BOT_EXPERIENCE_LIMIT,
            Self::BotWorklog => BOT_WORKLOG_LIMIT,
            Self::Project => PROJECT_LIMIT,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MemoryTarget {
    pub scope: MemoryScope,
    pub owner_id: String,
}

impl MemoryTarget {
    pub fn user(id: impl Into<String>) -> Self {
        Self {
            scope: MemoryScope::User,
            owner_id: id.into(),
        }
    }
    pub fn bot(id: impl Into<String>) -> Self {
        Self {
            scope: MemoryScope::Bot,
            owner_id: id.into(),
        }
    }
    pub fn project(id: impl Into<String>) -> Self {
        Self {
            scope: MemoryScope::Project,
            owner_id: id.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemorySource {
    pub bot_id: Option<String>,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
}

/// Authenticated visibility supplied by the gateway for one model run.
/// Memory targets are never authorized from model-provided ids alone.
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

    pub fn group_for_user(
        user_id: impl Into<String>,
        project_id: impl Into<String>,
        member_bot_ids: Vec<String>,
    ) -> Self {
        Self {
            user_id: Some(user_id.into()),
            project_id: Some(project_id.into()),
            project_member_bot_ids: member_bot_ids,
        }
    }

    pub fn is_project_member(&self, bot_id: &str, project_id: &str) -> bool {
        self.project_id.as_deref() == Some(project_id)
            && self
                .project_member_bot_ids
                .iter()
                .any(|member| member == bot_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEntry {
    pub id: String,
    pub target: MemoryTarget,
    pub kind: MemoryKind,
    pub content: String,
    #[serde(default = "initial_entry_version")]
    pub version: u64,
    pub source: MemorySource,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn initial_entry_version() -> u64 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOperation {
    Add,
    Replace,
    Remove,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryChangeKind {
    UserPreference,
    UserProfile,
    BotExperience,
    ProjectKnowledge,
    Worklog,
}

impl From<&MemoryKind> for MemoryChangeKind {
    fn from(kind: &MemoryKind) -> Self {
        match kind {
            MemoryKind::UserPreference => Self::UserPreference,
            MemoryKind::UserProfile => Self::UserProfile,
            MemoryKind::BotExperience => Self::BotExperience,
            MemoryKind::Project => Self::ProjectKnowledge,
            MemoryKind::BotWorklog => Self::Worklog,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryChangeActorKind {
    User,
    Bot,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryChangeActor {
    pub kind: MemoryChangeActorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl Default for MemoryChangeActor {
    fn default() -> Self {
        Self {
            kind: MemoryChangeActorKind::System,
            bot_id: None,
            run_id: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryChange {
    pub operation_id: String,
    pub revision: u64,
    pub op: MemoryOperation,
    pub entry_id: String,
    pub target: MemoryTarget,
    pub kind: MemoryChangeKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_content: Option<String>,
    pub actor: MemoryChangeActor,
    pub source: MemoryChangeSource,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_state: Option<Box<PersistedState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySegmentRecord {
    pub run_id: String,
    #[serde(default)]
    pub segment_no: u64,
    pub seen_revision: u64,
    #[serde(default)]
    pub delta_chars: usize,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryChangeSource {
    pub bot_id: Option<String>,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub by: Option<MemorySender>,
    pub chat_id: Option<String>,
    pub message_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MemoryThreadSegmentRecord {
    thread_id: String,
    current_segment_id: String,
    segment_no: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryAction {
    Add,
    Replace,
    Remove,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryRequest {
    pub target: MemoryTarget,
    pub action: MemoryAction,
    pub content: String,
    pub id: Option<String>,
    pub kind: Option<MemoryKind>,
    pub source: MemorySource,
}

/// Protocol provenance retained for user-preference entries.  The existing
/// memory index remains the single source of entry content and quota usage;
/// this metadata adds the user-facing version and source details.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MemorySender {
    User,
    System,
    Bot { bot_id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserMemorySource {
    pub by: MemorySender,
    pub chat_id: Option<String>,
    pub message_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserMemoryRecord {
    pub id: String,
    pub content: String,
    pub version: u64,
    pub source: UserMemorySource,
    pub updated_by: MemorySender,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserProfile {
    pub name: Option<String>,
    pub language: Option<String>,
    pub identity: Option<String>,
    pub timezone: String,
    pub onboarded: bool,
    pub version: u64,
    pub updated_by: MemorySender,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct UserProfileUpdate {
    pub timezone: String,
    pub expected_version: u64,
    pub name: Option<Option<String>>,
    pub language: Option<Option<String>>,
    pub identity: Option<Option<String>>,
    pub updated_by: MemorySender,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryMutationResult {
    pub entry: Option<MemoryEntry>,
    pub version: u64,
    pub revision: u64,
    pub remaining_chars: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct UserMemoryMetadata {
    version: u64,
    source: Option<UserMemorySource>,
    updated_by: Option<MemorySender>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedState {
    #[serde(default)]
    entries: Vec<MemoryEntry>,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    user_metadata: BTreeMap<String, UserMemoryMetadata>,
    #[serde(default)]
    user_profile: Option<UserProfile>,
    #[serde(default)]
    operation_receipts: BTreeMap<String, MemoryOperationReceipt>,
    #[serde(default)]
    deleted_user_entries: BTreeMap<String, u64>,
    #[serde(default)]
    snapshot_cache: BTreeMap<String, MemorySnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryOperationReceipt {
    pub signature: String,
    pub result: serde_json::Value,
    pub event_name: String,
    pub event_data: serde_json::Value,
    #[serde(default)]
    pub audit_data: Option<serde_json::Value>,
}

#[derive(Clone, Debug)]
pub struct MemoryTransactionResult {
    pub receipt: MemoryOperationReceipt,
    pub replayed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    entries: BTreeMap<String, MemoryEntry>,
    user_metadata: BTreeMap<String, UserMemoryMetadata>,
    user_profile: Option<UserProfile>,
    operation_receipts: BTreeMap<String, MemoryOperationReceipt>,
    revision: u64,
    deleted_user_entries: BTreeMap<String, u64>,
    storage_path: Option<std::path::PathBuf>,
    snapshot_cache: BTreeMap<String, MemorySnapshot>,
}

/// Thread-safe facade for gateway handlers. Quota checks and mutations happen
/// under one write lock, so concurrent runs cannot both pass a stale quota
/// check. A run still stages independently and only its commit is atomic.
#[derive(Clone, Default)]
pub struct SharedMemoryStore {
    inner: Arc<RwLock<MemoryStore>>,
}

impl SharedMemoryStore {
    pub fn new(store: MemoryStore) -> Self {
        Self {
            inner: Arc::new(RwLock::new(store)),
        }
    }

    pub fn add(&self, request: MemoryRequest) -> Result<MemoryEntry, MemoryError> {
        self.inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .add(request)
    }

    pub fn append_worklog(
        &self,
        target: MemoryTarget,
        content: impl Into<String>,
        source: MemorySource,
    ) -> Result<MemoryEntry, MemoryError> {
        self.inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .append_worklog(target, content, source)
    }

    pub fn snapshot(
        &self,
        targets: &[MemoryTarget],
        previous: Option<&MemorySnapshot>,
        now: DateTime<Utc>,
    ) -> Result<(MemorySnapshot, bool), MemoryError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| MemoryError::LockPoisoned)?
            .snapshot(targets, previous, now))
    }

    pub fn search(
        &self,
        query: &str,
        target: Option<&MemoryTarget>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| MemoryError::LockPoisoned)?
            .search(query, target))
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), MemoryError> {
        self.inner
            .read()
            .map_err(|_| MemoryError::LockPoisoned)?
            .save_to(path)
    }

    /// Assemble a context package while holding a consistent read view of the
    /// memory snapshot. Runtime callers should use this facade instead of
    /// reaching into the private `MemoryStore` held by the shared lock.
    pub fn assemble_context(
        &self,
        request: &ContextRequest,
    ) -> Result<ContextPackage, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok(assemble_context(&store, request))
    }

    pub fn revision(&self) -> Result<u64, MemoryError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| MemoryError::LockPoisoned)?
            .revision())
    }

    pub fn changes_since(&self, seen_revision: u64) -> Result<Vec<MemoryChange>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        let path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        read_memory_changes(path, seen_revision)
    }

    pub fn record_segment_seen(&self, run_id: &str, seen_revision: u64) -> Result<(), MemoryError> {
        let store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        let state_path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        write_segment_seen(state_path, run_id, seen_revision)
    }

    pub fn current_segment_id(&self, thread_id: &str) -> Result<Option<String>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        let state_path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        read_thread_segment(state_path, thread_id)
            .map(|record| record.map(|item| item.current_segment_id))
    }

    pub fn open_segment(&self, thread_id: &str, seen_revision: u64) -> Result<String, MemoryError> {
        let store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        let state_path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        let prior = read_thread_segment(state_path, thread_id)?;
        let segment_no = prior
            .map(|item| item.segment_no.saturating_add(1))
            .unwrap_or(1);
        let segment_id = Uuid::now_v7().to_string();
        write_segment_seen(state_path, &segment_id, seen_revision)?;
        if let Some(mut segment) = read_segment_record(state_path, &segment_id)? {
            segment.segment_no = segment_no;
            write_segment_record(state_path, &segment)?;
        }
        write_thread_segment(
            state_path,
            &MemoryThreadSegmentRecord {
                thread_id: thread_id.to_owned(),
                current_segment_id: segment_id.clone(),
                segment_no,
            },
        )?;
        Ok(segment_id)
    }

    pub fn segment_delta_chars(&self, run_id: &str) -> Result<usize, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        let path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        Ok(read_segment_record(path, run_id)?
            .map(|record| record.delta_chars)
            .unwrap_or(0))
    }

    pub fn add_segment_delta_chars(
        &self,
        run_id: &str,
        count: usize,
    ) -> Result<usize, MemoryError> {
        let store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        let state_path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        let mut record = read_segment_record(state_path, run_id)?.unwrap_or(MemorySegmentRecord {
            run_id: run_id.to_owned(),
            segment_no: 0,
            seen_revision: store.revision,
            delta_chars: 0,
            updated_at: Utc::now(),
        });
        record.delta_chars = record.delta_chars.saturating_add(count);
        record.updated_at = Utc::now();
        write_segment_record(state_path, &record)?;
        Ok(record.delta_chars)
    }

    pub fn snapshot_cache_get(
        &self,
        cache_key: &str,
    ) -> Result<Option<MemorySnapshot>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok(store.snapshot_cache.get(cache_key).cloned())
    }

    pub fn snapshot_cache_put(
        &self,
        cache_key: impl Into<String>,
        snapshot: MemorySnapshot,
    ) -> Result<(), MemoryError> {
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        store.snapshot_cache.insert(cache_key.into(), snapshot);
        if let Some(path) = store.storage_path.as_ref() {
            store.save_to(path)?;
        }
        Ok(())
    }

    pub fn segment_seen_revision(&self, run_id: &str) -> Result<Option<u64>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        let state_path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        read_segment_seen(state_path, run_id)
    }

    pub fn prune_expired_segments(&self, now: DateTime<Utc>) -> Result<(), MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        let state_path = store
            .storage_path
            .as_deref()
            .ok_or_else(|| MemoryError::Invalid("memory store has no storage path".into()))?;
        prune_expired_memory_files(state_path, now)
    }

    pub fn entries(&self) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(self
            .inner
            .read()
            .map_err(|_| MemoryError::LockPoisoned)?
            .entries()
            .cloned()
            .collect())
    }

    pub fn entry_version(&self, entry_id: &str) -> Result<Option<u64>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok(store
            .entries
            .get(entry_id)
            .map(|entry| entry.version.max(1)))
    }

    pub fn user_records(
        &self,
        target: &MemoryTarget,
    ) -> Result<Vec<UserMemoryRecord>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok(store.user_records(target))
    }

    pub fn user_quota(&self, target: &MemoryTarget) -> Result<(usize, usize), MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok((store.used(target, &MemoryKind::UserPreference), USER_LIMIT))
    }

    pub fn operation_receipts(&self) -> Result<Vec<(String, MemoryOperationReceipt)>, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok(store
            .operation_receipts
            .iter()
            .map(|(id, receipt)| (id.clone(), receipt.clone()))
            .collect())
    }

    pub fn has_operation_receipt(&self, request_id: &str) -> Result<bool, MemoryError> {
        let store = self.inner.read().map_err(|_| MemoryError::LockPoisoned)?;
        Ok(store.operation_receipts.contains_key(request_id))
    }

    /// Persist a user-memory mutation together with its response and event
    /// intent. A retry can recover the receipt even if the process stopped
    /// before appending the corresponding event.
    pub fn transact_user_persisted<F>(
        &self,
        path: impl AsRef<Path>,
        request_id: Option<&str>,
        signature: &str,
        operation: F,
    ) -> Result<MemoryTransactionResult, MemoryError>
    where
        F: FnOnce(
            &mut MemoryStore,
        ) -> Result<
            (
                serde_json::Value,
                String,
                serde_json::Value,
                Option<serde_json::Value>,
            ),
            MemoryError,
        >,
    {
        self.transact_user_persisted_internal(path, request_id, signature, None, operation)
    }

    pub fn transact_user_persisted_with_actor<F>(
        &self,
        path: impl AsRef<Path>,
        request_id: Option<&str>,
        signature: &str,
        actor: MemoryChangeActor,
        operation: F,
    ) -> Result<MemoryTransactionResult, MemoryError>
    where
        F: FnOnce(
            &mut MemoryStore,
        ) -> Result<
            (
                serde_json::Value,
                String,
                serde_json::Value,
                Option<serde_json::Value>,
            ),
            MemoryError,
        >,
    {
        self.transact_user_persisted_internal(path, request_id, signature, Some(actor), operation)
    }

    fn transact_user_persisted_internal<F>(
        &self,
        path: impl AsRef<Path>,
        request_id: Option<&str>,
        signature: &str,
        actor: Option<MemoryChangeActor>,
        operation: F,
    ) -> Result<MemoryTransactionResult, MemoryError>
    where
        F: FnOnce(
            &mut MemoryStore,
        ) -> Result<
            (
                serde_json::Value,
                String,
                serde_json::Value,
                Option<serde_json::Value>,
            ),
            MemoryError,
        >,
    {
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        if let Some(request_id) = request_id {
            if let Some(receipt) = store.operation_receipts.get(request_id) {
                if receipt.signature != signature {
                    return Err(MemoryError::IdempotencyConflict(request_id.to_owned()));
                }
                return Ok(MemoryTransactionResult {
                    receipt: receipt.clone(),
                    replayed: true,
                });
            }
        }
        let before = store.clone_for_transaction();
        let mut candidate = before.clone_for_transaction();
        let (result, event_name, event_data, audit_data) = operation(&mut candidate)?;
        let op_id = request_id
            .map(str::to_owned)
            .unwrap_or_else(|| format!("txn:{}", Uuid::now_v7()));
        let receipt = MemoryOperationReceipt {
            signature: signature.to_owned(),
            result,
            event_name,
            event_data,
            audit_data,
        };
        if let Some(request_id) = request_id {
            candidate
                .operation_receipts
                .insert(request_id.to_owned(), receipt.clone());
        }
        append_state_diff(path.as_ref(), &before, &mut candidate, &op_id, actor)?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(MemoryTransactionResult {
            receipt,
            replayed: false,
        })
    }

    /// Apply a model memory-tool mutation immediately with optional CAS.
    /// The snapshot is written before publishing the candidate in memory.
    pub fn mutate_persisted(
        &self,
        path: impl AsRef<Path>,
        request: MemoryRequest,
        expected_version: Option<u64>,
        operation_id: &str,
        actor: MemoryChangeActor,
    ) -> Result<MemoryMutationResult, MemoryError> {
        validate_request(&request)?;
        if operation_id.trim().is_empty() {
            return Err(MemoryError::Invalid("operation_id is empty".into()));
        }
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        let signature = serde_json::to_string(&(&request, expected_version, &actor))?;
        if let Some(receipt) = store.operation_receipts.get(operation_id) {
            if receipt.signature != signature {
                return Err(MemoryError::IdempotencyConflict(operation_id.to_owned()));
            }
            return Ok(serde_json::from_value(receipt.result.clone())?);
        }
        let mut candidate = store.clone_for_transaction();
        let prior = match &request.action {
            MemoryAction::Add => None,
            MemoryAction::Replace | MemoryAction::Remove => {
                let id = request
                    .id
                    .as_deref()
                    .ok_or_else(|| MemoryError::Invalid("mutation requires id".into()))?;
                let current = candidate
                    .entries
                    .get(id)
                    .filter(|entry| entry.target == request.target)
                    .cloned()
                    .ok_or_else(|| MemoryError::NotFound(id.to_owned()))?;
                let version = candidate
                    .entries
                    .get(id)
                    .map(|entry| entry.version.max(1))
                    .unwrap_or(1);
                if expected_version != Some(version) {
                    return Err(MemoryError::UserConflict(Box::new(
                        candidate.user_record(&current),
                    )));
                }
                Some(current)
            }
        };
        let changed = candidate.apply(request.clone(), expected_version)?;
        candidate.revision = candidate.revision.wrapping_add(1);
        if matches!(&request.action, MemoryAction::Remove)
            && prior
                .as_ref()
                .is_some_and(|entry| entry.kind == MemoryKind::UserPreference)
        {
            if let Some(entry) = &prior {
                candidate
                    .deleted_user_entries
                    .insert(entry.id.clone(), candidate.revision);
            }
        }
        let (entry, version, kind) = match &request.action {
            MemoryAction::Add => {
                let entry = changed.into_iter().next().expect("add creates entry");
                (Some(entry.clone()), 1, entry.kind)
            }
            MemoryAction::Replace => {
                let entry = changed.into_iter().next().expect("replace returns entry");
                let version = candidate
                    .user_metadata
                    .get(&entry.id)
                    .map(|metadata| metadata.version.max(1))
                    .unwrap_or_else(|| prior.as_ref().map(|_| 2).unwrap_or(1));
                (Some(entry.clone()), version, entry.kind)
            }
            MemoryAction::Remove => {
                let old = prior.as_ref().expect("remove has an existing entry");
                let version = expected_version.unwrap_or(1).saturating_add(1);
                (None, version, old.kind.clone())
            }
        };
        let used = candidate.used(&request.target, &kind);
        let remaining_chars = kind.limit().saturating_sub(used);
        let result = MemoryMutationResult {
            entry: entry.clone(),
            version,
            revision: candidate.revision,
            remaining_chars,
        };
        let changed_entry = entry
            .as_ref()
            .or(prior.as_ref())
            .expect("mutation has an entry");
        let mut change = MemoryChange {
            operation_id: operation_id.to_owned(),
            revision: candidate.revision,
            op: match &request.action {
                MemoryAction::Add => MemoryOperation::Add,
                MemoryAction::Replace => MemoryOperation::Replace,
                MemoryAction::Remove => MemoryOperation::Remove,
            },
            entry_id: changed_entry.id.clone(),
            target: changed_entry.target.clone(),
            kind: MemoryChangeKind::from(&changed_entry.kind),
            content: entry
                .as_ref()
                .map(|item| item.content.clone())
                .unwrap_or_default(),
            prev_content: prior.as_ref().map(|item| item.content.clone()),
            actor,
            source: change_source(
                changed_entry,
                candidate.user_metadata.get(&changed_entry.id),
            ),
            at: Utc::now(),
            recovery_state: None,
            legacy_payload: None,
        };
        candidate.operation_receipts.insert(
            operation_id.to_owned(),
            MemoryOperationReceipt {
                signature,
                result: serde_json::to_value(&result)?,
                event_name: "memory.updated".to_owned(),
                event_data: serde_json::to_value(change_without_recovery(&change))?,
                audit_data: None,
            },
        );
        change.recovery_state = Some(Box::new(candidate.persisted_state()));
        candidate
            .operation_receipts
            .get_mut(operation_id)
            .expect("receipt inserted")
            .event_data = serde_json::to_value(change_without_recovery(&change))?;
        append_memory_change(path.as_ref(), &change)?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(result)
    }

    pub fn user_create_persisted(
        &self,
        path: impl AsRef<Path>,
        target: MemoryTarget,
        content: String,
        by: MemorySender,
        chat_id: Option<String>,
        message_id: Option<String>,
    ) -> Result<UserMemoryRecord, MemoryError> {
        validate_user_content(&content)?;
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        let before = store.clone_for_transaction();
        let mut candidate = store.clone_for_transaction();
        let entry = match candidate.add(MemoryRequest {
            target,
            action: MemoryAction::Add,
            content,
            id: None,
            kind: Some(MemoryKind::UserPreference),
            source: source_for_sender(&by),
        }) {
            Ok(entry) => entry,
            Err(MemoryError::QuotaExceeded {
                used,
                requested,
                limit,
                ..
            }) => {
                return Err(MemoryError::UserQuotaFull {
                    used,
                    requested,
                    limit,
                });
            }
            Err(error) => return Err(error),
        };
        candidate.user_metadata.insert(
            entry.id.clone(),
            UserMemoryMetadata {
                version: 1,
                source: Some(UserMemorySource {
                    by: by.clone(),
                    chat_id,
                    message_id,
                }),
                updated_by: Some(by.clone()),
            },
        );
        let actor = actor_for_sender(&by, entry.source.run_id.clone());
        append_state_diff(
            path.as_ref(),
            &before,
            &mut candidate,
            &format!("user-create:{}", entry.id),
            Some(actor),
        )?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(store.user_record(&entry))
    }

    pub fn user_update_persisted(
        &self,
        path: impl AsRef<Path>,
        target: &MemoryTarget,
        entry_id: &str,
        content: String,
        expected_version: u64,
        updated_by: MemorySender,
    ) -> Result<UserMemoryRecord, MemoryError> {
        self.user_update_with_source_persisted(
            path,
            target,
            entry_id,
            content,
            expected_version,
            updated_by,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn user_update_with_source_persisted(
        &self,
        path: impl AsRef<Path>,
        target: &MemoryTarget,
        entry_id: &str,
        content: String,
        expected_version: u64,
        updated_by: MemorySender,
        source_chat_id: Option<String>,
        source_message_id: Option<String>,
    ) -> Result<UserMemoryRecord, MemoryError> {
        validate_user_content(&content)?;
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        let before = store.clone_for_transaction();
        let mut candidate = store.clone_for_transaction();
        let old = candidate
            .entries
            .get(entry_id)
            .cloned()
            .ok_or_else(|| MemoryError::NotFound(entry_id.to_owned()))?;
        if old.target != *target || old.kind != MemoryKind::UserPreference {
            return Err(MemoryError::NotFound(entry_id.to_owned()));
        }
        let old_record = candidate.user_record(&old);
        if old_record.version != expected_version {
            return Err(MemoryError::UserConflict(Box::new(old_record)));
        }
        let mut entry = match candidate.replace(
            MemoryRequest {
                target: target.clone(),
                action: MemoryAction::Replace,
                content,
                id: Some(entry_id.to_owned()),
                kind: Some(MemoryKind::UserPreference),
                source: old.source.clone(),
            },
            expected_version,
        ) {
            Ok(entry) => entry,
            Err(MemoryError::QuotaExceeded {
                used,
                requested,
                limit,
                ..
            }) => {
                return Err(MemoryError::UserQuotaFull {
                    used,
                    requested,
                    limit,
                });
            }
            Err(error) => return Err(error),
        };
        let metadata = candidate
            .user_metadata
            .entry(entry_id.to_owned())
            .or_default();
        metadata.version = old_record.version.saturating_add(1);
        metadata.source = Some(if matches!(&updated_by, MemorySender::System) {
            UserMemorySource {
                by: MemorySender::System,
                chat_id: source_chat_id,
                message_id: source_message_id,
            }
        } else {
            old_record.source
        });
        metadata.updated_by = Some(updated_by.clone());
        // Generic memory records preserve their own provenance. User-facing
        // source and updater are carried in the metadata map above.
        entry.source = old.source;
        candidate.entries.insert(entry_id.to_owned(), entry.clone());
        let actor = actor_for_sender(&updated_by, entry.source.run_id.clone());
        append_state_diff(
            path.as_ref(),
            &before,
            &mut candidate,
            &format!("user-update:{entry_id}:{}", entry.version),
            Some(actor),
        )?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(store.user_record(&entry))
    }

    pub fn user_delete_persisted(
        &self,
        path: impl AsRef<Path>,
        target: &MemoryTarget,
        entry_id: &str,
        expected_version: u64,
    ) -> Result<(), MemoryError> {
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        let before = store.clone_for_transaction();
        let mut candidate = store.clone_for_transaction();
        let old = candidate
            .entries
            .get(entry_id)
            .cloned()
            .ok_or_else(|| MemoryError::NotFound(entry_id.to_owned()))?;
        if old.target != *target || old.kind != MemoryKind::UserPreference {
            return Err(MemoryError::NotFound(entry_id.to_owned()));
        }
        let old_record = candidate.user_record(&old);
        if old_record.version != expected_version {
            return Err(MemoryError::UserConflict(Box::new(old_record)));
        }
        candidate.remove(
            MemoryRequest {
                target: target.clone(),
                action: MemoryAction::Remove,
                content: String::new(),
                id: Some(entry_id.to_owned()),
                kind: Some(MemoryKind::UserPreference),
                source: old.source.clone(),
            },
            expected_version,
        )?;
        candidate.user_metadata.remove(entry_id);
        append_state_diff(
            path.as_ref(),
            &before,
            &mut candidate,
            &format!("user-delete:{entry_id}:{expected_version}"),
            Some(actor_for_sender(
                &MemorySender::System,
                old.source.run_id.clone(),
            )),
        )?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(())
    }

    pub fn user_profile_get_or_init(
        &self,
        path: impl AsRef<Path>,
        timezone: &str,
    ) -> Result<UserProfile, MemoryError> {
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        if let Some(profile) = &store.user_profile {
            return Ok(profile.clone());
        }
        let profile = UserProfile {
            name: None,
            language: None,
            identity: None,
            timezone: if timezone.trim().is_empty() {
                "UTC"
            } else {
                timezone
            }
            .to_owned(),
            onboarded: false,
            version: 1,
            updated_by: MemorySender::System,
            updated_at: Utc::now(),
        };
        let mut candidate = store.clone_for_transaction();
        let before = store.clone_for_transaction();
        candidate.user_profile = Some(profile.clone());
        append_state_diff(
            path.as_ref(),
            &before,
            &mut candidate,
            &format!("profile-init:{}", profile.version),
            Some(actor_for_sender(&profile.updated_by, None)),
        )?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(profile)
    }

    pub fn user_profile_update_persisted(
        &self,
        path: impl AsRef<Path>,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, MemoryError> {
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        let before = store.clone_for_transaction();
        let actor = actor_for_sender(&update.updated_by, None);
        let mut candidate = store.clone_for_transaction();
        if candidate.user_profile.is_none() {
            candidate.user_profile = Some(UserProfile {
                name: None,
                language: None,
                identity: None,
                timezone: if update.timezone.trim().is_empty() {
                    "UTC"
                } else {
                    &update.timezone
                }
                .to_owned(),
                onboarded: false,
                version: 1,
                updated_by: MemorySender::System,
                updated_at: Utc::now(),
            });
        }
        let current = candidate.user_profile.as_mut().expect("initialized above");
        if current.version != update.expected_version {
            return Err(MemoryError::ProfileConflict(Box::new(current.clone())));
        }
        if let Some(value) = update.name {
            current.name = value;
        }
        if let Some(value) = update.language {
            current.language = value;
        }
        if let Some(value) = update.identity {
            current.identity = value;
        }
        if current
            .name
            .as_ref()
            .is_some_and(|value| value.chars().count() > 20)
            || current
                .identity
                .as_ref()
                .is_some_and(|value| value.chars().count() > 100)
            || current
                .language
                .as_ref()
                .is_some_and(|value| value.chars().count() > 35)
        {
            return Err(MemoryError::Invalid(
                "profile field exceeds its maximum length".into(),
            ));
        }
        current.version = current.version.saturating_add(1);
        current.updated_by = update.updated_by;
        current.updated_at = Utc::now();
        let profile = current.clone();
        append_state_diff(
            path.as_ref(),
            &before,
            &mut candidate,
            &format!("profile-update:{}", profile.version),
            Some(actor),
        )?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(profile)
    }

    /// Commit the conversational first-run profile flow. It uses the same
    /// field validation and CAS semantics as a normal profile update, while
    /// recording that onboarding was completed (including an explicit skip).
    pub fn user_profile_onboard_persisted(
        &self,
        path: impl AsRef<Path>,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, MemoryError> {
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        recover_pending_state(path.as_ref(), &mut store)?;
        let before = store.clone_for_transaction();
        let actor = actor_for_sender(&update.updated_by, None);
        let mut candidate = store.clone_for_transaction();
        let profile = candidate.apply_user_profile_onboarding(update)?;
        append_state_diff(
            path.as_ref(),
            &before,
            &mut candidate,
            &format!("profile-onboard:{}", profile.version),
            Some(actor),
        )?;
        candidate.save_to(path)?;
        *store = candidate;
        Ok(profile)
    }
}

impl MemoryStore {
    pub fn user_profile(&self) -> Option<UserProfile> {
        self.user_profile.clone()
    }

    pub fn apply_user_create(
        &mut self,
        target: MemoryTarget,
        content: String,
        by: MemorySender,
        chat_id: Option<String>,
        message_id: Option<String>,
    ) -> Result<UserMemoryRecord, MemoryError> {
        validate_user_content(&content)?;
        let entry = match self.add(MemoryRequest {
            target,
            action: MemoryAction::Add,
            content,
            id: None,
            kind: Some(MemoryKind::UserPreference),
            source: source_for_sender(&by),
        }) {
            Ok(entry) => entry,
            Err(MemoryError::QuotaExceeded {
                used,
                requested,
                limit,
                ..
            }) => {
                return Err(MemoryError::UserQuotaFull {
                    used,
                    requested,
                    limit,
                });
            }
            Err(error) => return Err(error),
        };
        self.user_metadata.insert(
            entry.id.clone(),
            UserMemoryMetadata {
                version: 1,
                source: Some(UserMemorySource {
                    by: by.clone(),
                    chat_id,
                    message_id,
                }),
                updated_by: Some(by),
            },
        );
        Ok(self.user_record(&entry))
    }

    pub fn apply_user_update(
        &mut self,
        target: &MemoryTarget,
        entry_id: &str,
        content: String,
        expected_version: u64,
        updated_by: MemorySender,
    ) -> Result<UserMemoryRecord, MemoryError> {
        self.apply_user_update_with_source(
            target,
            entry_id,
            content,
            expected_version,
            updated_by,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_user_update_with_source(
        &mut self,
        target: &MemoryTarget,
        entry_id: &str,
        content: String,
        expected_version: u64,
        updated_by: MemorySender,
        source_chat_id: Option<String>,
        source_message_id: Option<String>,
    ) -> Result<UserMemoryRecord, MemoryError> {
        validate_user_content(&content)?;
        let old = self
            .entries
            .get(entry_id)
            .cloned()
            .filter(|entry| entry.target == *target && entry.kind == MemoryKind::UserPreference)
            .ok_or_else(|| MemoryError::NotFound(entry_id.to_owned()))?;
        let old_record = self.user_record(&old);
        if old_record.version != expected_version {
            return Err(MemoryError::UserConflict(Box::new(old_record)));
        }
        let mut entry = match self.replace(
            MemoryRequest {
                target: target.clone(),
                action: MemoryAction::Replace,
                content,
                id: Some(entry_id.to_owned()),
                kind: Some(MemoryKind::UserPreference),
                source: old.source.clone(),
            },
            expected_version,
        ) {
            Ok(entry) => entry,
            Err(MemoryError::QuotaExceeded {
                used,
                requested,
                limit,
                ..
            }) => {
                return Err(MemoryError::UserQuotaFull {
                    used,
                    requested,
                    limit,
                });
            }
            Err(error) => return Err(error),
        };
        let metadata = self.user_metadata.entry(entry_id.to_owned()).or_default();
        metadata.version = old_record.version.saturating_add(1);
        metadata.source = Some(if matches!(&updated_by, MemorySender::System) {
            UserMemorySource {
                by: MemorySender::System,
                chat_id: source_chat_id,
                message_id: source_message_id,
            }
        } else {
            old_record.source
        });
        metadata.updated_by = Some(updated_by);
        entry.source = old.source;
        self.entries.insert(entry_id.to_owned(), entry.clone());
        Ok(self.user_record(&entry))
    }

    pub fn apply_user_delete(
        &mut self,
        target: &MemoryTarget,
        entry_id: &str,
        expected_version: u64,
    ) -> Result<(), MemoryError> {
        let old = self
            .entries
            .get(entry_id)
            .cloned()
            .filter(|entry| entry.target == *target && entry.kind == MemoryKind::UserPreference)
            .ok_or_else(|| MemoryError::NotFound(entry_id.to_owned()))?;
        let old_record = self.user_record(&old);
        if old_record.version != expected_version {
            return Err(MemoryError::UserConflict(Box::new(old_record)));
        }
        self.remove(
            MemoryRequest {
                target: target.clone(),
                action: MemoryAction::Remove,
                content: String::new(),
                id: Some(entry_id.to_owned()),
                kind: Some(MemoryKind::UserPreference),
                source: old.source,
            },
            expected_version,
        )?;
        self.user_metadata.remove(entry_id);
        Ok(())
    }

    pub fn apply_user_profile_update(
        &mut self,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, MemoryError> {
        if self.user_profile.is_none() {
            self.user_profile = Some(UserProfile {
                name: None,
                language: None,
                identity: None,
                timezone: if update.timezone.trim().is_empty() {
                    "UTC"
                } else {
                    &update.timezone
                }
                .to_owned(),
                onboarded: false,
                version: 1,
                updated_by: MemorySender::System,
                updated_at: Utc::now(),
            });
        }
        let current = self.user_profile.as_mut().expect("initialized above");
        if current.version != update.expected_version {
            return Err(MemoryError::ProfileConflict(Box::new(current.clone())));
        }
        if let Some(value) = update.name {
            current.name = value;
        }
        if let Some(value) = update.language {
            current.language = value;
        }
        if let Some(value) = update.identity {
            current.identity = value;
        }
        if current
            .name
            .as_ref()
            .is_some_and(|value| value.chars().count() > 20)
            || current
                .identity
                .as_ref()
                .is_some_and(|value| value.chars().count() > 100)
            || current
                .language
                .as_ref()
                .is_some_and(|value| value.chars().count() > 35)
        {
            return Err(MemoryError::Invalid(
                "profile field exceeds its maximum length".into(),
            ));
        }
        current.version = current.version.saturating_add(1);
        current.updated_by = update.updated_by;
        current.updated_at = Utc::now();
        Ok(current.clone())
    }

    pub fn apply_user_profile_onboarding(
        &mut self,
        update: UserProfileUpdate,
    ) -> Result<UserProfile, MemoryError> {
        if let Some(profile) = self
            .user_profile
            .as_ref()
            .filter(|profile| profile.onboarded)
        {
            if profile.version != update.expected_version {
                return Err(MemoryError::ProfileConflict(Box::new(profile.clone())));
            }
            return Err(MemoryError::Invalid(
                "profile onboarding has already completed".into(),
            ));
        }
        let mut profile = self.apply_user_profile_update(update)?;
        profile.onboarded = true;
        self.user_profile = Some(profile.clone());
        Ok(profile)
    }

    pub fn user_quota(&self, target: &MemoryTarget) -> (usize, usize) {
        (self.used(target, &MemoryKind::UserPreference), USER_LIMIT)
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn entries(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.entries.values()
    }

    pub fn add(&mut self, mut request: MemoryRequest) -> Result<MemoryEntry, MemoryError> {
        request.action = MemoryAction::Add;
        let changed = self.apply(request, None)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(changed.into_iter().next().expect("add creates entry"))
    }

    pub fn replace(
        &mut self,
        mut request: MemoryRequest,
        expected_version: u64,
    ) -> Result<MemoryEntry, MemoryError> {
        request.action = MemoryAction::Replace;
        let changed = self.apply(request, Some(expected_version))?;
        self.revision = self.revision.wrapping_add(1);
        Ok(changed.into_iter().next().expect("replace creates entry"))
    }

    pub fn remove(
        &mut self,
        mut request: MemoryRequest,
        expected_version: u64,
    ) -> Result<(), MemoryError> {
        request.action = MemoryAction::Remove;
        self.apply(request, Some(expected_version))?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    fn clone_for_transaction(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            user_metadata: self.user_metadata.clone(),
            user_profile: self.user_profile.clone(),
            operation_receipts: self.operation_receipts.clone(),
            revision: self.revision,
            deleted_user_entries: self.deleted_user_entries.clone(),
            storage_path: self.storage_path.clone(),
            snapshot_cache: self.snapshot_cache.clone(),
        }
    }

    fn persisted_state(&self) -> PersistedState {
        PersistedState {
            entries: self.entries.values().cloned().collect(),
            revision: self.revision,
            user_metadata: self.user_metadata.clone(),
            user_profile: self.user_profile.clone(),
            operation_receipts: self.operation_receipts.clone(),
            deleted_user_entries: self.deleted_user_entries.clone(),
            snapshot_cache: self.snapshot_cache.clone(),
        }
    }

    pub fn user_records(&self, target: &MemoryTarget) -> Vec<UserMemoryRecord> {
        let mut entries = self
            .entries
            .values()
            .filter(|entry| entry.target == *target && entry.kind == MemoryKind::UserPreference)
            .map(|entry| self.user_record(entry))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
        entries
    }

    fn user_record(&self, entry: &MemoryEntry) -> UserMemoryRecord {
        let metadata = self
            .user_metadata
            .get(&entry.id)
            .cloned()
            .unwrap_or_default();
        let legacy_sender = entry
            .source
            .bot_id
            .as_deref()
            .map(|bot_id| MemorySender::Bot {
                bot_id: bot_id.to_owned(),
            })
            .unwrap_or(MemorySender::System);
        let source = metadata.source.unwrap_or(UserMemorySource {
            by: legacy_sender.clone(),
            chat_id: None,
            message_id: None,
        });
        UserMemoryRecord {
            id: entry.id.clone(),
            content: entry.content.clone(),
            version: entry.version.max(metadata.version).max(1),
            source,
            updated_by: metadata.updated_by.unwrap_or(legacy_sender),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
        }
    }

    fn apply(
        &mut self,
        request: MemoryRequest,
        expected_version: Option<u64>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        validate_request(&request)?;
        let kind = request
            .kind
            .clone()
            .unwrap_or_else(|| MemoryKind::default_for(&request.target.scope));
        match request.action {
            MemoryAction::Add => {
                let used = self.used(&request.target, &kind);
                let requested = request.content.chars().count();
                ensure_quota(&request.target, &kind, used, requested)?;
                let now = Utc::now();
                let entry = MemoryEntry {
                    id: request.id.unwrap_or_else(new_id),
                    target: request.target,
                    kind,
                    content: request.content,
                    version: 1,
                    source: request.source,
                    created_at: now,
                    updated_at: now,
                };
                {
                    let sender = sender_for_memory_source(&entry.source);
                    self.user_metadata.insert(
                        entry.id.clone(),
                        UserMemoryMetadata {
                            version: 1,
                            source: Some(UserMemorySource {
                                by: sender.clone(),
                                chat_id: None,
                                message_id: None,
                            }),
                            updated_by: Some(sender),
                        },
                    );
                }
                self.entries.insert(entry.id.clone(), entry.clone());
                Ok(vec![entry])
            }
            MemoryAction::Replace => {
                let id = request
                    .id
                    .ok_or_else(|| MemoryError::Invalid("replace requires id".into()))?;
                let old = self
                    .entries
                    .get(&id)
                    .ok_or_else(|| MemoryError::NotFound(id.clone()))?
                    .clone();
                if old.target != request.target {
                    return Err(MemoryError::Invalid("entry target mismatch".into()));
                }
                if expected_version != Some(old.version) {
                    return Err(MemoryError::UserConflict(Box::new(self.user_record(&old))));
                }
                let used = self
                    .used(&request.target, &old.kind)
                    .saturating_sub(old.content.chars().count());
                ensure_quota(
                    &request.target,
                    &old.kind,
                    used,
                    request.content.chars().count(),
                )?;
                let mut entry = old;
                entry.content = request.content;
                entry.version = entry.version.saturating_add(1);
                entry.source = request.source;
                entry.updated_at = Utc::now();
                {
                    let sender = sender_for_memory_source(&entry.source);
                    let metadata = self.user_metadata.entry(id.clone()).or_default();
                    metadata.version = metadata.version.max(1).saturating_add(1);
                    if metadata.source.is_none() {
                        metadata.source = Some(UserMemorySource {
                            by: sender.clone(),
                            chat_id: None,
                            message_id: None,
                        });
                    }
                    metadata.updated_by = Some(sender);
                }
                self.entries.insert(id, entry.clone());
                Ok(vec![entry])
            }
            MemoryAction::Remove => {
                let id = request
                    .id
                    .ok_or_else(|| MemoryError::Invalid("remove requires id".into()))?;
                let old = self
                    .entries
                    .get(&id)
                    .ok_or_else(|| MemoryError::NotFound(id.clone()))?;
                if old.target != request.target {
                    return Err(MemoryError::Invalid("entry target mismatch".into()));
                }
                if expected_version != Some(old.version) {
                    return Err(MemoryError::UserConflict(Box::new(self.user_record(old))));
                }
                self.entries.remove(&id);
                self.user_metadata.remove(&id);
                Ok(Vec::new())
            }
        }
    }

    fn used(&self, target: &MemoryTarget, kind: &MemoryKind) -> usize {
        self.entries
            .values()
            .filter(|entry| &entry.target == target && &entry.kind == kind)
            .map(|entry| entry.content.chars().count())
            .sum()
    }

    pub fn append_worklog(
        &mut self,
        target: MemoryTarget,
        content: impl Into<String>,
        source: MemorySource,
    ) -> Result<MemoryEntry, MemoryError> {
        if target.scope != MemoryScope::Bot {
            return Err(MemoryError::Invalid("worklog target must be a Bot".into()));
        }
        let content = content.into();
        if content.chars().count() > BOT_WORKLOG_LIMIT {
            return Err(MemoryError::QuotaExceeded {
                scope: target.scope.to_string(),
                used: 0,
                requested: content.chars().count(),
                limit: BOT_WORKLOG_LIMIT,
            });
        }
        let mut rows: Vec<MemoryEntry> = self
            .entries
            .values()
            .filter(|x| x.target == target && x.kind == MemoryKind::BotWorklog)
            .cloned()
            .collect();
        rows.sort_by_key(|x| x.created_at);
        let mut total: usize = rows
            .iter()
            .map(|x| x.content.chars().count())
            .sum::<usize>()
            + content.chars().count();
        let summary_source = source.clone();
        let mut rolled = Vec::new();
        while total > BOT_WORKLOG_LIMIT && !rows.is_empty() {
            let old = rows.remove(0);
            self.entries.remove(&old.id);
            self.user_metadata.remove(&old.id);
            total = total.saturating_sub(old.content.chars().count());
            rolled.push(old.content);
        }
        if !rolled.is_empty() {
            let remaining = BOT_WORKLOG_LIMIT
                .saturating_sub(self.used(&target, &MemoryKind::BotWorklog))
                .saturating_sub(content.chars().count());
            if remaining > 20 {
                let now = Utc::now();
                let summary = format!(
                    "[{}-{:02}] {}",
                    now.year(),
                    now.month(),
                    truncate_chars(&rolled.join(" "), remaining - 20)
                );
                self.add(MemoryRequest {
                    target: target.clone(),
                    action: MemoryAction::Add,
                    content: summary,
                    id: None,
                    kind: Some(MemoryKind::BotWorklog),
                    source: summary_source,
                })?;
            }
        }
        let entry = self.add(MemoryRequest {
            target,
            action: MemoryAction::Add,
            content,
            id: None,
            kind: Some(MemoryKind::BotWorklog),
            source,
        })?;
        if self.used(&entry.target, &MemoryKind::BotWorklog) > BOT_WORKLOG_LIMIT {
            self.compact_worklog(&entry.target, &entry.source)?;
        }
        Ok(entry)
    }

    fn compact_worklog(
        &mut self,
        target: &MemoryTarget,
        source: &MemorySource,
    ) -> Result<(), MemoryError> {
        let mut rows: Vec<_> = self
            .entries
            .values()
            .filter(|x| x.target == *target && x.kind == MemoryKind::BotWorklog)
            .cloned()
            .collect();
        rows.sort_by_key(|x| x.created_at);
        let mut combined = String::new();
        for row in &rows[..rows.len().saturating_sub(1)] {
            if !combined.is_empty() {
                combined.push(' ');
            }
            combined.push_str(&row.content);
        }
        let latest = rows
            .last()
            .cloned()
            .ok_or_else(|| MemoryError::Invalid("empty worklog".into()))?;
        for row in &rows[..rows.len().saturating_sub(1)] {
            self.entries.remove(&row.id);
            self.user_metadata.remove(&row.id);
        }
        let month = format!(
            "{}-{:02}",
            latest.created_at.year(),
            latest.created_at.month()
        );
        let summary = format!(
            "[{month}] {}",
            truncate_chars(
                &combined,
                BOT_WORKLOG_LIMIT.saturating_sub(latest.content.chars().count() + 1)
            )
        );
        if !summary.trim().is_empty() {
            let now = Utc::now();
            let old = MemoryEntry {
                id: new_id(),
                target: target.clone(),
                kind: MemoryKind::BotWorklog,
                content: summary,
                version: 1,
                source: source.clone(),
                created_at: now,
                updated_at: now,
            };
            self.entries.insert(old.id.clone(), old);
        }
        Ok(())
    }

    /// Run the idle maintenance pass for one Bot's worklog. The provider can
    /// be backed by the configured maintenance model in production and by a
    /// deterministic mock in tests.
    pub fn maintain_worklog<P: MaintenanceProvider>(
        &mut self,
        target: &MemoryTarget,
        provider: &P,
    ) -> Result<Option<MemoryEntry>, MemoryError> {
        let mut rows: Vec<_> = self
            .entries
            .values()
            .filter(|entry| entry.target == *target && entry.kind == MemoryKind::BotWorklog)
            .cloned()
            .collect();
        if rows.len() < 2 {
            return Ok(None);
        }
        rows.sort_by_key(|entry| entry.created_at);
        let text = rows
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let summary = truncate_chars(&provider.summarize(&text)?, BOT_WORKLOG_LIMIT);
        for row in rows {
            self.entries.remove(&row.id);
            self.user_metadata.remove(&row.id);
        }
        let entry = self.add(MemoryRequest {
            target: target.clone(),
            action: MemoryAction::Add,
            content: summary,
            id: None,
            kind: Some(MemoryKind::BotWorklog),
            source: MemorySource::default(),
        })?;
        Ok(Some(entry))
    }

    pub fn search(&self, query: &str, target: Option<&MemoryTarget>) -> Vec<MemoryEntry> {
        let query = query.to_lowercase();
        self.entries
            .values()
            .filter(|entry| target.is_none_or(|target| &entry.target == target))
            .filter(|entry| entry.content.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    pub fn snapshot(
        &self,
        targets: &[MemoryTarget],
        previous: Option<&MemorySnapshot>,
        now: DateTime<Utc>,
    ) -> (MemorySnapshot, bool) {
        let stale = previous.is_none_or(|snapshot| snapshot.targets != targets);
        if !stale {
            return (previous.expect("checked above").clone(), false);
        }
        let entries = self
            .entries
            .values()
            .filter(|entry| targets.contains(&entry.target))
            .cloned()
            .collect();
        (
            MemorySnapshot {
                revision: self.revision,
                generated_at: now,
                targets: targets.to_vec(),
                entries,
                user_profile: targets
                    .iter()
                    .any(|target| {
                        target.scope == MemoryScope::User
                            && matches!(target.owner_id.as_str(), "user" | "local-user")
                    })
                    .then(|| self.user_profile.clone())
                    .flatten(),
            },
            true,
        )
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), MemoryError> {
        let state = self.persisted_state();
        let path = path.as_ref();
        let temp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(&state)?;
        let mut file = fs::File::create(&temp)?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temp, path)?;
        if let Some(parent) = path.parent() {
            // Sync the directory entry as well as the file. This closes the
            // rename window on macOS after a power loss.
            if let Ok(dir) = fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        if !path.as_ref().exists() {
            let changes = read_all_memory_changes(path.as_ref())?;
            if let Some(state) = changes
                .iter()
                .rev()
                .find_map(|change| change.recovery_state.clone())
            {
                write_persisted_state(path.as_ref(), &state)?;
                return Self::load_from(path);
            }
            return Ok(Self {
                storage_path: Some(path.as_ref().to_path_buf()),
                ..Self::new()
            });
        }
        let mut state: PersistedState = serde_json::from_slice(&fs::read(path.as_ref())?)?;
        let future_changes = read_all_memory_changes(path.as_ref())?
            .into_iter()
            .filter(|change| change.revision > state.revision)
            .collect::<Vec<_>>();
        if let Some(recovered) = future_changes
            .iter()
            .rev()
            .find_map(|change| change.recovery_state.clone())
        {
            state = *recovered;
            write_persisted_state(path.as_ref(), &state)?;
        }
        let entries = state
            .entries
            .into_iter()
            .map(|mut entry| {
                if let Some(metadata) = state.user_metadata.get(&entry.id) {
                    entry.version = entry.version.max(metadata.version).max(1);
                }
                (entry.id.clone(), entry)
            })
            .collect();
        let store = Self {
            entries,
            user_metadata: state.user_metadata,
            user_profile: state.user_profile,
            operation_receipts: state.operation_receipts,
            deleted_user_entries: state.deleted_user_entries,
            revision: state.revision,
            storage_path: Some(path.as_ref().to_path_buf()),
            snapshot_cache: state.snapshot_cache,
        };
        prune_memory_changes(path.as_ref(), store.revision, Utc::now())?;
        prune_expired_memory_files(path.as_ref(), Utc::now())?;
        Ok(store)
    }
}

fn validate_request(request: &MemoryRequest) -> Result<(), MemoryError> {
    if request.target.owner_id.is_empty() {
        return Err(MemoryError::Invalid(
            "memory target owner_id is empty".into(),
        ));
    }
    if request.action != MemoryAction::Remove && request.content.trim().is_empty() {
        return Err(MemoryError::Invalid("memory content is empty".into()));
    }
    if let Some(kind) = &request.kind {
        if !kind.valid_for(&request.target.scope) {
            return Err(MemoryError::Invalid(
                "memory kind does not match scope".into(),
            ));
        }
    }
    Ok(())
}

fn ensure_quota(
    target: &MemoryTarget,
    kind: &MemoryKind,
    used: usize,
    requested: usize,
) -> Result<(), MemoryError> {
    let limit = kind.limit();
    if used + requested > limit {
        return Err(MemoryError::QuotaExceeded {
            scope: format!("{}:{}", target.scope, target.owner_id),
            used,
            requested,
            limit,
        });
    }
    Ok(())
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

fn memory_directory(state_path: &Path) -> &Path {
    state_path.parent().unwrap_or_else(|| Path::new("."))
}

fn changelog_path(state_path: &Path) -> std::path::PathBuf {
    memory_directory(state_path).join("changelog.jsonl")
}

fn change_without_recovery(change: &MemoryChange) -> MemoryChange {
    MemoryChange {
        operation_id: change.operation_id.clone(),
        revision: change.revision,
        op: change.op.clone(),
        entry_id: change.entry_id.clone(),
        target: change.target.clone(),
        kind: change.kind.clone(),
        content: change.content.clone(),
        prev_content: change.prev_content.clone(),
        actor: change.actor.clone(),
        source: change.source.clone(),
        at: change.at,
        recovery_state: None,
        legacy_payload: change.legacy_payload.clone(),
    }
}

fn append_memory_change(state_path: &Path, change: &MemoryChange) -> Result<(), MemoryError> {
    let path = changelog_path(state_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, change)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn actor_for_sender(sender: &MemorySender, run_id: Option<String>) -> MemoryChangeActor {
    match sender {
        MemorySender::User => MemoryChangeActor {
            kind: MemoryChangeActorKind::User,
            bot_id: None,
            run_id,
        },
        MemorySender::System => MemoryChangeActor {
            kind: MemoryChangeActorKind::System,
            bot_id: None,
            run_id,
        },
        MemorySender::Bot { bot_id } => MemoryChangeActor {
            kind: MemoryChangeActorKind::Bot,
            bot_id: Some(bot_id.clone()),
            run_id,
        },
    }
}

fn change_source(entry: &MemoryEntry, metadata: Option<&UserMemoryMetadata>) -> MemoryChangeSource {
    let provenance = metadata.and_then(|item| item.source.as_ref());
    MemoryChangeSource {
        bot_id: entry.source.bot_id.clone(),
        run_id: entry.source.run_id.clone(),
        session_id: entry.source.session_id.clone(),
        by: provenance.map(|source| source.by.clone()),
        chat_id: provenance.and_then(|source| source.chat_id.clone()),
        message_id: provenance.and_then(|source| source.message_id.clone()),
    }
}

fn append_state_diff(
    state_path: &Path,
    before: &MemoryStore,
    candidate: &mut MemoryStore,
    operation_id: &str,
    actor_override: Option<MemoryChangeActor>,
) -> Result<(), MemoryError> {
    append_state_diff_with(
        state_path,
        before,
        candidate,
        operation_id,
        actor_override,
        append_memory_change,
    )
}

fn append_state_diff_with<F>(
    state_path: &Path,
    before: &MemoryStore,
    candidate: &mut MemoryStore,
    operation_id: &str,
    actor_override: Option<MemoryChangeActor>,
    mut append: F,
) -> Result<(), MemoryError>
where
    F: FnMut(&Path, &MemoryChange) -> Result<(), MemoryError>,
{
    let mut drafts: Vec<(
        MemoryOperation,
        MemoryEntry,
        Option<String>,
        MemoryChangeActor,
        MemoryChangeSource,
    )> = Vec::new();
    for (id, entry) in &candidate.entries {
        let old = before.entries.get(id);
        if old.is_some_and(|old| old == entry) {
            continue;
        }
        let sender = candidate
            .user_metadata
            .get(id)
            .and_then(|metadata| metadata.updated_by.as_ref())
            .cloned()
            .unwrap_or_else(|| sender_for_memory_source(&entry.source));
        drafts.push((
            if old.is_some() {
                MemoryOperation::Replace
            } else {
                MemoryOperation::Add
            },
            entry.clone(),
            old.map(|old| old.content.clone()),
            actor_override
                .clone()
                .unwrap_or_else(|| actor_for_sender(&sender, entry.source.run_id.clone())),
            change_source(entry, candidate.user_metadata.get(id)),
        ));
    }
    for (id, old) in &before.entries {
        if !candidate.entries.contains_key(id) {
            if old.kind == MemoryKind::UserPreference {
                candidate
                    .deleted_user_entries
                    .insert(id.clone(), candidate.revision.max(before.revision + 1));
            }
            let sender = before
                .user_metadata
                .get(id)
                .and_then(|metadata| metadata.updated_by.as_ref())
                .cloned()
                .unwrap_or_else(|| sender_for_memory_source(&old.source));
            drafts.push((
                MemoryOperation::Remove,
                old.clone(),
                Some(old.content.clone()),
                actor_override
                    .clone()
                    .unwrap_or_else(|| actor_for_sender(&sender, old.source.run_id.clone())),
                change_source(old, before.user_metadata.get(id)),
            ));
        }
    }
    if before.user_profile != candidate.user_profile {
        let old = before.user_profile.as_ref();
        let current = candidate.user_profile.as_ref();
        drafts.push((
            if old.is_some() {
                MemoryOperation::Replace
            } else {
                MemoryOperation::Add
            },
            MemoryEntry {
                id: "profile".into(),
                target: MemoryTarget::user("local-user"),
                kind: MemoryKind::UserProfile,
                content: current
                    .map(serde_json::to_string)
                    .transpose()?
                    .unwrap_or_default(),
                version: current.map(|profile| profile.version).unwrap_or(1),
                source: MemorySource::default(),
                created_at: current
                    .map(|profile| profile.updated_at)
                    .unwrap_or_else(Utc::now),
                updated_at: current
                    .map(|profile| profile.updated_at)
                    .unwrap_or_else(Utc::now),
            },
            old.map(serde_json::to_string).transpose()?,
            actor_override
                .clone()
                .or_else(|| current.map(|profile| actor_for_sender(&profile.updated_by, None)))
                .unwrap_or_default(),
            MemoryChangeSource {
                by: current.map(|profile| profile.updated_by.clone()),
                ..MemoryChangeSource::default()
            },
        ));
    }
    if drafts.is_empty() {
        return Ok(());
    }
    candidate.revision = candidate
        .revision
        .max(before.revision.saturating_add(drafts.len() as u64));
    let recovery_state = Box::new(candidate.persisted_state());
    let start_revision = candidate
        .revision
        .saturating_sub(drafts.len() as u64)
        .saturating_add(1);
    let last_index = drafts.len().saturating_sub(1);
    for (index, (op, entry, prev_content, actor, source)) in drafts.into_iter().enumerate() {
        let change = MemoryChange {
            operation_id: operation_id.to_owned(),
            revision: start_revision + index as u64,
            op: op.clone(),
            entry_id: entry.id,
            target: entry.target,
            kind: MemoryChangeKind::from(&entry.kind),
            content: if matches!(op, MemoryOperation::Remove) {
                String::new()
            } else {
                entry.content
            },
            prev_content,
            actor,
            source,
            at: Utc::now(),
            recovery_state: (index == last_index).then(|| recovery_state.clone()),
            legacy_payload: None,
        };
        append(state_path, &change)?;
    }
    Ok(())
}

fn read_all_memory_changes(state_path: &Path) -> Result<Vec<MemoryChange>, MemoryError> {
    let path = changelog_path(state_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(path)?;
    let mut changes = Vec::new();
    let lines = BufReader::new(file)
        .lines()
        .collect::<Result<Vec<_>, _>>()?;
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => {
                let change = serde_json::from_value::<MemoryChange>(value.clone())
                    .ok()
                    .or_else(|| parse_legacy_memory_change(value))
                    .ok_or_else(|| {
                        MemoryError::Invalid(format!(
                            "unsupported changelog row at line {}",
                            index + 1
                        ))
                    })?;
                changes.push(change);
            }
            Err(_) if index + 1 == lines.len() => break,
            Err(_) => {
                return Err(MemoryError::Invalid(format!(
                    "malformed changelog row at line {}",
                    index + 1
                )));
            }
        }
    }
    changes.sort_by_key(|change| change.revision);
    Ok(changes)
}

fn parse_legacy_memory_change(value: serde_json::Value) -> Option<MemoryChange> {
    let object = value.as_object()?;
    let revision = object.get("revision")?.as_u64()?;
    let op_text = object.get("op")?.as_str()?.to_ascii_lowercase();
    let op = match op_text.as_str() {
        "add" | "create" | "profile" => MemoryOperation::Add,
        "replace" | "update" | "profile_update" => MemoryOperation::Replace,
        "remove" | "delete" => MemoryOperation::Remove,
        _ => return None,
    };
    let kind_text = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("user_preference")
        .to_ascii_lowercase();
    let kind = match kind_text.as_str() {
        "user_preference" | "user" => MemoryChangeKind::UserPreference,
        "user_profile" | "profile" => MemoryChangeKind::UserProfile,
        "bot_experience" | "bot" => MemoryChangeKind::BotExperience,
        "project_knowledge" | "project" => MemoryChangeKind::ProjectKnowledge,
        "worklog" | "bot_worklog" => MemoryChangeKind::Worklog,
        _ => return None,
    };
    let target = object
        .get("target")
        .cloned()
        .and_then(|target| serde_json::from_value::<MemoryTarget>(target).ok())
        .unwrap_or_else(|| {
            if kind == MemoryChangeKind::UserProfile || kind == MemoryChangeKind::UserPreference {
                MemoryTarget::user("local-user")
            } else {
                MemoryTarget::bot("legacy")
            }
        });
    let entry_id = object
        .get("entry_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("profile")
        .to_owned();
    let actor_value = object
        .get("actor")
        .cloned()
        .unwrap_or(serde_json::json!({"kind":"system"}));
    let legacy_actor = actor_value.as_object();
    let actor_kind = match legacy_actor
        .and_then(|actor| actor.get("kind"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("system")
        .to_ascii_lowercase()
        .as_str()
    {
        "user" => MemoryChangeActorKind::User,
        "bot" => MemoryChangeActorKind::Bot,
        _ => MemoryChangeActorKind::System,
    };
    let actor = MemoryChangeActor {
        kind: actor_kind,
        bot_id: legacy_actor
            .and_then(|actor| actor.get("bot_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        run_id: legacy_actor
            .and_then(|actor| actor.get("run_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    };
    let sender = match &actor.kind {
        MemoryChangeActorKind::User => MemorySender::User,
        MemoryChangeActorKind::Bot => MemorySender::Bot {
            bot_id: actor.bot_id.clone().unwrap_or_else(|| "legacy".into()),
        },
        MemoryChangeActorKind::System => MemorySender::System,
    };
    let source = object
        .get("source")
        .cloned()
        .and_then(|source| serde_json::from_value::<MemoryChangeSource>(source).ok())
        .unwrap_or_else(|| MemoryChangeSource {
            by: Some(sender),
            chat_id: legacy_actor
                .and_then(|actor| actor.get("chat_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            message_id: legacy_actor
                .and_then(|actor| actor.get("message_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            ..MemoryChangeSource::default()
        });
    let at = object
        .get("at")
        .cloned()
        .and_then(|at| serde_json::from_value::<DateTime<Utc>>(at).ok())
        .unwrap_or_else(Utc::now);
    Some(MemoryChange {
        operation_id: object
            .get("operation_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("legacy:{revision}:{entry_id}")),
        revision,
        op,
        entry_id,
        target,
        kind,
        content: object
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        prev_content: object
            .get("prev_content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        actor,
        source,
        at,
        recovery_state: None,
        legacy_payload: Some(value),
    })
}

fn recover_pending_state(state_path: &Path, store: &mut MemoryStore) -> Result<(), MemoryError> {
    if let Some(state) = read_all_memory_changes(state_path)?
        .iter()
        .rev()
        .find(|change| change.revision > store.revision)
        .and_then(|change| change.recovery_state.clone())
    {
        write_persisted_state(state_path, &state)?;
        store.entries = state
            .entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        store.revision = state.revision;
        store.user_metadata = state.user_metadata;
        store.user_profile = state.user_profile;
        store.operation_receipts = state.operation_receipts;
        store.deleted_user_entries = state.deleted_user_entries;
        store.snapshot_cache = state.snapshot_cache;
    }
    truncate_future_changes(state_path, store.revision)?;
    Ok(())
}

fn truncate_future_changes(state_path: &Path, revision: u64) -> Result<(), MemoryError> {
    let path = changelog_path(state_path);
    if !path.exists() {
        return Ok(());
    }
    let raw = fs::read(&path)?;
    let changes = read_all_memory_changes(state_path)?;
    let has_future_revision = changes.iter().any(|change| change.revision > revision);
    let has_torn_tail = raw.last().is_some_and(|last| *last != b'\n')
        || raw
            .split(|byte| *byte == b'\n')
            .rfind(|line| !line.iter().all(u8::is_ascii_whitespace))
            .is_some_and(|line| serde_json::from_slice::<serde_json::Value>(line).is_err());
    if !has_future_revision && !has_torn_tail {
        return Ok(());
    }
    let changes = changes
        .into_iter()
        .filter(|change| change.revision <= revision)
        .collect::<Vec<_>>();
    let temp = path.with_extension("tmp");
    let mut file = fs::File::create(&temp)?;
    for change in changes {
        serde_json::to_writer(&mut file, &change_without_recovery(&change))?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    fs::rename(temp, path)?;
    Ok(())
}

fn read_memory_changes(
    state_path: &Path,
    seen_revision: u64,
) -> Result<Vec<MemoryChange>, MemoryError> {
    let changes = read_all_memory_changes(state_path)?;
    let state: PersistedState = if state_path.exists() {
        serde_json::from_slice(&fs::read(state_path)?)?
    } else {
        PersistedState::default()
    };
    if seen_revision < state.revision {
        let available = changes
            .iter()
            .map(|change| change.revision)
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(missing) =
            ((seen_revision + 1)..=state.revision).find(|revision| !available.contains(revision))
        {
            return Err(MemoryError::ChangeLogGap {
                seen_revision,
                earliest_revision: missing,
            });
        }
    }
    Ok(changes
        .into_iter()
        .filter(|change| change.revision > seen_revision && change.revision <= state.revision)
        .map(|mut change| {
            change.recovery_state = None;
            change
        })
        .collect())
}

fn write_persisted_state(path: &Path, state: &PersistedState) -> Result<(), MemoryError> {
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(state)?;
    let mut file = fs::File::create(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(temp, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn segment_directory(state_path: &Path) -> std::path::PathBuf {
    memory_directory(state_path).join("segments")
}

fn thread_segment_path(state_path: &Path, thread_id: &str) -> std::path::PathBuf {
    let safe_id = thread_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    memory_directory(state_path)
        .join("segments")
        .join("threads")
        .join(format!("{safe_id}.json"))
}

fn read_thread_segment(
    state_path: &Path,
    thread_id: &str,
) -> Result<Option<MemoryThreadSegmentRecord>, MemoryError> {
    let path = thread_segment_path(state_path, thread_id);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn write_thread_segment(
    state_path: &Path,
    record: &MemoryThreadSegmentRecord,
) -> Result<(), MemoryError> {
    let path = thread_segment_path(state_path, &record.thread_id);
    fs::create_dir_all(path.parent().expect("thread segment file has parent"))?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, serde_json::to_vec(record)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

fn segment_path(state_path: &Path, run_id: &str) -> std::path::PathBuf {
    let safe_id = run_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    segment_directory(state_path).join(format!("{safe_id}.json"))
}

fn write_segment_seen(
    state_path: &Path,
    run_id: &str,
    seen_revision: u64,
) -> Result<(), MemoryError> {
    let mut record = read_segment_record(state_path, run_id)?.unwrap_or(MemorySegmentRecord {
        run_id: run_id.to_owned(),
        segment_no: 0,
        seen_revision,
        delta_chars: 0,
        updated_at: Utc::now(),
    });
    record.seen_revision = seen_revision;
    record.updated_at = Utc::now();
    write_segment_record(state_path, &record)
}

fn write_segment_record(
    state_path: &Path,
    record: &MemorySegmentRecord,
) -> Result<(), MemoryError> {
    let path = segment_path(state_path, &record.run_id);
    fs::create_dir_all(path.parent().expect("segment file has a parent"))?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, serde_json::to_vec(record)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

fn read_segment_seen(state_path: &Path, run_id: &str) -> Result<Option<u64>, MemoryError> {
    Ok(read_segment_record(state_path, run_id)?.map(|record| record.seen_revision))
}

fn read_segment_record(
    state_path: &Path,
    run_id: &str,
) -> Result<Option<MemorySegmentRecord>, MemoryError> {
    let path = segment_path(state_path, run_id);
    if !path.exists() {
        return Ok(None);
    }
    let record: MemorySegmentRecord = serde_json::from_slice(&fs::read(path)?)?;
    Ok(Some(record))
}

fn prune_expired_memory_files(state_path: &Path, now: DateTime<Utc>) -> Result<(), MemoryError> {
    let dir = segment_directory(state_path);
    if !dir.exists() {
        return Ok(());
    }
    for item in fs::read_dir(dir)? {
        let item = item?;
        if !item.file_type()?.is_file() || item.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let record = match fs::read(item.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<MemorySegmentRecord>(&bytes).ok())
        {
            Some(record) => record,
            None => {
                fs::remove_file(item.path())?;
                continue;
            }
        };
        if now.signed_duration_since(record.updated_at) > Duration::days(SEGMENT_RETENTION_DAYS) {
            fs::remove_file(item.path())?;
        }
    }
    Ok(())
}

fn prune_memory_changes(
    state_path: &Path,
    state_revision: u64,
    now: DateTime<Utc>,
) -> Result<(), MemoryError> {
    let path = changelog_path(state_path);
    let changes = read_all_memory_changes(state_path)?
        .into_iter()
        .filter(|change| change.revision <= state_revision)
        .collect::<Vec<_>>();
    prune_expired_memory_files(state_path, now)?;
    let mut min_seen = state_revision;
    let dir = segment_directory(state_path);
    if dir.exists() {
        for item in fs::read_dir(dir)? {
            let item = item?;
            if let Ok(record) = fs::read(item.path()).and_then(|bytes| {
                serde_json::from_slice::<MemorySegmentRecord>(&bytes).map_err(std::io::Error::other)
            }) {
                min_seen = min_seen.min(record.seen_revision);
            }
        }
    }
    let cutoff = now - Duration::days(SEGMENT_RETENTION_DAYS);
    let keep = changes
        .into_iter()
        .filter(|change| {
            change.revision == state_revision || change.revision > min_seen || change.at >= cutoff
        })
        .collect::<Vec<_>>();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    let mut file = fs::File::create(&temp)?;
    for change in keep {
        serde_json::to_writer(&mut file, &change_without_recovery(&change))?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    fs::rename(temp, path)?;
    Ok(())
}

fn sender_for_memory_source(source: &MemorySource) -> MemorySender {
    source
        .bot_id
        .as_deref()
        .map(|bot_id| MemorySender::Bot {
            bot_id: bot_id.to_owned(),
        })
        .unwrap_or(MemorySender::System)
}

fn source_for_sender(sender: &MemorySender) -> MemorySource {
    MemorySource {
        bot_id: match sender {
            MemorySender::Bot { bot_id } => Some(bot_id.clone()),
            _ => None,
        },
        run_id: None,
        session_id: None,
    }
}

fn validate_user_content(content: &str) -> Result<(), MemoryError> {
    if content.trim().is_empty() {
        return Err(MemoryError::Invalid("user memory content is empty".into()));
    }
    if content.chars().count() > 500 {
        return Err(MemoryError::Invalid(
            "user memory content exceeds 500 characters".into(),
        ));
    }
    Ok(())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub revision: u64,
    pub generated_at: DateTime<Utc>,
    pub targets: Vec<MemoryTarget>,
    pub entries: Vec<MemoryEntry>,
    #[serde(default)]
    pub user_profile: Option<UserProfile>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMessage {
    pub id: String,
    pub session_id: String,
    pub chat_id: String,
    pub project_id: Option<String>,
    pub role: String,
    pub content: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConversationIndex {
    messages: Vec<SessionMessage>,
    projects: Vec<ProjectRecord>,
}

impl ConversationIndex {
    pub fn add_message(&mut self, message: SessionMessage) {
        self.messages.push(message);
    }
    pub fn add_project(&mut self, project: ProjectRecord) {
        self.projects.retain(|x| x.id != project.id);
        self.projects.push(project);
    }
    pub fn session_search(&self, session_id: &str, query: &str) -> Vec<SessionMessage> {
        self.messages
            .iter()
            .filter(|x| x.session_id == session_id && contains(&x.content, query))
            .cloned()
            .collect()
    }
    pub fn chat_history(&self, chat_id: &str, limit: usize) -> Vec<SessionMessage> {
        let start = self
            .messages
            .iter()
            .filter(|x| x.chat_id == chat_id)
            .count()
            .saturating_sub(limit);
        self.messages
            .iter()
            .filter(|x| x.chat_id == chat_id)
            .skip(start)
            .cloned()
            .collect()
    }
    pub fn project_find(&self, query: &str) -> Vec<ProjectRecord> {
        self.projects
            .iter()
            .filter(|x| contains(&format!("{} {} {}", x.name, x.goal, x.content), query))
            .cloned()
            .collect()
    }
}

fn contains(value: &str, query: &str) -> bool {
    value.to_lowercase().contains(&query.to_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationKind {
    Private,
    Group,
    Scheduled,
}

#[derive(Clone, Debug)]
pub struct ContextRequest {
    pub kind: ConversationKind,
    pub memory_targets: Vec<MemoryTarget>,
    pub l0_platform_rules: String,
    pub l1_bot_identity: String,
    pub announcement: String,
    pub task: String,
    pub trigger: String,
    pub references: Vec<String>,
    pub recent_messages: Vec<String>,
    pub segment_summary: String,
    pub recent_context: Vec<String>,
    pub run_events: Vec<String>,
    pub model_context_window: usize,
    pub previous_snapshot: Option<MemorySnapshot>,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ContextLayer {
    pub level: u8,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ContextPackage {
    pub layers: Vec<ContextLayer>,
    pub snapshot: MemorySnapshot,
    pub large_entries: Vec<MemoryCatalogEntry>,
    pub new_segment: bool,
    pub segment_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryCatalogEntry {
    pub entry_id: String,
    pub topic: String,
}

pub fn assemble_context(store: &MemoryStore, request: &ContextRequest) -> ContextPackage {
    let (snapshot, _snapshot_stale) = store.snapshot(
        &request.memory_targets,
        request.previous_snapshot.as_ref(),
        request.now,
    );
    let memory_entries = snapshot
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                MemoryKind::UserPreference | MemoryKind::BotExperience
            )
        })
        .map(|entry| {
            format!(
                "[{} v{}] {}",
                entry.kind_as_str(),
                entry.version,
                entry.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let profile_text = snapshot.user_profile.as_ref().map(render_user_profile);
    let memory_text = match (profile_text, memory_entries.is_empty()) {
        (Some(profile), true) => profile,
        (Some(profile), false) => format!("{profile}\n{memory_entries}"),
        (None, _) => memory_entries,
    };
    let mut layers = vec![
        ContextLayer {
            level: 0,
            content: request.l0_platform_rules.clone(),
        },
        ContextLayer {
            level: 1,
            content: request.l1_bot_identity.clone(),
        },
        ContextLayer {
            level: 2,
            content: memory_text,
        },
    ];
    let l3 = match request.kind {
        ConversationKind::Group => {
            let messages = request
                .recent_messages
                .iter()
                .rev()
                .take(GROUP_RECENT_MESSAGES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "公告:\n{}\n任务:\n{}\n触发消息:\n{}\n引用:\n{}\n最近群消息:\n{}",
                request.announcement,
                request.task,
                request.trigger,
                request.references.join("\n"),
                messages
            )
        }
        ConversationKind::Private | ConversationKind::Scheduled => format!(
            "段摘要:\n{}\n最近消息:\n{}\n任务:\n{}",
            request.segment_summary,
            request.recent_context.join("\n"),
            request.task
        ),
    };
    layers.push(ContextLayer {
        level: 3,
        content: l3,
    });
    layers.push(ContextLayer {
        level: 4,
        content: request.run_events.join("\n"),
    });
    let estimated_tokens = layers
        .iter()
        .map(|x| x.content.chars().count())
        .sum::<usize>()
        / 4;
    let window_limit = request.model_context_window.saturating_mul(80) / 100;
    let threshold = window_limit > 0 && estimated_tokens >= window_limit;
    let reason = if threshold {
        Some("context_80_percent".into())
    } else {
        None
    };
    let large_entries = snapshot
        .entries
        .iter()
        .filter(|entry| matches!(entry.kind, MemoryKind::Project | MemoryKind::BotWorklog))
        .map(|entry| MemoryCatalogEntry {
            entry_id: entry.id.clone(),
            topic: truncate_chars(
                entry
                    .content
                    .lines()
                    .next()
                    .unwrap_or(&entry.content)
                    .trim(),
                120,
            ),
        })
        .collect();
    ContextPackage {
        layers,
        snapshot,
        large_entries,
        new_segment: threshold,
        segment_reason: reason,
    }
}

fn render_user_profile(profile: &UserProfile) -> String {
    let mut lines = vec!["用户档案（优先参考）".to_owned()];
    if let Some(name) = profile
        .name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("称呼：{name}"));
    }
    if let Some(language) = profile
        .language
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("语言：{language}"));
    }
    if let Some(identity) = profile
        .identity
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("身份：{identity}"));
    }
    lines.push(format!("Host 时区：{}", profile.timezone));
    lines.join("\n")
}

trait KindName {
    fn kind_as_str(&self) -> &'static str;
}
impl KindName for MemoryEntry {
    fn kind_as_str(&self) -> &'static str {
        match self.kind {
            MemoryKind::UserPreference => "user",
            MemoryKind::UserProfile => "user_profile",
            MemoryKind::BotExperience => "bot",
            MemoryKind::BotWorklog => "worklog",
            MemoryKind::Project => "project",
        }
    }
}

/// A small provider boundary so the real gateway can inject its maintenance
/// model while tests use a deterministic provider.
pub trait MaintenanceProvider: Send + Sync {
    fn summarize(&self, text: &str) -> Result<String, MemoryError>;
    fn extract(&self, _text: &str) -> Result<Vec<MemoryDraft>, MemoryError> {
        Ok(Vec::new())
    }
}

/// Async boundary for the real provider/model client. The synchronous trait
/// above remains useful for deterministic maintenance jobs and unit tests.
#[async_trait::async_trait]
pub trait AsyncMaintenanceProvider: Send + Sync {
    async fn summarize(&self, text: &str) -> Result<String, MemoryError>;
    async fn compact(&self, text: &str) -> Result<String, MemoryError> {
        self.summarize(text).await
    }
    async fn extract(&self, _text: &str) -> Result<Vec<MemoryDraft>, MemoryError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryDraft {
    pub target: MemoryTarget,
    pub kind: MemoryKind,
    pub content: String,
    pub source: MemorySource,
}

pub fn compact_text<P: MaintenanceProvider>(
    provider: &P,
    text: &str,
) -> Result<String, MemoryError> {
    provider.summarize(text)
}

pub fn maintenance_extract<P: MaintenanceProvider>(
    store: &mut MemoryStore,
    provider: &P,
    run_id: &str,
    transcript: &str,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let drafts = provider.extract(transcript)?;
    drafts
        .into_iter()
        .map(|draft| {
            let entry = store.add(MemoryRequest {
                target: draft.target,
                action: MemoryAction::Add,
                content: draft.content,
                id: None,
                kind: Some(draft.kind),
                source: MemorySource {
                    run_id: Some(run_id.to_owned()),
                    ..draft.source
                },
            })?;
            Ok(entry)
        })
        .collect()
}

pub async fn async_maintenance_extract<P: AsyncMaintenanceProvider>(
    store: &mut MemoryStore,
    provider: &P,
    run_id: &str,
    transcript: &str,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let drafts = provider.extract(transcript).await?;
    drafts
        .into_iter()
        .map(|draft| {
            store.add(MemoryRequest {
                target: draft.target,
                action: MemoryAction::Add,
                content: draft.content,
                id: None,
                kind: Some(draft.kind),
                source: MemorySource {
                    run_id: Some(run_id.to_owned()),
                    ..draft.source
                },
            })
        })
        .collect()
}

pub async fn async_compact_text<P: AsyncMaintenanceProvider>(
    provider: &P,
    text: &str,
) -> Result<String, MemoryError> {
    provider.summarize(text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_memory_versions_conflicts_quota_and_profile_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory/state.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let shared = SharedMemoryStore::new(MemoryStore::new());
        let target = MemoryTarget::user("local-user");
        let created = shared
            .user_create_persisted(
                &path,
                target.clone(),
                "Reply in Chinese".into(),
                MemorySender::System,
                Some("chat-7".into()),
                Some("message-9".into()),
            )
            .unwrap();
        assert_eq!(created.version, 1);
        assert_eq!(created.source.by, MemorySender::System);
        assert_eq!(created.source.chat_id.as_deref(), Some("chat-7"));
        assert_eq!(created.source.message_id.as_deref(), Some("message-9"));

        let stale = shared
            .user_update_persisted(
                &path,
                &target,
                &created.id,
                "stale write".into(),
                0,
                MemorySender::User,
            )
            .unwrap_err();
        assert!(matches!(stale, MemoryError::UserConflict(current) if current.version == 1));
        let updated = shared
            .user_update_persisted(
                &path,
                &target,
                &created.id,
                "Updated preference".into(),
                1,
                MemorySender::User,
            )
            .unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.source, created.source);
        assert_eq!(updated.updated_by, MemorySender::User);

        let restored = SharedMemoryStore::new(MemoryStore::load_from(&path).unwrap());
        let entries = restored.user_records(&target).unwrap();
        assert_eq!(entries, vec![updated.clone()]);
        assert_eq!(
            restored.user_quota(&target).unwrap().0,
            "Updated preference".chars().count()
        );
        assert!(matches!(
            restored.user_delete_persisted(&path, &target, &updated.id, 1),
            Err(MemoryError::UserConflict(current)) if current.version == 2
        ));
        restored
            .user_delete_persisted(&path, &target, &updated.id, 2)
            .unwrap();
        assert_eq!(restored.user_quota(&target).unwrap().0, 0);

        for _ in 0..3 {
            restored
                .user_create_persisted(
                    &path,
                    target.clone(),
                    "x".repeat(500),
                    MemorySender::User,
                    None,
                    None,
                )
                .unwrap();
        }
        assert!(matches!(
            restored.user_create_persisted(
                &path,
                target.clone(),
                "x".into(),
                MemorySender::User,
                None,
                None,
            ),
            Err(MemoryError::UserQuotaFull {
                used: 1500,
                limit: 1500,
                ..
            })
        ));

        let profile = restored
            .user_profile_get_or_init(&path, "Asia/Tokyo")
            .unwrap();
        assert_eq!(profile.timezone, "Asia/Tokyo");
        let profile = restored
            .user_profile_update_persisted(
                &path,
                UserProfileUpdate {
                    timezone: "UTC".into(),
                    expected_version: 1,
                    name: Some(Some("Ari".into())),
                    language: None,
                    identity: None,
                    updated_by: MemorySender::User,
                },
            )
            .unwrap();
        assert_eq!(profile.name.as_deref(), Some("Ari"));
        assert_eq!(profile.timezone, "Asia/Tokyo");
        assert!(matches!(
            restored.user_profile_update_persisted(
                &path,
                UserProfileUpdate {
                    timezone: "UTC".into(),
                    expected_version: 1,
                    name: Some(None),
                    language: None,
                    identity: None,
                    updated_by: MemorySender::User,
                },
            ),
            Err(MemoryError::ProfileConflict(current)) if current.version == 2
        ));
    }

    fn request(target: MemoryTarget, content: &str, kind: MemoryKind) -> MemoryRequest {
        MemoryRequest {
            target,
            action: MemoryAction::Add,
            content: content.into(),
            id: None,
            kind: Some(kind),
            source: MemorySource {
                bot_id: Some("bot".into()),
                run_id: Some("run".into()),
                session_id: Some("session".into()),
            },
        }
    }

    #[test]
    fn persisted_entries_keep_source_and_reload() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory.json");
        let mut store = MemoryStore::new();
        store
            .add(request(
                MemoryTarget::project("p"),
                "decision",
                MemoryKind::Project,
            ))
            .unwrap();
        store.save_to(&path).unwrap();
        let restored = MemoryStore::load_from(&path).unwrap();
        assert_eq!(restored.entries().count(), 1);
        assert_eq!(
            restored.entries().next().unwrap().source.bot_id.as_deref(),
            Some("bot")
        );
    }

    #[test]
    fn immediate_mutations_require_versions_and_replay_by_operation_id() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("data/memory/state.json");
        let shared = SharedMemoryStore::new(MemoryStore::load_from(&path).unwrap());
        let add = request(
            MemoryTarget::user("local-user"),
            "likes quiet mornings",
            MemoryKind::UserPreference,
        );
        let first = shared
            .mutate_persisted(
                &path,
                add.clone(),
                None,
                "op-add",
                MemoryChangeActor {
                    kind: MemoryChangeActorKind::User,
                    bot_id: None,
                    run_id: None,
                },
            )
            .unwrap();
        assert_eq!(first.version, 1);
        assert_eq!(
            shared
                .mutate_persisted(
                    &path,
                    add,
                    None,
                    "op-add",
                    MemoryChangeActor {
                        kind: MemoryChangeActorKind::User,
                        bot_id: None,
                        run_id: None
                    }
                )
                .unwrap()
                .revision,
            first.revision
        );
        let mut replace = request(
            MemoryTarget::user("local-user"),
            "likes early walks",
            MemoryKind::UserPreference,
        );
        replace.action = MemoryAction::Replace;
        replace.id = first.entry.as_ref().map(|entry| entry.id.clone());
        assert!(matches!(
            shared.mutate_persisted(
                &path,
                replace.clone(),
                None,
                "missing-cas",
                MemoryChangeActor::default()
            ),
            Err(MemoryError::UserConflict(_))
        ));
        let result = shared
            .mutate_persisted(
                &path,
                replace,
                Some(1),
                "op-replace",
                MemoryChangeActor::default(),
            )
            .unwrap();
        assert_eq!(result.version, 2);
        assert_eq!(result.entry.unwrap().version, 2);
    }

    #[test]
    fn write_ahead_record_recovers_failed_state_write_and_idempotency_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        fs::create_dir(path.with_extension("tmp")).unwrap();
        let shared = SharedMemoryStore::new(MemoryStore::new());
        let mutation = request(
            MemoryTarget::bot("b"),
            "learned lesson",
            MemoryKind::BotExperience,
        );
        assert!(shared
            .mutate_persisted(
                &path,
                mutation.clone(),
                None,
                "durable-op",
                MemoryChangeActor {
                    kind: MemoryChangeActorKind::Bot,
                    bot_id: Some("b".into()),
                    run_id: Some("r".into())
                }
            )
            .is_err());
        fs::remove_dir(path.with_extension("tmp")).unwrap();
        let recovered = SharedMemoryStore::new(MemoryStore::load_from(&path).unwrap());
        assert_eq!(recovered.search("learned", None).unwrap().len(), 1);
        let replay = recovered
            .mutate_persisted(
                &path,
                mutation,
                None,
                "durable-op",
                MemoryChangeActor {
                    kind: MemoryChangeActorKind::Bot,
                    bot_id: Some("b".into()),
                    run_id: Some("r".into()),
                },
            )
            .unwrap();
        assert_eq!(replay.revision, 1);
        assert_eq!(recovered.changes_since(0).unwrap().len(), 1);
    }

    #[test]
    fn recovery_keeps_healthy_changelog_append_only_and_repairs_partial_transactions() {
        let temp = tempfile::tempdir().unwrap();
        let healthy_path = temp.path().join("healthy/state.json");
        fs::create_dir_all(healthy_path.parent().unwrap()).unwrap();
        let mut healthy = MemoryStore::new();
        healthy.revision = 1;
        write_persisted_state(&healthy_path, &healthy.persisted_state()).unwrap();
        let legacy_line = r#"{"operation_id":"legacy:1:profile","revision":1,"op":"profile_update","entry_id":null,"target":{"scope":"user","owner_id":"local-user"},"kind":"user_profile","content":"{}","actor":{"kind":"system"},"at":"2026-10-11T00:00:00Z"}"#;
        fs::write(changelog_path(&healthy_path), format!("{legacy_line}\n")).unwrap();
        let original_log = fs::read(changelog_path(&healthy_path)).unwrap();
        healthy.storage_path = Some(healthy_path.clone());
        recover_pending_state(&healthy_path, &mut healthy).unwrap();
        assert_eq!(
            fs::read(changelog_path(&healthy_path)).unwrap(),
            original_log
        );
        OpenOptions::new()
            .append(true)
            .open(changelog_path(&healthy_path))
            .unwrap()
            .write_all(b"{\"torn\":")
            .unwrap();
        recover_pending_state(&healthy_path, &mut healthy).unwrap();
        let repaired_log = fs::read_to_string(changelog_path(&healthy_path)).unwrap();
        assert!(repaired_log.ends_with('\n'));
        assert!(!repaired_log.contains("torn"));

        for restart_after_failure in [false, true] {
            let state_path = temp
                .path()
                .join(if restart_after_failure {
                    "restart"
                } else {
                    "retry"
                })
                .join("state.json");
            fs::create_dir_all(state_path.parent().unwrap()).unwrap();
            MemoryStore::new().save_to(&state_path).unwrap();
            let before = MemoryStore::load_from(&state_path).unwrap();
            let mut partial = before.clone_for_transaction();
            partial
                .add(request(
                    MemoryTarget::bot("worker"),
                    "first change",
                    MemoryKind::BotExperience,
                ))
                .unwrap();
            partial
                .add(request(
                    MemoryTarget::bot("worker"),
                    "second change",
                    MemoryKind::BotExperience,
                ))
                .unwrap();
            let mut append_count = 0;
            let failed = append_state_diff_with(
                &state_path,
                &before,
                &mut partial,
                "partially-written",
                None,
                |path, change| {
                    append_count += 1;
                    if append_count == 2 {
                        return Err(MemoryError::Invalid("injected append failure".into()));
                    }
                    append_memory_change(path, change)
                },
            );
            assert!(failed.is_err());
            assert_eq!(read_all_memory_changes(&state_path).unwrap().len(), 1);

            let mut live = if restart_after_failure {
                MemoryStore::load_from(&state_path).unwrap()
            } else {
                before.clone_for_transaction()
            };
            recover_pending_state(&state_path, &mut live).unwrap();
            assert_eq!(live.revision, 0);
            assert!(read_all_memory_changes(&state_path).unwrap().is_empty());

            let before_retry = live.clone_for_transaction();
            let mut retry = before_retry.clone_for_transaction();
            retry
                .add(request(
                    MemoryTarget::bot("worker"),
                    "first change",
                    MemoryKind::BotExperience,
                ))
                .unwrap();
            retry
                .add(request(
                    MemoryTarget::bot("worker"),
                    "second change",
                    MemoryKind::BotExperience,
                ))
                .unwrap();
            append_state_diff(
                &state_path,
                &before_retry,
                &mut retry,
                "partially-written",
                None,
            )
            .unwrap();
            retry.save_to(&state_path).unwrap();
            let revisions = read_all_memory_changes(&state_path)
                .unwrap()
                .into_iter()
                .map(|change| change.revision)
                .collect::<Vec<_>>();
            assert_eq!(revisions, vec![1, 2]);
            let reopened = MemoryStore::load_from(&state_path).unwrap();
            assert_eq!(reopened.revision, 2);
            assert_eq!(reopened.entries().count(), 2);
        }
    }

    #[test]
    fn changelog_gap_and_segment_retention_are_detected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        let shared = SharedMemoryStore::new(MemoryStore::load_from(&path).unwrap());
        for index in 0..3 {
            shared
                .mutate_persisted(
                    &path,
                    request(
                        MemoryTarget::bot("b"),
                        &format!("lesson {index}"),
                        MemoryKind::BotExperience,
                    ),
                    None,
                    &format!("op-{index}"),
                    MemoryChangeActor::default(),
                )
                .unwrap();
        }
        let changes = read_all_memory_changes(&path).unwrap();
        let retained = changes
            .into_iter()
            .filter(|change| change.revision != 2)
            .collect::<Vec<_>>();
        let log = changelog_path(&path);
        let mut file = fs::File::create(&log).unwrap();
        for change in retained {
            serde_json::to_writer(&mut file, &change_without_recovery(&change)).unwrap();
            file.write_all(b"\n").unwrap();
        }
        assert!(matches!(
            shared.changes_since(0),
            Err(MemoryError::ChangeLogGap {
                earliest_revision: 2,
                ..
            })
        ));
        write_segment_seen(&path, "old-run", 0).unwrap();
        let old_path = segment_path(&path, "old-run");
        let mut old: MemorySegmentRecord =
            serde_json::from_slice(&fs::read(&old_path).unwrap()).unwrap();
        old.updated_at = Utc::now() - Duration::days(SEGMENT_RETENTION_DAYS + 1);
        write_segment_record(&path, &old).unwrap();
        prune_expired_memory_files(&path, Utc::now()).unwrap();
        assert!(!old_path.exists());
    }

    #[test]
    fn empty_store_and_segment_snapshot_state_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("empty/state.json");
        let shared = SharedMemoryStore::new(MemoryStore::load_from(&path).unwrap());
        assert!(shared.changes_since(0).unwrap().is_empty());
        let segment_id = shared.open_segment("bot:chat", 0).unwrap();
        assert_eq!(
            shared.current_segment_id("bot:chat").unwrap().as_deref(),
            Some(segment_id.as_str())
        );
        let snapshot = MemorySnapshot {
            revision: 0,
            generated_at: Utc::now(),
            targets: vec![],
            entries: vec![],
            user_profile: None,
        };
        shared
            .snapshot_cache_put("bot:chat", snapshot.clone())
            .unwrap();
        let restarted = SharedMemoryStore::new(MemoryStore::load_from(&path).unwrap());
        assert_eq!(
            restarted.snapshot_cache_get("bot:chat").unwrap(),
            Some(snapshot)
        );
        assert_eq!(
            restarted.segment_seen_revision(&segment_id).unwrap(),
            Some(0)
        );
        let next_id = restarted.open_segment("bot:chat", 0).unwrap();
        assert_ne!(next_id, segment_id);
    }

    #[test]
    fn legacy_changelog_rows_are_normalized_and_preserved_during_prune() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("memory/state.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut state = PersistedState::default();
        state.revision = 1;
        write_persisted_state(&path, &state).unwrap();
        let legacy = r#"{"operation_id":"old-op","revision":1,"op":"create","entry_id":"m-old","target":{"scope":"user","owner_id":"local-user"},"kind":"user_preference","content":"prefers examples","actor":{"kind":"system","chat_id":"chat-old","message_id":"msg-old"},"at":"2026-10-11T00:00:00Z"}"#;
        fs::write(changelog_path(&path), format!("{legacy}\n")).unwrap();
        let changes = read_all_memory_changes(&path).unwrap();
        assert_eq!(changes[0].kind, MemoryChangeKind::UserPreference);
        assert_eq!(changes[0].source.chat_id.as_deref(), Some("chat-old"));
        prune_memory_changes(&path, 1, Utc::now()).unwrap();
        let retained = fs::read_to_string(changelog_path(&path)).unwrap();
        assert!(retained.contains("legacy_payload"));
        assert!(retained.contains("chat-old"));
    }

    #[test]
    fn shared_store_serializes_concurrent_quota_checks() {
        let shared = Arc::new(SharedMemoryStore::new(MemoryStore::new()));
        let mut workers = Vec::new();
        for _ in 0..4 {
            let shared = Arc::clone(&shared);
            workers.push(std::thread::spawn(move || {
                shared
                    .add(request(
                        MemoryTarget::user("u"),
                        &"x".repeat(500),
                        MemoryKind::UserPreference,
                    ))
                    .is_ok()
            }));
        }
        let successes = workers
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .filter(|ok| *ok)
            .count();
        assert_eq!(successes, 3);
        assert_eq!(shared.search("x", None).unwrap().len(), 3);
    }

    #[test]
    fn quotas_are_per_scope_and_kind() {
        let mut store = MemoryStore::new();
        let target = MemoryTarget::user("u");
        store
            .add(request(
                target.clone(),
                &"x".repeat(USER_LIMIT),
                MemoryKind::UserPreference,
            ))
            .unwrap();
        let err = store
            .add(request(target, "x", MemoryKind::UserPreference))
            .unwrap_err();
        assert!(matches!(err, MemoryError::QuotaExceeded { .. }));
    }

    #[test]
    fn worklog_rolls_old_entries_into_a_bounded_month_marker() {
        let mut store = MemoryStore::new();
        let target = MemoryTarget::bot("b");
        store
            .append_worklog(target.clone(), "a".repeat(1_000), MemorySource::default())
            .unwrap();
        store
            .append_worklog(target.clone(), "b".repeat(600), MemorySource::default())
            .unwrap();
        let rows: Vec<_> = store
            .entries()
            .filter(|entry| entry.target == target && entry.kind == MemoryKind::BotWorklog)
            .collect();
        assert!(
            rows.iter()
                .map(|entry| entry.content.chars().count())
                .sum::<usize>()
                <= BOT_WORKLOG_LIMIT
        );
        let summary = rows
            .iter()
            .find(|entry| entry.content.starts_with('['))
            .expect("rolled worklog should have a monthly summary");
        assert_eq!(summary.source.bot_id.as_deref(), None);

        let mut sourced = MemoryStore::new();
        let source = MemorySource {
            bot_id: Some("bot-source".into()),
            run_id: Some("run-source".into()),
            session_id: Some("session-source".into()),
        };
        sourced
            .append_worklog(MemoryTarget::bot("b"), "x".repeat(1_400), source.clone())
            .unwrap();
        sourced
            .append_worklog(MemoryTarget::bot("b"), "y".repeat(200), source)
            .unwrap();
        let summary = sourced
            .entries()
            .find(|entry| entry.content.starts_with('['))
            .expect("sourced worklog should have a monthly summary");
        assert_eq!(summary.source.bot_id.as_deref(), Some("bot-source"));
    }

    #[test]
    fn snapshot_stays_frozen_until_a_new_segment_rebuilds_it() {
        let mut store = MemoryStore::new();
        let target = MemoryTarget::bot("b");
        let at = Utc::now();
        let (snap, changed) = store.snapshot(std::slice::from_ref(&target), None, at);
        assert!(changed);
        let (_, changed) = store.snapshot(
            std::slice::from_ref(&target),
            Some(&snap),
            at + Duration::hours(25),
        );
        assert!(!changed);
        store
            .add(request(target.clone(), "new", MemoryKind::BotExperience))
            .unwrap();
        let (_, changed) = store.snapshot(&[target], Some(&snap), at + Duration::hours(25));
        assert!(!changed);
    }

    #[test]
    fn group_context_has_at_most_thirty_recent_messages_and_80_percent_boundary() {
        let store = MemoryStore::new();
        let recent: Vec<_> = (0..40).map(|i| format!("m{i}")).collect();
        let package = assemble_context(
            &store,
            &ContextRequest {
                kind: ConversationKind::Group,
                memory_targets: vec![],
                l0_platform_rules: "rules".into(),
                l1_bot_identity: "bot".into(),
                announcement: "a".into(),
                task: "t".into(),
                trigger: "tr".into(),
                references: vec![],
                recent_messages: recent,
                segment_summary: String::new(),
                recent_context: vec![],
                run_events: vec![],
                model_context_window: 100,
                previous_snapshot: None,
                now: Utc::now(),
            },
        );
        let l3 = &package.layers[3].content;
        assert!(!l3.contains("m0"));
        assert!(l3.contains("m10"));
        assert!(!package.new_segment); // rebuilding an initial snapshot alone does not roll the segment
    }

    #[test]
    fn user_profile_is_pinned_in_user_authorized_private_and_group_contexts() {
        let mut store = MemoryStore::new();
        store.user_profile = Some(UserProfile {
            name: Some("小林".into()),
            language: Some("zh-CN".into()),
            identity: Some("设计师".into()),
            timezone: "Asia/Shanghai".into(),
            onboarded: true,
            version: 2,
            updated_by: MemorySender::User,
            updated_at: Utc::now(),
        });
        let make_request = |kind, bot_id: &str, user_scope: bool| ContextRequest {
            kind,
            memory_targets: [
                user_scope.then(|| MemoryTarget::user("user")),
                Some(MemoryTarget::bot(bot_id)),
            ]
            .into_iter()
            .flatten()
            .collect(),
            l0_platform_rules: String::new(),
            l1_bot_identity: String::new(),
            announcement: String::new(),
            task: String::new(),
            trigger: String::new(),
            references: vec![],
            recent_messages: vec![],
            segment_summary: String::new(),
            recent_context: vec![],
            run_events: vec![],
            model_context_window: 128_000,
            previous_snapshot: None,
            now: Utc::now(),
        };
        for (kind, bot) in [
            (ConversationKind::Private, "main"),
            (ConversationKind::Private, "worker"),
            (ConversationKind::Group, "worker"),
        ] {
            let context = assemble_context(&store, &make_request(kind, bot, true));
            let l2 = &context.layers[2].content;
            assert!(l2.starts_with("用户档案（优先参考）"));
            assert!(l2.contains("称呼：小林"));
            assert!(l2.contains("语言：zh-CN"));
            assert!(l2.contains("身份：设计师"));
            assert!(l2.contains("Host 时区：Asia/Shanghai"));
        }

        store
            .apply_user_profile_update(UserProfileUpdate {
                timezone: "UTC".into(),
                expected_version: 2,
                name: Some(Some("林女士".into())),
                language: None,
                identity: None,
                updated_by: MemorySender::User,
            })
            .unwrap();
        let updated = assemble_context(
            &store,
            &make_request(ConversationKind::Private, "worker-2", true),
        );
        assert!(updated.layers[2]
            .content
            .starts_with("用户档案（优先参考）"));
        assert!(updated.layers[2].content.contains("称呼：林女士"));
        assert!(!updated.layers[2].content.contains("称呼：小林"));

        let bot_only = assemble_context(
            &store,
            &make_request(ConversationKind::Private, "worker-2", false),
        );
        assert!(!bot_only.layers[2].content.contains("用户档案"));
        assert!(!bot_only.layers[2].content.contains("林女士"));
        let other_user = assemble_context(
            &store,
            &ContextRequest {
                memory_targets: vec![MemoryTarget::user("user-other")],
                ..make_request(ConversationKind::Private, "worker-2", false)
            },
        );
        assert!(!other_user.layers[2].content.contains("用户档案"));
        assert!(!other_user.layers[2].content.contains("林女士"));
    }

    #[test]
    fn l2_uses_frozen_entry_and_profile_values_until_new_snapshot() {
        let mut store = MemoryStore::new();
        let user_target = MemoryTarget::user("local-user");
        let mut profile = UserProfile {
            name: Some("旧称呼".into()),
            language: Some("zh-CN".into()),
            identity: None,
            timezone: "Asia/Shanghai".into(),
            onboarded: true,
            version: 1,
            updated_by: MemorySender::User,
            updated_at: Utc::now(),
        };
        store.user_profile = Some(profile.clone());
        let entry = store
            .add(request(
                user_target.clone(),
                "喜欢旧版本",
                MemoryKind::UserPreference,
            ))
            .unwrap();
        let make_request = |previous_snapshot| ContextRequest {
            kind: ConversationKind::Private,
            memory_targets: vec![user_target.clone()],
            l0_platform_rules: "rules".into(),
            l1_bot_identity: "bot".into(),
            announcement: String::new(),
            task: String::new(),
            trigger: String::new(),
            references: vec![],
            recent_messages: vec![],
            segment_summary: String::new(),
            recent_context: vec![],
            run_events: vec![],
            model_context_window: 10000,
            previous_snapshot,
            now: Utc::now(),
        };
        let first = assemble_context(&store, &make_request(None));
        let frozen_l2 = first.layers[2].content.clone();
        profile.name = Some("新称呼".into());
        profile.version = 2;
        store.user_profile = Some(profile);
        let mut replacement = request(
            user_target.clone(),
            "喜欢新版本",
            MemoryKind::UserPreference,
        );
        replacement.action = MemoryAction::Replace;
        replacement.id = Some(entry.id);
        store.replace(replacement, 1).unwrap();
        let same_segment = assemble_context(&store, &make_request(Some(first.snapshot.clone())));
        assert_eq!(same_segment.layers[2].content, frozen_l2);
        let next_segment = assemble_context(&store, &make_request(None));
        assert!(next_segment.layers[2].content.contains("新称呼"));
        assert!(next_segment.layers[2].content.contains("喜欢新版本"));
    }

    #[test]
    fn searches_sessions_projects_and_history() {
        let mut index = ConversationIndex::default();
        let at = Utc::now();
        for i in 0..3 {
            index.add_message(SessionMessage {
                id: i.to_string(),
                session_id: "s".into(),
                chat_id: "c".into(),
                project_id: None,
                role: "user".into(),
                content: format!("hello {i}"),
                at,
            });
        }
        index.add_project(ProjectRecord {
            id: "p".into(),
            name: "Login".into(),
            goal: "ship auth".into(),
            content: "OAuth".into(),
        });
        assert_eq!(index.session_search("s", "HELLO").len(), 3);
        assert_eq!(index.chat_history("c", 2).len(), 2);
        assert_eq!(index.project_find("oauth").len(), 1);
    }

    struct MockProvider;
    impl MaintenanceProvider for MockProvider {
        fn summarize(&self, text: &str) -> Result<String, MemoryError> {
            Ok(format!(
                "summary: {}",
                text.chars().take(10).collect::<String>()
            ))
        }
        fn extract(&self, _text: &str) -> Result<Vec<MemoryDraft>, MemoryError> {
            Ok(vec![MemoryDraft {
                target: MemoryTarget::user("u"),
                kind: MemoryKind::UserPreference,
                content: "likes tests".into(),
                source: MemorySource::default(),
            }])
        }
    }

    #[test]
    fn maintenance_provider_is_injectable() {
        let mut store = MemoryStore::new();
        let rows =
            maintenance_extract(&mut store, &MockProvider, "maintenance", "transcript").unwrap();
        assert_eq!(rows.len(), 1);
        assert!(compact_text(&MockProvider, "abcdef")
            .unwrap()
            .starts_with("summary"));
        let target = MemoryTarget::bot("b");
        store
            .append_worklog(target.clone(), "one", MemorySource::default())
            .unwrap();
        store
            .append_worklog(target.clone(), "two", MemorySource::default())
            .unwrap();
        assert!(store
            .maintain_worklog(&target, &MockProvider)
            .unwrap()
            .is_some());
    }
}
