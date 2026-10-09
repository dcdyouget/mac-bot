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
    #[error("memory entry not found: {0}")]
    NotFound(String),
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

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct PersistedState {
    entries: Vec<MemoryEntry>,
    revision: u64,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    entries: BTreeMap<String, MemoryEntry>,
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
}

impl MemoryStore {
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
            staged: HashMap::new(),
            revision: self.revision,
        }
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
    let memory_text = snapshot
        .entries
        .iter()
        .map(|entry| format!("[{}] {}", entry.kind_as_str(), entry.content))
        .collect::<Vec<_>>()
        .join("\n");
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
}

pub async fn async_maintenance_extract<P: AsyncMaintenanceProvider>(
    store: &mut MemoryStore,
    provider: &P,
    run_id: &str,
    transcript: &str,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    store.begin_run(run_id)?;
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

pub async fn async_compact_text<P: AsyncMaintenanceProvider>(
    provider: &P,
    text: &str,
) -> Result<String, MemoryError> {
    provider.summarize(text).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
