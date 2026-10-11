//! Server-owned memory, search and prompt-context primitives.
//!
//! The gateway owns authorization and chooses the target identifiers.  This
//! crate owns quotas, run staging, stable snapshots and the small search index
//! used by memory/session/project tools.

use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
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

/// Stable L0 platform rules shared by every context. Keep this string fixed
/// across turns so prompt caching remains effective; dynamic Bot/project data
/// belongs in L1/L3.
pub const DEFAULT_L0_PLATFORM_RULES: &str = r#"Platform rules:
- Use tools only through their declared schemas. Return tool errors to the model so it can correct them.
- Bot-to-Bot communication uses send_msg. Every message must be self-contained; do not rely on private model context.
- Main-Bot-only actions (project creation, member assignment, and cross-Bot coordination) are unavailable to ordinary Bots.
- Before a write, exec, external request, browser payment, sudo, destructive rm, or git push, follow the approval policy.
- When the user expresses a preference, corrects the Bot, or a durable conclusion is reached, record it with the memory tool.
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
    #[error("run already exists: {0}")]
    RunExists(String),
    #[error("run has no staged changes: {0}")]
    NoStagedChanges(String),
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
                | (MemoryScope::Bot, Self::BotExperience | Self::BotWorklog)
                | (MemoryScope::Project, Self::Project)
        )
    }
    fn limit(&self) -> usize {
        match self {
            Self::UserPreference => USER_LIMIT,
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
    pub source: MemorySource,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

#[derive(Clone, Debug)]
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
    staged: HashMap<String, Vec<MemoryRequest>>,
    revision: u64,
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

    pub fn begin_run(&self, run_id: impl Into<String>) -> Result<(), MemoryError> {
        self.inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .begin_run(run_id)
    }

    pub fn stage(&self, run_id: &str, request: MemoryRequest) -> Result<(), MemoryError> {
        self.inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .stage(run_id, request)
    }

    pub fn commit_run(&self, run_id: &str) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .commit_run(run_id)
    }

    /// Commit a run and replace its durable snapshot as one logical operation.
    /// The candidate state is written before it becomes visible through the
    /// shared store, so a failed write cannot leave the process ahead of disk.
    pub fn commit_run_persisted(
        &self,
        run_id: &str,
        path: impl AsRef<Path>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .commit_run_persisted(run_id, path)
    }

    pub fn rollback_run(&self, run_id: &str) -> Result<bool, MemoryError> {
        Ok(self
            .inner
            .write()
            .map_err(|_| MemoryError::LockPoisoned)?
            .rollback_run(run_id))
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
        Ok(store.entries.contains_key(entry_id).then(|| {
            store
                .user_metadata
                .get(entry_id)
                .map(|metadata| metadata.version.max(1))
                .unwrap_or(1)
        }))
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
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
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
        let mut candidate = store.clone_without_staging();
        let (result, event_name, event_data, audit_data) = operation(&mut candidate)?;
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
    ) -> Result<MemoryMutationResult, MemoryError> {
        validate_request(&request)?;
        let mut store = self.inner.write().map_err(|_| MemoryError::LockPoisoned)?;
        let mut candidate = store.clone_without_staging();
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
                    .user_metadata
                    .get(id)
                    .map(|metadata| metadata.version.max(1))
                    .unwrap_or(1);
                if expected_version != Some(version) {
                    return Err(MemoryError::VersionConflict {
                        current: Box::new(current),
                        version,
                    });
                }
                Some(current)
            }
        };
        let changed = candidate.apply(request.clone())?;
        candidate.revision = candidate.revision.wrapping_add(1);
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
                let old = prior.expect("remove has an existing entry");
                let version = expected_version.unwrap_or(1).saturating_add(1);
                (None, version, old.kind)
            }
        };
        let used = candidate.used(&request.target, &kind);
        let remaining_chars = kind.limit().saturating_sub(used);
        candidate.save_to(path)?;
        *store = candidate;
        Ok(MemoryMutationResult {
            entry,
            version,
            revision: store.revision,
            remaining_chars,
        })
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
        let mut candidate = store.clone_without_staging();
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
                updated_by: Some(by),
            },
        );
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
        let mut candidate = store.clone_without_staging();
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
        let mut entry = match candidate.replace(MemoryRequest {
            target: target.clone(),
            action: MemoryAction::Replace,
            content,
            id: Some(entry_id.to_owned()),
            kind: Some(MemoryKind::UserPreference),
            source: old.source.clone(),
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
        let metadata = candidate
            .user_metadata
            .entry(entry_id.to_owned())
            .or_default();
        metadata.version = old_record.version.saturating_add(1);
        metadata.source = Some(if matches!(updated_by, MemorySender::System) {
            UserMemorySource {
                by: MemorySender::System,
                chat_id: source_chat_id,
                message_id: source_message_id,
            }
        } else {
            old_record.source
        });
        metadata.updated_by = Some(updated_by);
        // Generic memory records preserve their own provenance. User-facing
        // source and updater are carried in the metadata map above.
        entry.source = old.source;
        candidate.entries.insert(entry_id.to_owned(), entry.clone());
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
        let mut candidate = store.clone_without_staging();
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
        candidate.remove(MemoryRequest {
            target: target.clone(),
            action: MemoryAction::Remove,
            content: String::new(),
            id: Some(entry_id.to_owned()),
            kind: Some(MemoryKind::UserPreference),
            source: old.source,
        })?;
        candidate.user_metadata.remove(entry_id);
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
        let mut candidate = store.clone_without_staging();
        candidate.user_profile = Some(profile.clone());
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
        let mut candidate = store.clone_without_staging();
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
        let mut candidate = store.clone_without_staging();
        let profile = candidate.apply_user_profile_onboarding(update)?;
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
        let mut entry = match self.replace(MemoryRequest {
            target: target.clone(),
            action: MemoryAction::Replace,
            content,
            id: Some(entry_id.to_owned()),
            kind: Some(MemoryKind::UserPreference),
            source: old.source.clone(),
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
        let metadata = self.user_metadata.entry(entry_id.to_owned()).or_default();
        metadata.version = old_record.version.saturating_add(1);
        metadata.source = Some(old_record.source);
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
        self.remove(MemoryRequest {
            target: target.clone(),
            action: MemoryAction::Remove,
            content: String::new(),
            id: Some(entry_id.to_owned()),
            kind: Some(MemoryKind::UserPreference),
            source: old.source,
        })?;
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

    pub fn begin_run(&mut self, run_id: impl Into<String>) -> Result<(), MemoryError> {
        let run_id = run_id.into();
        if self.staged.contains_key(&run_id) {
            return Err(MemoryError::RunExists(run_id));
        }
        self.staged.insert(run_id, Vec::new());
        Ok(())
    }

    pub fn stage(&mut self, run_id: &str, request: MemoryRequest) -> Result<(), MemoryError> {
        if !self.staged.contains_key(run_id) {
            self.begin_run(run_id)?;
        }
        validate_request(&request)?;
        let staged = self.staged.get_mut(run_id).expect("run inserted");
        // Execution may ask for context more than once while refreshing a
        // snapshot.  Identical writes in one run are retries, not distinct
        // memories; dedupe before quota accounting and commit.
        if !staged.iter().any(|existing| existing == &request) {
            staged.push(request);
        }
        Ok(())
    }

    pub fn rollback_run(&mut self, run_id: &str) -> bool {
        self.staged.remove(run_id).is_some()
    }

    /// Apply all staged operations atomically from the caller's point of view.
    /// Quotas are checked against a cloned state before the live state changes.
    pub fn commit_run(&mut self, run_id: &str) -> Result<Vec<MemoryEntry>, MemoryError> {
        let requests = self
            .staged
            .remove(run_id)
            .ok_or_else(|| MemoryError::NoStagedChanges(run_id.to_string()))?;
        let mut candidate = self.clone_without_staging();
        let mut changed = Vec::new();
        for request in requests {
            changed.extend(candidate.apply(request)?);
        }
        self.entries = candidate.entries;
        self.user_metadata = candidate.user_metadata;
        self.user_profile = candidate.user_profile;
        self.operation_receipts = candidate.operation_receipts;
        self.revision = self.revision.wrapping_add(1);
        Ok(changed)
    }

    fn commit_run_persisted(
        &mut self,
        run_id: &str,
        path: impl AsRef<Path>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let requests = self
            .staged
            .get(run_id)
            .cloned()
            .ok_or_else(|| MemoryError::NoStagedChanges(run_id.to_string()))?;
        let mut candidate = self.clone_without_staging();
        let mut changed = Vec::new();
        for request in requests {
            changed.extend(candidate.apply(request)?);
        }
        candidate.revision = candidate.revision.wrapping_add(1);
        candidate.save_to(path)?;
        self.entries = candidate.entries;
        self.user_metadata = candidate.user_metadata;
        self.user_profile = candidate.user_profile;
        self.revision = candidate.revision;
        self.staged.remove(run_id);
        Ok(changed)
    }

    pub fn add(&mut self, mut request: MemoryRequest) -> Result<MemoryEntry, MemoryError> {
        request.action = MemoryAction::Add;
        let changed = self.apply(request)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(changed.into_iter().next().expect("add creates entry"))
    }

    pub fn replace(&mut self, mut request: MemoryRequest) -> Result<MemoryEntry, MemoryError> {
        request.action = MemoryAction::Replace;
        let changed = self.apply(request)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(changed.into_iter().next().expect("replace creates entry"))
    }

    pub fn remove(&mut self, mut request: MemoryRequest) -> Result<(), MemoryError> {
        request.action = MemoryAction::Remove;
        self.apply(request)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    fn clone_without_staging(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            user_metadata: self.user_metadata.clone(),
            user_profile: self.user_profile.clone(),
            operation_receipts: self.operation_receipts.clone(),
            staged: HashMap::new(),
            revision: self.revision,
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
            version: metadata.version.max(1),
            source,
            updated_by: metadata.updated_by.unwrap_or(legacy_sender),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
        }
    }

    fn entry_version(&self, entry_id: &str) -> u64 {
        self.user_metadata
            .get(entry_id)
            .map(|metadata| metadata.version.max(1))
            .unwrap_or(1)
    }

    fn apply(&mut self, request: MemoryRequest) -> Result<Vec<MemoryEntry>, MemoryError> {
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
        let stale = previous.is_none_or(|snapshot| {
            snapshot.targets != targets
                || snapshot.revision != self.revision
                || now.signed_duration_since(snapshot.generated_at)
                    > Duration::hours(SNAPSHOT_MAX_AGE_HOURS)
        });
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
            },
            true,
        )
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), MemoryError> {
        let state = PersistedState {
            entries: self.entries.values().cloned().collect(),
            revision: self.revision,
            user_metadata: self.user_metadata.clone(),
            user_profile: self.user_profile.clone(),
            operation_receipts: self.operation_receipts.clone(),
        };
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
            return Ok(Self::new());
        }
        let state: PersistedState = serde_json::from_slice(&fs::read(path)?)?;
        let entries = state
            .entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        Ok(Self {
            entries,
            user_metadata: state.user_metadata,
            user_profile: state.user_profile,
            operation_receipts: state.operation_receipts,
            staged: HashMap::new(),
            revision: state.revision,
        })
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
    pub new_segment: bool,
    pub segment_reason: Option<String>,
}

pub fn assemble_context(store: &MemoryStore, request: &ContextRequest) -> ContextPackage {
    let (snapshot, snapshot_stale) = store.snapshot(
        &request.memory_targets,
        request.previous_snapshot.as_ref(),
        request.now,
    );
    let memory_entries = snapshot
        .entries
        .iter()
        .map(|entry| {
            format!(
                "[{} v{}] {}",
                entry.kind_as_str(),
                store.entry_version(&entry.id),
                entry.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let profile_text = if request
        .memory_targets
        .iter()
        .any(|target| target.scope == MemoryScope::User && target.owner_id == "user")
    {
        store.user_profile.as_ref().map(render_user_profile)
    } else {
        None
    };
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
    let reason = if snapshot_stale {
        Some("memory_snapshot_stale".into())
    } else if threshold {
        Some("context_80_percent".into())
    } else {
        None
    };
    ContextPackage {
        layers,
        snapshot,
        new_segment: snapshot_stale || threshold,
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
    store.begin_run(run_id)?;
    let result = (|| {
        for draft in provider.extract(transcript)? {
            store.stage(
                run_id,
                MemoryRequest {
                    target: draft.target,
                    action: MemoryAction::Add,
                    content: draft.content,
                    id: None,
                    kind: Some(draft.kind),
                    source: draft.source,
                },
            )?;
        }
        store.commit_run(run_id)
    })();
    if result.is_err() {
        store.rollback_run(run_id);
    }
    result
}

pub async fn async_maintenance_extract<P: AsyncMaintenanceProvider>(
    store: &mut MemoryStore,
    provider: &P,
    run_id: &str,
    transcript: &str,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    store.begin_run(run_id)?;
    let result = async {
        for draft in provider.extract(transcript).await? {
            store.stage(
                run_id,
                MemoryRequest {
                    target: draft.target,
                    action: MemoryAction::Add,
                    content: draft.content,
                    id: None,
                    kind: Some(draft.kind),
                    source: draft.source,
                },
            )?;
        }
        store.commit_run(run_id)
    }
    .await;
    if result.is_err() {
        store.rollback_run(run_id);
    }
    result
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
    fn staged_memory_commits_only_after_success_and_rolls_back() {
        let mut store = MemoryStore::new();
        store.begin_run("r1").unwrap();
        store
            .stage(
                "r1",
                request(
                    MemoryTarget::user("u"),
                    "likes concise replies",
                    MemoryKind::UserPreference,
                ),
            )
            .unwrap();
        assert_eq!(store.entries().count(), 0);
        store.commit_run("r1").unwrap();
        assert_eq!(store.entries().count(), 1);
        store.begin_run("r2").unwrap();
        store
            .stage(
                "r2",
                request(
                    MemoryTarget::user("u"),
                    "interrupted",
                    MemoryKind::UserPreference,
                ),
            )
            .unwrap();
        store.rollback_run("r2");
        assert_eq!(store.search("interrupted", None).len(), 0);
    }

    #[test]
    fn identical_staged_memory_retries_are_deduplicated() {
        let mut store = MemoryStore::new();
        let write = request(
            MemoryTarget::bot("b"),
            "same context summary",
            MemoryKind::BotWorklog,
        );
        store.stage("retry", write.clone()).unwrap();
        store.stage("retry", write).unwrap();
        let entries = store.commit_run("retry").unwrap();
        assert_eq!(entries.len(), 1);
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
    fn failed_commit_is_atomic_against_live_entries() {
        let mut store = MemoryStore::new();
        store.begin_run("quota").unwrap();
        store
            .stage(
                "quota",
                request(
                    MemoryTarget::user("u"),
                    &"x".repeat(USER_LIMIT),
                    MemoryKind::UserPreference,
                ),
            )
            .unwrap();
        store
            .stage(
                "quota",
                request(
                    MemoryTarget::user("u"),
                    "overflow",
                    MemoryKind::UserPreference,
                ),
            )
            .unwrap();
        assert!(matches!(
            store.commit_run("quota"),
            Err(MemoryError::QuotaExceeded { .. })
        ));
        assert_eq!(store.entries().count(), 0);
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
    fn snapshot_expires_after_a_day_or_revision_change() {
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
        assert!(changed);
        store
            .add(request(target.clone(), "new", MemoryKind::BotExperience))
            .unwrap();
        let (_, changed) = store.snapshot(&[target], Some(&snap), at + Duration::hours(25));
        assert!(changed);
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
        assert!(package.new_segment); // initial snapshot is intentionally a new segment
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

    struct FailingExtractProvider;
    impl MaintenanceProvider for FailingExtractProvider {
        fn summarize(&self, _: &str) -> Result<String, MemoryError> {
            Ok("unused".into())
        }

        fn extract(&self, _: &str) -> Result<Vec<MemoryDraft>, MemoryError> {
            Err(MemoryError::Provider("extract failed".into()))
        }
    }

    struct AsyncFailingExtractProvider;
    #[async_trait::async_trait]
    impl AsyncMaintenanceProvider for AsyncFailingExtractProvider {
        async fn summarize(&self, _: &str) -> Result<String, MemoryError> {
            Ok("unused".into())
        }

        async fn extract(&self, _: &str) -> Result<Vec<MemoryDraft>, MemoryError> {
            Err(MemoryError::Provider("async extract failed".into()))
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn maintenance_extract_rolls_back_when_provider_fails() {
        let mut store = MemoryStore::new();
        assert!(maintenance_extract(
            &mut store,
            &FailingExtractProvider,
            "maintenance-fail",
            "transcript"
        )
        .is_err());
        assert!(matches!(
            store.commit_run("maintenance-fail"),
            Err(MemoryError::NoStagedChanges(_))
        ));
    }

    #[test]
    fn async_maintenance_extract_rolls_back_when_provider_fails() {
        let mut store = MemoryStore::new();
        let result = block_on(async_maintenance_extract(
            &mut store,
            &AsyncFailingExtractProvider,
            "async-maintenance-fail",
            "transcript",
        ));
        assert!(result.is_err());
        assert!(matches!(
            store.commit_run("async-maintenance-fail"),
            Err(MemoryError::NoStagedChanges(_))
        ));
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
