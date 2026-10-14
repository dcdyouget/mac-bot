//! Production RPC adapter.
//!
//! The gateway boundary is protocol JSON.  The orchestration crate keeps a
//! compact internal state model, so this module is the single place that
//! expands it to the complete `macbot-protocol` wire objects and validates the
//! result before it is exposed to a client.  Mutations are write-ahead:
//! operation JSONL is synced, then the global event log is synced, then the
//! event is published to connected clients.

mod proposal;

use crate::{
    features::FeatureService, now, rpc_error, GatewayState, RpcBackend, RpcError, RpcResult,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Local, Utc};
use macbot_browser::BrowserError;
use macbot_durable::DurableRuntime;
use macbot_orchestrator::{
    BotDmRoute, Orchestrator, OrchestratorSettings, ToolToggles, UsageTotals,
};
use macbot_protocol::{
    Announcement, Approval, ArtifactVersion, ArtifactVersionGetParams, ArtifactVersionGetResult,
    ArtifactVersionsParams, ArtifactVersionsResult, Assignment, Avatar, Bot, BotDuplicateResult,
    Chat, Device, FileRef, HeatmapResult, Hello, Message, PendingItems, Project, Question, Routine,
    Settings, UsageBreakdownResult, UsageSummaryResult, UsageTimeseriesResult,
};
use macbot_providers::registry::{ProviderRegistry, RegistryError};
use macbot_skills::BotSkillSettingsSnapshot;
use macbot_store::Store;
use macbot_usage::UsageLedger;
use serde::{
    de::{IgnoredAny, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::{Arc, Mutex as StdMutex},
};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

type BotStatusCounts = HashMap<String, (u32, u32, u32, bool)>;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("store: {0}")]
    Store(#[from] macbot_store::StoreError),
    #[error("durable: {0}")]
    Durable(#[from] macbot_durable::DurableError),
    #[error("usage: {0}")]
    Usage(#[from] macbot_usage::Error),
    #[error("provider registry: {0}")]
    Registry(#[from] RegistryError),
    #[error("orchestrator snapshot: {0}")]
    OrchestratorSnapshot(String),
}

const MAX_ARTIFACT_VERSION_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredArtifactVersion {
    version: ArtifactVersion,
    content_base64: String,
}

struct OperationLogRow {
    value: Value,
    has_snapshot: bool,
}

impl<'de> Deserialize<'de> for OperationLogRow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OperationLogRowVisitor;

        impl<'de> Visitor<'de> for OperationLogRowVisitor {
            type Value = OperationLogRow;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an orchestrator operation object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut object = Map::new();
                let mut has_snapshot = false;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "snapshot" {
                        map.next_value::<IgnoredAny>()?;
                        has_snapshot = true;
                    } else {
                        object.insert(key, map.next_value()?);
                    }
                }
                Ok(OperationLogRow {
                    value: Value::Object(object),
                    has_snapshot,
                })
            }
        }

        deserializer.deserialize_map(OperationLogRowVisitor)
    }
}

fn read_operation_log(store: &Store) -> Result<Vec<Value>, AdapterError> {
    let path = store.root().join("data/orchestrator/operations.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path).map_err(macbot_store::StoreError::Io)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut line_number = 0usize;
    let mut operations = Vec::new();
    let mut last_snapshot = None;
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(macbot_store::StoreError::Io)?;
        if read == 0 {
            break;
        }
        line_number += 1;
        if line.last().copied() != Some(b'\n') {
            break;
        }
        let record = &line[..line.len() - 1];
        if record.is_empty() {
            continue;
        }
        let row = serde_json::from_slice::<OperationLogRow>(record).map_err(|source| {
            AdapterError::Store(macbot_store::StoreError::Json {
                path: path.clone(),
                line: line_number,
                source,
            })
        })?;
        if row.has_snapshot {
            last_snapshot = Some((operations.len(), record.to_owned()));
        }
        operations.push(row.value);
    }
    if let Some((index, record)) = last_snapshot {
        let full = serde_json::from_slice::<Value>(&record).map_err(|source| {
            AdapterError::Store(macbot_store::StoreError::Json {
                path,
                line: line_number,
                source,
            })
        })?;
        if let Some(snapshot) = full.get("snapshot") {
            if let Some(operation) = operations.get_mut(index).and_then(Value::as_object_mut) {
                operation.insert("snapshot".into(), snapshot.clone());
            }
        }
    }
    Ok(operations)
}

/// Older orchestrator snapshots did not persist `Message.deleted`. Chat
/// message logs do, so restore the authoritative user tombstones before the
/// backend accepts RPCs; otherwise a queued message could be promoted after
/// an upgrade/restart.
fn restore_deleted_user_chat_tombstones(
    store: &Store,
    orchestrator: &Orchestrator,
) -> Result<bool, AdapterError> {
    let chats_root = store.root().join("data/chats");
    let entries = match fs::read_dir(&chats_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(macbot_store::StoreError::Io(error).into()),
    };
    let snapshot = orchestrator
        .snapshot()
        .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
    let messages = snapshot.get("messages").and_then(Value::as_object);
    let mut seen = HashSet::new();
    let mut migrated = false;
    for entry in entries {
        let entry = entry.map_err(macbot_store::StoreError::Io)?;
        if !entry
            .file_type()
            .map_err(macbot_store::StoreError::Io)?
            .is_dir()
        {
            continue;
        }
        let path = entry.path().join("messages.jsonl");
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(macbot_store::StoreError::Io(error).into()),
        };
        for tombstone in read_tombstones_compatibly(file, &path)? {
            if tombstone.get("deleted").and_then(Value::as_bool) != Some(true)
                || tombstone.pointer("/sender/kind").and_then(Value::as_str) != Some("user")
            {
                continue;
            }
            let Some(chat_id) = tombstone.get("chat_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(message_id) = tombstone.get("id").and_then(Value::as_str) else {
                continue;
            };
            let key = (chat_id.to_owned(), message_id.to_owned());
            let Some(stored_message) = messages.and_then(|messages| messages.get(message_id))
            else {
                continue;
            };
            if !seen.insert(key)
                || stored_message.get("chat_id").and_then(Value::as_str) != Some(chat_id)
                || stored_message.get("sender").and_then(Value::as_str) != Some("user")
            {
                continue;
            }
            orchestrator
                .redact_user_message(message_id)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            migrated = true;
        }
    }
    Ok(migrated)
}

fn read_tombstones_compatibly(file: fs::File, path: &Path) -> Result<Vec<Value>, AdapterError> {
    let mut values = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(macbot_store::StoreError::Io)?;
        match serde_json::from_str::<Value>(&line) {
            Ok(value) => values.push(value),
            Err(error) => tracing::warn!(
                path = %path.display(),
                line = line_number,
                %error,
                "skipping malformed legacy chat row during tombstone migration"
            ),
        }
    }
    Ok(values)
}

fn read_jsonl_values_lossy(file: fs::File) -> Vec<Value> {
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect()
}

#[derive(Clone)]
pub struct ProductionBackend {
    pub orchestrator: Orchestrator,
    pub store: Store,
    pub durable: Arc<Mutex<DurableRuntime>>,
    pub usage: Arc<Mutex<UsageLedger>>,
    pub providers: Arc<Mutex<ProviderRegistry>>,
    write_lock: Arc<Mutex<()>>,
    persist_lock: Arc<Mutex<()>>,
    event_lock: Arc<Mutex<()>>,
    idempotency: Arc<Mutex<HashMap<String, Value>>>,
    startup_repair_events: Arc<StdMutex<Option<Vec<macbot_store::Event>>>>,
}

#[derive(Clone)]
struct BotDmContext {
    source_chat_id: String,
    source_bot_id: String,
    route: BotDmRoute,
}

/// Event ids and message payloads observed before a batch of assignment cards
/// is repaired.  A single attention poll can repair several cards; keeping
/// this index for that batch avoids reparsing the complete event log for every
/// card while retaining the crash-recovery check against durable events.
#[derive(Default)]
struct AssignmentCardEventIndex {
    assignment_ids: HashSet<String>,
    message_rows: HashMap<String, Value>,
    chat_messages: HashMap<String, (Vec<Value>, HashSet<String>)>,
}

impl ProductionBackend {
    pub(crate) async fn record_assignment_recovered(
        &self,
        state: &GatewayState,
        assignment_id: &str,
        recovered_at: String,
    ) -> Result<(), RpcError> {
        let _guard = self.persist_lock.lock().await;
        let Some(assignment) = self
            .orchestrator
            .mark_assignment_recovered(assignment_id, recovered_at.clone())
            .map_err(Self::error)?
        else {
            return Ok(());
        };
        let operation = json!({
            "method":"assignment.recovered",
            "params":{"assignment_id":assignment_id},
            "result":{"assignment":assignment},
            "event_key":format!("assignment-recovered:{assignment_id}:{recovered_at}"),
            "status":"done",
            "at":recovered_at
        });
        self.persist_orchestrator_locked(operation.clone())?;
        for event in self.repair_operation_events(&operation)? {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(())
    }

    pub(crate) async fn update_assignment_usage(
        &self,
        assignment_id: &str,
        usage: &Value,
    ) -> Result<Value, RpcError> {
        let totals: UsageTotals =
            serde_json::from_value(usage.clone()).map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: format!("invalid assignment usage: {error}"),
                details: None,
            })?;
        let assignment = self
            .orchestrator
            .update_assignment_usage(assignment_id, totals)
            .map_err(Self::error)?;
        self.persist_orchestrator(json!({
            "method":"assignment.usage",
            "params":{"assignment_id":assignment_id,"usage":usage},
            "result":{"assignment":assignment},
            "status":"done",
            "at":now()
        }))
        .await
    }

    pub fn open(home: impl AsRef<Path>) -> Result<Self, AdapterError> {
        let durable = DurableRuntime::open(home)?;
        let store = durable.store().clone();
        let usage = UsageLedger::from_store(store.clone())?;
        let orchestrator = Orchestrator::default();
        let operations = read_operation_log(&store)?;
        let operation_snapshot = operations
            .iter()
            .rev()
            .find_map(|op| op.get("snapshot").cloned());
        let snapshot = match operation_snapshot {
            Some(snapshot) => Some(snapshot),
            None => match store.read_snapshot::<Value>("data/orchestrator/state.json") {
                Ok(Some(snapshot)) => Some(snapshot),
                Ok(None) | Err(_) => None,
            },
        };
        let mut terminal_approvals_migrated = false;
        if let Some(mut snapshot) = snapshot {
            let old_approvals = snapshot.get("approvals").cloned();
            migrate_legacy_approval_support(&mut snapshot);
            orchestrator
                .restore(snapshot)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            terminal_approvals_migrated = old_approvals
                != orchestrator
                    .snapshot()
                    .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?
                    .get("approvals")
                    .cloned();
        }
        let deleted_user_messages_migrated =
            restore_deleted_user_chat_tombstones(&store, &orchestrator)?;
        if let Some(mut settings) = store.read_snapshot::<Value>("data/settings.json")? {
            let tool_defaults = ensure_settings_tool_defaults(&mut settings)
                .map_err(AdapterError::OrchestratorSnapshot)?;
            let limits = scheduler_limits(&settings)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            orchestrator
                .configure(limits)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            orchestrator
                .configure_default_bot_tools(tool_defaults)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        }
        Self::migrate_legacy_chat_sequences(&store, &orchestrator)?;
        let mut idempotency = HashMap::new();
        for op in &operations {
            let Some(id) = op.get("client_request_id").and_then(Value::as_str) else {
                continue;
            };
            match op.get("status").and_then(Value::as_str) {
                Some("done") => {
                    if let Some(result) = op.get("result") {
                        idempotency.insert(id.into(), result.clone());
                    }
                }
                Some("rolled_back") => {
                    idempotency.remove(id);
                }
                _ => {}
            }
        }
        let secrets = macbot_providers::configured_secret_store()
            .map_err(RegistryError::from)
            .map_err(AdapterError::Registry)?;
        let registry = ProviderRegistry::from_store(store.clone(), secrets)?;
        let backend = Self {
            orchestrator,
            store,
            durable: Arc::new(Mutex::new(durable)),
            usage: Arc::new(Mutex::new(usage)),
            providers: Arc::new(Mutex::new(registry)),
            write_lock: Arc::new(Mutex::new(())),
            persist_lock: Arc::new(Mutex::new(())),
            event_lock: Arc::new(Mutex::new(())),
            idempotency: Arc::new(Mutex::new(idempotency)),
            startup_repair_events: Arc::new(StdMutex::new(None)),
        };
        if terminal_approvals_migrated || deleted_user_messages_migrated {
            // Constructor-only: persist snapshot migrations before any
            // request or runtime writer can observe restored state.
            backend
                .persist_orchestrator_locked(json!({
                    "method":"startup.snapshot_migration",
                    "params":{
                        "terminal_approvals":terminal_approvals_migrated,
                        "deleted_user_messages":deleted_user_messages_migrated
                    },
                    "status":"done","result":{}
                }))
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        }
        backend.reconcile_durable_decision_waits()?;
        backend.repair_decision_question_messages()?;
        backend
            .repair_pending_proposal_question_messages()
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.message))?;
        *backend
            .startup_repair_events
            .lock()
            .expect("startup event cache lock poisoned") = Some(backend.store.events_since(0)?);
        backend.repair_completed_operation_events(&operations)?;
        backend
            .terminal_approval_events(None)
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        backend
            .startup_repair_events
            .lock()
            .expect("startup event cache lock poisoned")
            .take();
        Ok(backend)
    }

    /// Reconcile only decisions proven to be waiting by a durable checkpoint.
    /// A historical Message alone cannot establish whether it was answered.
    fn reconcile_durable_decision_waits(&self) -> Result<(), AdapterError> {
        let jobs = match fs::read_dir(self.store.root().join("data/jobs")) {
            Ok(jobs) => jobs,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(macbot_store::StoreError::Io(error).into()),
        };
        let mut candidates: HashMap<String, Vec<(Option<String>, String, String)>> = HashMap::new();
        for entry in jobs {
            let entry = entry.map_err(macbot_store::StoreError::Io)?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let Some(job) = self.store.read_snapshot::<macbot_durable::Job>(format!(
                "data/jobs/{}",
                entry.file_name().to_string_lossy()
            ))?
            else {
                continue;
            };
            let checkpoint = &job.checkpoint;
            if checkpoint.get("waiting_reason").and_then(Value::as_str) == Some("decision") {
                tracing::info!(job_id = %job.id, unsafe_replay = job.unsafe_replay, pending_tool = !checkpoint.get("pending_tool").is_none_or(serde_json::Value::is_null), pending_tools_empty = checkpoint.get("pending_tools").is_none_or(|tools| tools.is_null() || tools.as_array().is_some_and(Vec::is_empty)), "checking durable decision recovery");
            }
            if job.unsafe_replay
                || !matches!(
                    job.status,
                    macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
                )
                || checkpoint.get("waiting_reason").and_then(Value::as_str) != Some("decision")
                || !checkpoint.get("pending_tool").is_none_or(Value::is_null)
                || !checkpoint.get("pending_tools").is_none_or(|tools| {
                    tools.is_null() || tools.as_array().is_some_and(Vec::is_empty)
                })
            {
                continue;
            }
            let Some(message_id) = checkpoint.get("waiting_message_id").and_then(Value::as_str)
            else {
                continue;
            };
            let Some(run_id) = checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            if takeover_component(run_id) != run_id {
                continue;
            }
            let Some(request) = self
                .store
                .read_snapshot::<Value>(format!("data/run_requests/{run_id}.json"))?
            else {
                continue;
            };
            tracing::info!(%run_id, %message_id, "decision recovery request loaded");
            if request.get("run_id").and_then(Value::as_str) != Some(run_id) {
                continue;
            }
            let (Some(bot_id), Some(chat_id)) = (
                request.get("bot_id").and_then(Value::as_str),
                request.get("chat_id").and_then(Value::as_str),
            ) else {
                continue;
            };
            let assignment_id = match request.get("assignment_id") {
                Some(Value::String(id)) => Some(id.clone()),
                None | Some(Value::Null) => None,
                _ => continue,
            };
            candidates.entry(message_id.to_owned()).or_default().push((
                assignment_id,
                bot_id.to_owned(),
                chat_id.to_owned(),
            ));
        }
        let mut assignment_claims: HashMap<String, usize> = HashMap::new();
        for claims in candidates.values() {
            for (assignment_id, _, _) in claims {
                if let Some(assignment_id) = assignment_id {
                    *assignment_claims.entry(assignment_id.clone()).or_default() += 1;
                }
            }
        }
        let mut changed = false;
        let mut recovered_assignments = Vec::new();
        for (message_id, candidates) in candidates {
            // Two jobs claiming the same canonical decision are ambiguous.
            if candidates.len() != 1 {
                tracing::warn!(%message_id, "skipping ambiguous decision jobs");
                continue;
            }
            let (assignment_id, bot_id, chat_id) = &candidates[0];
            if assignment_id
                .as_ref()
                .is_some_and(|id| assignment_claims.get(id).copied().unwrap_or(0) != 1)
            {
                tracing::warn!(%message_id, "skipping multiple decision jobs for one assignment");
                continue;
            }
            match self.orchestrator.reconcile_waiting_decision(
                &message_id,
                assignment_id.as_deref(),
                bot_id,
                chat_id,
            ) {
                Ok(reconciled) => {
                    changed |= reconciled;
                    if let Some(assignment_id) = assignment_id {
                        recovered_assignments.push((message_id.clone(), assignment_id.clone()));
                    }
                }
                Err(error) => {
                    tracing::warn!(%message_id, %error, "skipping inconsistent decision checkpoint")
                }
            }
        }
        if changed {
            // Constructor-only: no runtime writer can exist before open returns.
            let _guard = self
                .persist_lock
                .try_lock()
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            self.persist_orchestrator_locked(
                json!({"method":"decision.recovery","status":"done","result":{}}),
            )
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        }
        let snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        for (message_id, assignment_id) in recovered_assignments {
            let Some(assignment) = snapshot
                .get("assignments")
                .and_then(|items| items.get(&assignment_id))
            else {
                continue;
            };
            if assignment
                .pointer("/wait/message_id")
                .and_then(Value::as_str)
                != Some(message_id.as_str())
            {
                continue;
            }
            let mut assignment = assignment.clone();
            normalize_assignment(&mut assignment);
            self.store.append_event_once(
                &format!("decision-recovery:{message_id}:assignment"),
                "assignment.updated",
                json!({"assignment":assignment}),
            )?;
        }
        Ok(())
    }

    /// Older writers exposed option-bearing decisions as plain text. Repair
    /// their canonical rows without creating a new question, message or seq.
    fn repair_decision_question_messages(&self) -> Result<(), AdapterError> {
        let snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        for message in snapshot
            .get("messages")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|items| items.values())
        {
            let Some(question_id) = message.get("question_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(id) = message.get("id").and_then(Value::as_str) else {
                continue;
            };
            let mut wire = message.clone();
            normalize_message(&mut wire);
            let chat_id = message
                .get("chat_id")
                .and_then(Value::as_str)
                .unwrap_or("chat_main");
            let previous = self
                .store
                .read_jsonl::<Value>(format!(
                    "data/chats/{}/messages.jsonl",
                    takeover_component(chat_id)
                ))?
                .into_iter()
                .rev()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(id));
            let canonical = self
                .persist_client_message(&wire)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            if previous.as_ref().and_then(|value| value.get("blocks")) != canonical.get("blocks") {
                self.store.append_event_once(
                    &format!("decision-question:{question_id}:message"),
                    "message.updated",
                    json!({"message":canonical}),
                )?;
            }
            if let Some(question) = snapshot
                .get("questions")
                .and_then(|items| items.get(question_id))
                .filter(|question| question.get("state").and_then(Value::as_str) == Some("pending"))
            {
                self.store.append_event_once(
                    &format!("decision-question:{question_id}:asked"),
                    "question.asked",
                    json!({"question":question}),
                )?;
            }
        }
        Ok(())
    }

    /// Older servers persisted `propose_bot` as a pending Question without
    /// creating the corresponding chat message. Rebuild the message from the
    /// durable proposal and Question records during startup so existing
    /// clients can resolve the question by its normal message block.
    fn repair_pending_proposal_question_messages(&self) -> Result<(), RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let Some(questions) = snapshot.get("questions").and_then(Value::as_object) else {
            return Ok(());
        };
        let mut persist_snapshot = false;
        let mut answered_questions = Vec::new();
        for question in questions.values() {
            let Some(question_id) = question.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(mut proposal) = self.read_proposal(question_id)? else {
                continue;
            };
            if proposal.get("state").and_then(Value::as_str) != Some("pending") {
                continue;
            }
            match question.get("state").and_then(Value::as_str) {
                Some("pending") => {
                    self.ensure_proposal_question_message(question)?;
                }
                Some("answered") => {
                    let answer = question.get("answer").cloned().unwrap_or(Value::Null);
                    let assignment_id = proposal
                        .get("assignment_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| RpcError {
                            code: "internal".into(),
                            message: "pending proposal has no assignment".into(),
                            details: None,
                        })?;
                    if question.get("assignment_id").and_then(Value::as_str)
                        != Some(assignment_id.as_str())
                    {
                        return Err(RpcError {
                            code: "conflict".into(),
                            message: "proposal and Question assignment do not match".into(),
                            details: None,
                        });
                    }
                    let approved = answer_request_approves(&answer);
                    let (terminal_state, assignment_status) = if approved {
                        let creation = snapshot
                            .get("idempotent_bot_creations")
                            .and_then(|items| items.get(format!("proposal:{question_id}")));
                        let bot_id = creation
                            .and_then(|item| item.get("bot_id"))
                            .and_then(Value::as_str);
                        let created_bot = bot_id.and_then(|id| {
                            snapshot.get("bots").and_then(|bots| bots.get(id)).cloned()
                        });
                        if let Some(bot) = created_bot {
                            if let Some(object) = proposal.as_object_mut() {
                                object.insert("state".into(), json!("approved"));
                                object.insert("bot".into(), json!(bot));
                            }
                            ("approved", "done")
                        } else {
                            let requested_name =
                                proposal.pointer("/params/name").and_then(Value::as_str);
                            let name_conflict = requested_name.is_some_and(|name| {
                                snapshot.get("bots").and_then(Value::as_object).is_some_and(
                                    |bots| {
                                        bots.values().any(|bot| {
                                            bot.get("name").and_then(Value::as_str) == Some(name)
                                        })
                                    },
                                )
                            });
                            if let Some(object) = proposal.as_object_mut() {
                                object.insert("state".into(), json!("failed"));
                                object.insert(
                                    "failure_code".into(),
                                    json!(if name_conflict {
                                        "bot_name_conflict"
                                    } else {
                                        "bot_creation_interrupted"
                                    }),
                                );
                            }
                            ("failed", "failed")
                        }
                    } else {
                        if let Some(object) = proposal.as_object_mut() {
                            object.insert("state".into(), json!("rejected"));
                        }
                        ("rejected", "cancelled")
                    };
                    self.write_proposal(question_id, &proposal)?;
                    if let Some(current_status) = snapshot
                        .get("assignments")
                        .and_then(|items| items.get(&assignment_id))
                        .and_then(|assignment| assignment.get("status"))
                        .and_then(Value::as_str)
                    {
                        if !matches!(current_status, "done" | "failed" | "cancelled") {
                            self.orchestrator
                                .finish_assignment(&assignment_id, assignment_status)
                                .map_err(Self::error)?;
                            persist_snapshot = true;
                        }
                    }
                    let has_answer_event = self
                        .store
                        .events_since(0)
                        .map_err(store_error)?
                        .iter()
                        .any(|event| {
                            event.event == "question.answered"
                                && event.data.pointer("/question/id").and_then(Value::as_str)
                                    == Some(question_id)
                        });
                    if !has_answer_event {
                        answered_questions.push((question_id.to_owned(), question.clone()));
                    }
                    tracing::warn!(
                        %question_id,
                        proposal_state = terminal_state,
                        "reconciled answered proposal left pending by an interrupted response"
                    );
                }
                _ => {}
            }
        }
        if persist_snapshot {
            self.persist_orchestrator_locked(json!({
                "method":"proposal.reconcile_answered",
                "params":{},
                "result":{},
                "status":"done",
                "event_key":"proposal:reconcile-answered"
            }))?;
        }
        for (question_id, question) in answered_questions {
            self.append_repaired_event(
                &format!("question-answer:{question_id}"),
                "question.answered",
                json!({"question":question}),
            )?;
        }
        Ok(())
    }

    /// Persist one stable, visible Question message for a `propose_bot`
    /// request. The message id and event receipt are derived from the Question
    /// id, making retries and startup repair idempotent.
    fn ensure_proposal_question_message(
        &self,
        question: &Value,
    ) -> Result<Option<macbot_store::Event>, RpcError> {
        let question_id = question
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "proposal question has no id".into(),
                details: None,
            })?;
        let chat_id = question
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "proposal question has no chat_id".into(),
                details: None,
            })?;
        let bot_id = question
            .get("bot_id")
            .and_then(Value::as_str)
            .unwrap_or("main");
        let text = question
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("需要你确认一项 Bot 创建提议");
        let assignment_id = question
            .get("assignment_id")
            .cloned()
            .unwrap_or(Value::Null);
        let message_id = format!("msg_question_proposal_{}", takeover_component(question_id));
        let mut card = json!({
            "id":message_id,
            "chat_id":chat_id,
            "seq":0,
            "sender":{"kind":"bot","bot_id":bot_id},
            "created_at":now(),
            "edited_at":null,
            "deleted":false,
            "reply_to":null,
            "thread_count":0,
            "mentions":[],
            "blocks":[{"type":"question","question_id":question_id}],
            "fallback_text":text,
            "intent":null,
            "assignment_id":assignment_id,
            "streaming":false,
            "delivery":[],
            "reactions":[]
        });
        if let Some(existing) = self
            .load_chat_messages(chat_id)?
            .into_iter()
            .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id.as_str()))
        {
            card = existing;
            card["sender"] = json!({"kind":"bot","bot_id":bot_id});
            card["blocks"] = json!([{"type":"question","question_id":question_id}]);
            card["fallback_text"] = json!(text);
            card["assignment_id"] = assignment_id;
            card["deleted"] = json!(false);
        }
        let canonical = self.persist_client_message(&card)?;
        let event_name = if self
            .store
            .events_since(0)
            .map_err(store_error)?
            .iter()
            .any(|event| {
                matches!(event.event.as_str(), "message.created" | "message.updated")
                    && event.data["message"]["id"] == message_id
            }) {
            "message.updated"
        } else {
            "message.created"
        };
        self.append_repaired_event(
            &format!("proposal-question:{question_id}:message"),
            event_name,
            json!({"message":canonical}),
        )
    }

    fn migrate_legacy_chat_sequences(
        store: &Store,
        orchestrator: &Orchestrator,
    ) -> Result<(), AdapterError> {
        let mut chats: HashMap<String, Vec<Value>> = HashMap::new();
        if let Some(messages) = orchestrator
            .snapshot()
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?
            .get("messages")
            .and_then(Value::as_object)
        {
            for message in messages.values() {
                if let Some(chat_id) = message.get("chat_id").and_then(Value::as_str) {
                    chats
                        .entry(chat_id.to_owned())
                        .or_default()
                        .push(message.clone());
                }
            }
        }
        let dir = store.root().join("data/chats");
        let entries = match fs::read_dir(dir) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(AdapterError::Store(error.into())),
        };
        if let Some(entries) = entries {
            for entry in entries {
                let entry = entry.map_err(|error| AdapterError::Store(error.into()))?;
                let path = entry.path().join("messages.jsonl");
                let file = match fs::File::open(&path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(AdapterError::Store(error.into())),
                };
                let messages = read_jsonl_values_lossy(file);
                for message in messages {
                    if let Some(chat_id) = message.get("chat_id").and_then(Value::as_str) {
                        chats.entry(chat_id.to_owned()).or_default().push(message);
                    }
                }
            }
        }
        for (chat_id, messages) in chats {
            if store.last_chat_sequence(&chat_id)? != 0 {
                continue;
            }
            let mut latest = HashMap::new();
            for message in messages {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    latest.insert(id.to_owned(), message);
                }
            }
            let mut messages = latest.into_values().collect::<Vec<_>>();
            messages.sort_by(|left, right| {
                left.get("created_at")
                    .and_then(Value::as_str)
                    .cmp(&right.get("created_at").and_then(Value::as_str))
                    .then_with(|| {
                        left.get("seq")
                            .and_then(Value::as_u64)
                            .cmp(&right.get("seq").and_then(Value::as_u64))
                    })
                    .then_with(|| {
                        left.get("id")
                            .and_then(Value::as_str)
                            .cmp(&right.get("id").and_then(Value::as_str))
                    })
            });
            let canonical = store.sequence_chat_messages(&chat_id, &messages)?;
            for message in canonical {
                store.append_jsonl(
                    format!("data/chats/{}/messages.jsonl", takeover_component(&chat_id)),
                    &message,
                )?;
            }
        }
        Ok(())
    }

    fn error(error: macbot_orchestrator::OrchestratorError) -> RpcError {
        let text = error.to_string();
        let code = if text.starts_with("not found") {
            "not_found"
        } else if text.starts_with("forbidden") {
            "forbidden"
        } else if text.starts_with("conflict") {
            "conflict"
        } else {
            "invalid_params"
        };
        RpcError {
            code: code.into(),
            message: text,
            details: None,
        }
    }

    fn is_mutation(method: &str) -> bool {
        !matches!(
            method,
            "bootstrap"
                | "chat.list"
                | "chat.get"
                | "chat.history"
                | "chat.thread"
                | "artifact.versions.list"
                | "artifact.version.get"
                | "settings.get"
                | "bot.list"
                | "bot.get"
                | "bot.templates"
                | "project.list"
                | "project.get"
                | "project.status"
                | "project_status"
                | "assignment.list"
                | "assignment.get"
                | "trace.history"
                | "approval.list"
                | "question.list"
                | "routine.list"
                | "routine.runs"
                | "workbench.get"
                | "usage.summary"
                | "usage.heatmap"
                | "usage.timeseries"
                | "usage.breakdown"
        )
    }

    /// Internal execution bridge for a model-side takeover request.  Runtime
    /// sinks call this after committing the durable `run.wait` checkpoint;
    /// keeping the call here ensures the Bot question, takeover snapshot,
    /// operation log, and orchestrator snapshot use the same writer lock as
    /// ordinary RPC mutations.  This is deliberately a Rust API rather than
    /// a client-visible protocol method.
    pub async fn persist_execution_takeover(
        &self,
        state: &GatewayState,
        params: Value,
    ) -> RpcResult {
        self.execution_takeover_request(state, params).await
    }

    /// Internal execution bridge for model questions.  The execution sink
    /// supplies the real assignment/chat scope and receives the durable
    /// question id for its client-facing card.
    pub async fn persist_execution_question(
        &self,
        state: &GatewayState,
        params: Value,
    ) -> RpcResult {
        self.call("question.ask", params, state).await
    }

    /// Retire an invalid model request without granting permission or stopping
    /// its assignment. The receipt makes expiry-before-continuation restartable.
    pub async fn expire_invalid_tool_approval(
        &self,
        state: &GatewayState,
        approval_id: &str,
        receipt: &Value,
    ) -> Result<bool, RpcError> {
        let _guard = self.write_lock.lock().await;
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let Some(approval) = snapshot
            .get("approvals")
            .and_then(|items| items.get(approval_id))
        else {
            return Ok(false);
        };
        if approval.get("tool") != receipt.get("tool")
            || approval.get("bot_id") != receipt.get("bot_id")
            || approval.get("chat_id") != receipt.get("chat_id")
            || approval.get("assignment_id") != receipt.get("assignment_id")
            || approval
                .get("detail")
                .and_then(Value::as_str)
                .and_then(|detail| serde_json::from_str::<Value>(detail).ok())
                .as_ref()
                != receipt.get("args")
            || crate::execution::invalid_pending_tool_args(
                receipt["tool"].as_str().unwrap_or_default(),
                &receipt["args"],
                None,
            )
            .or_else(|| {
                crate::execution::invalid_memory_target_args(
                    receipt["tool"].as_str().unwrap_or_default(),
                    &receipt["args"],
                    &snapshot,
                )
            })
            .is_none()
        {
            return Ok(false);
        }
        let Some(run_id) = receipt
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|id| takeover_component(id) == *id)
        else {
            return Ok(false);
        };
        let Some(request) = self
            .store
            .read_snapshot::<crate::execution::ExecutionRequest>(format!(
                "data/run_requests/{run_id}.json"
            ))
            .map_err(store_error)?
        else {
            return Ok(false);
        };
        if request.run_id != run_id
            || !crate::backend::invalid_tool_approval_request_matches(&snapshot, approval, &request)
        {
            return Ok(false);
        }
        let Ok(entries) = std::fs::read_dir(self.store.root().join("data/jobs")) else {
            return Ok(false);
        };
        let matching_jobs = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                std::fs::File::open(entry.path())
                    .ok()
                    .and_then(|file| serde_json::from_reader::<_, macbot_durable::Job>(file).ok())
            })
            .filter(|job| {
                matches!(
                    job.status,
                    macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
                ) && job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id)
                    && job.checkpoint.pointer("/pending_tool/call_id") == receipt.get("call_id")
                    && job.checkpoint.pointer("/pending_tool/name") == receipt.get("tool")
                    && job.checkpoint.pointer("/pending_tool/args") == receipt.get("args")
                    && job
                        .checkpoint
                        .get("pending_tools")
                        .and_then(Value::as_array)
                        .is_none_or(|pending| {
                            pending.is_empty()
                                || pending.first() == job.checkpoint.get("pending_tool")
                        })
            })
            .count();
        if matching_jobs != 1 {
            return Ok(false);
        }
        let path = format!(
            "data/invalid-tool-recovery/{}.json",
            takeover_component(approval_id)
        );
        match approval.get("state").and_then(Value::as_str) {
            Some("pending") => {
                self.store
                    .write_snapshot(&path, receipt)
                    .map_err(store_error)?;
                if self
                    .orchestrator
                    .expire_invalid_approval(approval_id)
                    .map_err(Self::error)?
                    .is_none()
                {
                    return Ok(false);
                }
            }
            Some("expired") => {
                if self
                    .store
                    .read_snapshot::<Value>(&path)
                    .map_err(store_error)?
                    .as_ref()
                    != Some(receipt)
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        let current = self.orchestrator.snapshot().map_err(Self::error)?;
        self.persist(
            state,
            "approval.invalid",
            &json!({
                "approval_id":approval_id,
                "run_id":receipt["run_id"],
                "call_id":receipt["call_id"],
                "reason":"invalid tool arguments",
                "client_request_id":format!("invalid-tool:{approval_id}")
            }),
            &json!({"approval":current["approvals"][approval_id]}),
        )
        .await?;
        Ok(true)
    }

    /// Internal runtime action.  The returned snapshot is intentionally kept
    /// out of the public RPC response; the composed runtime backend uses it
    /// only to resume the waiting run after the user releases the browser.
    pub async fn execution_takeover_start(
        &self,
        state: &GatewayState,
        params: &Value,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        if let Some(request_id) = params.get("client_request_id").and_then(Value::as_str) {
            if self.idempotency.lock().await.contains_key(request_id) {
                // The operation is already durably committed. A retry must
                // not select another scope or emit another continuation;
                // the public protocol result is still the empty object.
                return Ok(json!({}));
            }
        }
        let result = self.takeover_start(state, params).await?;
        self.persist(state, "takeover.start", params, &json!({}))
            .await?;
        Ok(result)
    }

    /// Reconcile browser ownership while holding the same writer lock used by
    /// takeover.start/release. The durable request records are authoritative;
    /// the browser session file only carries tab metadata across restarts.
    pub(crate) async fn reconcile_browser_takeover(
        &self,
        state: &GatewayState,
        bot_id: &str,
    ) -> Result<(), RpcError> {
        let _guard = self.write_lock.lock().await;
        self.apply_browser_takeover_reconciliation(state, bot_id)
            .await
    }

    /// Apply the durable ownership decision while the caller already owns the
    /// writer lock.  Keep this separate from `reconcile_browser_takeover` so a
    /// start/release mutation can refresh the live browser state without
    /// attempting to acquire the writer lock recursively.
    async fn apply_browser_takeover_reconciliation(
        &self,
        state: &GatewayState,
        bot_id: &str,
    ) -> Result<(), RpcError> {
        let active = self.durable_browser_takeover_active(bot_id)?;
        state
            .browser
            .lock()
            .await
            .set_takeover_override(bot_id, active)
            .map_err(browser_error)
    }

    fn durable_browser_takeover_active(&self, bot_id: &str) -> Result<bool, RpcError> {
        let directory = self.store.root().join("data/takeovers");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(RpcError {
                    code: "internal".into(),
                    message: format!("read durable takeover records: {error}"),
                    details: None,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| RpcError {
                code: "internal".into(),
                message: format!("read durable takeover entry: {error}"),
                details: None,
            })?;
            if !entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| RpcError {
                code: "internal".into(),
                message: format!("read durable takeover record: {error}"),
                details: None,
            })?;
            let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if record.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                && record.get("state").and_then(Value::as_str) == Some("active")
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Project the durable takeover lifecycle onto the original group card
    /// and its private question card.  The JSONL row and its corresponding
    /// event are committed under the same writer lock so two concurrent
    /// release/start transitions cannot overwrite one another.  The keyed
    /// repair event also makes a successful row append recoverable if the
    /// process dies before the event append; startup message repair will
    /// publish the missing event from the canonical JSONL row.
    pub(crate) async fn project_takeover_message_state(
        &self,
        state: &GatewayState,
        request: &Value,
    ) -> Result<(), RpcError> {
        let Some(message_id) = request
            .get("message_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        else {
            // Manual browser takeover has no model-originated card to
            // project. Keep that legacy action path a no-op.
            return Ok(());
        };
        let run_id = required_text(request, "run_id")?;
        let Some(group_chat_id) = request
            .get("group_chat_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        else {
            return Ok(());
        };
        let _assignment_id = required_text(request, "assignment_id")?;
        let bot_id = required_text(request, "bot_id")?;
        let request_state = required_text(request, "state")?;
        if takeover_component(run_id.as_str()) != run_id
            || message_id != format!("msg_takeover_{}", run_id)
            || takeover_component(message_id.as_str()) != message_id
        {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "takeover request message_id/run_id mismatch".into(),
                details: None,
            });
        }
        let mut targets = vec![(group_chat_id.clone(), message_id.clone())];
        if let Some(dm_chat_id) = request
            .get("chat_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let dm_message_id = format!(
                "msg_takeover_question_{}",
                takeover_component(message_id.as_str())
            );
            targets.push((dm_chat_id.to_owned(), dm_message_id));
        }

        let _guard = self.write_lock.lock().await;
        let mut events = Vec::new();
        for (chat_id, target_message_id) in targets {
            let target_result = (|| -> Result<Option<macbot_store::Event>, RpcError> {
                let path = format!("data/chats/{}/messages.jsonl", takeover_component(&chat_id));
                let rows = self.store.read_jsonl::<Value>(&path).map_err(store_error)?;
                let Some(mut message) = rows.into_iter().rev().find(|message| {
                    message.get("id").and_then(Value::as_str) == Some(target_message_id.as_str())
                }) else {
                    return Ok(None);
                };
                if !crate::backend::takeover_message_scope_matches(
                    &message,
                    request,
                    &chat_id,
                    &target_message_id,
                ) {
                    return Ok(None);
                }
                let changed = crate::backend::transition_takeover_message(
                    &mut message,
                    request,
                    &chat_id,
                    &target_message_id,
                );
                if !changed
                    && message
                        .pointer("/blocks")
                        .and_then(Value::as_array)
                        .and_then(|blocks| {
                            blocks.iter().find(|block| {
                                block.get("type").and_then(Value::as_str)
                                    == Some("takeover_request")
                                    && block.get("bot_id").and_then(Value::as_str)
                                        == Some(bot_id.as_str())
                            })
                        })
                        .and_then(|block| block.get("state"))
                        .and_then(Value::as_str)
                        != Some(request_state.as_str())
                {
                    // A row with the same id but a different scope is
                    // unrelated historical data. Leave it untouched and
                    // avoid broadening an old request into a new card.
                    return Ok(None);
                }
                let canonical = if changed {
                    self.store
                        .sequence_chat_messages(&chat_id, &[message])
                        .map_err(store_error)?
                        .into_iter()
                        .next()
                        .ok_or_else(|| RpcError {
                            code: "internal".into(),
                            message: "chat message sequencing returned no message".into(),
                            details: None,
                        })?
                } else {
                    message
                };
                if changed {
                    self.store
                        .append_jsonl(&path, &canonical)
                        .map_err(store_error)?;
                }
                let data = json!({"message": canonical});
                let key = format!(
                    "takeover-message:{}:{}:{}",
                    target_message_id,
                    request_state,
                    serde_json::to_string(&data).map_err(|error| RpcError {
                        code: "internal".into(),
                        message: error.to_string(),
                        details: None,
                    })?
                );
                self.append_repaired_event(&key, "message.updated", data)
            })();
            match target_result {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(error) => {
                    drop(_guard);
                    for event in events {
                        state
                            .publish_event(event.seq, &event.event, event.data)
                            .await;
                    }
                    return Err(error);
                }
            }
        }
        drop(_guard);
        for event in events {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(())
    }

    fn repair_completed_operation_events(&self, operations: &[Value]) -> Result<(), AdapterError> {
        // Completed-operation repair only reads the restored state. Keep one
        // snapshot for the pass and refresh it after the sole repair path
        // that may add a task-stopped message to the orchestrator.
        let mut snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        for operation in operations {
            if operation.get("status").and_then(Value::as_str) != Some("done") {
                continue;
            }
            self.repair_operation_events_with_snapshot(operation, &snapshot)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.message))?;
            if operation.get("method").and_then(Value::as_str) == Some("approval.decide")
                && operation
                    .get("result")
                    .and_then(|result| result.get("approval"))
                    .and_then(|approval| approval.get("state"))
                    .and_then(Value::as_str)
                    == Some("denied")
            {
                snapshot = self
                    .orchestrator
                    .snapshot()
                    .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
            }
        }
        self.repair_persisted_message_events()
            .map_err(|error| AdapterError::OrchestratorSnapshot(error.message))?;
        Ok(())
    }

    fn repair_persisted_message_events(&self) -> Result<(), RpcError> {
        let root = self.store.root().join("data/chats");
        let mut persisted_message_ids = HashSet::new();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                });
            }
        };
        // Reuse the startup event cache when the constructor has already
        // loaded it for operation repair. Direct callers retain the same
        // one-load behavior as before.
        let (mut events, restore_startup_cache) = {
            let mut cache = self
                .startup_repair_events
                .lock()
                .expect("startup event cache lock poisoned");
            match cache.take() {
                Some(events) => (events, true),
                None => (self.store.events_since(0).map_err(store_error)?, false),
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            })?;
            if !entry
                .file_type()
                .map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?
                .is_dir()
            {
                continue;
            }
            let path = entry.path().join("messages.jsonl");
            let relative = path.strip_prefix(self.store.root()).unwrap_or(&path);
            let rows = self
                .store
                .read_jsonl::<Value>(relative)
                .map_err(store_error)?;
            let mut latest = HashMap::new();
            for mut row in rows {
                normalize_message(&mut row);
                if let Some(id) = row.get("id").and_then(Value::as_str) {
                    latest.insert(id.to_owned(), row);
                }
            }
            for mut message in latest.into_values() {
                let Some(id) = message.get("id").and_then(Value::as_str).map(str::to_owned) else {
                    continue;
                };
                if self.repair_takeover_approval_ref(&mut message)? {
                    let chat_id = message
                        .get("chat_id")
                        .and_then(Value::as_str)
                        .unwrap_or("chat_main")
                        .to_owned();
                    let canonical = self
                        .store
                        .sequence_chat_messages(&chat_id, &[message])
                        .map_err(store_error)?
                        .into_iter()
                        .next()
                        .ok_or_else(|| RpcError {
                            code: "internal".into(),
                            message: "chat message sequencing returned no message".into(),
                            details: None,
                        })?;
                    self.store
                        .append_jsonl(relative, &canonical)
                        .map_err(store_error)?;
                    message = canonical;
                }
                persisted_message_ids.insert(id.clone());
                let data = json!({"message":message});
                if events.iter().any(|event| {
                    matches!(event.event.as_str(), "message.created" | "message.updated")
                        && event.data == data
                }) {
                    continue;
                }
                let event_name = if events.iter().any(|event| {
                    matches!(event.event.as_str(), "message.created" | "message.updated")
                        && event.data["message"]["id"] == id
                }) {
                    "message.updated"
                } else {
                    "message.created"
                };
                let key = format!(
                    "repair:message:{id}:{}",
                    serde_json::to_string(&data).map_err(|error| RpcError {
                        code: "internal".into(),
                        message: error.to_string(),
                        details: None,
                    })?
                );
                if let Some(event) =
                    self.append_repaired_event_with_events(&key, event_name, data, &events)?
                {
                    events.push(event);
                }
            }
        }
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        if let Some(assignments) = snapshot.get("assignments").and_then(Value::as_object) {
            for assignment in assignments.values() {
                let Some(trigger_id) = assignment.get("trigger_message_id").and_then(Value::as_str)
                else {
                    continue;
                };
                if !persisted_message_ids.contains(trigger_id) {
                    continue;
                }
                let Some(id) = assignment.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let mut assignment = assignment.clone();
                normalize_assignment(&mut assignment);
                let data = json!({"assignment":assignment});
                let key = format!(
                    "repair:assignment:{id}:{}",
                    serde_json::to_string(&data).map_err(|error| RpcError {
                        code: "internal".into(),
                        message: error.to_string(),
                        details: None,
                    })?
                );
                if let Some(event) = self.append_repaired_event_with_events(
                    &key,
                    "assignment.created",
                    data,
                    &events,
                )? {
                    events.push(event);
                }
            }
        }
        if restore_startup_cache {
            self.startup_repair_events
                .lock()
                .expect("startup event cache lock poisoned")
                .replace(events);
        }
        Ok(())
    }

    /// Remove only the legacy takeover projection that mislabeled its private
    /// question id as a tool approval. The durable takeover record is the
    /// authority; unknown approval blocks remain untouched.
    fn repair_takeover_approval_ref(&self, message: &mut Value) -> Result<bool, RpcError> {
        let (Some(message_id), Some(chat_id)) = (
            message.get("id").and_then(Value::as_str),
            message.get("chat_id").and_then(Value::as_str),
        ) else {
            return Ok(false);
        };
        if !message_id.starts_with("msg_takeover_")
            || !message
                .get("blocks")
                .and_then(Value::as_array)
                .is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block.get("type").and_then(Value::as_str) == Some("approval_ref")
                    })
                })
        {
            return Ok(false);
        }
        let takeover_dir = self.store.root().join("data/takeovers");
        let entries = match fs::read_dir(takeover_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(store_error(error.into())),
        };
        let mut question_id = None;
        for entry in entries {
            let entry = entry.map_err(|error| store_error(error.into()))?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(self.store.root())
                .unwrap_or(&entry_path)
                .to_owned();
            let Some(request) = self
                .store
                .read_snapshot::<Value>(relative)
                .map_err(store_error)?
            else {
                continue;
            };
            let Some(run_id) = request.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            if request.get("message_id").and_then(Value::as_str) != Some(message_id)
                || request.get("group_chat_id").and_then(Value::as_str) != Some(chat_id)
                || takeover_component(run_id) != run_id
                || message_id != format!("msg_takeover_{run_id}")
            {
                continue;
            }
            let Some(id) = request.get("question_id").and_then(Value::as_str) else {
                continue;
            };
            question_id = Some(id.to_owned());
            break;
        }
        let Some(question_id) = question_id else {
            return Ok(false);
        };
        let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) else {
            return Ok(false);
        };
        let before = blocks.len();
        blocks.retain(|block| {
            !(block.get("type").and_then(Value::as_str) == Some("approval_ref")
                && block.get("approval_id").and_then(Value::as_str) == Some(question_id.as_str()))
        });
        Ok(blocks.len() != before)
    }

    fn repair_card_event(
        &self,
        key: &str,
        mut card: Value,
    ) -> Result<Option<macbot_store::Event>, RpcError> {
        let message_id = card
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "derived card has no id".into(),
                details: None,
            })?
            .to_owned();
        let target_chat_id = card
            .get("chat_id")
            .and_then(Value::as_str)
            .unwrap_or("chat_main");
        let existing = self
            .load_chat_messages(target_chat_id)?
            .into_iter()
            .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id.as_str()));
        let had_existing = existing.is_some();
        if let Some(existing) = existing {
            // A persisted message is the canonical card.  A retried older
            // operation must never move a confirmed/changed card backwards.
            card = existing;
        }
        normalize_message(&mut card);
        let canonical = self.persist_client_message(&card)?;
        let event_name = if had_existing {
            "message.updated"
        } else {
            "message.created"
        };
        self.append_repaired_event(key, event_name, json!({"message":canonical}))
    }

    fn repair_operation_cards(
        &self,
        method: &str,
        base_key: &str,
        params: &Value,
        canonical: &Value,
        snapshot: &Value,
    ) -> Result<Vec<macbot_store::Event>, RpcError> {
        let mut events = Vec::new();
        if method == "project.create" {
            if let Some(project) = canonical.get("project").filter(|value| value.is_object()) {
                let project_id = project.get("id").and_then(Value::as_str).unwrap_or("");
                let card = json!({
                    "id":format!("msg_project_card_{project_id}"),
                    "chat_id":"chat_main",
                    "seq":0,
                    "sender":{"kind":"bot","bot_id":"main"},
                    "created_at":project.get("created_at").cloned().unwrap_or_else(|| json!(now())),
                    "edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
                    "mentions":[],"blocks":[{"type":"project_card","project_id":project_id,"context":"created","note":null}],
                    "fallback_text":format!("项目「{}」已创建", project.get("name").and_then(Value::as_str).unwrap_or(project_id)),
                    "intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[]
                });
                if let Some(event) =
                    self.repair_card_event(&format!("{base_key}:project-card"), card)?
                {
                    events.push(event);
                }
            }
        }
        if matches!(
            method,
            "assignment.create" | "assign" | "delegate" | "assignment.retry"
        ) {
            let assignment = if canonical.get("assignment").is_some_and(Value::is_object) {
                canonical.get("assignment")
            } else if canonical.is_object() {
                Some(canonical)
            } else {
                None
            };
            if let Some(assignment) = assignment {
                let id = assignment.get("id").and_then(Value::as_str).unwrap_or("");
                let chat_id = self
                    .orchestrator
                    .resolve_assignment_return_chat(id)
                    .map_err(Self::error)?;
                if !chat_id.starts_with("dm_") {
                    let delegation = method == "delegate"
                        && assignment.get("project_id").is_none_or(Value::is_null);
                    let card = json!({
                        "id":if delegation { format!("msg_delegation_{id}") } else { format!("msg_task_card_{id}") },
                        "chat_id":chat_id,"seq":0,"sender":{"kind":"system"},
                        "created_at":assignment.get("created_at").cloned().unwrap_or_else(|| json!(now())),
                        "edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
                        "mentions":[],
                        "blocks":if delegation {
                            json!([{"type":"delegation","bot_id":assignment.get("bot_id").and_then(Value::as_str).unwrap_or(""),"assignment_id":id}])
                        } else {
                            json!([{"type":"task_card","assignment_id":id}])
                        },
                        "fallback_text":if delegation {
                            format!("已委派给 {}：{}", assignment.get("bot_id").and_then(Value::as_str).unwrap_or("Bot"), assignment.get("title").and_then(Value::as_str).unwrap_or("待处理任务"))
                        } else {
                            format!("任务：{}", assignment.get("title").and_then(Value::as_str).unwrap_or("待处理任务"))
                        },
                        "intent":null,"assignment_id":id,"streaming":false,"delivery":[],"reactions":[]
                    });
                    if let Some(event) =
                        self.repair_card_event(&format!("{base_key}:assignment-card"), card)?
                    {
                        events.push(event);
                    }
                }
            }
        }
        if method == "send_msg"
            && matches!(
                canonical.get("intent").and_then(Value::as_str),
                Some("done" | "blocked")
            )
        {
            let assignment_id = canonical
                .get("assignment_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if snapshot
                .get("assignments")
                .and_then(|items| items.get(assignment_id))
                .is_some()
            {
                let return_chat_id = self
                    .orchestrator
                    .resolve_assignment_return_chat(assignment_id)
                    .ok();
                let execution_chat_id = canonical.get("chat_id").and_then(Value::as_str);
                if let Some(return_chat_id) =
                    return_chat_id.filter(|chat_id| Some(chat_id.as_str()) != execution_chat_id)
                {
                    let message_id = canonical.get("id").and_then(Value::as_str).unwrap_or("");
                    let summary = canonical
                        .get("text")
                        .and_then(Value::as_str)
                        .or_else(|| canonical.get("fallback_text").and_then(Value::as_str))
                        .unwrap_or_default();
                    let is_done = canonical["intent"] == "done";
                    let result_artifacts = if is_done {
                        assignment_result_artifacts(
                            snapshot,
                            canonical,
                            assignment_id,
                            &return_chat_id,
                        )
                    } else {
                        Vec::new()
                    };
                    let result_next = if is_done {
                        assignment_result_next(snapshot, canonical, summary, &return_chat_id)
                    } else {
                        Vec::new()
                    };
                    let card = json!({
                        "id":format!("msg_assignment_result_{assignment_id}_{message_id}"),
                        "chat_id":return_chat_id,
                        "seq":0,
                        "sender":{"kind":"system"},
                        "created_at":canonical.get("created_at").cloned().unwrap_or_else(|| json!(now())),
                        "edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
                        "mentions":[],
                        "blocks":if is_done {
                            json!([{"type":"completion","summary":summary,"artifacts":result_artifacts,"next":result_next,"notify_main":true}])
                        } else {
                            json!([{"type":"blocked","reason":summary}])
                        },
                        "fallback_text":if is_done { summary.to_owned() } else { format!("任务受阻：{summary}") },
                        "intent":null,
                        "assignment_id":assignment_id,
                        "streaming":false,"delivery":[],"reactions":[]
                    });
                    if let Some(event) = self.repair_card_event(
                        &format!("{base_key}:assignment-result:{message_id}"),
                        card,
                    )? {
                        events.push(event);
                    }
                }
            }
        }
        if matches!(
            method,
            "project.request_review" | "project.request_changes" | "project.confirm_done"
        ) {
            let project_id = params
                .get("project_id")
                .and_then(Value::as_str)
                .or_else(|| canonical.get("project").and_then(|p| p["id"].as_str()));
            let Some(project_id) = project_id else {
                return Ok(events);
            };
            if !canonical.get("project").is_some_and(Value::is_object) {
                return Ok(events);
            }
            let state = match method {
                "project.request_review" => "pending",
                "project.request_changes" => "changes_requested",
                _ => "confirmed",
            };
            let fallback = params
                .get("summary")
                .or_else(|| params.get("text"))
                .and_then(Value::as_str)
                .unwrap_or(if state == "confirmed" {
                    "项目已确认完成"
                } else {
                    "项目待审阅"
                });
            let card = json!({
                "id":format!("msg_project_review_{project_id}"),
                "chat_id":"chat_main","seq":0,
                "sender":{"kind":"bot","bot_id":"main"},
                "created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,
                "thread_count":0,"mentions":[{"kind":"user"}],
                "blocks":[{"type":"review_card","project_id":project_id,"artifacts":[],"state":state}],
                "fallback_text":fallback,"intent":null,"assignment_id":null,
                "streaming":false,"delivery":[],"reactions":[]
            });
            if let Some(event) = self.repair_card_event(&format!("{base_key}:review-card"), card)? {
                events.push(event);
            }
            if method == "project.confirm_done" {
                let project = canonical.get("project").expect("checked above");
                let name = canonical
                    .get("project")
                    .and_then(|project| project.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or(project_id);
                let completion_chat_id = project
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .unwrap_or("chat_main");
                let artifacts = snapshot
                    .get("artifacts")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|items| items.values())
                    .filter(|artifact| {
                        artifact.get("project_id").and_then(Value::as_str) == Some(project_id)
                    })
                    .filter_map(|artifact| {
                        Some(json!({
                            "artifact_id":artifact.get("id")?.clone(),
                            "title":artifact.get("title")?.clone(),
                            "path_or_url":artifact.get("path_or_url")?.clone(),
                            "bot_id":artifact.get("bot_id")?.clone(),
                            "kind":artifact.get("kind")?.clone()
                        }))
                    })
                    .collect::<Vec<_>>();
                let card = json!({
                    "id":format!("msg_project_completion_{project_id}"),
                    "chat_id":completion_chat_id,"seq":0,
                    "sender":{"kind":"bot","bot_id":"main"},
                    "created_at":now(),"edited_at":null,"deleted":false,"reply_to":null,
                    "thread_count":0,"mentions":[{"kind":"user"}],
                    "blocks":[{"type":"completion","summary":format!("项目「{name}」已完成"),"artifacts":artifacts,"next":[],"notify_main":true}],
                    "fallback_text":format!("项目「{name}」已完成"),"intent":null,
                    "assignment_id":null,"streaming":false,"delivery":[],"reactions":[]
                });
                if let Some(event) =
                    self.repair_card_event(&format!("{base_key}:completion-card"), card)?
                {
                    events.push(event);
                }
            }
        }
        Ok(events)
    }

    fn append_repaired_event(
        &self,
        key: &str,
        event_name: &str,
        data: Value,
    ) -> Result<Option<macbot_store::Event>, RpcError> {
        // New keyed events can be deduplicated from the in-memory receipt
        // index. Older releases wrote unkeyed events, so only a fingerprint
        // hit takes the expensive exact event-log compatibility path.
        if self.store.has_event_key(key) {
            return Ok(None);
        }
        if self.store.event_payload_might_contain(event_name, &data) {
            let found = {
                let cache = self
                    .startup_repair_events
                    .lock()
                    .expect("startup event cache lock poisoned");
                match cache.as_ref() {
                    Some(events) => events
                        .iter()
                        .any(|event| event.event == event_name && event.data == data),
                    None => self
                        .store
                        .events_since(0)
                        .map_err(store_error)?
                        .iter()
                        .any(|event| event.event == event_name && event.data == data),
                }
            };
            if found {
                return Ok(None);
            }
        }
        let event = self
            .store
            .append_event_once(key, event_name, data)
            .map_err(store_error)?;
        if let Some(event) = event.as_ref() {
            let mut cache = self
                .startup_repair_events
                .lock()
                .expect("startup event cache lock poisoned");
            if let Some(events) = cache.as_mut() {
                events.push(event.clone());
            }
        }
        Ok(event)
    }

    fn append_repaired_event_with_events(
        &self,
        key: &str,
        event_name: &str,
        data: Value,
        events: &[macbot_store::Event],
    ) -> Result<Option<macbot_store::Event>, RpcError> {
        // New keyed events can be deduplicated from the in-memory receipt
        // index. Older releases wrote unkeyed events, so only a fingerprint
        // hit takes the expensive exact event-log compatibility path.
        if self.store.has_event_key(key) {
            return Ok(None);
        }
        if self.store.event_payload_might_contain(event_name, &data)
            && events
                .iter()
                .any(|event| event.event == event_name && event.data == data)
        {
            return Ok(None);
        }
        self.store
            .append_event_once(key, event_name, data)
            .map_err(store_error)
    }

    fn remember_startup_repair_event(&self, event: &macbot_store::Event) {
        let mut cache = self
            .startup_repair_events
            .lock()
            .expect("startup event cache lock poisoned");
        if let Some(events) = cache.as_mut() {
            events.push(event.clone());
        }
    }

    fn canonical_operation_result(
        &self,
        method: &str,
        params: &Value,
        result: &Value,
        snapshot: &Value,
    ) -> Result<Value, RpcError> {
        let mut canonical = result.clone();
        let current = |collection: &str, id: Option<&str>| {
            id.and_then(|id| {
                snapshot
                    .get(collection)
                    .and_then(Value::as_object)
                    .and_then(|items| items.get(id))
                    .cloned()
            })
        };
        match method {
            "bot.create" | "bot.update" | "bot.duplicate" => {
                let id = result
                    .get("bot")
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| params.get("bot_id").and_then(Value::as_str));
                if let Some(mut bot) = current("bots", id) {
                    normalize_bot(&mut bot);
                    canonical["bot"] = bot;
                } else {
                    canonical["bot"] = Value::Null;
                }
            }
            "bot.create_from_template" => {
                if let Some(items) = canonical.get_mut("bots").and_then(Value::as_array_mut) {
                    let mut retained = Vec::with_capacity(items.len());
                    for item in items.iter() {
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            if let Some(mut bot) = current("bots", Some(id)) {
                                normalize_bot(&mut bot);
                                retained.push(bot);
                            }
                        }
                    }
                    *items = retained;
                }
                let known_chat_ids = canonical
                    .get("bots")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|bot| {
                        bot.get("dm_chat_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .collect::<HashSet<_>>();
                if let Some(chats) = canonical.get_mut("dm_chats").and_then(Value::as_array_mut) {
                    chats.retain(|chat| {
                        chat.get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| known_chat_ids.contains(id))
                    });
                }
            }
            "project.create"
            | "project.update"
            | "project.add_member"
            | "project.remove_member"
            | "project.confirm_done"
            | "project.request_review"
            | "project.archive"
            | "project.reopen"
            | "project.request_changes" => {
                let id = result
                    .get("project")
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| params.get("project_id").and_then(Value::as_str));
                if let Some(mut project) = current("projects", id) {
                    normalize_project(&mut project);
                    canonical["project"] = project;
                } else {
                    canonical["project"] = Value::Null;
                }
                if method == "project.request_changes" {
                    if let Some(id) = result
                        .get("message")
                        .and_then(|value| value.get("id"))
                        .and_then(Value::as_str)
                    {
                        if let Ok((_, mut message)) = self.find_chat_message(id) {
                            normalize_message(&mut message);
                            canonical["message"] = message;
                        }
                    }
                }
            }
            "assignment.create"
            | "assign"
            | "delegate"
            | "assignment.stop"
            | "assignment.recovered"
            | "assignment.retry"
            | "assignment.steer"
            | "steer" => {
                let id = result
                    .get("assignment")
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| result.get("id").and_then(Value::as_str))
                    .or_else(|| params.get("assignment_id").and_then(Value::as_str));
                if let Some(mut assignment) = current("assignments", id) {
                    normalize_assignment(&mut assignment);
                    if result.get("assignment").is_some() {
                        canonical["assignment"] = assignment;
                    } else {
                        canonical = assignment;
                    }
                } else if result.get("assignment").is_some() {
                    canonical["assignment"] = Value::Null;
                } else {
                    canonical = Value::Null;
                }
            }
            "message.steer" => {
                if let Some(id) = result
                    .get("assignment")
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                {
                    if let Some(mut assignment) = current("assignments", Some(id)) {
                        normalize_assignment(&mut assignment);
                        canonical["assignment"] = assignment;
                    }
                }
                if let Some(id) = params.get("message_id").and_then(Value::as_str) {
                    if let Ok((_, mut message)) = self.find_chat_message(id) {
                        normalize_message(&mut message);
                        canonical["message"] = message;
                    }
                }
            }
            "send_msg" | "chat.send" | "chat.react" | "execution.steer.delivery" => {
                let id = result
                    .get("message")
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| result.get("id").and_then(Value::as_str))
                    .or_else(|| params.get("message_id").and_then(Value::as_str));
                if let Some(id) = id {
                    if let Ok((_, mut message)) = self.find_chat_message(id) {
                        normalize_message(&mut message);
                        if result.get("message").is_some() {
                            canonical["message"] = message;
                        } else {
                            canonical = message;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(canonical)
    }

    /// Expired approvals tied to terminal assignments are no longer actions.
    /// Stable receipt keys repair a crash between state persistence and event
    /// publication without emitting another resolution on repeated stop/open.
    fn terminal_approval_events(
        &self,
        assignment_id: Option<&str>,
    ) -> Result<Vec<macbot_store::Event>, RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        self.terminal_approval_events_with_snapshot(assignment_id, &snapshot)
    }

    fn terminal_approval_events_with_snapshot(
        &self,
        assignment_id: Option<&str>,
        snapshot: &Value,
    ) -> Result<Vec<macbot_store::Event>, RpcError> {
        let mut events = Vec::new();
        for approval in snapshot
            .get("approvals")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|items| items.values())
        {
            let Some(id) = approval.get("assignment_id").and_then(Value::as_str) else {
                continue;
            };
            if assignment_id.is_some_and(|expected| id != expected)
                || approval.get("state").and_then(Value::as_str) != Some("expired")
                || !matches!(
                    snapshot
                        .get("assignments")
                        .and_then(|items| items.get(id))
                        .and_then(|assignment| assignment.get("status"))
                        .and_then(Value::as_str),
                    Some("cancelled" | "done" | "failed")
                )
            {
                continue;
            }
            let approval_id = approval
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(event) = self.append_repaired_event(
                &format!("terminal-assignment:{id}:approval:{approval_id}"),
                "approval.resolved",
                json!({"approval":approval}),
            )? {
                events.push(event);
            }
        }
        Ok(events)
    }

    fn repair_operation_events(
        &self,
        operation: &Value,
    ) -> Result<Vec<macbot_store::Event>, RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        self.repair_operation_events_with_snapshot(operation, &snapshot)
    }

    fn repair_operation_events_with_snapshot(
        &self,
        operation: &Value,
        snapshot: &Value,
    ) -> Result<Vec<macbot_store::Event>, RpcError> {
        let method = operation
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "completed operation has no method".into(),
                details: None,
            })?;
        let params = operation
            .get("params")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let result = operation.get("result").cloned().unwrap_or(Value::Null);
        let canonical = self.canonical_operation_result(method, &params, &result, snapshot)?;
        let base_key = operation
            .get("event_key")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| legacy_event_key(method, &params, &canonical));
        let mut events = Vec::new();
        if operation_event_entity_present(method, &canonical) {
            if let Some(event_name) = Self::event_name(method) {
                let data = event_data(method, &params, &canonical);
                if let Some(event) = self.append_repaired_event(&base_key, event_name, data)? {
                    events.push(event);
                }
            }
        }
        if method == "approval.decide" && canonical["approval"]["state"] == "denied" {
            if let Some(id) = canonical["approval"]["assignment_id"].as_str() {
                if let Some(assignment) =
                    snapshot.get("assignments").and_then(|items| items.get(id))
                {
                    let mut assignment = assignment.clone();
                    normalize_assignment(&mut assignment);
                    if let Some(event) = self.append_repaired_event(
                        &format!("{base_key}:assignment"),
                        "assignment.updated",
                        json!({"assignment":assignment}),
                    )? {
                        events.push(event);
                    }
                }
                events.extend(self.terminal_approval_events_with_snapshot(Some(id), snapshot)?);
                let message = self
                    .orchestrator
                    .create_task_stopped_message(id)
                    .map_err(Self::error)?;
                let mut message = serde_json::to_value(message).map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
                normalize_message(&mut message);
                let message = self.persist_client_message(&message)?;
                if let Some(event) = self.append_repaired_event(
                    &format!("task-stopped-message:{id}"),
                    "message.created",
                    json!({"message":message}),
                )? {
                    events.push(event);
                }
            }
        }
        if method == "assignment.stop" {
            events.extend(self.terminal_approval_events_with_snapshot(
                params.get("assignment_id").and_then(Value::as_str),
                snapshot,
            )?);
        }
        if matches!(method, "bot.create" | "bot.duplicate") {
            if let Some(chat) = canonical.get("dm_chat").filter(|value| value.is_object()) {
                if let Some(event) = self.append_repaired_event(
                    &format!("{base_key}:chat"),
                    "chat.created",
                    json!({"chat":chat}),
                )? {
                    events.push(event);
                }
            }
        }
        if method == "project.create" {
            if let Some(project) = canonical.get("project") {
                let chat = project_chat(project);
                if let Some(event) = self.append_repaired_event(
                    &format!("{base_key}:chat"),
                    "chat.created",
                    json!({"chat":chat}),
                )? {
                    events.push(event);
                }
            }
        }
        if method == "bot.create_from_template" {
            let bots = canonical
                .get("bots")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let chats = canonical
                .get("dm_chats")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (index, bot) in bots.iter().enumerate() {
                let id = bot
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("index-{index}"));
                if let Some(event) = self.append_repaired_event(
                    &format!("{base_key}:bot:{id}"),
                    "bot.created",
                    json!({"bot":bot}),
                )? {
                    events.push(event);
                }
            }
            for (index, chat) in chats.iter().enumerate() {
                let id = chat
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("index-{index}"));
                if let Some(event) = self.append_repaired_event(
                    &format!("{base_key}:chat:{id}"),
                    "chat.created",
                    json!({"chat":chat}),
                )? {
                    events.push(event);
                }
            }
        }
        events
            .extend(self.repair_operation_cards(method, &base_key, &params, &canonical, snapshot)?);
        if method == "send_msg" {
            for block in canonical
                .get("blocks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if block.get("type").and_then(Value::as_str) != Some("question") {
                    continue;
                }
                let Some(id) = block.get("question_id").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(question) = snapshot
                    .get("questions")
                    .and_then(|items| items.get(id))
                    .filter(|question| {
                        question.get("state").and_then(Value::as_str) == Some("pending")
                    })
                {
                    if let Some(event) = self
                        .store
                        .append_event_once(
                            &format!("decision-question:{id}:asked"),
                            "question.asked",
                            json!({"question":question}),
                        )
                        .map_err(store_error)?
                    {
                        self.remember_startup_repair_event(&event);
                        events.push(event);
                    }
                }
            }
        }
        Ok(events)
    }

    /// Internal runtime action.  See [`Self::execution_takeover_start`].
    pub async fn execution_takeover_release(
        &self,
        state: &GatewayState,
        params: &Value,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        if let Some(request_id) = params.get("client_request_id").and_then(Value::as_str) {
            if self.idempotency.lock().await.contains_key(request_id) {
                // Do not consume a second active takeover when a caller
                // retries after the first release was durably committed.
                return Ok(json!({}));
            }
        }
        let result = self.takeover_release(state, params).await?;
        self.persist(state, "takeover.release", params, &json!({}))
            .await?;
        Ok(result)
    }

    /// Internal model-tool action. `takeover.request` is intentionally absent
    /// from the public RPC method set; only an execution sink may create this
    /// durable approval card.
    pub async fn execution_takeover_request(
        &self,
        state: &GatewayState,
        params: Value,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        if let Some(request_id) = params.get("client_request_id").and_then(Value::as_str) {
            if let Some(value) = self.idempotency.lock().await.get(request_id).cloned() {
                return Ok(value);
            }
        }
        let result = self.takeover_request(&params).await?;
        self.persist(state, "takeover.request", &params, &result)
            .await
    }

    /// Merge only the delivery transition into the canonical user message.
    /// Runtime placeholders must never replace its identity, mentions or seq.
    pub async fn execution_update_steer_delivery(
        &self,
        state: &GatewayState,
        message_id: &str,
        delivery: &Value,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        let bot_id = delivery
            .get("bot_id")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::rpc_error("invalid_params", "steer bot_id is required", None))?;
        let assignment_id = delivery
            .get("assignment_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                crate::rpc_error("invalid_params", "steer assignment_id is required", None)
            })?;
        let next = delivery
            .get("state")
            .and_then(Value::as_str)
            .filter(|next| matches!(*next, "delivered" | "read"))
            .ok_or_else(|| {
                crate::rpc_error("invalid_params", "invalid steer delivery state", None)
            })?;
        let (_, mut message) = self.find_chat_message(message_id)?;
        let current = message
            .get_mut("delivery")
            .and_then(Value::as_array_mut)
            .and_then(|items| {
                items.iter_mut().find(|item| {
                    item.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                        && item.get("assignment_id").and_then(Value::as_str) == Some(assignment_id)
                })
            })
            .ok_or_else(|| {
                crate::rpc_error("not_found", "canonical steer delivery not found", None)
            })?;
        if current.get("state").and_then(Value::as_str) == Some("read")
            || current.get("state").and_then(Value::as_str) == Some(next)
        {
            return Ok(json!({"message":message}));
        }
        let transition = self
            .orchestrator
            .mark_steer_for_assignment(message_id, assignment_id, next)
            .map_err(Self::error)?;
        if transition.bot_id != bot_id || transition.assignment_id.as_deref() != Some(assignment_id)
        {
            return Err(crate::rpc_error(
                "invalid_params",
                "steer identity mismatch",
                None,
            ));
        }
        current["state"] = json!(next);
        current["at"] = json!(transition.at);
        let message = self.persist_client_message(&message)?;
        self.persist(
            state,
            "execution.steer.delivery",
            &json!({"message_id":message_id}),
            &json!({"message":message}),
        )
        .await?;
        let assignment = self
            .orchestrator
            .rpc("assignment.get", json!({"assignment_id":assignment_id}))
            .await
            .map_err(Self::error)?
            .get("assignment")
            .cloned()
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "assignment lookup returned no assignment".into(),
                details: None,
            })?;
        let data = json!({"assignment":assignment});
        let event = self
            .store
            .append_event("assignment.updated", data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        Ok(json!({"message":message}))
    }

    fn event_name(method: &str) -> Option<&'static str> {
        Some(match method {
            "bot.create" | "bot.duplicate" => "bot.created",
            "bot.update" => "bot.updated",
            "bot.delete" => "bot.deleted",
            "project.create" => "project.created",
            "announcement.update" => "announcement.updated",
            "project.update"
            | "project.add_member"
            | "project.remove_member"
            | "project.confirm_done"
            | "project.request_review"
            | "project.archive"
            | "project.reopen" => "project.updated",
            "assignment.create" | "assign" | "delegate" | "assignment.retry" => {
                "assignment.created"
            }
            "assignment.stop" | "assignment.steer" | "steer" | "message.steer" => {
                "assignment.updated"
            }
            "assignment.recovered" => "assignment.updated",
            "send_msg" | "chat.send" => "message.created",
            "message.delete" => "message.deleted",
            "message.edit" => "message.updated",
            "chat.react" | "execution.steer.delivery" => "message.updated",
            "chat.set_pinned" | "chat.set_muted" => "chat.updated",
            "approval.request" => "approval.requested",
            "approval.decide" | "approval.invalid" => "approval.resolved",
            "question.ask" => "question.asked",
            "propose_bot" => "question.asked",
            "question.answer" => "question.answered",
            "routine.create" | "routine.update" | "routine.set_enabled" => "routine.updated",
            "routine.delete" => "routine.deleted",
            "routine.test_run" => "routine.run",
            "routine.execution" => "routine.run",
            "chat.mark_read" => "read.updated",
            "settings.update" => "settings.updated",
            _ => return None,
        })
    }

    /// Persist one orchestrator operation while serializing the fresh snapshot
    /// with every runtime writer. This lock is intentionally independent from
    /// `write_lock`: model/runtime callbacks may call this entry point without
    /// holding the RPC request lock.
    pub async fn persist_orchestrator(&self, operation: Value) -> RpcResult {
        let _guard = self.persist_lock.lock().await;
        let request_id = operation
            .get("client_request_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let status = operation
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("done")
            .to_owned();
        let result = self.persist_orchestrator_locked(operation)?;
        if let Some(request_id) = request_id {
            if status == "rolled_back" {
                self.idempotency.lock().await.remove(&request_id);
            } else if status == "done" {
                self.idempotency
                    .lock()
                    .await
                    .insert(request_id, result.clone());
            }
        }
        Ok(result)
    }

    // All callers hold persist_lock, including the single-threaded constructor.
    fn persist_orchestrator_locked(&self, mut operation: Value) -> RpcResult {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let object = operation.as_object_mut().ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: "orchestrator operation must be an object".into(),
            details: None,
        })?;
        object.insert("snapshot".into(), snapshot.clone());
        object.entry("status").or_insert_with(|| json!("done"));
        object.entry("at").or_insert_with(|| json!(now()));
        let result = object.get("result").cloned().unwrap_or(Value::Null);
        self.store
            .append_jsonl("data/orchestrator/operations.jsonl", &operation)
            .map_err(store_error)?;
        self.store
            .write_snapshot("data/orchestrator/state.json", &snapshot)
            .map_err(store_error)?;
        Ok(result)
    }

    async fn persist(
        &self,
        state: &GatewayState,
        method: &str,
        params: &Value,
        result: &Value,
    ) -> RpcResult {
        let request_id = params.get("client_request_id").and_then(Value::as_str);
        let event_key = mutation_event_key(method, request_id, params);
        let audit_params = if method == "settings.update" {
            redact_settings_params(params)
        } else {
            params.clone()
        };
        let operation = json!({
            "method":method,
            "params":audit_params,
            "client_request_id":request_id,
            "result":result,
            "event_key":event_key,
            "status":"done",
            "at":now()
        });
        self.persist_orchestrator(operation.clone()).await?;
        if method == "announcement.update" {
            self.persist_announcement_text(result)?;
        }
        let mut events = Vec::new();
        if method == "propose_bot" {
            if let Some(question) = result.get("question") {
                if let Some(event) = self.ensure_proposal_question_message(question)? {
                    events.push(event);
                }
            }
        }
        events.extend(self.repair_operation_events(&operation)?);
        events.sort_by_key(|event| event.seq);
        for event in events {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        let project_id = params
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                result
                    .get("project")
                    .and_then(|project| project.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| {
                let assignment_id = params.get("assignment_id").and_then(Value::as_str)?;
                self.orchestrator
                    .snapshot()
                    .ok()?
                    .get("assignments")
                    .and_then(Value::as_object)
                    .and_then(|assignments| assignments.get(assignment_id))
                    .and_then(|assignment| assignment.get("project_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        if let Some(project_id) = project_id {
            self.refresh_project_events(state, &project_id).await?;
        }
        Ok(result.clone())
    }

    fn persist_announcement_text(&self, result: &Value) -> Result<(), RpcError> {
        let announcement = result.get("announcement").ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "announcement update result has no announcement".into(),
            details: None,
        })?;
        let project_id = announcement
            .get("project_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "announcement has no project_id".into(),
                details: None,
            })?;
        let version = announcement
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let project_component = announcement_path_component(project_id);
        let path = format!("data/projects/{project_component}/announcement.json");
        let previous = self
            .store
            .read_snapshot::<Value>(path.clone())
            .map_err(store_error)?;
        if previous
            .as_ref()
            .and_then(|value| value.get("version"))
            .and_then(Value::as_u64)
            .is_some_and(|old| old >= version)
        {
            return Ok(());
        }
        let updated_by = announcement
            .get("text_updated_by")
            .cloned()
            .unwrap_or_else(|| json!({"kind":"user"}));
        let updated_at = announcement
            .get("text_updated_at")
            .cloned()
            .unwrap_or_else(|| json!(now()));
        let entry = json!({
            "text":announcement.get("text").and_then(Value::as_str).unwrap_or(""),
            "version":version,
            "updated_by":updated_by,
            "updated_at":updated_at
        });
        self.store
            .append_jsonl(
                format!("data/projects/{project_component}/announcement.log.jsonl"),
                &entry,
            )
            .map_err(store_error)?;
        self.store.write_snapshot(path, &entry).map_err(store_error)
    }

    async fn ensure_announcement_updated_message(
        &self,
        state: &GatewayState,
        result: &Value,
    ) -> Result<(), RpcError> {
        let announcement = result.get("announcement").ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "announcement update result has no announcement".into(),
            details: None,
        })?;
        let project_id = announcement
            .get("project_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "announcement has no project_id".into(),
                details: None,
            })?;
        let version = announcement
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let project = self
            .orchestrator
            .rpc("project.get", json!({"project_id":project_id}))
            .await
            .map_err(Self::error)?;
        let chat_id = project
            .pointer("/project/chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "project has no chat_id".into(),
                details: None,
            })?;
        let updated_by = announcement
            .get("text_updated_by")
            .cloned()
            .unwrap_or_else(|| json!({"kind":"user"}));
        let label = if updated_by.get("kind").and_then(Value::as_str) == Some("user") {
            "你".to_owned()
        } else {
            let bot_id = updated_by
                .get("bot_id")
                .and_then(Value::as_str)
                .unwrap_or("main");
            self.orchestrator
                .rpc("bot.get", json!({"bot_id":bot_id}))
                .await
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/bot/name")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "Bot".to_owned())
        };
        let text = format!("{label} 更新了公告");
        let message_id = format!("msg_announcement_{project_id}_{version}");
        let raw = json!({
            "id":message_id,
            "chat_id":chat_id,
            "seq":0,
            "sender":{"kind":"system"},
            "created_at":announcement.get("text_updated_at").cloned().unwrap_or_else(|| json!(now())),
            "edited_at":null,
            "deleted":false,
            "reply_to":null,
            "quote":null,
            "thread_count":0,
            "mentions":[],
            "blocks":[{"type":"system","code":"announcement_updated","text":text}],
            "fallback_text":text,
            "intent":null,
            "assignment_id":null,
            "streaming":false,
            "delivery":[],
            "reactions":[]
        });
        let message = self.persist_client_message(&raw)?;
        if let Some(event) = self.append_repaired_event(
            &format!("announcement-system:{project_id}:{version}"),
            "message.created",
            json!({"message":message}),
        )? {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(())
    }

    /// Reconcile the public project announcement after an operation changes
    /// project membership, assignment state, highlights, or artifacts.
    /// Canonical payload comparison makes retries and runtime callbacks
    /// idempotent while still repairing an event lost after a JSONL write.
    pub async fn refresh_project_events(
        &self,
        state: &GatewayState,
        project_id: &str,
    ) -> Result<(), RpcError> {
        let _event_guard = self.event_lock.lock().await;
        let mut result = self
            .orchestrator
            .rpc("project.get", json!({"project_id":project_id}))
            .await
            .map_err(Self::error)?;
        let mut announcement = result
            .get_mut("announcement")
            .cloned()
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "project.get did not return announcement".into(),
                details: None,
            })?;
        normalize_announcement(&mut announcement);
        serde_json::from_value::<Announcement>(announcement.clone()).map_err(|error| RpcError {
            code: "internal".into(),
            message: format!("invalid announcement: {error}"),
            details: None,
        })?;
        let events = self.store.events_since(0).map_err(store_error)?;
        let announcement_changed = events
            .iter()
            .rev()
            .find(|event| {
                event.event == "announcement.updated"
                    && event.data["announcement"]["project_id"] == project_id
            })
            .is_none_or(|event| event.data["announcement"] != announcement);
        if announcement_changed {
            let data = json!({"announcement":announcement});
            let event = self
                .store
                .append_event("announcement.updated", data.clone())
                .map_err(store_error)?;
            state.publish_event(event.seq, &event.event, data).await;
        }
        if let Some(artifacts) = announcement.get("artifacts").and_then(Value::as_array) {
            for artifact in artifacts {
                let artifact_id = artifact.get("id").and_then(Value::as_str).unwrap_or("");
                let known = events.iter().rev().any(|event| {
                    event.event == "artifact.registered"
                        && event.data["artifact"]["id"] == artifact_id
                        && event.data["artifact"] == *artifact
                });
                if known {
                    continue;
                }
                let data = json!({"artifact":artifact});
                let event = self
                    .store
                    .append_event("artifact.registered", data.clone())
                    .map_err(store_error)?;
                state.publish_event(event.seq, &event.event, data).await;
            }
        }
        Ok(())
    }

    /// Advance due routines from the durable orchestrator state. This is kept
    /// separate from RPC handling so the gateway can run it from a bounded
    /// background tick without opening a second writer.
    pub async fn tick_routines(&self, state: &GatewayState, at: DateTime<Utc>) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        let runs = self.orchestrator.tick_routines(at).map_err(Self::error)?;
        if runs.is_empty() {
            return Ok(json!({"runs": [], "dispatch": []}));
        }
        let values = runs
            .iter()
            .map(|run| {
                serde_json::to_value(run).map_err(|e| RpcError {
                    code: "internal".into(),
                    message: e.to_string(),
                    details: None,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.persist_orchestrator(json!({
            "method":"routine.tick",
            "params":{"at":at.to_rfc3339()},
            "result":{"runs":values},
            "status":"done",
            "at":now()
        }))
        .await?;
        let state_snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        for run in values.iter() {
            if let Some(assignment_id) = run.get("assignment_id").and_then(Value::as_str) {
                if let Some(mut assignment) = state_snapshot
                    .get("assignments")
                    .and_then(Value::as_object)
                    .and_then(|items| items.get(assignment_id))
                    .cloned()
                {
                    normalize_assignment(&mut assignment);
                    let assignment_data = json!({"assignment":assignment});
                    let assignment_event = self
                        .store
                        .append_event("assignment.created", assignment_data.clone())
                        .map_err(store_error)?;
                    state
                        .publish_event(
                            assignment_event.seq,
                            &assignment_event.event,
                            assignment_data,
                        )
                        .await;
                    self.ensure_assignment_cards(state, &assignment, "assignment.create")
                        .await?;
                }
            }
            let data = json!({"run": run});
            let event = self
                .store
                .append_event("routine.run", data.clone())
                .map_err(store_error)?;
            state.publish_event(event.seq, &event.event, data).await;
        }
        let dispatch = values.iter().filter_map(|run| {
            let assignment_id = run.get("assignment_id").and_then(Value::as_str)?;
            let assignment = state_snapshot.get("assignments")?.get(assignment_id)?;
            Some(json!({"run_id":run["id"],"assignment_id":assignment_id,"bot_id":assignment["bot_id"],"chat_id":assignment["origin_chat_id"],"instruction":assignment["instruction"],"model":assignment["model"]}))
        }).collect::<Vec<_>>();
        Ok(json!({"runs": values, "dispatch": dispatch}))
    }

    /// Poll durable project attention markers and expose them as canonical
    /// system messages.  The orchestrator owns notice deduplication; this
    /// bridge owns wire normalization, chat sequencing, and replay events.
    fn canonical_attention_message(message: &Value, code: &str) -> Result<Value, RpcError> {
        let mut message = message.clone();
        let code = if code == "task_no_report" {
            "task_no_report"
        } else {
            "info"
        };
        let text = message
            .get("fallback_text")
            .and_then(Value::as_str)
            .or_else(|| message.get("text").and_then(Value::as_str))
            .unwrap_or("主 Bot 需要跟进任务")
            .to_owned();
        message["sender"] = json!({"kind":"system"});
        message["intent"] = Value::Null;
        message["blocks"] = json!([{"type":"system","code":code,"text":text}]);
        message["fallback_text"] = json!(text);
        normalize_message(&mut message);
        serde_json::from_value::<Message>(message.clone()).map_err(|error| RpcError {
            code: "internal".into(),
            message: format!("invalid attention message: {error}"),
            details: None,
        })?;
        Ok(message)
    }

    pub async fn refresh_project_attention(
        &self,
        state: &GatewayState,
        at: DateTime<Utc>,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        let before = self.orchestrator.snapshot().map_err(Self::error)?;
        let notices = self
            .orchestrator
            .poll_project_attention(at)
            .map_err(Self::error)?;
        let mut canonical_messages = Vec::new();
        let mut seen_message_ids = HashSet::new();
        for notice in &notices {
            let message = serde_json::to_value(&notice.message).map_err(|error| RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            })?;
            let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
            let message = Self::canonical_attention_message(&message, &notice.code)?;
            seen_message_ids.insert(message_id.to_owned());
            canonical_messages.push(message);
        }
        let after = self.orchestrator.snapshot().map_err(Self::error)?;
        if let Some(messages) = after.get("messages").and_then(Value::as_object) {
            for message in messages.values() {
                let Some(message_id) = message.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let mut parts = message_id.split(':');
                if parts.next() != Some("task_attention") || seen_message_ids.contains(message_id) {
                    continue;
                }
                let Some(code) = parts.next() else {
                    continue;
                };
                let canonical = Self::canonical_attention_message(message, code)?;
                seen_message_ids.insert(message_id.to_owned());
                canonical_messages.push(canonical);
            }
        }
        // Attention polling can return a large historical set after restart.
        // Load each chat once and sequence changed rows once per chat; the old
        // per-message path reread and rewrote the same large JSONL chat for
        // every historical attention message.
        canonical_messages = self.persist_attention_messages(canonical_messages)?;
        let previous_ids = before
            .get("assignments")
            .and_then(Value::as_object)
            .map(|items| items.keys().collect::<HashSet<_>>())
            .unwrap_or_default();
        let mut created_assignments = Vec::new();
        if let Some(assignments) = after.get("assignments").and_then(Value::as_object) {
            for (id, value) in assignments {
                if previous_ids.contains(id) {
                    continue;
                }
                let mut assignment = value.clone();
                normalize_assignment(&mut assignment);
                created_assignments.push(assignment);
            }
        }
        let attention_message_ids = canonical_messages
            .iter()
            .filter_map(|message| message.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<HashSet<_>>();
        if let Some(assignments) = after.get("assignments").and_then(Value::as_object) {
            let known_assignment_ids = created_assignments
                .iter()
                .filter_map(|assignment| {
                    assignment
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<HashSet<_>>();
            for assignment in assignments.values() {
                if assignment
                    .get("trigger_message_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| attention_message_ids.contains(id))
                    && assignment
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !known_assignment_ids.contains(id))
                {
                    let mut assignment = assignment.clone();
                    normalize_assignment(&mut assignment);
                    created_assignments.push(assignment);
                }
            }
        }
        let project_by_chat = after
            .get("projects")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|projects| projects.values())
            .filter_map(|project| {
                Some((
                    project.get("chat_id")?.as_str()?.to_owned(),
                    project.get("id")?.as_str()?.to_owned(),
                ))
            })
            .collect::<HashMap<_, _>>();
        let mut assignment_triggers = after
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|assignments| assignments.values())
            .filter_map(|assignment| {
                Some(assignment.get("trigger_message_id")?.as_str()?.to_owned())
            })
            .collect::<HashSet<_>>();
        // `poll_project_attention` already routes the first worker notice
        // through the orchestrator.  Historical notices are repaired below,
        // so keep a project-level view while deriving assignments here too;
        // otherwise every worker notice bypasses the orchestrator's active
        // main gate and creates another queued coordination assignment.
        let mut active_main_projects = after
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|assignments| assignments.values())
            .filter(|assignment| {
                assignment.get("bot_id").and_then(Value::as_str) == Some("main")
                    && matches!(
                        assignment.get("status").and_then(Value::as_str),
                        Some("queued" | "working" | "waiting_user" | "waiting_bot" | "blocked")
                    )
            })
            .filter_map(|assignment| {
                assignment
                    .get("project_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<HashSet<_>>();
        let mut derived_assignment = false;
        for message in &canonical_messages {
            let Some(message_id) = message.get("id").and_then(Value::as_str) else {
                continue;
            };
            // A task_no_report notice is still persisted and shown when its
            // source assignment belongs to main, but main must not create a
            // new main follow-up for its own coordination turn.  Worker
            // notices continue through the normal derivation below.
            let main_no_report = message_id.starts_with("task_attention:task_no_report:")
                && message
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .and_then(|assignment_id| {
                        after
                            .get("assignments")
                            .and_then(Value::as_object)
                            .and_then(|assignments| assignments.get(assignment_id))
                    })
                    .and_then(|assignment| assignment.get("bot_id").and_then(Value::as_str))
                    == Some("main");
            if main_no_report {
                continue;
            }
            if assignment_triggers.contains(message_id) {
                continue;
            }
            let Some(project_id) = message
                .get("chat_id")
                .and_then(Value::as_str)
                .and_then(|chat_id| project_by_chat.get(chat_id))
            else {
                continue;
            };
            if active_main_projects.contains(project_id) {
                continue;
            }
            let assignment = self
                .orchestrator
                .rpc(
                    "assignment.create",
                    json!({
                        "project_id":project_id,
                        "origin_chat_id":message["chat_id"],
                        "bot_id":"main",
                        "title":"主 Bot 跟进任务",
                        "instruction":message["fallback_text"],
                        "from":"system",
                        "trigger_message_id":message_id,
                        "parent_assignment_id":message.get("assignment_id").cloned().unwrap_or(Value::Null),
                        "root_message_id":message_id
                    }),
                )
                .await
                .map_err(Self::error)?;
            let mut assignment =
                normalize_result("assignment.create", assignment).map_err(|message| RpcError {
                    code: "internal".into(),
                    message,
                    details: None,
                })?;
            normalize_assignment(&mut assignment);
            created_assignments.push(assignment);
            assignment_triggers.insert(message_id.to_owned());
            active_main_projects.insert(project_id.to_owned());
            derived_assignment = true;
        }
        if canonical_messages.is_empty() && created_assignments.is_empty() {
            return Ok(json!({"notices":[]}));
        }
        if !notices.is_empty() || derived_assignment {
            self.persist_orchestrator(json!({
            "method":"attention.poll",
            "params":{"at":at.to_rfc3339()},
            "result":{"notice_ids":canonical_messages.iter().filter_map(|message| message.get("id")).collect::<Vec<_>>()},
            "status":"done",
            "at":now()
            }))
            .await?;
        }
        // Repair from one durable snapshot. Re-reading the complete event log
        // for every historical notice holds the RPC write lock for O(N * log)
        // parsing work on each attention tick.
        let mut event_index = AssignmentCardEventIndex::default();
        for event in self.store.events_since(0).map_err(store_error)? {
            match event.event.as_str() {
                "assignment.created" => {
                    if let Some(id) = event.data["assignment"]["id"].as_str() {
                        event_index.assignment_ids.insert(id.to_owned());
                    }
                }
                "message.created" | "message.updated" => {
                    if let Some(id) = event.data["message"]["id"].as_str() {
                        event_index
                            .message_rows
                            .insert(id.to_owned(), event.data["message"].clone());
                    }
                }
                _ => {}
            }
        }
        for assignment in created_assignments {
            let assignment_id = assignment.get("id").and_then(Value::as_str).unwrap_or("");
            let event_exists = !event_index.assignment_ids.insert(assignment_id.to_owned());
            if !event_exists {
                let data = json!({"assignment":assignment});
                let event = self
                    .store
                    .append_event("assignment.created", data.clone())
                    .map_err(store_error)?;
                state.publish_event(event.seq, &event.event, data).await;
            }
            self.ensure_assignment_cards_with_event_index(
                state,
                &assignment,
                "assignment.create",
                Some(&mut event_index),
            )
            .await?;
        }
        let mut emitted_messages = Vec::new();
        for message in canonical_messages.iter() {
            let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
            let event_exists = event_index.message_rows.contains_key(message_id);
            if event_exists {
                continue;
            }
            let data = json!({"message":message});
            let event = self
                .store
                .append_event("message.created", data.clone())
                .map_err(store_error)?;
            state.publish_event(event.seq, &event.event, data).await;
            emitted_messages.push(message.clone());
        }
        Ok(json!({"notices":emitted_messages}))
    }

    /// Commit a runtime routine result through the same durable operation and
    /// event path as ordinary RPC mutations.
    pub async fn execution_finish_routine_run(
        &self,
        state: &GatewayState,
        id: &str,
        status: &str,
        error: Option<String>,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        let params = json!({"run_id":id,"status":status,"error":error});
        let run = self
            .orchestrator
            .finish_routine_run(id, status, error)
            .map_err(Self::error)?;
        let Some(run) = run else {
            return Ok(json!({}));
        };
        self.persist(state, "routine.execution", &params, &json!({"run":run}))
            .await
    }

    /// Admit a `send_msg` emitted by `ExecutionEngine` through the same
    /// durable RPC path as a client request.  The execution payload carries
    /// the durable receipt, so replay retains the `run_id:call_id` key instead
    /// of creating a second orchestrator message after a crash.
    pub async fn execution_send_msg(&self, state: &GatewayState, envelope: Value) -> RpcResult {
        let mut params = envelope
            .get("message")
            .cloned()
            .unwrap_or_else(|| envelope.clone());
        let receipt = envelope.get("receipt");
        if let (Some(run_id), Some(call_id)) = (
            receipt
                .and_then(|value| value.get("run_id"))
                .and_then(Value::as_str),
            receipt
                .and_then(|value| value.get("call_id"))
                .and_then(Value::as_str),
        ) {
            if let Some(object) = params.as_object_mut() {
                object.insert("run_id".into(), json!(run_id));
                object.insert("call_id".into(), json!(call_id));
                object.insert(
                    "client_request_id".into(),
                    json!(format!("execution:{run_id}:{call_id}")),
                );
            }
        } else if let Some(message_id) = params
            .get("message_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            if let Some(object) = params.as_object_mut() {
                object.insert(
                    "client_request_id".into(),
                    json!(format!("execution:{message_id}")),
                );
            }
        }
        if let Some(request_id) = params.get("client_request_id").and_then(Value::as_str) {
            if let Some(value) = self.idempotency.lock().await.get(request_id).cloned() {
                return Ok(value);
            }
        }
        if params.get("intent").and_then(Value::as_str) == Some("done") {
            if let Some(review) = self
                .review_pending_messages_before_done(state, &params)
                .await?
            {
                return Ok(review);
            }
        }
        self.call("send_msg", params, state).await
    }

    async fn review_pending_messages_before_done(
        &self,
        state: &GatewayState,
        params: &Value,
    ) -> Result<Option<Value>, RpcError> {
        let (Some(assignment_id), Some(bot_id), Some(chat_id)) = (
            params.get("assignment_id").and_then(Value::as_str),
            params.get("bot_id").and_then(Value::as_str),
            params.get("chat_id").and_then(Value::as_str),
        ) else {
            return Ok(None);
        };
        let assignment = self
            .orchestrator
            .rpc("assignment.get", json!({"assignment_id":assignment_id}))
            .await
            .map_err(Self::error)?;
        let assignment = assignment.get("assignment").ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "assignment lookup returned no assignment".into(),
            details: None,
        })?;
        if assignment.get("bot_id").and_then(Value::as_str) != Some(bot_id)
            || assignment.get("origin_chat_id").and_then(Value::as_str) != Some(chat_id)
        {
            return Ok(None);
        }
        let return_chat_id = self
            .orchestrator
            .resolve_assignment_return_chat(assignment_id)
            .map_err(Self::error)?;
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let queued = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|messages| messages.values())
            .filter(|message| {
                message.get("chat_id").and_then(Value::as_str) == Some(return_chat_id.as_str())
                    && message.get("deleted").and_then(Value::as_bool) != Some(true)
                    && message
                        .get("delivery")
                        .and_then(Value::as_array)
                        .is_some_and(|deliveries| {
                            deliveries.iter().any(|delivery| {
                                delivery.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                                    && delivery.get("assignment_id").and_then(Value::as_str)
                                        == Some(assignment_id)
                                    && delivery.get("state").and_then(Value::as_str)
                                        == Some("queued")
                            })
                        })
            })
            .cloned()
            .collect::<Vec<_>>();
        if queued.is_empty() {
            return Ok(None);
        }
        let mut reviewed = Vec::with_capacity(queued.len());
        for message in queued {
            let message_id = message
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError {
                    code: "internal".into(),
                    message: "queued message has no id".into(),
                    details: None,
                })?;
            let update = self
                .execution_update_steer_delivery(
                    state,
                    message_id,
                    &json!({"bot_id":bot_id,"assignment_id":assignment_id,"state":"read"}),
                )
                .await?;
            reviewed.push(update.get("message").cloned().unwrap_or(message));
        }
        let ids = reviewed
            .iter()
            .filter_map(|message| message.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        let result = json!({"_queue_review":{"message_ids":ids,"messages":reviewed}});
        let operation = json!({
            "method":"execution.queue.review",
            "params":{"assignment_id":assignment_id,"bot_id":bot_id,"message_ids":ids},
            "client_request_id":params.get("client_request_id"),
            "result":result,
            "status":"done",
            "at":now()
        });
        self.persist_orchestrator(operation).await?;
        Ok(Some(result))
    }
}

#[async_trait]
impl RpcBackend for ProductionBackend {
    async fn export_usage_csv(&self, params: &Value, timezone: &str) -> Result<String, RpcError> {
        self.usage
            .lock()
            .await
            .export_csv(params, timezone)
            .map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: error.to_string(),
                details: None,
            })
    }

    async fn call(&self, method: &str, params: Value, state: &GatewayState) -> RpcResult {
        // Liveness probes must not wait behind the mutation writer.  The
        // protocol requires a fresh server timestamp in the response.
        if method == "ping" {
            return Ok(json!({"server_time": now()}));
        }
        if method == "chat.send" {
            crate::validate_client_locale(&params)?;
        }
        // Duplicate uses a shared FeatureService and must own the writer lock
        // across Bot creation and persistence. Route it before the generic
        // lock so the state-aware helper cannot deadlock on a second lock.
        if method == "bot.duplicate" {
            let feature_service = FeatureService::with_store(
                self.store.clone(),
                self.store.root().to_path_buf(),
                Vec::<std::path::PathBuf>::new(),
            )
            .map_err(|error| RpcError {
                code: error.code().into(),
                message: error.to_string(),
                details: None,
            })?;
            return self
                .duplicate_bot_with_feature_service_and_state(
                    state,
                    &params,
                    Arc::new(feature_service),
                )
                .await;
        }
        // Usage queries only read the in-memory ledger and the settings
        // timezone. Keep dashboard refreshes independent from mutation
        // persistence; they must not queue behind a long snapshot/fsync.
        if matches!(
            method,
            "usage.summary" | "usage.heatmap" | "usage.timeseries" | "usage.breakdown"
        ) {
            return self.usage_query(state, method, &params).await;
        }
        let _guard = self.write_lock.lock().await;
        let mut params = params;
        if matches!(method, "bot.create" | "bot.update") {
            let avatar = if method == "bot.create" {
                params.get("avatar")
            } else {
                params.get("patch").and_then(|patch| patch.get("avatar"))
            };
            if let Some(avatar) = avatar.filter(|avatar| !avatar.is_null()) {
                // Keep accepting the former emoji-string representation from
                // older clients, while validating new structured avatars.
                if !avatar.is_string() && serde_json::from_value::<Avatar>(avatar.clone()).is_err()
                {
                    return Err(RpcError {
                        code: "invalid_params".into(),
                        message: "avatar must be a valid Avatar object".into(),
                        details: None,
                    });
                }
            }
        }
        if method == "send_msg" {
            self.execution_validate_send_msg_target(&params)?;
            let mut validation_only = json!({"blocks":[]});
            append_project_card_blocks(self, &mut validation_only, &params).await?;
        }
        let bot_dm_context = if method == "send_msg" {
            self.prepare_bot_dm_send(&mut params)?
        } else {
            None
        };
        if let Some(context) = bot_dm_context.as_ref() {
            if self.ensure_bot_dm_chat(&context.route)? {
                self.publish_bot_dm_chat(state, &context.route).await?;
            }
        }
        if method.starts_with("provider.") || method.starts_with("model.") {
            let in_use_models = self.in_use_models();
            let mut registry = self.providers.lock().await;
            let reply = registry
                .rpc(method, params, &in_use_models)
                .await
                .map_err(registry_error)?;
            for event in reply.events {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
            return Ok(reply.result);
        }
        if let Some(request_id) = params.get("client_request_id").and_then(Value::as_str) {
            if let Some(value) = self.idempotency.lock().await.get(request_id).cloned() {
                let operation = self
                    .store
                    .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
                    .map_err(store_error)?
                    .into_iter()
                    .rev()
                    .find(|operation| {
                        operation.get("status").and_then(Value::as_str) == Some("done")
                            && operation.get("method").and_then(Value::as_str) == Some(method)
                            && operation.get("client_request_id").and_then(Value::as_str)
                                == Some(request_id)
                    })
                    .unwrap_or_else(|| {
                        json!({
                            "method":method,
                            "params":params,
                            "result":value,
                            "event_key":format!("rpc:{method}:{request_id}"),
                            "status":"done"
                        })
                    });
                let mut events = Vec::new();
                if method == "propose_bot" {
                    if let Some(question) = value.get("question") {
                        if let Some(event) = self.ensure_proposal_question_message(question)? {
                            events.push(event);
                        }
                    }
                }
                events.extend(self.repair_operation_events(&operation)?);
                events.sort_by_key(|event| event.seq);
                for event in events {
                    state
                        .publish_event(event.seq, &event.event, event.data)
                        .await;
                }
                match method {
                    "project.create" => self.ensure_project_card(state, &value).await?,
                    "announcement.update" => {
                        self.persist_announcement_text(&value)?;
                        self.ensure_announcement_updated_message(state, &value)
                            .await?;
                    }
                    "assignment.create" | "assign" | "delegate" | "assignment.retry" => {
                        self.ensure_assignment_cards(state, &value, method).await?
                    }
                    "send_msg" => {
                        if let Some(context) = bot_dm_context.as_ref() {
                            self.ensure_bot_dm_ref(state, context, &value, &params)
                                .await?;
                        }
                    }
                    _ => {}
                }
                return Ok(value);
            }
        }
        if method == "message.delete" {
            let message_id = required_text(&params, "message_id")?;
            let (chat_id, message) = self.find_chat_message(&message_id)?;
            ensure_user_message(&message)?;
            if message.get("deleted").and_then(Value::as_bool) == Some(true) {
                let data = json!({"chat_id":chat_id,"message_id":message_id});
                if let Some(event) = self.append_repaired_event(
                    &format!("rpc:message.delete:message-delete:{message_id}"),
                    "message.deleted",
                    data,
                )? {
                    state
                        .publish_event(event.seq, &event.event, event.data)
                        .await;
                }
                return Ok(json!({}));
            }
            params["chat_id"] = json!(chat_id);
            if params
                .get("client_request_id")
                .and_then(Value::as_str)
                .is_none()
            {
                params["client_request_id"] = json!(format!("message-delete:{message_id}"));
            }
        }
        if method == "settings.update" {
            validate_settings_secret_params(&params)?;
        }
        if Self::is_mutation(method) {
            let audit_params = if method == "settings.update" {
                redact_settings_params(&params)
            } else {
                params.clone()
            };
            let start = json!({ "method": method, "params": audit_params, "client_request_id": params.get("client_request_id").and_then(Value::as_str), "status": "started", "at": now() });
            self.store
                .append_jsonl("data/orchestrator/operations.jsonl", &start)
                .map_err(store_error)?;
        }
        let result = match method {
            "bootstrap" => self.bootstrap(state).await?,
            "bot.duplicate" => self.duplicate_bot_with_skills(&params).await?,
            "chat.send" => self.chat_send(params.clone()).await?,
            "chat.history" => self.chat_history(&params).await?,
            "chat.thread" => self.chat_thread(&params).await?,
            "message.delete" => self.message_delete(&params)?,
            "message.edit" => self.message_edit(&params)?,
            "artifact.versions.list" => self.artifact_versions_list(&params)?,
            "artifact.version.get" => self.artifact_version_get(&params)?,
            "chat.mark_read" => self.chat_mark_read(&params).await?,
            // Whether a Bot still needs this message is represented by the
            // assignment-scoped delivery state, not the user's chat read cursor.
            // The orchestrator validates the exact queued delivery and task.
            "message.steer" => self
                .orchestrator
                .rpc(method, params.clone())
                .await
                .map_err(Self::error)?,
            "chat.react" => self.chat_react(&params).await?,
            "chat.set_pinned" => self.chat_set_flag(&params, "pinned").await?,
            "chat.set_muted" => self.chat_set_flag(&params, "muted").await?,
            "chat.list" => self.chat_list().await?,
            "chat.get" => self.chat_get(&params).await?,
            "settings.get" => json!({"settings": self.settings(state).await?}),
            "host.permissions" => host_permissions(),
            "settings.update" => self.settings_update(state, &params).await?,
            "usage.summary" | "usage.heatmap" | "usage.timeseries" | "usage.breakdown" => {
                self.usage_query(state, method, &params).await?
            }
            "workbench.get" => self.workbench().map_err(|message| RpcError {
                code: "internal".into(),
                message,
                details: None,
            })?,
            "device.register" => self.device_register(&params).await?,
            // Protocol authority for both actions is `{}`.  The internal
            // runtime bridge calls the typed helpers above when it needs the
            // durable request snapshot for continuation.
            "takeover.start" => {
                self.takeover_start(state, &params).await?;
                json!({})
            }
            "takeover.release" => {
                self.takeover_release(state, &params).await?;
                json!({})
            }
            "assignment.stop" => self.assignment_stop(state, &params).await?,
            "assignment.retry" => self.assignment_retry(state, &params).await?,
            "loop.resolve" => self.loop_resolve(state, &params).await?,
            "project.request_review" => project_request_review(self, &params, state).await?,
            "project.request_changes" => project_request_changes(self, &params, state).await?,
            "announcement.update" => match self.orchestrator.rpc(method, params.clone()).await {
                Ok(result) => result,
                Err(error @ macbot_orchestrator::OrchestratorError::Conflict(_)) => {
                    let project_id = params
                        .get("project_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let current = self
                        .orchestrator
                        .rpc("project.get", json!({"project_id":project_id}))
                        .await
                        .ok();
                    let announcement = current.and_then(|value| value.get("announcement").cloned());
                    return Err(RpcError {
                        code: "conflict".into(),
                        message: error.to_string(),
                        details: announcement
                            .map(|announcement| json!({"announcement":announcement})),
                    });
                }
                Err(error) => return Err(Self::error(error)),
            },
            "propose_bot" => {
                let proposal = self
                    .orchestrator
                    .rpc("propose_bot", params.clone())
                    .await
                    .map_err(Self::error)?;
                let proposal_id = proposal
                    .get("proposal_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError {
                        code: "internal".into(),
                        message: "proposal did not return proposal_id".into(),
                        details: None,
                    })?;
                let chat_id = params
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .unwrap_or("chat_main");
                let bot_id = params
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .unwrap_or("main");
                // A model run supplies its real assignment through
                // CollaborationIdentity.  Client callers may also provide
                // it explicitly.  For a standalone proposal, create a real
                // main-Bot assignment so the protocol Question remains
                // referentially valid instead of using proposal_id as a
                // fabricated assignment.
                let assignment_id = if let Some(id) = params
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    id.to_owned()
                } else {
                    let assignment_params = json!({
                        "origin_chat_id": chat_id,
                        "bot_id": bot_id,
                        "title": format!("确认创建 Bot「{}」", proposal.get("name").and_then(Value::as_str).unwrap_or("新 Bot")),
                        "instruction": "等待用户确认创建 Bot",
                        "from": "main"
                    });
                    let assignment = self
                        .orchestrator
                        .rpc("assignment.create", assignment_params.clone())
                        .await
                        .map_err(Self::error)?;
                    let assignment =
                        normalize_result("assignment.create", assignment).map_err(|message| {
                            RpcError {
                                code: "internal".into(),
                                message,
                                details: None,
                            }
                        })?;
                    let assignment_id = assignment
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| RpcError {
                            code: "internal".into(),
                            message: "assignment.create did not return id".into(),
                            details: None,
                        })?
                        .to_owned();
                    self.persist(state, "assignment.create", &assignment_params, &assignment)
                        .await?;
                    self.ensure_assignment_cards(state, &assignment, "assignment.create")
                        .await?;
                    assignment_id
                };
                let question = self
                    .orchestrator
                    .rpc(
                        "question.ask",
                        json!({
                            "bot_id": bot_id,
                            "assignment_id": assignment_id,
                            "chat_id": chat_id,
                            "text": format!("批准创建 Bot「{}」？", proposal.get("name").and_then(Value::as_str).unwrap_or("新 Bot")),
                            "options": ["批准", "拒绝"],
                            "allow_free_text": true
                        }),
                    )
                    .await
                    .map_err(Self::error)?;
                let question_id =
                    question
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| RpcError {
                            code: "internal".into(),
                            message: "question.ask did not return id".into(),
                            details: None,
                        })?;
                self.write_proposal(
                    question_id,
                    &json!({
                        "question_id": question_id,
                        "proposal_id": proposal_id,
                        "assignment_id": assignment_id,
                        "state": "pending",
                        "params": params
                    }),
                )?;
                json!({"proposal":proposal,"question":question})
            }
            "question.answer" => self.answer_proposal(state, &params).await?,
            "project.confirm_done" => {
                let project_id = params
                    .get("project_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError {
                        code: "invalid_params".into(),
                        message: "project_id is required".into(),
                        details: None,
                    })?;
                let current = self
                    .orchestrator
                    .rpc("project.get", json!({"project_id":project_id}))
                    .await
                    .map_err(Self::error)?;
                if !matches!(
                    current["project"]["status"].as_str(),
                    Some("active") | Some("review")
                ) {
                    return Err(RpcError {
                        code: "conflict".into(),
                        message: "only active or review projects can be finished".into(),
                        details: None,
                    });
                }
                let result = self
                    .orchestrator
                    .rpc(method, params.clone())
                    .await
                    .map_err(Self::error)?;
                self.set_review_card(
                    state,
                    project_id,
                    "confirmed",
                    &Value::Null,
                    "项目已确认完成",
                )
                .await?;
                self.set_completion_card(state, project_id).await?;
                result
            }
            _ => self
                .orchestrator
                .rpc(method, params.clone())
                .await
                .map_err(Self::error)?,
        };
        if method == "approval.decide" {
            self.sync_approval_rule(state, &result).await?;
        }
        let result = normalize_result(method, result).map_err(|message| RpcError {
            code: "internal".into(),
            message,
            details: None,
        })?;
        let mut result = result;
        if method == "send_msg" {
            append_link_preview_blocks(&mut result, &params)?;
            append_project_card_blocks(self, &mut result, &params).await?;
            result = self.persist_client_message(&result)?;
        } else if method == "chat.send" {
            let canonical = self.persist_client_message(&result["message"])?;
            result["message"] = canonical;
        }
        // `bootstrap` already enriches its Bot list before assembling the
        // complete snapshot. Avoid a second full durable-job walk here: the
        // generic RPC tail is shared by all methods, but bootstrap is the
        // main WebSocket handshake and must not pay for the same scan twice.
        if method != "bootstrap" {
            self.enrich_bot_status(&mut result)
                .map_err(|message| RpcError {
                    code: "internal".into(),
                    message,
                    details: None,
                })?;
        }
        validate_result(method, &result).map_err(|message| RpcError {
            code: "internal".into(),
            message,
            details: None,
        })?;
        if Self::is_mutation(method) {
            let persisted = self.persist(state, method, &params, &result).await?;
            if method == "send_msg" {
                if let Err(error) = self.record_artifact_versions(&params) {
                    tracing::warn!(%error, "could not record artifact version snapshot");
                }
            }
            if method == "project.create" {
                self.ensure_project_card(state, &result).await?;
            } else if method == "announcement.update" {
                self.ensure_announcement_updated_message(state, &result)
                    .await?;
            }
            if matches!(
                method,
                "assignment.create" | "assign" | "delegate" | "assignment.retry"
            ) {
                self.ensure_assignment_cards(state, &result, method).await?;
            } else if matches!(method, "send_msg" | "chat.send") {
                self.ensure_trigger_assignment_cards(
                    state,
                    result
                        .get("id")
                        .or_else(|| result.pointer("/message/id"))
                        .and_then(Value::as_str),
                )
                .await?;
                if let Some(context) = bot_dm_context.as_ref() {
                    self.ensure_bot_dm_ref(state, context, &result, &params)
                        .await?;
                }
            }
            Ok(persisted)
        } else {
            Ok(result)
        }
    }
}

/// Turn a worker's done handoff into the durable project review state and
/// one user-visible message.  The orchestrator deliberately keeps this
/// composition out of its compact RPC model; the gateway adapter owns the
/// wire-level project/message event shape.
async fn project_request_review(
    backend: &ProductionBackend,
    params: &Value,
    state: &GatewayState,
) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: "project_id is required".into(),
            details: None,
        })?;
    let summary = params
        .get("summary")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: "summary is required".into(),
            details: None,
        })?;
    let project = backend
        .orchestrator
        .mark_project_review(project_id)
        .map_err(ProductionBackend::error)?;
    let project = serde_json::to_value(project).map_err(|error| RpcError {
        code: "internal".into(),
        message: error.to_string(),
        details: None,
    })?;
    let project_result = backend
        .orchestrator
        .rpc("project.get", json!({"project_id":project_id}))
        .await
        .map_err(ProductionBackend::error)?;
    let artifacts = project_result
        .get("announcement")
        .and_then(|announcement| announcement.get("artifacts"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|artifact| {
                    json!({
                        "artifact_id":artifact.get("id").cloned().unwrap_or(Value::Null),
                        "title":artifact.get("title").cloned().unwrap_or_else(|| json!("")),
                        "path_or_url":artifact.get("path_or_url").cloned().unwrap_or_else(|| json!("")),
                        "bot_id":artifact.get("bot_id").cloned().unwrap_or_else(|| json!("main")),
                        "kind":artifact.get("kind").cloned().unwrap_or_else(|| json!("file"))
                    })
                })
                .collect::<Vec<_>>()
        })
        .map(Value::Array)
        .unwrap_or_else(|| json!([]));
    backend
        .set_review_card(state, project_id, "pending", &artifacts, summary)
        .await?;
    Ok(json!({"project":project}))
}

async fn project_request_changes(
    backend: &ProductionBackend,
    params: &Value,
    state: &GatewayState,
) -> RpcResult {
    if params.get("text").and_then(Value::as_str).is_none() {
        return Err(RpcError {
            code: "invalid_params".into(),
            message: "text is required".into(),
            details: None,
        });
    }
    let project_result = backend
        .orchestrator
        .rpc("project.request_changes", params.clone())
        .await
        .map_err(ProductionBackend::error)?;
    let raw_message = project_result
        .get("message")
        .cloned()
        .ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "project.request_changes returned no message".into(),
            details: None,
        })?;
    let project = backend
        .orchestrator
        .rpc(
            "project.get",
            json!({"project_id": params.get("project_id").cloned().unwrap_or(Value::Null)}),
        )
        .await
        .map_err(ProductionBackend::error)?
        .get("project")
        .cloned()
        .ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "project.request_changes project lookup returned no project".into(),
            details: None,
        })?;
    let mut raw_message = raw_message;
    normalize_message(&mut raw_message);
    let message = backend.persist_client_message(&raw_message)?;
    let mut project = project;
    normalize_project(&mut project);
    let project_data = json!({"project":project});
    let project_event = backend
        .store
        .append_event("project.updated", project_data.clone())
        .map_err(store_error)?;
    state
        .publish_event(project_event.seq, &project_event.event, project_data)
        .await;
    let message_data = json!({"message":message.clone()});
    let message_event = backend
        .store
        .append_event("message.created", message_data.clone())
        .map_err(store_error)?;
    state
        .publish_event(message_event.seq, &message_event.event, message_data)
        .await;
    backend
        .set_review_card(
            state,
            params
                .get("project_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError {
                    code: "invalid_params".into(),
                    message: "project_id is required".into(),
                    details: None,
                })?,
            "changes_requested",
            &Value::Null,
            params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("已请求修改"),
        )
        .await?;
    Ok(json!({"message":message}))
}

impl ProductionBackend {
    async fn settings(&self, state: &GatewayState) -> Result<Value, RpcError> {
        let path = "data/settings.json";
        let host_name = state.host_name.read().await.clone();
        let mut value = self
            .store
            .read_snapshot::<Value>(path)
            .map_err(store_error)?
            .unwrap_or_else(|| default_settings(&host_name));
        merge_missing_settings(&mut value, default_settings(&host_name));
        ensure_settings_tool_defaults(&mut value).map_err(|message| RpcError {
            code: "internal".into(),
            message,
            details: None,
        })?;
        ensure_builtin_approval_rules(&mut value);
        serde_json::from_value::<Settings>(value.clone()).map_err(|error| RpcError {
            code: "internal".into(),
            message: error.to_string(),
            details: None,
        })?;
        let provider_id = value
            .pointer("/web_search/provider")
            .and_then(Value::as_str)
            .filter(|provider| !provider.is_empty());
        let has_key = provider_id.is_some_and(|provider| {
            self.providers
                .try_lock()
                .ok()
                .and_then(|registry| registry.secret_store().get(provider).ok())
                .flatten()
                .is_some()
        });
        if let Some(web_search) = value.get_mut("web_search").and_then(Value::as_object_mut) {
            web_search.insert("has_key".into(), json!(has_key));
        }
        Ok(value)
    }

    async fn settings_update(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let current = self.settings(state).await?;
        let mut value = current.clone();
        let patch = params
            .get("patch")
            .and_then(Value::as_object)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "patch is required".into(),
                details: None,
            })?;
        merge_json(
            value.as_object_mut().ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "settings must be an object".into(),
                details: None,
            })?,
            patch,
        );
        protect_builtin_approval_rules(&current, &value, patch)?;
        ensure_builtin_approval_rules(&mut value);
        let tool_defaults =
            ensure_settings_tool_defaults(&mut value).map_err(|message| RpcError {
                code: "invalid_params".into(),
                message,
                details: None,
            })?;
        serde_json::from_value::<Settings>(value.clone()).map_err(|error| RpcError {
            code: "invalid_params".into(),
            message: error.to_string(),
            details: None,
        })?;

        if let Some(web_search_key) = params.get("web_search_key") {
            let key = web_search_key.as_str().ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "web_search_key must be a string".into(),
                details: None,
            })?;
            let provider = value
                .pointer("/web_search/provider")
                .and_then(Value::as_str)
                .filter(|provider| !provider.is_empty())
                .ok_or_else(|| RpcError {
                    code: "invalid_params".into(),
                    message: "web_search.provider is required when setting web_search_key".into(),
                    details: None,
                })?;
            let secrets = self.providers.lock().await.secret_store();
            if key.is_empty() {
                secrets.delete(provider).map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
            } else {
                secrets.set(provider, key).map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
            }
        }
        self.store
            .write_snapshot("data/settings.json", &value)
            .map_err(store_error)?;
        self.orchestrator
            .configure(scheduler_limits(&value).map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: error.to_string(),
                details: None,
            })?)
            .map_err(Self::error)?;
        self.orchestrator
            .configure_default_bot_tools(tool_defaults)
            .map_err(Self::error)?;
        *state.host_name.write().await = value
            .get("host_name")
            .and_then(Value::as_str)
            .unwrap_or("Mac Bot")
            .into();
        Ok(json!({"settings": self.settings(state).await?}))
    }

    async fn sync_approval_rule(
        &self,
        state: &GatewayState,
        result: &Value,
    ) -> Result<(), RpcError> {
        let Some(approval) = result.get("approval") else {
            return Ok(());
        };
        if approval.get("state").and_then(Value::as_str) != Some("always_allowed") {
            return Ok(());
        }
        if approval
            .get("always_allow_supported")
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Ok(());
        }
        let mut settings = self.settings(state).await?;
        let approvals = settings
            .get_mut("approvals")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "settings.approvals is missing".into(),
                details: None,
            })?;
        let rules = approvals
            .entry("rules")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "settings.approvals.rules must be an array".into(),
                details: None,
            })?;
        let id = approval.get("id").cloned().unwrap_or(Value::Null);
        if !rules.iter().any(|rule| rule.get("id") == Some(&id)) {
            rules.push(json!({
                "id": id,
                "kind": "auto_allow",
                "text": approval.get("summary").cloned().unwrap_or(Value::String(String::new())),
                "created_at": approval.get("decided_at").cloned().unwrap_or_else(|| json!(now())),
                "bot_id": approval.get("bot_id").cloned().unwrap_or(Value::Null),
                "builtin": false
            }));
        }
        serde_json::from_value::<Settings>(settings.clone()).map_err(|error| RpcError {
            code: "internal".into(),
            message: error.to_string(),
            details: None,
        })?;
        self.store
            .write_snapshot("data/settings.json", &settings)
            .map_err(store_error)
    }

    async fn usage_query(&self, state: &GatewayState, method: &str, params: &Value) -> RpcResult {
        let settings = self.settings(state).await?;
        let timezone = settings
            .get("timezone")
            .and_then(Value::as_str)
            .unwrap_or("Asia/Shanghai")
            .to_owned();
        let mut result = self
            .usage
            .lock()
            .await
            .query(method, params, &timezone)
            .map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: error.to_string(),
                details: None,
            })?;
        normalize_usage_result(method, &mut result);
        Ok(result)
    }

    async fn takeover_request(&self, params: &Value) -> RpcResult {
        let bot_id = required_text(params, "bot_id")?;
        let group_chat_id = required_text(params, "chat_id")?;
        // A private execution request has no orchestrator assignment.  Keep
        // the question in the bot's direct-chat scope (`null` on the
        // question RPC), while the durable takeover record gets a stable
        // synthetic key for start/release lookup.
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("用户接管浏览器")
            .to_owned();
        let dm_chat_id = self
            .orchestrator
            .snapshot()
            .map_err(Self::error)?
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(&bot_id))
            .and_then(|bot| bot.get("dm_chat_id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("dm_{bot_id}"));
        let private_scope = format!("dm_{}", takeover_component(&group_chat_id));
        let source_assignment_id = params
            .get("assignment_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .filter(|value| !(dm_chat_id == group_chat_id && *value == private_scope))
            .map(str::to_owned);
        let assignment_id = source_assignment_id.clone().unwrap_or(private_scope);
        let question = self
            .orchestrator
            .rpc(
                "question.ask",
                json!({
                    "bot_id": bot_id,
                    "assignment_id": source_assignment_id,
                    "chat_id": dm_chat_id,
                    "text": format!("{reason}。是否接管浏览器？"),
                    "options": ["接管", "取消"],
                    "allow_free_text": false
                }),
            )
            .await
            .map_err(Self::error)?;
        let mut request = json!({
            "bot_id": bot_id,
            "assignment_id": assignment_id,
            "chat_id": dm_chat_id,
            "group_chat_id": group_chat_id,
            "reason": reason,
            "question_id": question.get("id").cloned().unwrap_or(Value::Null),
            "state": "pending",
            "created_at": now()
        });
        if let Some(message_id) = params
            .get("message_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            request["message_id"] = json!(message_id);
        }
        if let Some(run_id) = params
            .get("run_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            request["run_id"] = json!(run_id);
        }
        self.store
            .write_snapshot(
                format!("data/takeovers/{}.json", takeover_component(&assignment_id)),
                &request,
            )
            .map_err(store_error)?;
        Ok(json!({"takeover_request": request, "question": question}))
    }

    async fn takeover_start(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let bot_id = required_text(params, "bot_id")?;
        let has_assignment_hint = params
            .get("assignment_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());

        // A client-visible start has no assignment id in the v1 protocol. If
        // this Bot is already owned by the user, make the action idempotent
        // and keep the original run/message/question scope intact. An
        // explicit hint still goes through the exact pending record below.
        let (assignment_id, mut request, already_active) = if !has_assignment_hint {
            match self.takeover_for_action(params, "active") {
                Ok((assignment_id, request)) => (assignment_id, request, true),
                Err(error) if error.code == "not_found" => {
                    let (assignment_id, request) = self.takeover_for_action(params, "pending")
                        .or_else(|error| {
                            if error.code != "not_found" {
                                return Err(error);
                            }
                            let assignment_id = format!("browser_takeover_{bot_id}");
                            let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
                            let bot = snapshot
                                .get("bots")
                                .and_then(Value::as_object)
                                .and_then(|bots| bots.get(&bot_id))
                                .ok_or_else(|| RpcError {
                                    code: "not_found".into(),
                                    message: format!("bot {bot_id} not found"),
                                    details: None,
                                })?;
                            let chat_id = bot
                                .get("dm_chat_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    if bot_is_main(bot) {
                                        "chat_main".into()
                                    } else {
                                        format!("dm_{bot_id}")
                                    }
                                });
                            Ok((
                                assignment_id.clone(),
                                json!({"bot_id":bot_id,"assignment_id":assignment_id,"chat_id":chat_id,"group_chat_id":Value::Null,"reason":"用户接管浏览器","question_id":Value::Null,"state":"pending","created_at":now()}),
                            ))
                        })?;
                    (assignment_id, request, false)
                }
                Err(error) => return Err(error),
            }
        } else {
            let (assignment_id, request) = self.takeover_for_action(params, "pending")?;
            (assignment_id, request, false)
        };
        if request.get("bot_id").and_then(Value::as_str) != Some(bot_id.as_str()) {
            return Err(RpcError {
                code: "forbidden".into(),
                message: "takeover belongs to another bot".into(),
                details: None,
            });
        }
        state
            .browser
            .lock()
            .await
            .takeover_start(&bot_id)
            .map_err(browser_error)?;
        if already_active {
            // A previous start may have persisted the active record before
            // answering its exact pending question. Retrying must finish
            // that durable question, without changing the original scope or
            // replaying the waiting run.
            self.answer_takeover_question(state, &request).await?;
            return Ok(json!({"takeover_request":request}));
        }
        request["state"] = json!("active");
        request["started_at"] = json!(now());
        self.write_takeover(&assignment_id, &request)?;
        self.answer_takeover_question(state, &request).await?;
        Ok(json!({"takeover_request":request}))
    }

    async fn takeover_release(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let bot_id = required_text(params, "bot_id")?;
        let (assignment_id, mut request) = self.takeover_for_action(params, "active")?;
        if request.get("bot_id").and_then(Value::as_str) != Some(bot_id.as_str()) {
            return Err(RpcError {
                code: "forbidden".into(),
                message: "takeover belongs to another bot".into(),
                details: None,
            });
        }
        request["state"] = json!("done");
        request["released_at"] = json!(now());
        if let Some(note) = params.get("note") {
            request["note"] = note.clone();
        }
        self.write_takeover(&assignment_id, &request)?;
        // A different active scope may still own the browser. Recompute from
        // durable records rather than unconditionally leaving the session in
        // bot mode after this one scope is released.
        self.apply_browser_takeover_reconciliation(state, &bot_id)
            .await?;
        self.answer_takeover_question(state, &request).await?;
        Ok(json!({"takeover_request":request}))
    }

    /// Starting the browser is the user's affirmative answer to the exact
    /// private takeover question. Close only that question after validating
    /// its Bot/chat/assignment scope; never sweep synthetic-scope questions.
    async fn answer_takeover_question(&self, state: &GatewayState, request: &Value) -> RpcResult {
        let Some(question_id) = request
            .get("question_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Ok(Value::Null);
        };
        let (Some(bot_id), Some(chat_id), Some(assignment_id)) = (
            request.get("bot_id").and_then(Value::as_str),
            request.get("chat_id").and_then(Value::as_str),
            request.get("assignment_id").and_then(Value::as_str),
        ) else {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "takeover question scope is incomplete".into(),
                details: None,
            });
        };
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let Some(question) = snapshot
            .get("questions")
            .and_then(Value::as_object)
            .and_then(|questions| questions.get(question_id))
        else {
            return Err(RpcError {
                code: "conflict".into(),
                message: "takeover question not found".into(),
                details: None,
            });
        };
        if question.get("bot_id").and_then(Value::as_str) != Some(bot_id)
            || question.get("chat_id").and_then(Value::as_str) != Some(chat_id)
            || question.get("assignment_id").and_then(Value::as_str) != Some(assignment_id)
        {
            return Err(RpcError {
                code: "conflict".into(),
                message: "takeover question scope mismatch".into(),
                details: None,
            });
        }
        if question.get("state").and_then(Value::as_str) != Some("pending") {
            return Ok(Value::Null);
        }
        let params = json!({
            "question_id": question_id,
            "option_index": 0,
            "client_request_id": format!("takeover:question-answer:{question_id}")
        });
        let result = self
            .orchestrator
            .rpc("question.answer", params.clone())
            .await
            .map_err(Self::error)?;
        self.persist(state, "question.answer", &params, &result)
            .await
    }

    /// Resolve the takeover selected by a user action.  The protocol only
    /// requires `bot_id` for `takeover.start` and `takeover.release`; an
    /// assignment id is accepted as a narrowing hint when a client has one.
    /// Looking up the durable request here keeps the action bound to the
    /// pending request created by the Bot and prevents arbitrary assignment
    /// ids from being used to drive another Bot's browser.
    fn takeover_for_action(
        &self,
        params: &Value,
        expected_state: &str,
    ) -> Result<(String, Value), RpcError> {
        if let Some(assignment_id) = params
            .get("assignment_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let request = self.read_takeover(assignment_id)?;
            if request.get("state").and_then(Value::as_str) != Some(expected_state) {
                return Err(RpcError {
                    code: "conflict".into(),
                    message: format!("takeover request is not {expected_state}"),
                    details: None,
                });
            }
            return Ok((assignment_id.to_owned(), request));
        }

        let dir = self.store.root().join("data/takeovers");
        let bot_id = required_text(params, "bot_id")?;
        let mut matches = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries {
                let entry = entry.map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                    continue;
                };
                let Some(request) = self
                    .store
                    .read_snapshot::<Value>(format!("data/takeovers/{stem}.json"))
                    .map_err(store_error)?
                else {
                    continue;
                };
                if request.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str())
                    && request.get("state").and_then(Value::as_str) == Some(expected_state)
                {
                    matches.push((stem.to_owned(), request));
                }
            }
        } else if dir.exists() {
            return Err(RpcError {
                code: "internal".into(),
                message: format!("cannot read {}", dir.display()),
                details: None,
            });
        }
        matches.sort_by(|left, right| {
            right.1["created_at"]
                .as_str()
                .cmp(&left.1["created_at"].as_str())
        });
        if let Some(match_item) = matches.into_iter().next() {
            return Ok(match_item);
        }
        if expected_state == "pending" {
            if let Some(recovered) = self.recover_waiting_takeover(&bot_id)? {
                return Ok(recovered);
            }
        }
        Err(RpcError {
            code: "not_found".into(),
            message: format!("no {expected_state} takeover request for bot {bot_id}"),
            details: None,
        })
    }

    /// Bind a private execution waiting marker to the browser action without
    /// replaying the model tool.  Older runtimes wrote the card and durable
    /// marker before the orchestrator takeover record, so only an exact
    /// waiting job + message + Bot DM scope may reconstruct that record.
    fn recover_waiting_takeover(&self, bot_id: &str) -> Result<Option<(String, Value)>, RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let Some(bot_chat_id) = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(bot_id))
            .and_then(|bot| bot.get("dm_chat_id"))
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        let waiting_dir = self.store.root().join("data/waiting");
        let entries = match fs::read_dir(waiting_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(store_error(error.into())),
        };
        let jobs_dir = self.store.root().join("data/jobs");
        let mut matches = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| store_error(error.into()))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(marker) = self
                .store
                .read_snapshot::<Value>(path.strip_prefix(self.store.root()).unwrap_or(&path))
                .map_err(store_error)?
            else {
                continue;
            };
            if marker.get("kind").and_then(Value::as_str) != Some("takeover")
                || marker
                    .get("assignment_id")
                    .is_some_and(|value| !value.is_null())
                || marker.get("chat_id").and_then(Value::as_str) != Some(bot_chat_id)
            {
                continue;
            }
            let Some(run_id) = marker.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let message_id = marker
                .get("message_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("msg_takeover_{}", takeover_component(run_id)));
            if takeover_component(run_id) != run_id
                || message_id != format!("msg_takeover_{}", run_id)
                || takeover_component(&message_id) != message_id
            {
                continue;
            }
            let Some(run_request) = self
                .store
                .read_snapshot::<Value>(format!("data/run_requests/{run_id}.json"))
                .map_err(store_error)?
            else {
                continue;
            };
            if run_request.get("run_id").and_then(Value::as_str) != Some(run_id)
                || run_request.get("bot_id").and_then(Value::as_str) != Some(bot_id)
                || run_request.get("chat_id").and_then(Value::as_str) != Some(bot_chat_id)
                || !run_request.get("assignment_id").is_none_or(Value::is_null)
            {
                continue;
            }
            let rows = self
                .store
                .read_jsonl::<Value>(format!(
                    "data/chats/{}/messages.jsonl",
                    takeover_component(bot_chat_id)
                ))
                .map_err(store_error)?;
            let Some(message) = rows.into_iter().rev().find(|message| {
                message.get("id").and_then(Value::as_str) == Some(message_id.as_str())
                    && message.get("chat_id").and_then(Value::as_str) == Some(bot_chat_id)
                    && message.pointer("/sender/bot_id").and_then(Value::as_str) == Some(bot_id)
            }) else {
                continue;
            };
            let Some(block) = message
                .get("blocks")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks.iter().find(|block| {
                        block.get("type").and_then(Value::as_str) == Some("takeover_request")
                            && block.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                            && block.get("state").and_then(Value::as_str) == Some("pending")
                    })
                })
            else {
                continue;
            };
            let mut exact_job = false;
            if let Ok(job_entries) = fs::read_dir(&jobs_dir) {
                for job_entry in job_entries {
                    let job_entry = job_entry.map_err(|error| store_error(error.into()))?;
                    if job_entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let file = fs::File::open(job_entry.path())
                        .map_err(|error| store_error(error.into()))?;
                    let job: macbot_durable::Job =
                        serde_json::from_reader(file).map_err(|error| RpcError {
                            code: "internal".into(),
                            message: error.to_string(),
                            details: None,
                        })?;
                    if matches!(
                        job.status,
                        macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
                    ) && job.owner == bot_id
                        && job.unsafe_replay
                        && job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id)
                        && job
                            .checkpoint
                            .pointer("/pending_tool/name")
                            .and_then(Value::as_str)
                            == Some("request_takeover")
                        && job
                            .checkpoint
                            .pointer("/pending_tool/args/reason")
                            .and_then(Value::as_str)
                            == block.get("reason").and_then(Value::as_str)
                    {
                        exact_job = true;
                        break;
                    }
                }
            }
            if !exact_job {
                continue;
            }
            let assignment_id = format!("dm_{}", takeover_component(bot_chat_id));
            matches.push((
                message
                    .get("created_at")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                assignment_id.clone(),
                json!({
                    "bot_id": bot_id,
                    "assignment_id": assignment_id,
                    "chat_id": bot_chat_id,
                    "group_chat_id": bot_chat_id,
                    "reason": block.get("reason").cloned().unwrap_or_else(|| json!("用户接管浏览器")),
                    "question_id": null,
                    "state": "pending",
                    "created_at": message.get("created_at").cloned().unwrap_or_else(|| json!(now())),
                    "message_id": message_id,
                    "run_id": run_id
                }),
            ));
        }
        matches.sort_by(|left, right| right.0.cmp(&left.0));
        Ok(matches
            .into_iter()
            .next()
            .map(|(_, assignment_id, request)| (assignment_id, request)))
    }

    fn read_takeover(&self, assignment_id: &str) -> RpcResult {
        self.store
            .read_snapshot(format!(
                "data/takeovers/{}.json",
                takeover_component(assignment_id)
            ))
            .map_err(store_error)?
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: "takeover request not found".into(),
                details: None,
            })
    }

    fn write_takeover(&self, assignment_id: &str, value: &Value) -> Result<(), RpcError> {
        self.store
            .write_snapshot(
                format!("data/takeovers/{}.json", takeover_component(assignment_id)),
                value,
            )
            .map_err(store_error)
    }

    fn read_proposal(&self, question_id: &str) -> Result<Option<Value>, RpcError> {
        self.store
            .read_snapshot(format!(
                "data/proposals/{}.json",
                takeover_component(question_id)
            ))
            .map_err(store_error)
    }

    fn write_proposal(&self, question_id: &str, value: &Value) -> Result<(), RpcError> {
        self.store
            .write_snapshot(
                format!("data/proposals/{}.json", takeover_component(question_id)),
                value,
            )
            .map_err(store_error)
    }

    fn finish_proposal_assignment(
        &self,
        assignment_id: &str,
        status: &str,
    ) -> Result<(), RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let current = snapshot
            .get("assignments")
            .and_then(|assignments| assignments.get(assignment_id))
            .and_then(|assignment| assignment.get("status"))
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: format!("proposal assignment {assignment_id} not found"),
                details: None,
            })?;
        if matches!(current, "done" | "failed" | "cancelled") {
            // A completed proposal's task may already have been terminalized
            // by run recovery. Keep that terminal state; never resurrect it.
            return Ok(());
        }
        self.orchestrator
            .finish_assignment(assignment_id, status)
            .map(|_| ())
            .map_err(Self::error)
    }

    async fn fail_answered_proposal(
        &self,
        state: &GatewayState,
        proposal: &mut Value,
        params: &Value,
        question_result: &Value,
        failure_code: &str,
        error: RpcError,
    ) -> RpcResult {
        let question_id = proposal
            .get("question_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "proposal question id is missing".into(),
                details: None,
            })?;
        let assignment_id = proposal
            .get("assignment_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "proposal assignment id is missing".into(),
                details: None,
            })?;
        if let Some(object) = proposal.as_object_mut() {
            object.insert("state".into(), json!("failed"));
            object.insert("failure_code".into(), json!(failure_code));
        }
        self.write_proposal(&question_id, proposal)?;
        self.finish_proposal_assignment(&assignment_id, "failed")?;
        // Persist the already accepted answer even though its secondary
        // proposal action failed. This closes the partial-write window that
        // previously left an answered Question with a pending proposal.
        self.persist_question_answer_recovery(state, params, question_result)
            .await?;
        Err(error)
    }

    async fn persist_question_answer_recovery(
        &self,
        state: &GatewayState,
        params: &Value,
        question_result: &Value,
    ) -> RpcResult {
        let mut recovery_params = params.clone();
        if let Some(object) = recovery_params.as_object_mut() {
            // Keep a failed proposal retryable with the caller's request id;
            // cache the Question result only after proposal reconciliation
            // reaches a terminal state.
            object.remove("client_request_id");
        }
        self.persist(state, "question.answer", &recovery_params, question_result)
            .await
    }

    async fn answer_proposal(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let question_id = params
            .get("question_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "question_id is required".into(),
                details: None,
            })?;
        let Some(mut proposal) = self.read_proposal(question_id)? else {
            return self
                .orchestrator
                .rpc("question.answer", params.clone())
                .await
                .map_err(Self::error);
        };
        if proposal.get("question_id").and_then(Value::as_str) != Some(question_id) {
            return Err(RpcError {
                code: "conflict".into(),
                message: "proposal is linked to a different Question".into(),
                details: None,
            });
        }
        let proposal_assignment_id = proposal
            .get("assignment_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let current_question = snapshot
            .get("questions")
            .and_then(|questions| questions.get(question_id))
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: format!("question {question_id} not found"),
                details: None,
            })?;
        if current_question.get("id").and_then(Value::as_str) != Some(question_id)
            || current_question
                .get("assignment_id")
                .and_then(Value::as_str)
                != proposal_assignment_id.as_deref()
        {
            return Err(RpcError {
                code: "conflict".into(),
                message: "proposal and Question assignment do not match".into(),
                details: None,
            });
        }

        let requested_approval = answer_request_approves(params);
        let proposal_state = proposal.get("state").and_then(Value::as_str).unwrap_or("");
        let mut create_params = if requested_approval || proposal_state == "approved" {
            let source = proposal
                .get("params")
                .and_then(Value::as_object)
                .ok_or_else(|| RpcError {
                    code: "internal".into(),
                    message: "proposal parameters are missing".into(),
                    details: None,
                })?;
            let mut create_params = Map::new();
            for key in [
                "name",
                "label",
                "description",
                "avatar",
                "model",
                "max_parallel",
                "tools",
                "browser_mode",
            ] {
                if let Some(value) = source.get(key) {
                    create_params.insert(key.into(), value.clone());
                }
            }
            create_params.insert(
                "client_request_id".into(),
                json!(format!("proposal:{question_id}")),
            );
            Some(Value::Object(create_params))
        } else {
            None
        };
        let create_request_id = format!("proposal:{question_id}");
        let previous_create = self
            .idempotency
            .lock()
            .await
            .get(&create_request_id)
            .cloned();
        if let (Some(create_params), None) = (create_params.as_ref(), previous_create.as_ref()) {
            let requested_name = create_params.get("name").and_then(Value::as_str);
            let name_conflict = requested_name.is_some_and(|name| {
                snapshot
                    .get("bots")
                    .and_then(Value::as_object)
                    .is_some_and(|bots| {
                        bots.values()
                            .any(|bot| bot.get("name").and_then(Value::as_str) == Some(name))
                    })
            });
            if name_conflict
                && current_question.get("state").and_then(Value::as_str) == Some("pending")
            {
                // Validate the deterministic create constraint before
                // accepting the user's answer. A duplicate name must leave
                // the exact Question, proposal, and waiting task untouched.
                return Err(RpcError {
                    code: "conflict".into(),
                    message: "a Bot with this name already exists".into(),
                    details: None,
                });
            }
        }

        let result = self
            .orchestrator
            .rpc("question.answer", params.clone())
            .await
            .map_err(Self::error)?;
        let answered_question = result.get("question").ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "question.answer returned no Question".into(),
            details: None,
        })?;
        if answered_question.get("id").and_then(Value::as_str) != Some(question_id) {
            return Err(RpcError {
                code: "conflict".into(),
                message: "question.answer returned a different Question".into(),
                details: None,
            });
        }
        let answer = answered_question
            .get("answer")
            .cloned()
            .unwrap_or(Value::Null);
        let approved = answer_request_approves(&answer);

        if proposal_state == "approved" || proposal_state == "rejected" {
            if approved != (proposal_state == "approved") {
                return Err(RpcError {
                    code: "conflict".into(),
                    message: "Question answer conflicts with the terminal proposal state".into(),
                    details: None,
                });
            }
            if proposal_state == "approved" {
                self.ensure_proposal_completion_message(state, question_id, &proposal)
                    .await?;
            }
            return Ok(result);
        }
        if proposal_state == "failed" {
            return Err(RpcError {
                code: "conflict".into(),
                message: "proposal could not be completed".into(),
                details: None,
            });
        }
        if proposal_state != "pending" {
            return Err(RpcError {
                code: "conflict".into(),
                message: "proposal is not answerable".into(),
                details: None,
            });
        }

        let assignment_id = proposal_assignment_id.as_deref().ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "proposal assignment is missing".into(),
            details: None,
        })?;
        if !approved {
            self.finish_proposal_assignment(assignment_id, "cancelled")?;
            if let Some(object) = proposal.as_object_mut() {
                object.insert("state".into(), json!("rejected"));
            }
            if let Err(error) = self.write_proposal(question_id, &proposal) {
                self.persist_question_answer_recovery(state, params, &result)
                    .await?;
                return Err(error);
            }
            return Ok(result);
        }

        if previous_create.is_none() {
            let requested_name = create_params
                .as_ref()
                .and_then(|params| params.get("name"))
                .and_then(Value::as_str);
            let name_conflict = requested_name.is_some_and(|name| {
                snapshot
                    .get("bots")
                    .and_then(Value::as_object)
                    .is_some_and(|bots| {
                        bots.values()
                            .any(|bot| bot.get("name").and_then(Value::as_str) == Some(name))
                    })
            });
            if name_conflict {
                let error = RpcError {
                    code: "conflict".into(),
                    message: "a Bot with this name already exists".into(),
                    details: None,
                };
                return self
                    .fail_answered_proposal(
                        state,
                        &mut proposal,
                        params,
                        &result,
                        "bot_name_conflict",
                        error,
                    )
                    .await;
            }
        }

        let created = if let Some(created) = previous_create {
            created
        } else {
            let create_params = create_params.take().ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "approved proposal has no Bot creation parameters".into(),
                details: None,
            })?;
            let mut created = match self
                .orchestrator
                .rpc("bot.create", create_params.clone())
                .await
            {
                Ok(created) => created,
                Err(error) => {
                    let error = Self::error(error);
                    return self
                        .fail_answered_proposal(
                            state,
                            &mut proposal,
                            params,
                            &result,
                            "bot_create_failed",
                            error,
                        )
                        .await;
                }
            };
            created = normalize_result("bot.create", created).map_err(|message| RpcError {
                code: "internal".into(),
                message,
                details: None,
            })?;
            self.enrich_bot_status(&mut created)
                .map_err(|message| RpcError {
                    code: "internal".into(),
                    message,
                    details: None,
                })?;
            validate_result("bot.create", &created).map_err(|message| RpcError {
                code: "internal".into(),
                message,
                details: None,
            })?;
            self.persist(state, "bot.create", &create_params, &created)
                .await?;
            created
        };

        self.finish_proposal_assignment(assignment_id, "done")?;
        if let Some(object) = proposal.as_object_mut() {
            object.insert("state".into(), json!("approved"));
            object.insert(
                "bot".into(),
                created.get("bot").cloned().unwrap_or(Value::Null),
            );
        }
        if let Err(error) = self.write_proposal(question_id, &proposal) {
            // The deterministic bot.create key lets a retry reuse the Bot and
            // complete this file write instead of creating or answering any
            // other pending Question.
            self.persist_question_answer_recovery(state, params, &result)
                .await?;
            return Err(error);
        }
        self.ensure_proposal_completion_message(state, question_id, &proposal)
            .await?;
        Ok(result)
    }

    async fn device_register(&self, params: &Value) -> RpcResult {
        let device: macbot_protocol::DeviceRegisterParams = serde_json::from_value(params.clone())
            .map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: error.to_string(),
                details: None,
            })?;
        let mut devices = self
            .store
            .read_snapshot::<Vec<Value>>("data/devices.json")
            .map_err(store_error)?
            .unwrap_or_default();
        let row = json!({"id":device.device_id,"platform":device.platform,"app_version":device.app_version,"device_name":device.device_name,"push_token":device.push_token,"last_seen_at":now()});
        devices.retain(|item| item.get("id") != Some(&json!(device.device_id)));
        devices.push(row.clone());
        self.store
            .write_snapshot("data/devices.json", &devices)
            .map_err(store_error)?;
        serde_json::from_value::<Device>(row.clone()).map_err(|error| RpcError {
            code: "internal".into(),
            message: error.to_string(),
            details: None,
        })?;
        Ok(json!({"device": row}))
    }

    async fn loop_resolve(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let root_message_id = required_text(params, "root_message_id")?;
        let action = required_text(params, "action")?;
        if !matches!(action.as_str(), "continue" | "end") {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "action must be continue or end".into(),
                details: None,
            });
        }
        let before = self.orchestrator.snapshot().map_err(Self::error)?;
        self.orchestrator
            .rpc("loop.resolve", params.clone())
            .await
            .map_err(Self::error)?;
        let after = self.orchestrator.snapshot().map_err(Self::error)?;
        let state_name = if action == "continue" {
            "continued"
        } else {
            "ended"
        };

        // The loop block is attached to the wire message returned by send_msg;
        // the compact orchestrator snapshot only retains the pre-normalized
        // message. Recover the durable wire row from the event log so the
        // update keeps the original message id and sequence.
        let event_message = self
            .store
            .events_since(0)
            .map_err(store_error)?
            .into_iter()
            .rev()
            .find_map(|event| {
                let message = event.data.get("message")?;
                let has_pause =
                    message
                        .get("blocks")
                        .and_then(Value::as_array)
                        .is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block.get("type").and_then(Value::as_str) == Some("loop_paused")
                                    && block.get("root_message_id").and_then(Value::as_str)
                                        == Some(root_message_id.as_str())
                            })
                        });
                has_pause.then(|| message.clone())
            });
        let previous_message = event_message.or_else(|| {
            before
                .get("messages")
                .and_then(Value::as_object)
                .and_then(|messages| messages.get(&root_message_id))
                .cloned()
        });
        if let Some(mut message) = previous_message {
            let mut changed = false;
            if let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("loop_paused")
                        && block.get("root_message_id").and_then(Value::as_str)
                            == Some(root_message_id.as_str())
                        && block.get("state").and_then(Value::as_str) != Some(state_name)
                    {
                        block["state"] = json!(state_name);
                        changed = true;
                    }
                }
            }
            if changed {
                normalize_message(&mut message);
                let canonical = self.persist_client_message(&message)?;
                let data = json!({"message": canonical});
                let event = self
                    .store
                    .append_event("message.updated", data.clone())
                    .map_err(store_error)?;
                state.publish_event(event.seq, &event.event, data).await;
            }
        }

        let previous_ids = before
            .get("assignments")
            .and_then(Value::as_object)
            .map(|items| items.keys().collect::<std::collections::HashSet<_>>())
            .unwrap_or_default();
        if let Some(assignments) = after.get("assignments").and_then(Value::as_object) {
            for (id, value) in assignments {
                if previous_ids.contains(id) {
                    continue;
                }
                let mut assignment = value.clone();
                normalize_assignment(&mut assignment);
                let data = json!({"assignment": assignment});
                let event = self
                    .store
                    .append_event("assignment.created", data.clone())
                    .map_err(store_error)?;
                state.publish_event(event.seq, &event.event, data).await;
                self.ensure_assignment_cards(state, &assignment, "assignment.create")
                    .await?;
            }
        }
        Ok(json!({}))
    }

    async fn assignment_stop(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let assignment_id = required_text(params, "assignment_id")?;
        let before = self
            .orchestrator
            .rpc("assignment.get", json!({"assignment_id":assignment_id}))
            .await
            .map_err(Self::error)?;
        let was_cancelled = before["assignment"]["status"].as_str() == Some("cancelled");
        let assignment = self
            .orchestrator
            .finish_assignment(&assignment_id, "cancelled")
            .map_err(Self::error)?;
        if !was_cancelled {
            self.publish_task_stopped_message(state, &assignment_id)
                .await?;
        }
        Ok(json!({"assignment":assignment}))
    }

    async fn assignment_retry(&self, _state: &GatewayState, params: &Value) -> RpcResult {
        let assignment_id = required_text(params, "assignment_id")?;
        let client_request_id = required_text(params, "client_request_id")?;
        let assignment = self
            .orchestrator
            .retry_assignment(&assignment_id, Some(&client_request_id))
            .map_err(Self::error)?;
        Ok(json!({"assignment":assignment}))
    }

    async fn publish_task_stopped_message(
        &self,
        state: &GatewayState,
        assignment_id: &str,
    ) -> RpcResult {
        let message = self
            .orchestrator
            .create_task_stopped_message(assignment_id)
            .map_err(Self::error)?;
        let mut data = serde_json::to_value(message).map_err(|error| RpcError {
            code: "internal".into(),
            message: error.to_string(),
            details: None,
        })?;
        normalize_message(&mut data);
        let data = self.persist_client_message(&data)?;
        if let Some(event) = self.append_repaired_event(
            &format!("task-stopped-message:{assignment_id}"),
            "message.created",
            json!({"message":data}),
        )? {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(json!({}))
    }

    async fn duplicate_bot_with_skills(&self, params: &Value) -> RpcResult {
        let feature_service = FeatureService::with_store(
            self.store.clone(),
            self.store.root().to_path_buf(),
            Vec::<std::path::PathBuf>::new(),
        )
        .map_err(|error| RpcError {
            code: error.code().into(),
            message: error.to_string(),
            details: None,
        })?;
        self.duplicate_bot_with_feature_service(params, Arc::new(feature_service))
            .await
    }

    /// Duplicate a Bot while sharing the process-wide FeatureService used by
    /// model runs. ComposedBackend can call this entry point so its in-memory
    /// skill registry observes the same copy and rollback immediately.
    async fn duplicate_bot_with_feature_service_unpersisted(
        &self,
        params: &Value,
        feature_service: Arc<FeatureService>,
    ) -> Result<(Value, String, BotSkillSettingsSnapshot), RpcError> {
        let source_bot_id = required_text(params, "bot_id")?;
        let name = required_text(params, "name")?;
        let target_bot_id = params
            .get("target_bot_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        if target_bot_id == "main" || target_bot_id.contains('/') {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "target_bot_id must be a non-main id".into(),
                details: None,
            });
        }

        let skill_snapshot = feature_service
            .prepare_duplicate_bot_skill_settings(&source_bot_id, &target_bot_id)
            .map_err(|error| RpcError {
                code: error.code().into(),
                message: error.to_string(),
                details: None,
            })?;

        let mut duplicate_params = params.clone();
        let object = duplicate_params.as_object_mut().ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: "params must be an object".into(),
            details: None,
        })?;
        object.insert("bot_id".into(), json!(source_bot_id));
        object.insert("name".into(), json!(name));
        object.insert("target_bot_id".into(), json!(target_bot_id));
        let result = match self
            .orchestrator
            .rpc("bot.duplicate", duplicate_params.clone())
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let _ = feature_service.restore_duplicate_bot_skill_settings(&skill_snapshot);
                return Err(Self::error(error));
            }
        };
        let mut result = match normalize_result("bot.duplicate", result) {
            Ok(result) => result,
            Err(message) => {
                self.rollback_duplicate_bot(&target_bot_id, &feature_service, &skill_snapshot)
                    .await;
                return Err(RpcError {
                    code: "internal".into(),
                    message,
                    details: None,
                });
            }
        };
        if let Err(message) = self.enrich_bot_status(&mut result) {
            self.rollback_duplicate_bot(&target_bot_id, &feature_service, &skill_snapshot)
                .await;
            return Err(RpcError {
                code: "internal".into(),
                message,
                details: None,
            });
        }
        if let Err(message) = validate_result("bot.duplicate", &result) {
            self.rollback_duplicate_bot(&target_bot_id, &feature_service, &skill_snapshot)
                .await;
            return Err(RpcError {
                code: "internal".into(),
                message,
                details: None,
            });
        }
        Ok((result, target_bot_id, skill_snapshot))
    }

    /// Public non-dispatching helper used by gateway tests and internal callers.
    /// The regular RPC dispatcher persists after this returns; callers that
    /// bypass it must use `duplicate_bot_with_feature_service_and_state`.
    pub async fn duplicate_bot_with_feature_service(
        &self,
        params: &Value,
        feature_service: Arc<FeatureService>,
    ) -> RpcResult {
        let (result, _target_bot_id, _skill_snapshot) = self
            .duplicate_bot_with_feature_service_unpersisted(params, feature_service)
            .await?;
        Ok(result)
    }

    /// ComposedBackend entry point for Bot duplication.  The regular RPC path
    /// persists in `call` after this helper returns; the composed path bypasses
    /// that dispatcher, so it must take the writer lock and use the same
    /// operation/event persistence path here.
    pub async fn duplicate_bot_with_feature_service_and_state(
        &self,
        state: &GatewayState,
        params: &Value,
        feature_service: Arc<FeatureService>,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        if let Some(request_id) = params.get("client_request_id").and_then(Value::as_str) {
            if let Some(value) = self.idempotency.lock().await.get(request_id).cloned() {
                return Ok(value);
            }
        }
        let (result, target_bot_id, skill_snapshot) = self
            .duplicate_bot_with_feature_service_unpersisted(params, feature_service.clone())
            .await?;
        match self.persist(state, "bot.duplicate", params, &result).await {
            Ok(result) => Ok(result),
            Err(error) => {
                self.rollback_duplicate_bot(&target_bot_id, &feature_service, &skill_snapshot)
                    .await;
                if let Err(compensation_error) = self
                    .persist_duplicate_rollback_snapshot(
                        &target_bot_id,
                        params.get("client_request_id").and_then(Value::as_str),
                    )
                    .await
                {
                    tracing::error!(
                        %compensation_error,
                        target_bot_id,
                        "failed to persist duplicate rollback snapshot"
                    );
                }
                Err(error)
            }
        }
    }

    /// The initial operation WAL record can outlive a failed snapshot rename.
    /// Append a non-`done` compensation record with the post-rollback snapshot
    /// so restart chooses the clean snapshot and never reconstructs the target.
    async fn persist_duplicate_rollback_snapshot(
        &self,
        target_bot_id: &str,
        client_request_id: Option<&str>,
    ) -> Result<(), RpcError> {
        self.persist_orchestrator(json!({
            "method":"bot.duplicate.rollback",
            "params":{"target_bot_id":target_bot_id},
            "client_request_id":client_request_id,
            "result":{"rolled_back":true},
            "status":"rolled_back",
            "at":now()
        }))
        .await
        .map(|_| ())
    }

    async fn rollback_duplicate_bot(
        &self,
        target_bot_id: &str,
        feature_service: &FeatureService,
        skill_snapshot: &BotSkillSettingsSnapshot,
    ) {
        let _ = feature_service.restore_duplicate_bot_skill_settings(skill_snapshot);
        if let Ok(routines) = self
            .orchestrator
            .rpc("routine.list", json!({"bot_id":target_bot_id}))
            .await
        {
            if let Some(items) = routines.get("routines").and_then(Value::as_array) {
                for routine in items {
                    if let Some(id) = routine.get("id").and_then(Value::as_str) {
                        let _ = self
                            .orchestrator
                            .rpc("routine.delete", json!({"routine_id":id}))
                            .await;
                    }
                }
            }
        }
        let _ = self
            .orchestrator
            .rpc("bot.delete", json!({"bot_id":target_bot_id}))
            .await;
    }

    pub fn execution_validate_send_msg_target(&self, params: &Value) -> Result<(), RpcError> {
        let mut resolved = params.clone();
        self.prepare_bot_dm_send(&mut resolved)?;
        let chat_id = resolved
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                crate::rpc_error("invalid_params", "send_msg.chat_id is required", None)
            })?;
        let bot_id = resolved
            .get("bot_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                crate::rpc_error("invalid_params", "send_msg.bot_id is required", None)
            })?;
        self.orchestrator
            .validate_send_msg_target(chat_id, bot_id)
            .map_err(Self::error)
    }

    fn prepare_bot_dm_send(&self, params: &mut Value) -> Result<Option<BotDmContext>, RpcError> {
        let source_bot_id = params
            .get("bot_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "send_msg.bot_id is required for Bot DM".into(),
                details: None,
            })?
            .to_owned();
        let assignment_id = params
            .get("assignment_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let source_chat_id = params
            .get("_send_scope_chat_id")
            .and_then(Value::as_str)
            .or_else(|| params.get("chat_id").and_then(Value::as_str))
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                crate::rpc_error(
                    "invalid_params",
                    "send_msg execution chat is required",
                    None,
                )
            })?
            .to_owned();
        let scope_chat_id = self
            .orchestrator
            .resolve_send_scope(assignment_id, &source_bot_id, &source_chat_id)
            .map_err(Self::error)?;
        if params
            .get("chat_id")
            .and_then(Value::as_str)
            .is_some_and(|requested| requested != scope_chat_id)
        {
            return Err(crate::rpc_error(
                "forbidden",
                "send_msg cannot override its execution chat",
                None,
            ));
        }

        let to = params.get("to").cloned();
        let mut bot_dm_context = None;
        let destination = match to {
            None => scope_chat_id.clone(),
            Some(Value::String(to)) if to == "task_chat" => scope_chat_id.clone(),
            Some(Value::String(to)) if to == "user" => {
                if self.is_project_send_scope(params, &scope_chat_id)? {
                    return Err(crate::rpc_error(
                        "forbidden",
                        "a group incarnation cannot target a private chat",
                        None,
                    ));
                }
                // `user` is retained for older prompts. Its canonical private
                // destination is still the scope assigned to this run.
                scope_chat_id.clone()
            }
            Some(Value::String(to)) if to == scope_chat_id => scope_chat_id.clone(),
            Some(Value::Object(to)) => {
                let target_chat_id = to
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty());
                let target_bot_id = to
                    .get("bot")
                    .or_else(|| to.get("bot_id"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty());
                if let Some(target_chat_id) = target_chat_id {
                    if target_chat_id != scope_chat_id {
                        return Err(crate::rpc_error(
                            "forbidden",
                            "send_msg cannot target another chat incarnation",
                            None,
                        ));
                    }
                    scope_chat_id.clone()
                } else if let Some(target_bot_id) = target_bot_id {
                    if target_bot_id == source_bot_id {
                        // Addressing the current Bot is a shorthand for this
                        // execution incarnation's immutable scope. In
                        // particular, a group assignment must never escape
                        // its bound project chat through a Bot-DM route.
                        scope_chat_id.clone()
                    } else {
                        bot_dm_context = Some(self.resolve_bot_dm_context(
                            params,
                            &source_bot_id,
                            &scope_chat_id,
                            target_bot_id,
                        )?);
                        bot_dm_context
                            .as_ref()
                            .expect("Bot DM context was just resolved")
                            .route
                            .chat_id
                            .clone()
                    }
                } else {
                    return Err(crate::rpc_error(
                        "invalid_params",
                        "send_msg.to must identify the bound chat or a bot_id",
                        None,
                    ));
                }
            }
            Some(Value::String(to)) => {
                let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
                let is_bot_id = snapshot
                    .get("bots")
                    .and_then(Value::as_object)
                    .is_some_and(|bots| bots.contains_key(&to));
                if !is_bot_id {
                    return Err(crate::rpc_error(
                        "forbidden",
                        "send_msg cannot target another private or group chat",
                        None,
                    ));
                }
                if to == source_bot_id {
                    // String-form Bot ids follow the same current-scope
                    // semantics as `{bot: current_bot_id}`.
                    scope_chat_id.clone()
                } else {
                    bot_dm_context = Some(self.resolve_bot_dm_context(
                        params,
                        &source_bot_id,
                        &scope_chat_id,
                        &to,
                    )?);
                    bot_dm_context
                        .as_ref()
                        .expect("Bot DM context was just resolved")
                        .route
                        .chat_id
                        .clone()
                }
            }
            Some(_) => {
                return Err(crate::rpc_error(
                    "invalid_params",
                    "send_msg.to must be task_chat, user, a bound chat id, or {bot_id}",
                    None,
                ));
            }
        };

        if let Some(object) = params.as_object_mut() {
            object.insert("chat_id".into(), json!(destination));
            object.insert("_send_scope_chat_id".into(), json!(scope_chat_id));
            object.remove("to");
        }
        Ok(bot_dm_context)
    }

    fn resolve_bot_dm_context(
        &self,
        params: &Value,
        source_bot_id: &str,
        source_chat_id: &str,
        target_bot_id: &str,
    ) -> Result<BotDmContext, RpcError> {
        if self.is_project_send_scope(params, source_chat_id)? {
            return Err(crate::rpc_error(
                "forbidden",
                "a group incarnation can only send to its bound group chat",
                None,
            ));
        }
        let route = self
            .orchestrator
            .bot_dm_route(source_bot_id, target_bot_id)
            .map_err(Self::error)?;
        Ok(BotDmContext {
            source_chat_id: source_chat_id.to_owned(),
            source_bot_id: source_bot_id.to_owned(),
            route,
        })
    }

    fn is_project_send_scope(
        &self,
        params: &Value,
        source_chat_id: &str,
    ) -> Result<bool, RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        if let Some(project_id) = params
            .get("assignment_id")
            .and_then(Value::as_str)
            .and_then(|assignment_id| {
                snapshot
                    .get("assignments")?
                    .get(assignment_id)?
                    .get("project_id")
                    .and_then(Value::as_str)
            })
        {
            if snapshot
                .get("projects")
                .and_then(Value::as_object)
                .and_then(|projects| projects.get(project_id))
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(snapshot
            .get("projects")
            .and_then(Value::as_object)
            .is_some_and(|projects| {
                projects.values().any(|project| {
                    project.get("chat_id").and_then(Value::as_str) == Some(source_chat_id)
                })
            }))
    }

    fn ensure_bot_dm_chat(&self, route: &BotDmRoute) -> Result<bool, RpcError> {
        let path = format!(
            "data/chats/{}/metadata.json",
            takeover_component(&route.chat_id)
        );
        if let Some(existing) = self
            .store
            .read_snapshot::<Value>(&path)
            .map_err(store_error)?
        {
            if existing.get("kind").and_then(Value::as_str) != Some("bot_dm") {
                return Err(RpcError {
                    code: "conflict".into(),
                    message: format!("chat {} is not a bot_dm route", route.chat_id),
                    details: None,
                });
            }
            let event_exists = self
                .store
                .events_since(0)
                .map_err(store_error)?
                .into_iter()
                .any(|event| {
                    event.event == "chat.created" && event.data["chat"]["id"] == route.chat_id
                });
            return Ok(!event_exists);
        }
        self.store
            .write_snapshot(
                path,
                &json!({
                    "id":route.chat_id,
                    "kind":"bot_dm",
                    "title":route.title,
                    "bot_id":null,
                    "project_id":null,
                    "member_bot_ids":route.member_bot_ids,
                    "read_only":route.read_only,
                    "created_at":now()
                }),
            )
            .map_err(store_error)?;
        Ok(true)
    }

    async fn publish_bot_dm_chat(
        &self,
        state: &GatewayState,
        route: &BotDmRoute,
    ) -> Result<(), RpcError> {
        let chats = self.chat_list().await?;
        let chat = chats["chats"]
            .as_array()
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("id").and_then(Value::as_str) == Some(route.chat_id.as_str())
                })
            })
            .cloned()
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "new bot_dm chat is missing from chat.list".into(),
                details: None,
            })?;
        let data = json!({"chat":chat});
        let event = self
            .store
            .append_event("chat.created", data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        Ok(())
    }

    async fn ensure_bot_dm_ref(
        &self,
        state: &GatewayState,
        context: &BotDmContext,
        message: &Value,
        params: &Value,
    ) -> Result<(), RpcError> {
        let target_message = message.get("message").unwrap_or(message);
        let message_id = target_message
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "Bot DM message has no id".into(),
                details: None,
            })?;
        let count = self.load_chat_messages(&context.route.chat_id)?.len() as u64;
        let ref_id = format!("msg_bot_dm_ref_{message_id}");
        let existing = self
            .load_chat_messages(&context.source_chat_id)?
            .into_iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(ref_id.as_str()));
        let event_name = if existing.is_some() {
            "message.updated"
        } else {
            "message.created"
        };
        let card = json!({
            "id":ref_id,
            "chat_id":context.source_chat_id,
            "seq":0,
            "sender":{"kind":"bot","bot_id":context.source_bot_id},
            "created_at":target_message.get("created_at").cloned().unwrap_or_else(|| json!(now())),
            "edited_at":null,
            "deleted":false,
            "reply_to":null,
            "thread_count":0,
            "mentions":[],
            "blocks":[{"type":"bot_dm_ref","chat_id":context.route.chat_id,"count":count}],
            "fallback_text":format!("✉ Bot 私信了 {}", context.route.title),
            "intent":null,
            "assignment_id":params.get("assignment_id").cloned().unwrap_or(Value::Null),
            "streaming":false,
            "delivery":[],
            "reactions":[]
        });
        let canonical = self.persist_client_message(&card)?;
        let event_exists = self
            .store
            .events_since(0)
            .map_err(store_error)?
            .into_iter()
            .rev()
            .find(|event| {
                matches!(event.event.as_str(), "message.created" | "message.updated")
                    && event.data["message"]["id"] == ref_id
            })
            .is_some_and(|event| event.data["message"]["blocks"] == canonical["blocks"]);
        if event_exists {
            return Ok(());
        }
        let data = json!({"message":canonical});
        let event = self
            .store
            .append_event(event_name, data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        Ok(())
    }

    async fn chat_send(&self, params: Value) -> RpcResult {
        let chat_id = params
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "chat_id is required".into(),
                details: None,
            })?;
        if self.is_read_only_bot_dm(chat_id)? {
            return Err(RpcError {
                code: "forbidden".into(),
                message: "bot_dm chats are read-only".into(),
                details: None,
            });
        }
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "text is required".into(),
                details: None,
            })?;
        let attachment_blocks = self.attachment_blocks(&params)?;
        let mut mentions = self.expand_chat_mentions(chat_id, &params).await?;
        let quote_id = params
            .get("quote")
            .or_else(|| params.get("reply_to"))
            .and_then(Value::as_str);
        if quote_id.is_some() {
            if let Some(mode) = params.get("quote_mode").and_then(Value::as_str) {
                if !matches!(mode, "reply" | "reference") {
                    return Err(RpcError {
                        code: "invalid_argument".into(),
                        message: "quote_mode must be reply or reference".into(),
                        details: None,
                    });
                }
            }
        }
        let quote = if let Some(quote_id) = quote_id {
            let messages = self.load_chat_messages(chat_id)?;
            let source = messages
                .iter()
                .find(|message| message.get("id").and_then(Value::as_str) == Some(quote_id))
                .ok_or_else(|| RpcError {
                    code: "not_found".into(),
                    message: "quoted message not found in this chat".into(),
                    details: None,
                })?;
            let sender = source
                .get("sender")
                .cloned()
                .unwrap_or_else(|| json!({"kind":"system"}));
            let quote_mode = params
                .get("quote_mode")
                .and_then(Value::as_str)
                .unwrap_or("reply");
            if quote_mode == "reply" && sender.get("kind").and_then(Value::as_str) == Some("bot") {
                let bot_id = sender.get("bot_id").and_then(Value::as_str).unwrap_or("");
                if !bot_id.is_empty()
                    && !mentions.iter().any(|mention| {
                        mention.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                    })
                {
                    mentions.push(json!({"kind":"bot","bot_id":bot_id,"instruction":null}));
                }
            }
            let excerpt = source
                .get("fallback_text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(120)
                .collect::<String>();
            Some(json!({"message_id":quote_id,"sender":sender,"excerpt":excerpt}))
        } else {
            None
        };
        let mut send = json!({"bot_id":"user","chat_id":chat_id,"text":text,"intent":"ack","mentions":mentions});
        if let Some(id) = params.get("client_request_id") {
            send["client_request_id"] = id.clone();
        }
        let message = self
            .orchestrator
            .rpc("send_msg", send)
            .await
            .map_err(ProductionBackend::error)?;
        let mut result = json!({"message": message});
        result["message"]["reply_to"] = Value::Null;
        result["message"]["quote"] = quote.unwrap_or(Value::Null);
        if !attachment_blocks.is_empty() {
            let attachment_only = text.trim().is_empty();
            let fallback_text = attachment_blocks
                .iter()
                .filter_map(|block| block.pointer("/file/name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            let mut blocks =
                Vec::with_capacity(usize::from(!attachment_only) + attachment_blocks.len());
            if !attachment_only {
                blocks.push(json!({"type":"text","markdown":text}));
            }
            blocks.extend(attachment_blocks);
            result["message"]["blocks"] = Value::Array(blocks);
            if attachment_only {
                result["message"]["fallback_text"] = json!(fallback_text);
            }
        }
        Ok(result)
    }

    async fn expand_chat_mentions(
        &self,
        chat_id: &str,
        params: &Value,
    ) -> Result<Vec<Value>, RpcError> {
        let Some(items) = params.get("mentions").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut mentions = Vec::new();
        for item in items {
            let kind = item
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError {
                    code: "invalid_params".into(),
                    message: "mention.kind is required".into(),
                    details: None,
                })?;
            match kind {
                "bot" => {
                    let bot_id = item
                        .get("bot_id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| RpcError {
                            code: "invalid_params".into(),
                            message: "bot mention requires bot_id".into(),
                            details: None,
                        })?;
                    mentions.push(json!({
                        "bot_id": bot_id,
                        "instruction": item.get("instruction").cloned().unwrap_or(Value::Null)
                    }));
                }
                "main" => mentions.push(json!("main")),
                "everyone" => {
                    let chat = self
                        .chat_list()
                        .await?
                        .get("chats")
                        .and_then(Value::as_array)
                        .and_then(|chats| {
                            chats.iter().find(|chat| {
                                chat.get("id").and_then(Value::as_str) == Some(chat_id)
                            })
                        })
                        .cloned()
                        .ok_or_else(|| RpcError {
                            code: "not_found".into(),
                            message: format!("chat {chat_id} not found"),
                            details: None,
                        })?;
                    for member in chat
                        .get("member_bot_ids")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        if member == "main" {
                            mentions.push(json!("main"));
                        } else {
                            mentions.push(json!({"bot_id":member,"instruction":null}));
                        }
                    }
                }
                "user" => {
                    return Err(RpcError {
                        code: "invalid_params".into(),
                        message: "user mentions are not allowed by the protocol".into(),
                        details: None,
                    });
                }
                other => {
                    return Err(RpcError {
                        code: "invalid_params".into(),
                        message: format!("unsupported mention kind {other}"),
                        details: None,
                    });
                }
            }
        }
        Ok(mentions)
    }

    pub(crate) fn load_chat_messages(&self, chat_id: &str) -> Result<Vec<Value>, RpcError> {
        self.load_chat_messages_with_durable_ids(chat_id)
            .map(|(messages, _)| messages)
    }

    fn load_chat_messages_with_durable_ids(
        &self,
        chat_id: &str,
    ) -> Result<(Vec<Value>, HashSet<String>), RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let mut messages = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|items| items.values())
            .filter(|item| item.get("chat_id").and_then(Value::as_str) == Some(chat_id))
            .cloned()
            .collect::<Vec<_>>();
        let persisted = self
            .store
            .read_jsonl::<Value>(format!(
                "data/chats/{}/messages.jsonl",
                takeover_component(chat_id)
            ))
            .map_err(store_error)?;
        let mut durable_ids = HashSet::new();
        for message in persisted {
            if message.get("chat_id").and_then(Value::as_str) != Some(chat_id) {
                continue;
            }
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                durable_ids.insert(id.to_owned());
            }
            if let Some(existing) = messages
                .iter_mut()
                .find(|item| item.get("id") == message.get("id"))
            {
                *existing = message;
            } else {
                messages.push(message);
            }
        }
        // Normalize legacy rows on read as well as on write. Older writers
        // could leave a non-empty text block with an empty markdown value;
        // folding it here repairs history and persists the canonical row via
        // sequence_chat_messages without changing message ids or seqs.
        for message in &mut messages {
            // Persisted wire rows override compact snapshot rows. Retain the
            // internal decision/question association restored by state.
            if let Some(question_id) = message
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| snapshot.get("messages").and_then(|items| items.get(id)))
                .and_then(|item| item.get("question_id"))
                .filter(|id| id.is_string())
            {
                message["question_id"] = question_id.clone();
            }
            normalize_message(message);
        }
        messages.sort_by(|left, right| {
            left.get("created_at")
                .and_then(Value::as_str)
                .cmp(&right.get("created_at").and_then(Value::as_str))
                .then_with(|| {
                    left.get("seq")
                        .and_then(Value::as_u64)
                        .cmp(&right.get("seq").and_then(Value::as_u64))
                })
                .then_with(|| {
                    left.get("id")
                        .and_then(Value::as_str)
                        .cmp(&right.get("id").and_then(Value::as_str))
                })
        });
        Ok((messages, durable_ids))
    }

    fn attachment_blocks(&self, params: &Value) -> Result<Vec<Value>, RpcError> {
        let Some(attachments) = params.get("attachments") else {
            return Ok(Vec::new());
        };
        let attachments = attachments.as_array().ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: "attachments must be an array".into(),
            details: None,
        })?;
        attachments
            .iter()
            .map(|attachment| {
                let id = attachment.as_str().ok_or_else(|| RpcError {
                    code: "invalid_params".into(),
                    message: "attachment ids must be strings".into(),
                    details: None,
                })?;
                let metadata = self
                    .store
                    .read_snapshot::<Value>(format!(
                        "data/uploads/{}.json",
                        takeover_component(id)
                    ))
                    .map_err(store_error)?;
                let file = metadata
                    .as_ref()
                    .and_then(|value| value.get("file").or(Some(value)))
                    .cloned()
                    .ok_or_else(|| RpcError {
                        code: "not_found".into(),
                        message: format!("upload {id} metadata not found"),
                        details: None,
                    })?;
                Ok(json!({
                    "type":"file",
                    "file": {
                        "root":"upload",
                        "root_id":id,
                        "path":"",
                        "name":file.get("name").and_then(Value::as_str).unwrap_or(id),
                        "size":file.get("size").and_then(Value::as_u64).unwrap_or(0),
                        "mime":file.get("mime").and_then(Value::as_str).unwrap_or("application/octet-stream")
                    }
                }))
            })
            .collect()
    }

    fn persist_client_message(&self, message: &Value) -> Result<Value, RpcError> {
        let chat_id = message
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "chat message has no chat_id".into(),
                details: None,
            })?;
        self.persist_client_message_with_messages(
            chat_id,
            self.load_chat_messages(chat_id)?,
            message,
        )
    }

    /// Persist a successfully loaded explicit skill on its originating user
    /// message. This update is independent of the message's delivery state.
    pub(crate) async fn record_loaded_skill(
        &self,
        state: &GatewayState,
        chat_id: &str,
        message_id: &str,
        loaded_skill: Value,
    ) -> Result<(), RpcError> {
        let _writer = self.write_lock.lock().await;
        let mut message = self
            .load_chat_messages(chat_id)?
            .into_iter()
            .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: "loaded skill source message not found".into(),
                details: None,
            })?;
        if message.pointer("/sender/kind").and_then(Value::as_str) != Some("user") {
            return Err(RpcError {
                code: "forbidden".into(),
                message: "loaded skills can only be recorded on user messages".into(),
                details: None,
            });
        }
        let records = message
            .get_mut("loaded_skills")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "message loaded_skills must be an array".into(),
                details: None,
            })?;
        let duplicate = records.iter().any(|record| {
            record.get("name") == loaded_skill.get("name")
                && record.get("bot_id") == loaded_skill.get("bot_id")
                && record.get("assignment_id") == loaded_skill.get("assignment_id")
        });
        if duplicate {
            return Ok(());
        }
        records.push(loaded_skill);
        let canonical = self.persist_client_message(&message)?;
        let data = json!({"message":canonical});
        let event = self
            .store
            .append_event("message.updated", data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        Ok(())
    }

    fn persist_attention_messages(&self, messages: Vec<Value>) -> Result<Vec<Value>, RpcError> {
        if messages.is_empty() {
            return Ok(messages);
        }
        let mut chat_messages: HashMap<String, (Vec<Value>, HashSet<String>)> = HashMap::new();
        let mut dirty_chats = HashSet::new();
        let mut message_ids = HashSet::new();
        for message in &messages {
            let chat_id = message
                .get("chat_id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError {
                    code: "internal".into(),
                    message: "chat message has no chat_id".into(),
                    details: None,
                })?;
            let message_id = message
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError {
                    code: "internal".into(),
                    message: "chat message has no id".into(),
                    details: None,
                })?;
            message_ids.insert(message_id.to_owned());
            if !chat_messages.contains_key(chat_id) {
                chat_messages.insert(
                    chat_id.to_owned(),
                    self.load_chat_messages_with_durable_ids(chat_id)?,
                );
            }
            let (rows, durable_ids) = chat_messages
                .get_mut(chat_id)
                .expect("chat message cache inserted above");
            if let Some(existing) = rows
                .iter_mut()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(message_id))
            {
                let sequence_missing = existing
                    .get("seq")
                    .and_then(Value::as_u64)
                    .is_none_or(|seq| seq == 0);
                if !durable_ids.contains(message_id)
                    || sequence_missing
                    || !Self::messages_match_without_seq(existing, message)
                {
                    *existing = message.clone();
                    dirty_chats.insert(chat_id.to_owned());
                }
            } else {
                rows.push(message.clone());
                dirty_chats.insert(chat_id.to_owned());
            }
        }

        let mut canonical_by_id = chat_messages
            .values()
            .flat_map(|(rows, _)| rows.iter())
            .filter_map(|message| {
                let id = message.get("id").and_then(Value::as_str)?;
                message_ids
                    .contains(id)
                    .then(|| (id.to_owned(), message.clone()))
            })
            .collect::<HashMap<_, _>>();
        for chat_id in dirty_chats {
            let (rows, _) = chat_messages
                .remove(&chat_id)
                .expect("dirty chat remains in message cache");
            for message in self.sequence_chat_messages(&chat_id, rows)? {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    if message_ids.contains(id) {
                        canonical_by_id.insert(id.to_owned(), message);
                    }
                }
            }
        }
        messages
            .into_iter()
            .map(|message| {
                let id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError {
                        code: "internal".into(),
                        message: "chat message has no id".into(),
                        details: None,
                    })?;
                canonical_by_id.remove(id).ok_or_else(|| RpcError {
                    code: "internal".into(),
                    message: "attention message disappeared".into(),
                    details: None,
                })
            })
            .collect()
    }

    fn messages_match_without_seq(left: &Value, right: &Value) -> bool {
        let mut left = left.clone();
        let mut right = right.clone();
        if let Some(object) = left.as_object_mut() {
            object.remove("seq");
        }
        if let Some(object) = right.as_object_mut() {
            object.remove("seq");
        }
        left == right
    }

    fn persist_client_message_with_messages(
        &self,
        chat_id: &str,
        mut messages: Vec<Value>,
        message: &Value,
    ) -> Result<Value, RpcError> {
        let mut wire = message.clone();
        normalize_message(&mut wire);
        let message_id = wire.get("id").cloned();
        if let Some(existing) = messages
            .iter_mut()
            .find(|item| item.get("id") == message_id.as_ref())
        {
            *existing = wire;
        } else {
            messages.push(wire);
        }
        let canonical = self.sequence_chat_messages(chat_id, messages)?;
        canonical
            .into_iter()
            .find(|item| item.get("id") == message_id.as_ref())
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "sequenced chat message disappeared".into(),
                details: None,
            })
    }

    /// Ensure every newly-created project is represented by one durable card
    /// in the main conversation.  The project id is the message id key, so a
    /// retry or restart updates the same row and never consumes another chat
    /// sequence.
    async fn ensure_project_card(
        &self,
        state: &GatewayState,
        result: &Value,
    ) -> Result<(), RpcError> {
        let project = result.get("project").ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "project.create result has no project".into(),
            details: None,
        })?;
        let project_id = project
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "project.create result has no project id".into(),
                details: None,
            })?;
        let announcement = self
            .orchestrator
            .rpc("project.get", json!({"project_id":project_id}))
            .await
            .map_err(Self::error)?;
        if let Some(announcement) = announcement.get("announcement") {
            self.persist_announcement_text(&json!({"announcement":announcement}))?;
        }
        let message_id = format!("msg_project_card_{project_id}");
        // The chat row and the global event are separate durable writes. A
        // crash between them leaves the row present but makes resume blind to
        // it, so deduplicate against the event log independently.
        let event_exists = self
            .store
            .events_since(0)
            .map_err(store_error)?
            .into_iter()
            .any(|event| {
                event.event == "message.created" && event.data["message"]["id"] == message_id
            });
        let card = json!({
            "id": message_id,
            "chat_id": "chat_main",
            "seq": 0,
            "sender": {"kind":"bot", "bot_id":"main"},
            "created_at": project.get("created_at").cloned().unwrap_or_else(|| json!(now())),
            "edited_at": null,
            "deleted": false,
            "reply_to": null,
            "thread_count": 0,
            "mentions": [],
            "blocks": [{"type":"project_card", "project_id":project_id, "context":"created", "note":null}],
            "fallback_text": format!("项目「{}」已创建", project.get("name").and_then(Value::as_str).unwrap_or(project_id)),
            "intent": null,
            "assignment_id": null,
            "streaming": false,
            "delivery": [],
            "reactions": []
        });
        let canonical = self.persist_client_message(&card)?;
        if !event_exists {
            let data = json!({"message":canonical});
            let event = self
                .store
                .append_event("message.created", data.clone())
                .map_err(store_error)?;
            state.publish_event(event.seq, &event.event, data).await;
        }
        Ok(())
    }

    /// Publish the durable system card associated with an assignment.  Cards
    /// are ordinary messages in the assignment's origin conversation, so they
    /// use the same per-chat sequence allocator and JSONL recovery path as
    /// user and bot messages.  The message id is derived solely from the
    /// assignment id; retrying an operation therefore repairs a missing
    /// event without allocating another row or sequence.
    async fn ensure_assignment_cards(
        &self,
        state: &GatewayState,
        assignment: &Value,
        method: &str,
    ) -> Result<(), RpcError> {
        self.ensure_assignment_cards_with_event_index(state, assignment, method, None)
            .await
    }

    async fn ensure_assignment_cards_with_event_index(
        &self,
        state: &GatewayState,
        assignment: &Value,
        method: &str,
        event_index: Option<&mut AssignmentCardEventIndex>,
    ) -> Result<(), RpcError> {
        // Most assignment mutations return the Assignment directly, while
        // assignment.retry follows the public `{ assignment }` envelope.
        // Card repair operates on the canonical object in either case.
        let assignment = assignment.get("assignment").unwrap_or(assignment);
        assignment
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "assignment result has no id".into(),
                details: None,
            })?;
        let is_delegation =
            method == "delegate" && assignment.get("project_id").is_none_or(Value::is_null);
        let assignment_id = assignment["id"].as_str().unwrap_or_default();
        let return_chat_id = self
            .orchestrator
            .resolve_assignment_return_chat(assignment_id)
            .ok();
        if is_delegation {
            self.ensure_assignment_card_with_event_index(state, assignment, true, event_index)
                .await?;
        } else if return_chat_id.is_some_and(|chat_id| !chat_id.starts_with("dm_")) {
            self.ensure_assignment_card_with_event_index(state, assignment, false, event_index)
                .await?;
        }
        Ok(())
    }

    async fn ensure_assignment_card_with_event_index(
        &self,
        state: &GatewayState,
        assignment: &Value,
        delegation: bool,
        mut event_index: Option<&mut AssignmentCardEventIndex>,
    ) -> Result<(), RpcError> {
        let assignment_id = assignment
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "assignment result has no id".into(),
                details: None,
            })?;
        let resolved_chat_id = self
            .orchestrator
            .resolve_assignment_return_chat(assignment_id)
            .map_err(Self::error)?;
        let chat_id = resolved_chat_id.as_str();
        let message_id = if delegation {
            format!("msg_delegation_{assignment_id}")
        } else {
            format!("msg_task_card_{assignment_id}")
        };
        let mut uncached_messages = if event_index.is_none() {
            Some(self.load_chat_messages(chat_id)?)
        } else {
            None
        };
        if let Some(index) = event_index.as_deref_mut() {
            if !index.chat_messages.contains_key(chat_id) {
                index.chat_messages.insert(
                    chat_id.to_owned(),
                    self.load_chat_messages_with_durable_ids(chat_id)?,
                );
            }
        }
        let existing = event_index
            .as_deref()
            .and_then(|index| index.chat_messages.get(chat_id))
            .and_then(|(messages, _)| {
                messages.iter().find(|message| {
                    message.get("id").and_then(Value::as_str) == Some(message_id.as_str())
                })
            })
            .cloned()
            .or_else(|| {
                uncached_messages
                    .as_ref()
                    .and_then(|messages| {
                        messages.iter().find(|message| {
                            message.get("id").and_then(Value::as_str) == Some(message_id.as_str())
                        })
                    })
                    .cloned()
            });
        let event_name = if existing.is_some() {
            "message.updated"
        } else {
            "message.created"
        };
        let fallback = if delegation {
            format!(
                "已委派给 {}：{}",
                assignment
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .unwrap_or("Bot"),
                assignment
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("待处理任务")
            )
        } else {
            format!(
                "任务：{}",
                assignment
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("待处理任务")
            )
        };
        let mut card = existing.clone().unwrap_or_else(|| {
            json!({
                "id":message_id,
                "chat_id":chat_id,
                "seq":0,
                "sender":{"kind":"system"},
                "created_at":assignment.get("created_at").cloned().unwrap_or_else(|| json!(now())),
                "edited_at":null,
                "deleted":false,
                "reply_to":null,
                "thread_count":0,
                "mentions":[],
                "blocks":[],
                "fallback_text":"",
                "intent":null,
                "assignment_id":assignment_id,
                "streaming":false,
                "delivery":[],
                "reactions":[]
            })
        });
        card["chat_id"] = json!(chat_id);
        card["sender"] = json!({"kind":"system"});
        card["assignment_id"] = json!(assignment_id);
        card["blocks"] = if delegation {
            json!([{"type":"delegation","bot_id":assignment.get("bot_id").and_then(Value::as_str).unwrap_or(""),"assignment_id":assignment_id}])
        } else {
            json!([{"type":"task_card","assignment_id":assignment_id}])
        };
        card["fallback_text"] = json!(fallback);
        normalize_message(&mut card);
        let canonical = if let Some(index) = event_index.as_deref_mut() {
            let (messages, durable_ids) = index
                .chat_messages
                .get_mut(chat_id)
                .expect("assignment card chat cache inserted above");
            let sequence_missing = existing.as_ref().is_none_or(|message| {
                message
                    .get("seq")
                    .and_then(Value::as_u64)
                    .is_none_or(|seq| seq == 0)
            });
            let needs_sequence = existing.as_ref().is_none_or(|message| {
                !durable_ids.contains(&message_id)
                    || sequence_missing
                    || !Self::messages_match_without_seq(message, &card)
            });
            if needs_sequence {
                if let Some(existing) = messages.iter_mut().find(|message| {
                    message.get("id").and_then(Value::as_str) == Some(message_id.as_str())
                }) {
                    *existing = card;
                } else {
                    messages.push(card);
                }
                let sequenced = self.sequence_chat_messages(chat_id, std::mem::take(messages))?;
                *messages = sequenced;
                durable_ids.clear();
                durable_ids.extend(
                    messages
                        .iter()
                        .filter_map(|message| message.get("id").and_then(Value::as_str))
                        .map(str::to_owned),
                );
            }
            messages
                .iter()
                .find(|message| {
                    message.get("id").and_then(Value::as_str) == Some(message_id.as_str())
                })
                .cloned()
                .ok_or_else(|| RpcError {
                    code: "internal".into(),
                    message: "sequenced assignment card disappeared".into(),
                    details: None,
                })?
        } else {
            self.persist_client_message_with_messages(
                chat_id,
                uncached_messages
                    .take()
                    .expect("uncached assignment card messages loaded above"),
                &card,
            )?
        };
        let event_exists = match event_index.as_deref() {
            Some(index) => index
                .message_rows
                .get(&message_id)
                .is_some_and(|message| Self::messages_match_without_seq(message, &canonical)),
            None => self
                .store
                .events_since(0)
                .map_err(store_error)?
                .into_iter()
                .rev()
                .find(|event| {
                    matches!(event.event.as_str(), "message.created" | "message.updated")
                        && event.data["message"]["id"] == message_id
                })
                .is_some_and(|event| event.data["message"]["blocks"] == canonical["blocks"]),
        };
        if event_exists {
            return Ok(());
        }
        let data = json!({"message":canonical});
        let event = self
            .store
            .append_event(event_name, data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        if let Some(index) = event_index {
            index.message_rows.insert(
                canonical["id"].as_str().unwrap_or_default().to_owned(),
                canonical,
            );
        }
        Ok(())
    }

    /// `send_msg` can create assignments as a side effect of a bot mention.
    /// The message id is the durable trigger key, which lets us discover only
    /// the assignments created by this dispatch instead of replaying every
    /// historical card on each send.
    async fn ensure_trigger_assignment_cards(
        &self,
        state: &GatewayState,
        trigger_message_id: Option<&str>,
    ) -> Result<(), RpcError> {
        let Some(trigger_message_id) = trigger_message_id else {
            return Ok(());
        };
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let assignments = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|items| items.values())
            .filter(|assignment| {
                assignment.get("trigger_message_id").and_then(Value::as_str)
                    == Some(trigger_message_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for assignment in assignments {
            let assignment_id = assignment.get("id").and_then(Value::as_str).unwrap_or("");
            let event_exists = self
                .store
                .events_since(0)
                .map_err(store_error)?
                .into_iter()
                .any(|event| {
                    event.event == "assignment.created"
                        && event.data["assignment"]["id"] == assignment_id
                });
            if !event_exists {
                let mut wire = assignment.clone();
                normalize_assignment(&mut wire);
                let data = json!({"assignment":wire});
                let event = self
                    .store
                    .append_event("assignment.created", data.clone())
                    .map_err(store_error)?;
                state.publish_event(event.seq, &event.event, data).await;
            }
            self.ensure_assignment_cards(state, &assignment, "assign")
                .await?;
        }
        Ok(())
    }

    async fn set_review_card(
        &self,
        state: &GatewayState,
        project_id: &str,
        review_state: &str,
        artifacts: &Value,
        fallback_text: &str,
    ) -> Result<(), RpcError> {
        let message_id = format!("msg_project_review_{project_id}");
        let existing = self
            .load_chat_messages("chat_main")?
            .into_iter()
            .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id.as_str()));
        let event_name = if existing.is_some() {
            "message.updated"
        } else {
            "message.created"
        };
        let mut card = existing.unwrap_or_else(|| {
            json!({
                "id":message_id,
                "chat_id":"chat_main",
                "seq":0,
                "sender":{"kind":"bot","bot_id":"main"},
                "created_at":now(),
                "edited_at":null,
                "deleted":false,
                "reply_to":null,
                "thread_count":0,
                "mentions":[{"kind":"user"}],
                "blocks":[],
                "fallback_text":"",
                "intent":null,
                "assignment_id":null,
                "streaming":false,
                "delivery":[],
                "reactions":[]
            })
        });
        let artifacts = if artifacts.is_null() {
            card["blocks"][0]
                .get("artifacts")
                .filter(|value| value.is_array())
                .cloned()
                .unwrap_or_else(|| json!([]))
        } else {
            artifacts.clone()
        };
        card["sender"] = json!({"kind":"bot","bot_id":"main"});
        card["mentions"] = json!([{"kind":"user"}]);
        card["blocks"] = json!([{
            "type":"review_card",
            "project_id":project_id,
            "artifacts":artifacts,
            "state":review_state
        }]);
        card["fallback_text"] = json!(fallback_text);
        let canonical = self.persist_client_message(&card)?;
        let event_exists = self
            .store
            .events_since(0)
            .map_err(store_error)?
            .into_iter()
            .rev()
            .find(|event| {
                matches!(event.event.as_str(), "message.created" | "message.updated")
                    && event.data["message"]["id"] == message_id
            })
            .is_some_and(|event| {
                event.data["message"]["blocks"][0]["state"] == review_state
                    && event.data["message"]["blocks"][0]["artifacts"] == artifacts
            });
        if event_exists {
            return Ok(());
        }
        let data = json!({"message":canonical});
        let event = self
            .store
            .append_event(event_name, data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        Ok(())
    }

    async fn set_completion_card(
        &self,
        state: &GatewayState,
        project_id: &str,
    ) -> Result<(), RpcError> {
        let project_result = self
            .orchestrator
            .rpc("project.get", json!({"project_id":project_id}))
            .await
            .map_err(Self::error)?;
        let project = project_result
            .get("project")
            .cloned()
            .unwrap_or(Value::Null);
        let announcement = project_result
            .get("announcement")
            .cloned()
            .unwrap_or_else(|| json!({"artifacts":[]}));
        let artifacts = announcement
            .get("artifacts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|artifact| {
                let artifact_id = artifact
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| RpcError {
                        code: "internal".into(),
                        message: "announcement artifact has no id".into(),
                        details: None,
                    })?;
                let title = artifact
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| RpcError {
                        code: "internal".into(),
                        message: format!("artifact {artifact_id} has no title"),
                        details: None,
                    })?;
                let path_or_url = artifact
                    .get("path_or_url")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| RpcError {
                        code: "internal".into(),
                        message: format!("artifact {artifact_id} has no path_or_url"),
                        details: None,
                    })?;
                Ok(json!({
                    "artifact_id":artifact_id,
                    "title":title,
                    "path_or_url":path_or_url,
                    "bot_id":artifact.get("bot_id").cloned().unwrap_or_else(|| json!("main")),
                    "kind":artifact.get("kind").cloned().unwrap_or_else(|| json!("file"))
                }))
            })
            .collect::<Result<Vec<_>, RpcError>>()?;
        let message_id = format!("msg_project_completion_{project_id}");
        let completion_chat_id = project
            .get("chat_id")
            .and_then(Value::as_str)
            .filter(|chat_id| !chat_id.is_empty())
            .unwrap_or("chat_main");
        let existing = self
            .load_chat_messages(completion_chat_id)?
            .into_iter()
            .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id.as_str()));
        let event_name = if existing.is_some() {
            "message.updated"
        } else {
            "message.created"
        };
        let mut card = existing.unwrap_or_else(|| {
            json!({
                "id":message_id,
                "chat_id":completion_chat_id,
                "seq":0,
                "sender":{"kind":"bot","bot_id":"main"},
                "created_at":project.get("updated_at").cloned().unwrap_or_else(|| json!(now())),
                "edited_at":null,
                "deleted":false,
                "reply_to":null,
                "thread_count":0,
                "mentions":[{"kind":"user"}],
                "blocks":[],
                "fallback_text":"",
                "intent":null,
                "assignment_id":null,
                "streaming":false,
                "delivery":[],
                "reactions":[]
            })
        });
        card["blocks"] = json!([{
            "type":"completion",
            "summary":format!("项目「{}」已完成", project.get("name").and_then(Value::as_str).unwrap_or(project_id)),
            "artifacts":artifacts,
            "next":[],
            "notify_main":true
        }]);
        card["fallback_text"] = json!(format!(
            "项目「{}」已完成",
            project
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(project_id)
        ));
        normalize_message(&mut card);
        let canonical = self.persist_client_message(&card)?;
        let event_exists = self
            .store
            .events_since(0)
            .map_err(store_error)?
            .into_iter()
            .rev()
            .find(|event| {
                matches!(event.event.as_str(), "message.created" | "message.updated")
                    && event.data["message"]["id"] == message_id
            })
            .is_some_and(|event| event.data["message"]["blocks"] == canonical["blocks"]);
        if event_exists {
            return Ok(());
        }
        let data = json!({"message":canonical});
        let event = self
            .store
            .append_event(event_name, data.clone())
            .map_err(store_error)?;
        state.publish_event(event.seq, &event.event, data).await;
        Ok(())
    }

    fn sequence_chat_messages(
        &self,
        chat_id: &str,
        messages: Vec<Value>,
    ) -> Result<Vec<Value>, RpcError> {
        let canonical = self
            .store
            .sequence_chat_messages(chat_id, &messages)
            .map_err(store_error)?;
        let existing = self
            .store
            .read_jsonl::<Value>(format!(
                "data/chats/{}/messages.jsonl",
                takeover_component(chat_id)
            ))
            .map_err(store_error)?;
        let existing = existing
            .into_iter()
            .filter_map(|item| {
                let id = item.get("id").and_then(Value::as_str)?.to_owned();
                Some((id, item))
            })
            .collect::<HashMap<_, _>>();
        for item in &canonical {
            let changed = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| existing.get(id))
                .is_none_or(|old| old != item);
            if changed {
                self.store
                    .append_jsonl(
                        format!("data/chats/{}/messages.jsonl", takeover_component(chat_id)),
                        item,
                    )
                    .map_err(store_error)?;
            }
        }
        Ok(canonical)
    }

    async fn chat_history(&self, params: &Value) -> RpcResult {
        let chat_id = params
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "chat_id is required".into(),
                details: None,
            })?;
        let mut messages =
            self.sequence_chat_messages(chat_id, self.load_chat_messages(chat_id)?)?;
        for message in &mut messages {
            deduplicate_review_card_artifacts(message);
        }
        messages.sort_by_key(|item| item.get("seq").and_then(Value::as_u64).unwrap_or(0));
        let after = params.get("after_seq").and_then(Value::as_u64).unwrap_or(0);
        let before = params.get("before_seq").and_then(Value::as_u64);
        messages.retain(|item| item.get("seq").and_then(Value::as_u64).unwrap_or(0) > after);
        if let Some(before) = before {
            messages.retain(|item| item.get("seq").and_then(Value::as_u64).unwrap_or(0) < before);
        }
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .min(100) as usize;
        let has_more = messages.len() > limit;
        if after == 0 {
            if messages.len() > limit {
                let start = messages.len() - limit;
                messages = messages.split_off(start);
            }
        } else {
            messages.truncate(limit);
        }
        Ok(json!({"messages":messages,"has_more":has_more}))
    }

    fn artifact_versions_list(&self, params: &Value) -> RpcResult {
        let params: ArtifactVersionsParams =
            serde_json::from_value(params.clone()).map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: error.to_string(),
                details: None,
            })?;
        self.require_artifact(&params.artifact_id)?;
        let path = artifact_version_log_path(&params.artifact_id)?;
        let mut versions = self
            .store
            .read_jsonl::<ArtifactVersion>(path)
            .map_err(store_error)?;
        if let Some(before_version) = params.before_version {
            versions.retain(|version| version.version < before_version);
        }
        versions.sort_by_key(|version| std::cmp::Reverse(version.version));
        let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
        let has_more = versions.len() > limit;
        versions.truncate(limit);
        serde_json::to_value(ArtifactVersionsResult { versions, has_more }).map_err(|error| {
            RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            }
        })
    }

    fn artifact_version_get(&self, params: &Value) -> RpcResult {
        let params: ArtifactVersionGetParams =
            serde_json::from_value(params.clone()).map_err(|error| RpcError {
                code: "invalid_params".into(),
                message: error.to_string(),
                details: None,
            })?;
        self.require_artifact(&params.artifact_id)?;
        let component = artifact_history_component(&params.artifact_id)?;
        let stored: StoredArtifactVersion = self
            .store
            .read_snapshot(format!(
                "data/artifact-history/{component}/versions/{}.json",
                params.version
            ))
            .map_err(store_error)?
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: format!(
                    "artifact version {}:{} not found",
                    params.artifact_id, params.version
                ),
                details: None,
            })?;
        if stored.version.artifact_id != params.artifact_id
            || stored.version.version != params.version
        {
            return Err(RpcError {
                code: "not_found".into(),
                message: "artifact version not found".into(),
                details: None,
            });
        }
        let bytes = BASE64
            .decode(stored.content_base64)
            .map_err(|error| RpcError {
                code: "internal".into(),
                message: format!("artifact version content is corrupt: {error}"),
                details: None,
            })?;
        let (content, content_encoding) = match String::from_utf8(bytes) {
            Ok(content) => (content, macbot_protocol::ArtifactContentEncoding::Utf8),
            Err(error) => (
                BASE64.encode(error.into_bytes()),
                macbot_protocol::ArtifactContentEncoding::Base64,
            ),
        };
        serde_json::to_value(ArtifactVersionGetResult {
            version: stored.version,
            content,
            content_encoding,
        })
        .map_err(|error| RpcError {
            code: "internal".into(),
            message: error.to_string(),
            details: None,
        })
    }

    fn require_artifact(&self, artifact_id: &str) -> Result<(), RpcError> {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        if snapshot
            .get("artifacts")
            .and_then(Value::as_object)
            .is_some_and(|artifacts| artifacts.contains_key(artifact_id))
        {
            Ok(())
        } else {
            Err(RpcError {
                code: "not_found".into(),
                message: format!("artifact {artifact_id} not found"),
                details: None,
            })
        }
    }

    fn record_artifact_versions(&self, params: &Value) -> Result<(), RpcError> {
        let Some(inputs) = params.get("artifacts").and_then(Value::as_array) else {
            return Ok(());
        };
        if inputs.is_empty() {
            return Ok(());
        }
        let chat_id = params.get("chat_id").and_then(Value::as_str).unwrap_or("");
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let projects = snapshot
            .get("projects")
            .and_then(Value::as_object)
            .ok_or_else(|| rpc_error("internal", "project snapshot is missing", None))?;
        let Some((project_id, project)) = projects
            .iter()
            .find(|(_, project)| project.get("chat_id").and_then(Value::as_str) == Some(chat_id))
        else {
            return Ok(());
        };
        let slug = project.get("slug").and_then(Value::as_str).unwrap_or("");
        if !is_normal_component(slug) {
            return Ok(());
        }
        let project_root = self.store.root().join("projects").join(slug);
        let artifact_rows = snapshot
            .get("artifacts")
            .and_then(Value::as_object)
            .ok_or_else(|| rpc_error("internal", "artifact snapshot is missing", None))?;
        for input in inputs {
            let path_or_url = input
                .get("path_or_url")
                .and_then(Value::as_str)
                .unwrap_or("");
            if path_or_url.is_empty() || is_http_url(path_or_url) {
                continue;
            }
            if is_sensitive_artifact_path(path_or_url) {
                continue;
            }
            let Some((artifact_id, artifact)) = artifact_rows.iter().find(|(_, artifact)| {
                artifact.get("project_id").and_then(Value::as_str) == Some(project_id.as_str())
                    && artifact.get("path_or_url").and_then(Value::as_str) == Some(path_or_url)
            }) else {
                continue;
            };
            let Some(file_path) = resolve_project_artifact_file(&project_root, path_or_url) else {
                continue;
            };
            let file = match fs::File::open(&file_path) {
                Ok(file) => file,
                Err(_) => continue,
            };
            if file
                .metadata()
                .map(|metadata| metadata.len() > MAX_ARTIFACT_VERSION_BYTES)
                .unwrap_or(true)
            {
                continue;
            }
            let mut bytes = Vec::new();
            if file
                .take(MAX_ARTIFACT_VERSION_BYTES + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() as u64 > MAX_ARTIFACT_VERSION_BYTES
            {
                continue;
            }
            let component = artifact_history_component(artifact_id)?;
            let log_path = artifact_version_log_path(artifact_id)?;
            let mut history = self
                .store
                .read_jsonl::<ArtifactVersion>(&log_path)
                .map_err(store_error)?;
            if let Some(latest) = history.iter().max_by_key(|version| version.version) {
                let stored_path = format!(
                    "data/artifact-history/{component}/versions/{}.json",
                    latest.version
                );
                if let Some(stored) = self
                    .store
                    .read_snapshot::<StoredArtifactVersion>(stored_path)
                    .map_err(store_error)?
                {
                    if BASE64.decode(stored.content_base64).ok().as_deref()
                        == Some(bytes.as_slice())
                    {
                        continue;
                    }
                }
            }
            let version_number = history
                .iter()
                .map(|version| version.version)
                .max()
                .unwrap_or(0)
                + 1;
            let version = ArtifactVersion {
                artifact_id: artifact_id.clone(),
                version: version_number,
                title: artifact
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                path_or_url: path_or_url.into(),
                bot_id: artifact
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                assignment_id: artifact
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                kind: macbot_protocol::ArtifactKind::File,
                created_at: now(),
                byte_size: bytes.len() as u64,
                mime: mime_guess::from_path(&file_path)
                    .first_or_octet_stream()
                    .to_string(),
            };
            let stored = StoredArtifactVersion {
                version: version.clone(),
                content_base64: BASE64.encode(&bytes),
            };
            self.store
                .write_snapshot(
                    format!("data/artifact-history/{component}/versions/{version_number}.json"),
                    &stored,
                )
                .map_err(store_error)?;
            self.store
                .append_jsonl(&log_path, &version)
                .map_err(store_error)?;
            history.push(version);
        }
        Ok(())
    }

    async fn chat_thread(&self, params: &Value) -> RpcResult {
        let chat_id = required_text(params, "chat_id")?;
        let root_id = required_text(params, "root_message_id")?;
        let mut messages =
            self.sequence_chat_messages(&chat_id, self.load_chat_messages(&chat_id)?)?;
        messages.sort_by_key(|message| message.get("seq").and_then(Value::as_u64).unwrap_or(0));
        let root = messages
            .iter()
            .find(|message| message.get("id").and_then(Value::as_str) == Some(root_id.as_str()))
            .cloned()
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: format!("message {root_id} not found"),
                details: None,
            })?;
        let replies = messages
            .into_iter()
            .filter(|message| {
                message.get("reply_to").and_then(Value::as_str) == Some(root_id.as_str())
            })
            .collect::<Vec<_>>();
        Ok(json!({"root":root,"replies":replies}))
    }

    fn message_delete(&self, params: &Value) -> RpcResult {
        let message_id = required_text(params, "message_id")?;
        let (chat_id, mut message) = self.find_chat_message(&message_id)?;
        normalize_message(&mut message);
        ensure_user_message(&message)?;
        if message.get("deleted").and_then(Value::as_bool) == Some(true) {
            return Ok(json!({}));
        }
        message["deleted"] = json!(true);
        message["blocks"] = json!([]);
        message["fallback_text"] = json!("");
        message["mentions"] = json!([]);
        message["quote"] = Value::Null;
        message["reactions"] = json!([]);
        message["edited_at"] = json!(now());
        message["chat_id"] = json!(chat_id);
        self.persist_client_message(&message)?;
        self.orchestrator
            .redact_user_message(&message_id)
            .map_err(Self::error)?;
        Ok(json!({}))
    }

    fn message_edit(&self, params: &Value) -> RpcResult {
        let message_id = required_text(params, "message_id")?;
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "text is required and must not be blank".into(),
                details: None,
            })?;
        let (chat_id, mut message) = self.find_chat_message(&message_id)?;
        normalize_message(&mut message);
        ensure_user_message(&message)?;
        if message.get("deleted").and_then(Value::as_bool) == Some(true)
            || message.get("streaming").and_then(Value::as_bool) == Some(true)
        {
            return Err(RpcError {
                code: "conflict".into(),
                message: "deleted or streaming messages cannot be edited".into(),
                details: None,
            });
        }
        let is_plain_text = message
            .get("blocks")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks.len() == 1 && blocks[0].get("type").and_then(Value::as_str) == Some("text")
            })
            && message
                .get("mentions")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty);
        if !is_plain_text {
            return Err(RpcError {
                code: "conflict".into(),
                message: "only plain-text messages can be edited".into(),
                details: None,
            });
        }
        message["blocks"][0]["markdown"] = json!(text);
        message["fallback_text"] = json!(text);
        message["edited_at"] = json!(now());
        message["chat_id"] = json!(chat_id);
        let message = self.persist_client_message(&message)?;
        self.orchestrator
            .edit_user_message_text(&message_id, text)
            .map_err(Self::error)?;
        Ok(json!({"message":message}))
    }

    async fn chat_mark_read(&self, params: &Value) -> RpcResult {
        let chat_id = required_text(params, "chat_id")?;
        let seq = params
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "seq is required".into(),
                details: None,
            })?;
        let mut overlay = self.read_chat_overlay(&chat_id)?;
        let current = overlay
            .get("last_read_seq")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        overlay["last_read_seq"] = json!(current.max(seq));
        self.write_chat_overlay(&chat_id, &overlay)?;
        Ok(json!({}))
    }

    async fn chat_react(&self, params: &Value) -> RpcResult {
        let message_id = required_text(params, "message_id")?;
        let emoji = required_text(params, "emoji")?;
        let on = params
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "on is required".into(),
                details: None,
            })?;
        let (_, mut message) = self.find_chat_message(&message_id)?;
        normalize_message(&mut message);
        let reactions = message
            .get_mut("reactions")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "message reactions are not an array".into(),
                details: None,
            })?;
        if on {
            if !reactions.iter().any(|reaction| {
                reaction.get("emoji").and_then(Value::as_str) == Some(emoji.as_str())
            }) {
                reactions.push(json!({"emoji":emoji,"by":[{"kind":"user"}]}));
            }
        } else {
            reactions.retain(|reaction| {
                reaction.get("emoji").and_then(Value::as_str) != Some(emoji.as_str())
            });
        }
        let message = self.persist_client_message(&message)?;
        Ok(json!({"message":message}))
    }

    async fn chat_set_flag(&self, params: &Value, flag: &str) -> RpcResult {
        let chat_id = required_text(params, "chat_id")?;
        let value = params
            .get(flag)
            .and_then(Value::as_bool)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: format!("{flag} is required"),
                details: None,
            })?;
        let mut overlay = self.read_chat_overlay(&chat_id)?;
        overlay[flag] = json!(value);
        self.write_chat_overlay(&chat_id, &overlay)?;
        let mut chat = self.chat_list().await?["chats"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == chat_id))
            .cloned()
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: format!("chat {chat_id} not found"),
                details: None,
            })?;
        apply_chat_overlay(&mut chat, &overlay);
        Ok(json!({"chat":chat}))
    }

    pub(crate) fn read_chat_overlay(&self, chat_id: &str) -> Result<Value, RpcError> {
        self.store
            .read_snapshot(format!(
                "data/chats/{}/metadata.json",
                takeover_component(chat_id)
            ))
            .map_err(store_error)
            .map(|value| value.unwrap_or_else(|| json!({})))
    }

    fn is_read_only_bot_dm(&self, chat_id: &str) -> Result<bool, RpcError> {
        let overlay = self.read_chat_overlay(chat_id)?;
        Ok(overlay.get("kind").and_then(Value::as_str) == Some("bot_dm"))
    }

    fn write_chat_overlay(&self, chat_id: &str, overlay: &Value) -> Result<(), RpcError> {
        self.store
            .write_snapshot(
                format!("data/chats/{}/metadata.json", takeover_component(chat_id)),
                overlay,
            )
            .map_err(store_error)
    }

    fn find_chat_message(&self, message_id: &str) -> Result<(String, Value), RpcError> {
        let chats = self.store.root().join("data/chats");
        if chats.exists() {
            for entry in fs::read_dir(chats).map_err(|error| RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            })? {
                let entry = entry.map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
                if !entry
                    .file_type()
                    .map_err(|error| RpcError {
                        code: "internal".into(),
                        message: error.to_string(),
                        details: None,
                    })?
                    .is_dir()
                {
                    continue;
                }
                let relative = entry.path().join("messages.jsonl");
                let records = self
                    .store
                    .read_jsonl::<Value>(
                        relative
                            .strip_prefix(self.store.root())
                            .unwrap_or(&relative),
                    )
                    .map_err(store_error)?;
                if let Some(message) = records
                    .into_iter()
                    .rev()
                    .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
                {
                    let chat_id = message
                        .get("chat_id")
                        .and_then(Value::as_str)
                        .unwrap_or(entry.file_name().to_string_lossy().as_ref())
                        .to_owned();
                    return Ok((chat_id, message));
                }
            }
        }
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        if let Some(message) = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .and_then(|messages| {
                messages
                    .values()
                    .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
            })
        {
            return Ok((
                message
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                message.clone(),
            ));
        }
        Err(RpcError {
            code: "not_found".into(),
            message: format!("message {message_id} not found"),
            details: None,
        })
    }

    async fn chat_list(&self) -> RpcResult {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let mut chats = vec![main_chat()];
        if let Some(bots) = snapshot.get("bots").and_then(Value::as_object) {
            for bot in bots.values().filter(|bot| {
                bot.get("hidden").and_then(Value::as_bool) != Some(true) && !bot_is_main(bot)
            }) {
                let id = canonical_bot_dm_id(bot);
                chats.push(json!({"id":id,"kind":"direct","title":bot["name"],"bot_id":bot["id"],"project_id":null,"member_bot_ids":[bot["id"]],"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":bot["pinned"],"muted":false,"updated_at":bot["updated_at"]}));
            }
        }
        if let Some(projects) = snapshot.get("projects").and_then(Value::as_object) {
            for project in projects.values() {
                chats.push(project_chat(project));
            }
        }
        let known_chat_ids = chats
            .iter()
            .filter_map(|chat| chat.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<HashSet<_>>();
        let chats_dir = self.store.root().join("data/chats");
        if chats_dir.exists() {
            for entry in fs::read_dir(&chats_dir).map_err(|error| RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            })? {
                let entry = entry.map_err(|error| RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
                let id = entry.file_name().to_string_lossy().into_owned();
                if known_chat_ids.contains(&id) {
                    continue;
                }
                let Some(metadata) = self
                    .store
                    .read_snapshot::<Value>(format!("data/chats/{id}/metadata.json"))
                    .map_err(store_error)?
                else {
                    continue;
                };
                if metadata.get("kind").and_then(Value::as_str) != Some("bot_dm") {
                    continue;
                }
                chats.push(json!({
                    "id":id,
                    "kind":"bot_dm",
                    "title":metadata.get("title").cloned().unwrap_or_else(|| json!("Bot DM")),
                    "bot_id":null,
                    "project_id":null,
                    "member_bot_ids":metadata.get("member_bot_ids").cloned().unwrap_or_else(|| json!([])),
                    "last_message":null,
                    "last_seq":0,
                    "last_read_seq":0,
                    "unread":0,
                    "attention":"none",
                    "pinned":false,
                    "muted":false,
                    "updated_at":metadata.get("created_at").cloned().unwrap_or_else(|| json!(now()))
                }));
            }
        }
        for chat in &mut chats {
            let Some(id) = chat.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if let Ok(Some(metadata)) = self.store.read_snapshot::<Value>(format!(
                "data/chats/{}/metadata.json",
                takeover_component(&id)
            )) {
                apply_chat_overlay(chat, &metadata);
            }
            chat["last_seq"] = json!(self.store.last_chat_sequence(&id).map_err(store_error)?);
        }
        Ok(json!({"chats":chats}))
    }

    async fn chat_get(&self, params: &Value) -> RpcResult {
        let id = params
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "chat_id is required".into(),
                details: None,
            })?;
        let chats = self.chat_list().await?;
        chats["chats"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
            })
            .cloned()
            .map(|chat| json!({"chat":chat}))
            .ok_or_else(|| RpcError {
                code: "not_found".into(),
                message: "chat not found".into(),
                details: None,
            })
    }

    async fn bootstrap(&self, state: &GatewayState) -> RpcResult {
        let bots = self
            .orchestrator
            .rpc("bot.list", json!({"include_hidden":false}))
            .await
            .map_err(Self::error)?["bots"]
            .clone();
        let mut bot_result = json!({"bots":bots});
        self.enrich_bot_status(&mut bot_result)
            .map_err(|message| RpcError {
                code: "internal".into(),
                message,
                details: None,
            })?;
        let bots = bot_result["bots"].clone();
        let projects = self
            .orchestrator
            .rpc("project.list", json!({}))
            .await
            .map_err(Self::error)?["projects"]
            .clone();
        let chats = self.chat_list().await?["chats"].clone();
        let settings = self.settings(state).await?;
        let pending = self
            .orchestrator
            .rpc("approval.list", json!({}))
            .await
            .map_err(Self::error)?;
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let questions = snapshot
            .get("questions")
            .and_then(Value::as_object)
            .map(|items| {
                items
                    .values()
                    .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let approvals = pending
            .get("approvals")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let reviews = projects
            .as_array()
            .into_iter()
            .flatten()
            .filter(|project| project.get("status").and_then(Value::as_str) == Some("review"))
            .filter_map(|project| project.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<Vec<_>>();
        let seq = self.store.last_event_seq().map_err(store_error)?;
        let host_name = state.host_name.read().await.clone();
        let node_id = state.node_id.read().await.clone();
        let hello = json!({"protocol":1,"server_version":"0.1.0","node_id":node_id,"host_name":host_name,"server_time":now(),"last_seq":seq,"timezone":settings["timezone"],"currency":settings["currency"],"features":["browser"]});
        Ok(
            json!({"seq":seq,"hello":hello,"bots":bots,"chats":chats,"projects":projects,"settings":settings,"pending":{"approvals":approvals,"questions":questions,"reviews":reviews}}),
        )
    }

    fn in_use_models(&self) -> Vec<String> {
        let Ok(snapshot) = self.orchestrator.snapshot() else {
            return Vec::new();
        };
        let mut models = Vec::new();
        for key in ["bots", "assignments"] {
            if let Some(items) = snapshot.get(key).and_then(Value::as_object) {
                for item in items.values() {
                    if let Some(model) = item
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|model| !model.is_empty())
                    {
                        models.push(model.to_owned());
                    }
                }
            }
        }
        models.sort();
        models.dedup();
        models
    }

    fn enrich_bot_status(&self, result: &mut Value) -> Result<(), String> {
        let snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        self.enrich_bot_status_from_snapshot(result, &snapshot, None)
    }

    fn enrich_bot_status_from_snapshot(
        &self,
        result: &mut Value,
        snapshot: &Value,
        private_counts: Option<&BotStatusCounts>,
    ) -> Result<(), String> {
        let counts = self.bot_counts_from_snapshot(snapshot, private_counts)?;
        Self::apply_bot_status(result, &counts);
        Ok(())
    }

    fn bot_counts_from_snapshot(
        &self,
        snapshot: &Value,
        private_counts: Option<&BotStatusCounts>,
    ) -> Result<BotStatusCounts, String> {
        let Some(assignments) = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .cloned()
        else {
            return Ok(HashMap::new());
        };
        let mut counts: BotStatusCounts = HashMap::new();
        let assignment_status = assignments
            .iter()
            .filter_map(|(id, item)| {
                item.get("status")
                    .and_then(Value::as_str)
                    .map(|status| (id.clone(), status.to_owned()))
            })
            .collect::<HashMap<_, _>>();
        for assignment in assignments.values() {
            let Some(bot_id) = assignment.get("bot_id").and_then(Value::as_str) else {
                continue;
            };
            let entry = counts.entry(bot_id.into()).or_default();
            match assignment
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
            {
                "working" => entry.0 += 1,
                "queued" => entry.1 += 1,
                "waiting_user" | "waiting_bot" => entry.2 += 1,
                "blocked" => entry.3 = true,
                _ => {}
            }
        }
        if let Some(items) = snapshot.get("approvals").and_then(Value::as_object) {
            for approval in items
                .values()
                .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
            {
                let already_waiting = approval
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .and_then(|id| assignment_status.get(id))
                    .is_some_and(|status| {
                        matches!(status.as_str(), "waiting_user" | "waiting_bot")
                    });
                if !already_waiting {
                    if let Some(bot_id) = approval.get("bot_id").and_then(Value::as_str) {
                        counts.entry(bot_id.into()).or_default().2 += 1;
                    }
                }
            }
        }
        if let Some(items) = snapshot.get("questions").and_then(Value::as_object) {
            for question in items
                .values()
                .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
            {
                let already_waiting = question
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .and_then(|id| assignment_status.get(id))
                    .is_some_and(|status| {
                        matches!(status.as_str(), "waiting_user" | "waiting_bot")
                    });
                if !already_waiting {
                    if let Some(bot_id) = question.get("bot_id").and_then(Value::as_str) {
                        counts.entry(bot_id.into()).or_default().2 += 1;
                    }
                }
            }
        }
        if let Some(private_counts) = private_counts {
            for (bot_id, (active, queued, waiting, blocked)) in private_counts {
                let entry = counts.entry(bot_id.clone()).or_default();
                entry.0 += active;
                entry.1 += queued;
                entry.2 += waiting;
                entry.3 |= blocked;
            }
        } else {
            self.add_private_durable_counts(snapshot, &mut counts)?;
        }
        Ok(counts)
    }

    fn apply_bot_status(result: &mut Value, counts: &BotStatusCounts) {
        let set = |bot: &mut Value| {
            let Some(id) = bot.get("id").and_then(Value::as_str) else {
                return;
            };
            let (active, queued, waiting, blocked) = counts.get(id).copied().unwrap_or_default();
            let summary = if blocked {
                "blocked"
            } else if waiting > 0 {
                "waiting_user"
            } else if active > 0 {
                "working"
            } else {
                "idle"
            };
            if let Some(object) = bot.as_object_mut() {
                object.insert(
                    "status".into(),
                    json!({"summary":summary,"active":active,"queued":queued,"waiting":waiting}),
                );
            }
        };
        if let Some(bot) = result.get_mut("bot") {
            set(bot);
        }
        if let Some(bots) = result.get_mut("bots").and_then(Value::as_array_mut) {
            for bot in bots {
                set(bot);
            }
        }
    }

    /// Private chat runs have no orchestrator Assignment. Their durable job
    /// and request snapshot are the authoritative source for Bot status.
    /// Assignment-owned jobs are deliberately skipped because their
    /// orchestrator status is already included above.
    fn add_private_durable_counts(
        &self,
        snapshot: &Value,
        counts: &mut BotStatusCounts,
    ) -> Result<(), String> {
        let pending_waits = snapshot
            .get("approvals")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|items| items.values())
            .chain(
                snapshot
                    .get("questions")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|items| items.values()),
            )
            .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
            .filter_map(|item| {
                Some((
                    item.get("bot_id")?.as_str()?.to_owned(),
                    item.get("chat_id")?.as_str()?.to_owned(),
                ))
            })
            .collect::<std::collections::HashSet<_>>();
        if let Ok(durable) = self.durable.try_lock() {
            if self.durable_job_set_matches_disk(&durable)? {
                for job in durable.jobs() {
                    self.add_private_durable_job(&pending_waits, counts, job)?;
                }
                return Ok(());
            }
        }
        let jobs_dir = self.store.root().join("data/jobs");
        let entries = match fs::read_dir(&jobs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let path = entry.path();
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            let job: macbot_durable::Job = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid durable job {}: {error}", path.display()))?;
            self.add_private_durable_job(&pending_waits, counts, &job)?;
        }
        Ok(())
    }

    fn add_private_durable_job(
        &self,
        pending_waits: &HashSet<(String, String)>,
        counts: &mut BotStatusCounts,
        job: &macbot_durable::Job,
    ) -> Result<(), String> {
        let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
            return Ok(());
        };
        if matches!(
            job.status,
            macbot_durable::JobStatus::Done
                | macbot_durable::JobStatus::Failed
                | macbot_durable::JobStatus::Cancelled
        ) {
            return Ok(());
        }
        let request = self
            .store
            .read_snapshot::<crate::execution::ExecutionRequest>(format!(
                "data/run_requests/{run_id}.json"
            ))
            .map_err(|error| error.to_string())?;
        let Some(request) = request else {
            return Ok(());
        };
        if request.assignment_id.is_some() {
            return Ok(());
        }
        if request.phase.as_deref() == Some("subagent") || request.parent_run_id.is_some() {
            // Child runs are represented by Workbench.subagents_running;
            // they must not inflate the parent Bot's ordinary active count.
            return Ok(());
        }
        if matches!(
            job.status,
            macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
        ) && pending_waits.contains(&(request.bot_id.clone(), request.chat_id.clone()))
        {
            return Ok(());
        }
        let entry = counts.entry(request.bot_id).or_default();
        match job.status {
            macbot_durable::JobStatus::Queued => entry.1 += 1,
            macbot_durable::JobStatus::Running => entry.0 += 1,
            macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended => {
                entry.2 += 1
            }
            macbot_durable::JobStatus::Done
            | macbot_durable::JobStatus::Failed
            | macbot_durable::JobStatus::Cancelled => {}
        }
        Ok(())
    }

    fn private_subagent_count(&self) -> Result<u32, String> {
        if let Ok(durable) = self.durable.try_lock() {
            if self.durable_job_set_matches_disk(&durable)? {
                let mut count = 0;
                for job in durable.jobs() {
                    if job.status != macbot_durable::JobStatus::Running {
                        continue;
                    }
                    let Some(request) = self.private_subagent_request(job)? else {
                        continue;
                    };
                    if request.assignment_id.is_none()
                        && (request.phase.as_deref() == Some("subagent")
                            || request.parent_run_id.is_some())
                    {
                        count += 1;
                    }
                }
                return Ok(count);
            }
        }
        let jobs_dir = self.store.root().join("data/jobs");
        let entries = match fs::read_dir(&jobs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.to_string()),
        };
        let mut count = 0;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let path = entry.path();
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            let job: macbot_durable::Job = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid durable job {}: {error}", path.display()))?;
            if job.status != macbot_durable::JobStatus::Running {
                continue;
            }
            if let Some(request) = self.private_subagent_request(&job)? {
                if request.assignment_id.is_none()
                    && (request.phase.as_deref() == Some("subagent")
                        || request.parent_run_id.is_some())
                {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn private_subagent_request(
        &self,
        job: &macbot_durable::Job,
    ) -> Result<Option<crate::execution::ExecutionRequest>, String> {
        let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        self.store
            .read_snapshot::<crate::execution::ExecutionRequest>(format!(
                "data/run_requests/{run_id}.json"
            ))
            .map_err(|error| error.to_string())
    }

    fn durable_job_set_matches_disk(
        &self,
        durable: &macbot_durable::DurableRuntime,
    ) -> Result<bool, String> {
        let jobs_dir = self.store.root().join("data/jobs");
        let entries = match fs::read_dir(&jobs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(durable.jobs().next().is_none());
            }
            Err(error) => return Err(error.to_string()),
        };
        let mut disk_metadata = HashMap::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = entry
                .file_name()
                .to_str()
                .map(|name| name.trim_end_matches(".json").to_owned())
            else {
                return Ok(false);
            };
            let metadata = entry.metadata().map_err(|error| error.to_string())?;
            let modified_nanos = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_nanos())
                .unwrap_or_default();
            disk_metadata.insert(id, (metadata.len(), modified_nanos));
        }
        let memory_ids = durable
            .jobs()
            .map(|job| job.id.clone())
            .collect::<HashSet<_>>();
        if disk_metadata.len() != memory_ids.len()
            || memory_ids.iter().any(|id| !disk_metadata.contains_key(id))
        {
            return Ok(false);
        }
        Ok(durable.jobs().all(|job| {
            disk_metadata.get(&job.id).copied()
                == durable
                    .job_snapshot_metadata(&job.id)
                    .map(|metadata| (metadata.len, metadata.modified_nanos))
        }))
    }

    fn workbench(&self) -> Result<Value, String> {
        let snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        self.workbench_from_snapshot(&snapshot, None, None)
    }

    fn workbench_from_snapshot(
        &self,
        snapshot: &Value,
        counts_override: Option<&BotStatusCounts>,
        subagents_override: Option<u32>,
    ) -> Result<Value, String> {
        let assignments = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut waiting = Vec::new();
        if let Some(items) = snapshot.get("approvals").and_then(Value::as_object) {
            for approval in items
                .values()
                .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
            {
                waiting.push(json!({"kind":"approval","approval":approval}));
            }
        }
        let pending_question_ids =
            if let Some(items) = snapshot.get("questions").and_then(Value::as_object) {
                let mut ids = std::collections::HashSet::new();
                for question in items
                    .values()
                    .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
                {
                    if let Some(id) = question.get("id").and_then(Value::as_str) {
                        ids.insert(id.to_owned());
                    }
                    waiting.push(json!({"kind":"question","question":question}));
                }
                ids
            } else {
                std::collections::HashSet::new()
            };
        if let Some(items) = snapshot.get("projects").and_then(Value::as_object) {
            for project in items
                .values()
                .filter(|item| item.get("status").and_then(Value::as_str) == Some("review"))
            {
                let Some(project_id) = project.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let since = project
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        project
                            .get("created_at")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    });
                if !since.is_empty() {
                    waiting.push(json!({"kind":"review","project_id":project_id,"since":since}));
                }
            }
        }
        let takeover_dir = self.store.root().join("data/takeovers");
        if let Ok(entries) = fs::read_dir(&takeover_dir) {
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let entry_path = entry.path();
                let stem = entry_path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_owned();
                let Some(request) = self
                    .store
                    .read_snapshot::<Value>(format!("data/takeovers/{stem}.json"))
                    .map_err(|error| error.to_string())?
                else {
                    continue;
                };
                let state = request.get("state").and_then(Value::as_str).unwrap_or("");
                if !matches!(state, "pending" | "active") {
                    continue;
                }
                let question_id = request.get("question_id").and_then(Value::as_str);
                // A pending takeover request is already represented by its
                // decision Question. Active takeovers remain visible.
                if state == "pending"
                    && question_id.is_some_and(|id| pending_question_ids.contains(id))
                {
                    continue;
                }
                let Some(bot_id) = request.get("bot_id").and_then(Value::as_str) else {
                    continue;
                };
                let assignment_id = request
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .unwrap_or(stem.as_str());
                let reason = request
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("用户接管浏览器");
                waiting.push(json!({
                    "kind":"takeover",
                    "bot_id":bot_id,
                    "assignment_id":assignment_id,
                    "reason":reason
                }));
            }
        } else if takeover_dir.exists() {
            return Err(format!("cannot read {}", takeover_dir.display()));
        }
        let counts = match counts_override {
            Some(counts) => counts.clone(),
            None => self.bot_counts_from_snapshot(snapshot, None)?,
        };
        let bots = snapshot.get("bots").and_then(Value::as_object).map(|items| items.values().filter(|bot| !bot_is_main(bot)).map(|bot| {
            let bot_id = bot.get("id").and_then(Value::as_str).unwrap_or("");
            let (active, _, _, _) = counts.get(bot_id).copied().unwrap_or_default();
            let assignments = assignments.values().filter(|item| item.get("bot_id").and_then(Value::as_str) == Some(bot_id) && matches!(item.get("status").and_then(Value::as_str), Some("working" | "queued" | "waiting_user" | "waiting_bot" | "blocked"))).cloned().map(|mut item| { normalize_assignment(&mut item); item }).collect::<Vec<_>>();
            json!({"bot_id":bot_id,"active":active,"max_parallel":bot.get("max_parallel").and_then(Value::as_u64).unwrap_or(1),"assignments":assignments})
        }).collect::<Vec<_>>()).unwrap_or_default();
        let today = Local::now().date_naive();
        let done_today = assignments
            .values()
            .filter(|item| item.get("status").and_then(Value::as_str) == Some("done"))
            .filter(|item| {
                item.get("finished_at")
                    .and_then(Value::as_str)
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                    .is_some_and(|value| value.with_timezone(&Local).date_naive() == today)
            })
            .cloned()
            .map(|mut item| {
                normalize_assignment(&mut item);
                item
            })
            .collect::<Vec<_>>();
        let main_bot_ids = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|bots| bots.values())
            .filter(|bot| bot_is_main(bot))
            .filter_map(|bot| bot.get("id").and_then(Value::as_str))
            .collect::<std::collections::HashSet<_>>();
        let running = assignments
            .values()
            .filter(|assignment| {
                assignment.get("status").and_then(Value::as_str) == Some("working")
                    && assignment
                        .get("bot_id")
                        .and_then(Value::as_str)
                        .is_some_and(|bot_id| !main_bot_ids.contains(bot_id))
            })
            .count() as u32;
        let global_limit = snapshot
            .get("settings")
            .and_then(|settings| settings.get("global_limit"))
            .and_then(Value::as_u64)
            .unwrap_or(8);
        let private_subagents = match subagents_override {
            Some(count) => count,
            None => self.private_subagent_count()?,
        };
        let subagents_running = assignments
            .values()
            .map(|item| {
                item.get("subagents_active")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32
            })
            .sum::<u32>()
            + private_subagents;
        Ok(
            json!({"running":running,"global_limit":global_limit,"subagents_running":subagents_running,"waiting":waiting,"bots":bots,"done_today":done_today}),
        )
    }

    /// Read-only status bridge for runtime's ephemeral host/bot status events.
    /// It deliberately reuses the same durable and Workbench accounting path
    /// as the public RPC so status events cannot drift from `workbench.get`.
    pub fn live_status(&self) -> Result<Value, String> {
        let snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        let mut result = json!({
            "bots": snapshot
                .get("bots")
                .and_then(Value::as_object)
                .map(|items| items.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        });
        let mut private_counts = HashMap::new();
        self.add_private_durable_counts(&snapshot, &mut private_counts)?;
        let counts = self.bot_counts_from_snapshot(&snapshot, Some(&private_counts))?;
        Self::apply_bot_status(&mut result, &counts);
        let private_subagents = self.private_subagent_count()?;
        let workbench =
            self.workbench_from_snapshot(&snapshot, Some(&counts), Some(private_subagents))?;
        let queued = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|assignments| assignments.values())
            .filter(|assignment| assignment.get("status").and_then(Value::as_str) == Some("queued"))
            .filter(|assignment| {
                assignment
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .is_some_and(|bot_id| {
                        snapshot
                            .get("bots")
                            .and_then(Value::as_object)
                            .and_then(|bots| bots.get(bot_id))
                            .is_none_or(|bot| !bot_is_main(bot))
                    })
            })
            .count() as u32;
        let (_, waiting, _) = result
            .get("bots")
            .and_then(Value::as_array)
            .into_iter()
            .flat_map(|bots| bots.iter())
            .filter(|bot| !bot_is_main(bot))
            .fold((0_u32, 0_u32, 0_u32), |(queued, waiting, running), bot| {
                (
                    queued,
                    waiting + bot["status"]["waiting"].as_u64().unwrap_or(0) as u32,
                    running + bot["status"]["active"].as_u64().unwrap_or(0) as u32,
                )
            });
        Ok(json!({
            "running": workbench["running"],
            "queued": queued,
            "waiting": waiting,
            "global_limit": workbench["global_limit"],
            "subagents_running": workbench["subagents_running"],
            "bots": result["bots"]
        }))
    }
}

fn registry_error(error: RegistryError) -> RpcError {
    let message = error.to_string();
    let code = match &error {
        RegistryError::ProviderNotFound(_) | RegistryError::ModelNotFound(_) => "not_found",
        RegistryError::Conflict(_) => "conflict",
        RegistryError::Invalid(_) => "invalid_params",
        RegistryError::Store(_) | RegistryError::Provider(_) => "internal",
    };
    RpcError {
        code: code.into(),
        message,
        details: None,
    }
}

fn store_error(error: macbot_store::StoreError) -> RpcError {
    RpcError {
        code: "internal".into(),
        message: error.to_string(),
        details: None,
    }
}

fn is_settings_secret_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "api_key" || name == "key" || name.ends_with("_key")
}

pub(crate) fn validate_settings_secret_params(params: &Value) -> Result<(), RpcError> {
    let Some(patch) = params.get("patch") else {
        return Ok(());
    };
    if contains_settings_secret_field(patch) {
        return Err(RpcError {
            code: "invalid_params".into(),
            message: "settings secrets must use the top-level *_key parameter".into(),
            details: None,
        });
    }
    if patch
        .get("web_search")
        .and_then(Value::as_object)
        .is_some_and(|web_search| web_search.contains_key("has_key"))
    {
        return Err(RpcError {
            code: "invalid_params".into(),
            message: "web_search.has_key is read-only".into(),
            details: None,
        });
    }
    if patch.get("push").is_some() {
        return Err(RpcError {
            code: "invalid_params".into(),
            message: "settings.push is read-only".into(),
            details: None,
        });
    }
    if let Some(web_search_key) = params.get("web_search_key") {
        if !web_search_key.is_string() {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "web_search_key must be a string".into(),
                details: None,
            });
        }
    }
    Ok(())
}

fn contains_settings_secret_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(name, value)| {
            (is_settings_secret_field(name) && name != "has_key")
                || contains_settings_secret_field(value)
        }),
        Value::Array(values) => values.iter().any(contains_settings_secret_field),
        _ => false,
    }
}

fn redact_settings_params(params: &Value) -> Value {
    let mut redacted = params.clone();
    if let Some(object) = redacted.as_object_mut() {
        for (name, value) in object.iter_mut() {
            if is_settings_secret_field(name) {
                *value = Value::String("<redacted>".into());
            }
        }
        if let Some(patch) = object.get_mut("patch").and_then(Value::as_object_mut) {
            for (name, value) in patch.iter_mut() {
                if is_settings_secret_field(name) {
                    *value = Value::String("<redacted>".into());
                }
            }
        }
    }
    redacted
}

fn browser_error(error: BrowserError) -> RpcError {
    RpcError {
        code: "invalid_params".into(),
        message: error.to_string(),
        details: None,
    }
}

fn answer_request_approves(answer: &Value) -> bool {
    answer.get("option_index").and_then(Value::as_u64) == Some(0)
        || answer
            .get("option_indexes")
            .and_then(Value::as_array)
            .is_some_and(|indexes| indexes.iter().any(|index| index.as_u64() == Some(0)))
        || answer.get("text").and_then(Value::as_str) == Some("批准")
}

fn mutation_event_key(method: &str, request_id: Option<&str>, params: &Value) -> String {
    if method == "question.answer" {
        if let Some(question_id) = params.get("question_id").and_then(Value::as_str) {
            // A Question has exactly one committed answer, so event replay is
            // keyed by that Question even when a client generates a new RPC
            // request id for each retry.
            return format!("question-answer:{question_id}");
        }
    }
    match request_id {
        Some(request_id) => format!("rpc:{method}:{request_id}"),
        None => format!("op:{method}:{}", Uuid::new_v4()),
    }
}

fn legacy_event_key(method: &str, params: &Value, result: &Value) -> String {
    let id = result
        .get("bot")
        .or_else(|| result.get("project"))
        .or_else(|| result.get("assignment"))
        .or_else(|| result.get("message"))
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .or_else(|| params.get("bot_id").and_then(Value::as_str))
        .or_else(|| params.get("project_id").and_then(Value::as_str))
        .or_else(|| params.get("assignment_id").and_then(Value::as_str))
        .or_else(|| params.get("message_id").and_then(Value::as_str))
        .unwrap_or("unknown");
    format!("legacy:{method}:{id}")
}

fn operation_event_entity_present(method: &str, result: &Value) -> bool {
    match method {
        "bot.create" | "bot.update" | "bot.duplicate" => result["bot"].is_object(),
        "project.create"
        | "project.update"
        | "project.add_member"
        | "project.remove_member"
        | "project.confirm_done"
        | "project.request_review"
        | "project.archive"
        | "project.reopen"
        | "project.request_changes" => result["project"].is_object(),
        "assignment.create"
        | "assign"
        | "delegate"
        | "assignment.stop"
        | "assignment.recovered"
        | "assignment.retry"
        | "assignment.steer"
        | "steer" => result["assignment"].is_object() || result["id"].is_string(),
        "message.steer" => result["assignment"].is_object() && result["message"].is_object(),
        "send_msg" | "chat.send" | "chat.react" => {
            result["message"].is_object() || result["id"].is_string()
        }
        _ => true,
    }
}

fn event_data(method: &str, params: &Value, result: &Value) -> Value {
    match method {
        "bot.delete" => json!({ "bot_id": params.get("bot_id").cloned().unwrap_or(Value::Null) }),
        "bot.create" | "bot.update" | "bot.duplicate" => {
            json!({ "bot": result.get("bot").cloned().unwrap_or(Value::Null) })
        }
        "project.create"
        | "project.update"
        | "project.add_member"
        | "project.remove_member"
        | "project.confirm_done"
        | "project.request_review"
        | "project.archive"
        | "project.reopen" => {
            json!({ "project": result.get("project").cloned().unwrap_or(Value::Null) })
        }
        "announcement.update" => {
            json!({ "announcement": result.get("announcement").cloned().unwrap_or(Value::Null) })
        }
        "message.steer" => {
            json!({ "assignment": result.get("assignment").cloned().unwrap_or(Value::Null) })
        }
        "message.delete" => json!({
            "chat_id": params.get("chat_id").cloned().unwrap_or(Value::Null),
            "message_id": params.get("message_id").cloned().unwrap_or(Value::Null)
        }),
        "message.edit" => json!({
            "message": result.get("message").cloned().unwrap_or(Value::Null)
        }),
        "project.request_changes" => {
            json!({ "message": result.get("message").cloned().unwrap_or(Value::Null) })
        }
        "question.ask" | "propose_bot" => {
            json!({ "question": result.get("question").cloned().unwrap_or(Value::Null) })
        }
        "assignment.create"
        | "assign"
        | "delegate"
        | "assignment.stop"
        | "assignment.recovered"
        | "assignment.retry"
        | "assignment.steer"
        | "steer" => {
            json!({ "assignment": result.get("assignment").cloned().unwrap_or_else(|| result.clone()) })
        }
        "send_msg" | "chat.send" | "chat.react" => {
            json!({ "message": result.get("message").cloned().unwrap_or_else(|| result.clone()) })
        }
        "chat.set_pinned" | "chat.set_muted" => {
            json!({ "chat": result.get("chat").cloned().unwrap_or_else(|| result.clone()) })
        }
        "settings.update" => result.clone(),
        "chat.mark_read" => json!({
            "chat_id": params.get("chat_id").cloned().unwrap_or(Value::Null),
            "last_read_seq": params.get("seq").cloned().unwrap_or(Value::Null)
        }),
        "routine.delete" => {
            json!({ "routine_id": params.get("routine_id").cloned().unwrap_or(Value::Null) })
        }
        "routine.test_run" | "routine.execution" => {
            json!({"run": result.get("run").cloned().unwrap_or(Value::Null)})
        }
        _ => result.clone(),
    }
}

fn normalize_result(method: &str, mut result: Value) -> Result<Value, String> {
    match method {
        "bootstrap" => {
            if let Some(items) = result.get_mut("bots").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_bot(item);
                }
            }
            if let Some(items) = result.get_mut("projects").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_project(item);
                }
            }
        }
        "bot.list" => {
            if let Some(items) = result.get_mut("bots").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_bot(item);
                }
            }
        }
        "bot.get" | "bot.create" | "bot.update" | "bot.duplicate" => {
            if let Some(item) = result.get_mut("bot") {
                normalize_bot(item);
            }
            if matches!(method, "bot.create" | "bot.duplicate") {
                normalize_created_bot_chat(&mut result);
            }
        }
        "bot.create_from_template" => {
            if let Some(items) = result.get_mut("bots").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_bot(item);
                }
            }
            normalize_template_bot_chats(&mut result);
        }
        "project.get" => {
            if let Some(item) = result.get_mut("project") {
                normalize_project(item);
            }
            if let Some(item) = result.get_mut("announcement") {
                normalize_announcement(item);
            }
        }
        "announcement.update" => {
            if let Some(item) = result.get_mut("announcement") {
                normalize_announcement(item);
            }
        }
        "project.list" => {
            if let Some(items) = result.get_mut("projects").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_project(item);
                }
            }
        }
        "project.create" => {
            if let Some(item) = result.get_mut("project") {
                normalize_project(item);
                result["chat"] = project_chat(item);
            }
        }
        "project.add_member"
        | "project.remove_member"
        | "project.confirm_done"
        | "project.archive"
        | "project.reopen"
        | "project.status"
        | "project_status" => {
            if let Some(item) = result.get_mut("project") {
                normalize_project(item);
            }
        }
        "project.request_changes" => {
            if let Some(item) = result.get_mut("message") {
                normalize_message(item);
            }
        }
        "assignment.create" | "assign" | "delegate" => normalize_assignment(&mut result),
        "assignment.get" | "assignment.stop" | "assignment.retry" => {
            if let Some(item) = result.get_mut("assignment") {
                normalize_assignment(item);
            }
        }
        "message.steer" => {
            if let Some(item) = result.get_mut("assignment") {
                normalize_assignment(item);
            }
            if let Some(item) = result.get_mut("message") {
                normalize_message(item);
            }
        }
        "assignment.list" => {
            if let Some(items) = result.get_mut("items").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_assignment(item);
                }
            }
        }
        "send_msg" => normalize_message(&mut result),
        "project.request_review" => {
            if let Some(item) = result.get_mut("project") {
                normalize_project(item);
            }
        }
        "propose_bot" => {
            if let Some(item) = result.get_mut("question") {
                normalize_question(item);
            }
        }
        "question.ask" => {
            let mut question = result
                .get("question")
                .cloned()
                .unwrap_or_else(|| result.clone());
            normalize_question(&mut question);
            result = json!({"question": question});
        }
        "chat.send" => {
            if let Some(item) = result.get_mut("message") {
                normalize_message(item);
            }
        }
        "chat.thread" => {
            if let Some(item) = result.get_mut("root") {
                normalize_message(item);
            }
            if let Some(items) = result.get_mut("replies").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_message(item);
                }
            }
        }
        "chat.react" | "message.edit" => {
            if let Some(item) = result.get_mut("message") {
                normalize_message(item);
            }
        }
        "chat.set_pinned" | "chat.set_muted" => {
            if let Some(item) = result.get_mut("chat") {
                normalize_chat(item);
            }
        }
        "chat.history" => {
            if let Some(items) = result.get_mut("messages").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_message(item);
                }
            }
        }
        "approval.decide" => {
            if let Some(item) = result.get_mut("approval") {
                normalize_approval(item);
            }
        }
        "approval.list" => {
            if let Some(items) = result.get_mut("approvals").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_approval(item);
                }
            }
        }
        "question.answer" => {
            if let Some(item) = result.get_mut("question") {
                normalize_question(item);
            }
        }
        "question.list" => {
            if let Some(items) = result.get_mut("questions").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_question(item);
                }
            }
        }
        "routine.create" | "routine.update" | "routine.set_enabled" => {
            if let Some(item) = result.get_mut("routine") {
                normalize_routine(item);
            }
        }
        "routine.list" => {
            if let Some(items) = result.get_mut("routines").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_routine(item);
                }
            }
        }
        "routine.test_run" => {
            if let Some(item) = result.get_mut("run") {
                normalize_routine_run(item);
            }
        }
        "settings.get" | "settings.update" => {}
        _ => {}
    }
    validate_json_shape(method, &result)?;
    Ok(result)
}

fn normalize_usage_result(method: &str, result: &mut Value) {
    if method != "usage.heatmap" {
        return;
    }
    if let Some(days) = result.get_mut("days").and_then(Value::as_array_mut) {
        for day in days {
            if let Some(tokens) = day.get("tokens").and_then(Value::as_f64) {
                day["tokens"] = json!(tokens.max(0.0).round() as u64);
            }
        }
    }
}

fn obj(value: &mut Value) -> &mut Map<String, Value> {
    value
        .as_object_mut()
        .expect("normalizer only receives objects")
}
fn normalize_bot(value: &mut Value) {
    let o = obj(value);
    let id = o.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
    let avatar = o.get("avatar").cloned().unwrap_or(Value::Null);
    o.insert(
        "avatar".into(),
        match avatar {
            Value::Object(_) => avatar,
            Value::String(emoji) => json!({"kind":"emoji","emoji":emoji}),
            _ => json!({"kind":"bean","color":0}),
        },
    );
    o.entry("tools").or_insert_with(
        || json!({"files":true,"bash":true,"browser":true,"subagent":true,"web":true,"mcp":true}),
    );
    if o.get("browser_mode")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        o.insert("browser_mode".into(), json!("headless"));
    }
    let dm_chat_id = if id == "main" {
        "chat_main".to_owned()
    } else {
        o.get("dm_chat_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("dm_{id}"))
    };
    o.insert("dm_chat_id".into(), json!(dm_chat_id));
    o.entry("status")
        .or_insert_with(|| json!({"summary":"idle","active":0,"queued":0,"waiting":0}));
}

fn bot_is_main(bot: &Value) -> bool {
    bot.get("id").and_then(Value::as_str) == Some("main")
        || bot.get("is_main").and_then(Value::as_bool) == Some(true)
}

fn canonical_bot_dm_id(bot: &Value) -> String {
    if bot_is_main(bot) {
        return "chat_main".into();
    }
    bot.get("dm_chat_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            bot.get("id")
                .and_then(Value::as_str)
                .map(|id| format!("dm_{id}"))
        })
        .unwrap_or_else(|| "dm_unknown".into())
}

fn normalize_created_bot_chat(result: &mut Value) {
    let Some(bot) = result.get("bot").cloned() else {
        return;
    };
    let id = canonical_bot_dm_id(&bot);
    let kind = if bot_is_main(&bot) { "main" } else { "direct" };
    if let Some(chat) = result.get_mut("dm_chat").and_then(Value::as_object_mut) {
        complete_bot_chat(chat, &bot, &id, kind);
    }
}

fn normalize_template_bot_chats(result: &mut Value) {
    let Some(bots) = result.get("bots").and_then(Value::as_array).cloned() else {
        return;
    };
    let Some(chats) = result.get_mut("dm_chats").and_then(Value::as_array_mut) else {
        return;
    };
    for (bot, chat) in bots.iter().zip(chats.iter_mut()) {
        let id = canonical_bot_dm_id(bot);
        let kind = if bot_is_main(bot) { "main" } else { "direct" };
        if let Some(object) = chat.as_object_mut() {
            complete_bot_chat(object, bot, &id, kind);
        }
    }
}

fn complete_bot_chat(chat: &mut Map<String, Value>, bot: &Value, id: &str, kind: &str) {
    chat.insert("id".into(), json!(id));
    chat.insert("kind".into(), json!(kind));
    chat.insert("title".into(), bot.get("name").cloned().unwrap_or_default());
    chat.insert(
        "bot_id".into(),
        bot.get("id").cloned().unwrap_or(Value::Null),
    );
    chat.entry("project_id").or_insert(Value::Null);
    chat.entry("member_bot_ids").or_insert_with(|| json!([]));
    chat.entry("last_message").or_insert(Value::Null);
    chat.entry("last_seq").or_insert(json!(0));
    chat.entry("last_read_seq").or_insert(json!(0));
    chat.entry("unread").or_insert(json!(0));
    chat.entry("attention").or_insert(json!("none"));
    chat.entry("pinned")
        .or_insert_with(|| bot.get("pinned").cloned().unwrap_or(json!(false)));
    chat.entry("muted").or_insert(json!(false));
    chat.entry("updated_at").or_insert_with(|| {
        bot.get("updated_at")
            .cloned()
            .unwrap_or_else(|| json!(now()))
    });
}

fn apply_chat_overlay(chat: &mut Value, overlay: &Value) {
    if let Some(value) = overlay.get("pinned").and_then(Value::as_bool) {
        chat["pinned"] = json!(value);
    }
    if let Some(value) = overlay.get("muted").and_then(Value::as_bool) {
        chat["muted"] = json!(value);
    }
    if let Some(value) = overlay.get("last_read_seq").and_then(Value::as_u64) {
        chat["last_read_seq"] = json!(value);
    }
}
fn normalize_sender(value: &mut Value) {
    if let Some(s) = value.as_str().map(str::to_owned) {
        *value = if s == "user" {
            json!({"kind":"user"})
        } else if s == "system" {
            json!({"kind":"system"})
        } else {
            json!({"kind":"bot","bot_id":s})
        };
    }
}
fn normalize_project(value: &mut Value) {
    let o = obj(value);
    if let Some(sender) = o.get_mut("created_by") {
        normalize_sender(sender);
    }
}
fn normalize_announcement(value: &mut Value) {
    let o = obj(value);
    if let Some(sender) = o.get_mut("text_updated_by") {
        normalize_sender(sender);
    }
    o.entry("text").or_insert_with(|| json!(""));
    o.entry("version").or_insert(json!(1));
    o.entry("text_updated_by").or_insert(json!({"kind":"user"}));
    let updated_at = o.get("updated_at").cloned().unwrap_or_else(|| json!(now()));
    o.entry("text_updated_at").or_insert(updated_at);
}
fn normalize_assignment(value: &mut Value) {
    let o = obj(value);
    if let Some(sender) = o.get_mut("from") {
        normalize_sender(sender);
    }
    if o.get("model").is_none_or(Value::is_null) {
        o.insert("model".into(), json!(""));
    }
    o.entry("usage").or_insert_with(||json!({"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"requests":0,"cost":null}));
    if let Some(usage) = o.get_mut("usage") {
        obj(usage).entry("requests").or_insert(json!(0));
    }
}
fn normalize_chat(value: &mut Value) {
    let _ = obj(value);
}

async fn append_project_card_blocks(
    backend: &ProductionBackend,
    message: &mut Value,
    params: &Value,
) -> Result<(), RpcError> {
    let Some(projects) = params.get("projects") else {
        return Ok(());
    };
    if projects.is_null() {
        return Ok(());
    }
    let projects = projects.as_array().ok_or_else(|| RpcError {
        code: "invalid_params".into(),
        message: "projects must be an array".into(),
        details: None,
    })?;
    if projects.len() > 8 {
        return Err(RpcError {
            code: "invalid_params".into(),
            message: "at most 8 project cards may be attached to one message".into(),
            details: None,
        });
    }
    let bot_id = required_text(params, "bot_id")?;
    let mut cards = Vec::with_capacity(projects.len());
    let mut seen = HashSet::new();
    for item in projects {
        if !item.is_object() {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "each project card must be an object".into(),
                details: None,
            });
        }
        let project_id = item
            .get("project_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "project card project_id is required".into(),
                details: None,
            })?;
        let note = match item.get("note") {
            None | Some(Value::Null) => None,
            Some(Value::String(note)) => Some(note.clone()),
            Some(_) => {
                return Err(RpcError {
                    code: "invalid_params".into(),
                    message: "project card note must be a string or null".into(),
                    details: None,
                });
            }
        };
        if !seen.insert(project_id) {
            continue;
        }
        let result = backend
            .orchestrator
            .rpc("project.get", json!({"project_id":project_id}))
            .await
            .map_err(ProductionBackend::error)?;
        let project = result.get("project").ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "project lookup returned no project".into(),
            details: None,
        })?;
        let is_member = project
            .get("members")
            .and_then(Value::as_array)
            .is_some_and(|members| {
                members.iter().any(|member| {
                    member.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str())
                })
            });
        if !is_member {
            return Err(RpcError {
                code: "forbidden".into(),
                message: format!("Bot {bot_id} is not a member of project {project_id}"),
                details: None,
            });
        }
        cards.push(json!({
            "type":"project_card",
            "project_id":project_id,
            "context":"mentioned",
            "note":note
        }));
    }
    if !cards.is_empty() {
        let blocks = message
            .get_mut("blocks")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RpcError {
                code: "internal".into(),
                message: "normalized send_msg has no blocks array".into(),
                details: None,
            })?;
        blocks.extend(cards);
    }
    Ok(())
}

fn append_link_preview_blocks(message: &mut Value, params: &Value) -> Result<(), RpcError> {
    let Some(previews) = params.get("link_previews") else {
        return Ok(());
    };
    if previews.is_null() {
        return Ok(());
    }
    let previews = previews.as_array().ok_or_else(|| RpcError {
        code: "invalid_params".into(),
        message: "link_previews must be an array".into(),
        details: None,
    })?;
    if previews.len() > 8 {
        return Err(RpcError {
            code: "invalid_params".into(),
            message: "at most 8 link previews may be attached to one message".into(),
            details: None,
        });
    }
    let mut blocks = Vec::with_capacity(previews.len());
    for preview in previews {
        if !preview.is_object() {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "each link preview must be an object".into(),
                details: None,
            });
        }
        let url = preview
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "link preview url is required".into(),
                details: None,
            })?;
        if url.len() > 2_048 {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "link preview url is too long".into(),
                details: None,
            });
        }
        let parsed = reqwest::Url::parse(url).map_err(|_| RpcError {
            code: "invalid_params".into(),
            message: "link preview url is invalid".into(),
            details: None,
        })?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "link preview url must be an http(s) URL without credentials".into(),
                details: None,
            });
        }
        let host = parsed.host_str().unwrap_or_default();
        for field in ["site", "title", "description"] {
            if preview
                .get(field)
                .is_some_and(|value| !value.is_null() && !value.is_string())
            {
                return Err(RpcError {
                    code: "invalid_params".into(),
                    message: format!("link preview {field} must be a string"),
                    details: None,
                });
            }
        }
        let site = preview
            .get("site")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(host);
        let title = preview
            .get("title")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(host);
        let description = preview.get("description").and_then(Value::as_str);
        if site.chars().count() > 128
            || title.chars().count() > 256
            || description.is_some_and(|value| value.chars().count() > 1_024)
            || [site, title]
                .iter()
                .any(|value| value.chars().any(char::is_control))
            || description.is_some_and(|value| value.chars().any(char::is_control))
        {
            return Err(RpcError {
                code: "invalid_params".into(),
                message: "link preview metadata is too long or contains control characters".into(),
                details: None,
            });
        }
        let image = match preview.get("image") {
            None | Some(Value::Null) => Value::Null,
            Some(value) => {
                let file: FileRef =
                    serde_json::from_value(value.clone()).map_err(|_| RpcError {
                        code: "invalid_params".into(),
                        message: "link preview image must be a FileRef".into(),
                        details: None,
                    })?;
                if file.path.starts_with('/')
                    || file
                        .path
                        .split('/')
                        .any(|part| matches!(part, "" | "." | ".."))
                    || file.path.chars().any(char::is_control)
                {
                    return Err(RpcError {
                        code: "invalid_params".into(),
                        message: "link preview image path is invalid".into(),
                        details: None,
                    });
                }
                serde_json::to_value(file).expect("FileRef is serializable")
            }
        };
        blocks.push(json!({
            "type":"link_preview", "url":url, "site":site, "title":title,
            "description":description, "image":image
        }));
    }
    if let Some(message) = message.get_mut("message") {
        let target = message;
        let Some(blocks_target) = target.get_mut("blocks").and_then(Value::as_array_mut) else {
            return Err(RpcError {
                code: "internal".into(),
                message: "send_msg result has no blocks array".into(),
                details: None,
            });
        };
        blocks_target.extend(blocks);
    } else if let Some(blocks_target) = message.get_mut("blocks").and_then(Value::as_array_mut) {
        blocks_target.extend(blocks);
    } else {
        return Err(RpcError {
            code: "internal".into(),
            message: "send_msg result has no blocks array".into(),
            details: None,
        });
    }
    Ok(())
}

fn deduplicate_review_card_artifacts(message: &mut Value) {
    let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("review_card") {
            continue;
        }
        let Some(artifacts) = block.get_mut("artifacts").and_then(Value::as_array_mut) else {
            continue;
        };
        let mut positions = HashMap::<String, usize>::new();
        let mut unique = Vec::with_capacity(artifacts.len());
        for artifact in artifacts.drain(..) {
            let Some(path) = artifact
                .get("path_or_url")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())
            else {
                unique.push(artifact);
                continue;
            };
            if let Some(position) = positions.get(path).copied() {
                // Keep the latest title and artifact id from the historical
                // card while collapsing repeated registrations of the same
                // logical path.
                unique[position] = artifact;
            } else {
                positions.insert(path.to_owned(), unique.len());
                unique.push(artifact);
            }
        }
        *artifacts = unique;
    }
}

/// Copy only artifacts that are part of this completion and visible from its
/// return chat. Project results stay inside their project; private artifacts
/// can be surfaced to the user's main chat, but never leaked into another
/// Bot's private DM.
fn assignment_result_artifacts(
    snapshot: &Value,
    canonical: &Value,
    assignment_id: &str,
    target_chat_id: &str,
) -> Vec<Value> {
    let target_project_id = snapshot
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| {
            projects.iter().find_map(|(project_id, project)| {
                (project.get("chat_id").and_then(Value::as_str) == Some(target_chat_id))
                    .then_some(project_id.as_str())
            })
        });
    let target_dm_bot = snapshot
        .get("bots")
        .and_then(Value::as_object)
        .and_then(|bots| {
            bots.values()
                .find(|bot| bot.get("dm_chat_id").and_then(Value::as_str) == Some(target_chat_id))
        });
    let Some(requested) = canonical.get("artifacts").and_then(Value::as_array) else {
        return Vec::new();
    };
    let Some(artifacts) = snapshot.get("artifacts").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut copied = Vec::new();
    let mut seen = HashSet::new();
    for reference in requested {
        let Some(path_or_url) = reference.get("path_or_url").and_then(Value::as_str) else {
            continue;
        };
        let title = reference.get("title").and_then(Value::as_str).unwrap_or("");
        let found = artifacts.iter().find(|(_, artifact)| {
            artifact.get("assignment_id").and_then(Value::as_str) == Some(assignment_id)
                && artifact.get("path_or_url").and_then(Value::as_str) == Some(path_or_url)
                && artifact.get("title").and_then(Value::as_str) == Some(title)
        });
        let Some((artifact_id, artifact)) = found else {
            continue;
        };
        let project_id = artifact.get("project_id").and_then(Value::as_str);
        let visible = if let Some(target_project_id) = target_project_id {
            project_id == Some(target_project_id)
        } else if target_chat_id == "chat_main" {
            project_id.is_none()
        } else if let Some(target_bot_id) = target_dm_bot
            .and_then(|bot| bot.get("id"))
            .and_then(Value::as_str)
        {
            project_id.is_none()
                && artifact.get("bot_id").and_then(Value::as_str) == Some(target_bot_id)
        } else {
            false
        };
        if !visible || !seen.insert(artifact_id) {
            continue;
        }
        copied.push(json!({
            "artifact_id":artifact_id,
            "title":artifact.get("title").cloned().unwrap_or_else(|| json!(title)),
            "path_or_url":path_or_url,
            "bot_id":artifact.get("bot_id").cloned().unwrap_or(Value::Null),
            "kind":artifact.get("kind").cloned().unwrap_or_else(|| json!("file"))
        }));
    }
    copied
}

/// `next` is derived from canonical message mentions, never arbitrary client
/// completion payloads. Keep it within the return chat's existing audience.
fn assignment_result_next(
    snapshot: &Value,
    canonical: &Value,
    summary: &str,
    target_chat_id: &str,
) -> Vec<Value> {
    let target_project = snapshot
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| {
            projects.values().find(|project| {
                project.get("chat_id").and_then(Value::as_str) == Some(target_chat_id)
            })
        });
    let target_dm_bot_id = snapshot
        .get("bots")
        .and_then(Value::as_object)
        .and_then(|bots| {
            bots.values().find_map(|bot| {
                (bot.get("dm_chat_id").and_then(Value::as_str) == Some(target_chat_id))
                    .then(|| bot.get("id").and_then(Value::as_str))
                    .flatten()
            })
        });
    let Some(mentions) = canonical.get("mentions").and_then(Value::as_array) else {
        return Vec::new();
    };
    let Some(bots) = snapshot.get("bots").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut next = Vec::new();
    let mut seen = HashSet::new();
    for mention in mentions {
        if mention.get("kind").and_then(Value::as_str) != Some("bot") {
            continue;
        }
        let Some(bot_id) = mention.get("bot_id").and_then(Value::as_str) else {
            continue;
        };
        if bot_id == "main" || !seen.insert(bot_id) {
            continue;
        }
        let Some(bot) = bots.get(bot_id) else {
            continue;
        };
        let visible = if let Some(project) = target_project {
            project
                .get("members")
                .and_then(Value::as_array)
                .is_some_and(|members| {
                    members
                        .iter()
                        .filter(|member| {
                            member.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                        })
                        .count()
                        == 1
                })
        } else if target_chat_id == "chat_main" {
            true
        } else {
            target_dm_bot_id == Some(bot_id)
        };
        if !visible {
            continue;
        }
        let instruction = mention
            .get("instruction")
            .and_then(Value::as_str)
            .filter(|instruction| !instruction.trim().is_empty())
            .unwrap_or(summary);
        next.push(json!({"bot_id":bot.get("id").cloned().unwrap_or_else(|| json!(bot_id)),"instruction":instruction}));
    }
    next
}

fn normalize_message(value: &mut Value) {
    let o = obj(value);
    o.entry("seq").or_insert(json!(0));
    if let Some(sender) = o.get_mut("sender") {
        normalize_sender(sender);
    }
    if let Some(quote) = o.get_mut("quote").and_then(Value::as_object_mut) {
        if let Some(sender) = quote.get_mut("sender") {
            normalize_sender(sender);
        }
    }
    o.entry("edited_at").or_insert(Value::Null);
    o.entry("deleted").or_insert(json!(false));
    o.entry("reply_to").or_insert(Value::Null);
    o.entry("quote").or_insert(Value::Null);
    o.entry("thread_count").or_insert(json!(0));
    o.entry("streaming").or_insert(json!(false));
    o.entry("reactions").or_insert(json!([]));
    o.entry("delivery").or_insert(json!([]));
    o.entry("loaded_skills").or_insert(json!([]));
    let intent = o.get("intent").and_then(Value::as_str).unwrap_or("ack");
    let text = o
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| o.get("fallback_text").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    let block = match intent {
        "decision" if o.get("question_id").and_then(Value::as_str).is_some() => {
            json!({"type":"question","question_id":o["question_id"]})
        }
        "progress" => json!({"type":"progress","text":text}),
        "blocked" => json!({"type":"blocked","reason":text}),
        "task_stopped" => {
            o.insert("intent".into(), Value::Null);
            json!({"type":"system","code":"task_stopped","text":text})
        }
        "done" => {
            json!({"type":"completion","summary":text,"artifacts":[],"next":[],"notify_main":true})
        }
        _ => json!({"type":"text","markdown":text}),
    };
    // The internal question id is authoritative even for legacy text blocks.
    if block.get("type").and_then(Value::as_str) == Some("question") {
        o.insert("blocks".into(), json!([block]));
    }
    o.remove("question_id");
    let mut repaired_empty_text = false;
    if let Some(blocks) = o.get_mut("blocks").and_then(Value::as_array_mut) {
        if !text.is_empty() {
            for existing in blocks.iter_mut() {
                if existing.get("type").and_then(Value::as_str) == Some("text")
                    && existing.get("markdown").and_then(Value::as_str) == Some("")
                {
                    existing["markdown"] = json!(text);
                    repaired_empty_text = true;
                    break;
                }
            }
        }
    }
    if !repaired_empty_text
        && o.get("blocks")
            .and_then(Value::as_array)
            .is_none_or(|blocks| blocks.is_empty())
    {
        o.insert("blocks".into(), json!([block]));
    }
    o.remove("text");
    if let Some(delivery) = o.get_mut("delivery").and_then(Value::as_array_mut) {
        for d in delivery {
            if let Some(x) = d.as_object_mut() {
                x.entry("state").or_insert(json!("queued"));
            }
        }
    }
}
fn normalize_approval(value: &mut Value) {
    let _ = obj(value);
}
fn normalize_question(value: &mut Value) {
    let o = obj(value);
    o.entry("multi").or_insert(json!(false));
    if let Some(answer) = o.get_mut("answer").and_then(Value::as_object_mut) {
        answer.entry("option_indexes").or_insert(json!([]));
    }
}
fn normalize_routine(value: &mut Value) {
    let _ = obj(value);
    if let Some(run) = value.as_object_mut().and_then(|x| x.get_mut("last_run")) {
        if run.is_object() {
            normalize_routine_run(run);
        }
    }
}
fn normalize_routine_run(value: &mut Value) {
    let _ = obj(value);
}

fn merge_json(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (key, value) in patch {
        if let (Some(existing), Some(update)) = (target.get_mut(key), value.as_object()) {
            if let Some(existing) = existing.as_object_mut() {
                merge_json(existing, update);
                continue;
            }
        }
        target.insert(key.clone(), value.clone());
    }
}

fn scheduler_limits(settings: &Value) -> Result<OrchestratorSettings, serde_json::Error> {
    let concurrency: macbot_protocol::Concurrency =
        serde_json::from_value(settings.get("concurrency").cloned().unwrap_or_else(|| {
            json!({
                "global":8,"bot_default":3,"subagent_per_run":4,"subagent_global":12,"loop_hops":8
            })
        }))?;
    Ok(OrchestratorSettings {
        global_limit: concurrency.global.max(1) as usize,
        bot_default_limit: concurrency.bot_default.max(1) as usize,
        subagent_per_run: concurrency.subagent_per_run.max(1) as usize,
        subagent_global: concurrency.subagent_global.max(1) as usize,
        loop_hops: concurrency.loop_hops as usize,
    })
}

fn ensure_settings_tool_defaults(settings: &mut Value) -> Result<ToolToggles, String> {
    let default_fields = serde_json::to_value(ToolToggles::default())
        .map_err(|error| error.to_string())?
        .as_object()
        .cloned()
        .ok_or_else(|| "default tool settings must be an object".to_owned())?;
    let settings = settings
        .as_object_mut()
        .ok_or_else(|| "settings must be an object".to_owned())?;
    let tools = settings.entry("tools").or_insert_with(|| json!({}));
    let tools = tools
        .as_object_mut()
        .ok_or_else(|| "settings.tools must be an object".to_owned())?;
    let defaults = tools
        .entry("defaults")
        .or_insert_with(|| Value::Object(default_fields.clone()));
    let defaults = defaults
        .as_object_mut()
        .ok_or_else(|| "settings.tools.defaults must be an object".to_owned())?;
    for (key, value) in default_fields {
        defaults.entry(key).or_insert(value);
    }
    serde_json::from_value(Value::Object(defaults.clone())).map_err(|error| error.to_string())
}

/// Older snapshots did not record whether an approval could become a
/// persistent allow rule. Preserve that behavior for ordinary calls, while
/// applying the current built-in ask-first guard where the old detail retains
/// enough information to classify the call. An old rm -rf approval is
/// conservatively ask-first because its original working directory was not
/// persisted and cannot be checked against MACBOT_HOME.
fn migrate_legacy_approval_support(snapshot: &mut Value) {
    let Some(approvals) = snapshot.get_mut("approvals").and_then(Value::as_object_mut) else {
        return;
    };
    for approval in approvals.values_mut().filter_map(Value::as_object_mut) {
        if approval.contains_key("always_allow_supported") {
            continue;
        }
        let tool = approval
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let detail = approval.get("detail").and_then(Value::as_str);
        let args = detail.and_then(|detail| serde_json::from_str::<Value>(detail).ok());
        let mut builtin_ask_first =
            matches!(tool, "browser_eval" | "pay" | "payment") || tool.contains("payment");
        if let Some(command) = args
            .as_ref()
            .and_then(|args| args.get("command"))
            .and_then(Value::as_str)
        {
            let tokens = command.split_whitespace().collect::<Vec<_>>();
            builtin_ask_first |= tokens
                .iter()
                .any(|token| *token == "sudo" || token.ends_with("/sudo"))
                || tokens.windows(2).any(|pair| pair == ["git", "push"]);

            if let Some(rm_index) = tokens
                .iter()
                .position(|token| *token == "rm" || token.ends_with("/rm"))
            {
                let mut recursive = false;
                let mut force = false;
                let mut options_done = false;
                for token in tokens.iter().skip(rm_index + 1) {
                    if !options_done && *token == "--" {
                        options_done = true;
                        continue;
                    }
                    if !options_done && token.starts_with('-') {
                        recursive |= *token == "--recursive" || token[1..].contains('r');
                        force |= *token == "--force" || token[1..].contains('f');
                        continue;
                    }
                    if recursive && force {
                        builtin_ask_first = true;
                        break;
                    }
                }
            }
        }
        approval.insert("always_allow_supported".into(), json!(!builtin_ask_first));
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

fn host_permissions() -> Value {
    #[cfg(target_os = "macos")]
    let (screen_recording, accessibility) = unsafe {
        (
            if CGPreflightScreenCaptureAccess() {
                "granted"
            } else {
                "denied"
            },
            if AXIsProcessTrusted() {
                "granted"
            } else {
                "denied"
            },
        )
    };
    #[cfg(not(target_os = "macos"))]
    let (screen_recording, accessibility) = ("unknown", "unknown");

    // macOS does not provide a non-invasive public preflight API for Full
    // Disk Access or per-target Apple Events authorization. Keep these as
    // unknown instead of probing protected user data or triggering a prompt.
    json!({
        "screen_recording":screen_recording,
        "accessibility":accessibility,
        "full_disk_access":"unknown",
        "automation_chrome":"unknown"
    })
}

fn builtin_approval_rules() -> Vec<Value> {
    [
        ("builtin-sudo", "sudo"),
        ("builtin-home-external-rm", "Home 外删除"),
        ("builtin-git-push", "git push"),
        ("builtin-browser-payment", "浏览器付款"),
        ("builtin-browser-eval", "browser_eval"),
    ]
    .into_iter()
    .map(|(id, text)| {
        json!({
            "id":id,
            "kind":"ask_first",
            "text":text,
            "created_at":"2026-10-11T00:00:00Z",
            "bot_id":null,
            "builtin":true
        })
    })
    .collect()
}

fn ensure_builtin_approval_rules(settings: &mut Value) {
    let Some(approvals) = settings.get_mut("approvals").and_then(Value::as_object_mut) else {
        return;
    };
    let rules = approvals.entry("rules").or_insert_with(|| json!([]));
    let Some(rules) = rules.as_array_mut() else {
        return;
    };
    for builtin in builtin_approval_rules() {
        let id = builtin.get("id");
        if let Some(existing) = rules.iter_mut().find(|rule| rule.get("id") == id) {
            *existing = builtin;
        } else {
            rules.push(builtin);
        }
    }
}

fn protect_builtin_approval_rules(
    current: &Value,
    candidate: &Value,
    patch: &serde_json::Map<String, Value>,
) -> Result<(), RpcError> {
    if patch
        .get("approvals")
        .and_then(|approvals| approvals.get("rules"))
        .is_none()
    {
        return Ok(());
    }
    let existing = current
        .pointer("/approvals/rules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter(|rule| rule.get("builtin").and_then(Value::as_bool) == Some(true))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let proposed = candidate
        .pointer("/approvals/rules")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: "settings.approvals.rules must be an array".into(),
            details: None,
        })?;
    for builtin in &existing {
        if !proposed.iter().any(|rule| rule == *builtin) {
            return Err(RpcError {
                code: "forbidden".into(),
                message: "built-in approval rules are read-only".into(),
                details: None,
            });
        }
    }
    for rule in proposed
        .iter()
        .filter(|rule| rule.get("builtin").and_then(Value::as_bool) == Some(true))
    {
        if !existing.contains(&rule) {
            return Err(RpcError {
                code: "forbidden".into(),
                message: "built-in approval rules are read-only".into(),
                details: None,
            });
        }
    }
    Ok(())
}

fn default_settings(host_name: &str) -> Value {
    json!({
        "host_name":host_name,"timezone":"Asia/Shanghai","currency":"CNY",
        "concurrency":{"global":8,"bot_default":3,"subagent_per_run":4,"subagent_global":12,"loop_hops":8},
        "models":{"bot_default":null,"main":null,"subagent":"inherit","maintenance":null},
        "tools":{"defaults":{"files":true,"bash":true,"browser":true,"subagent":true,"web":true,"mcp":false}},
        "main_bot":{"auto_create_project":true},"approvals":{"mode":"always_allow","rules":[]},
        "browser":{"default_mode":"headless","chrome_profile":"Default","stream":{"desktop":{"max_width":1280,"quality":70,"max_fps":15},"mobile":{"max_width":720,"quality":50,"max_fps":10}}},
        "skills":{"extra_dirs":[]},"trace":{"save_full_requests":false},
        "web_search":{"provider":null,"endpoint":null,"has_key":false},"push":{"apns_configured":false}
    })
}

/// Fill settings omitted by older or partial snapshots while preserving all
/// stored values, including unknown forward-compatible fields.
fn merge_missing_settings(value: &mut Value, defaults: Value) {
    let (Some(value), Some(defaults)) = (value.as_object_mut(), defaults.as_object()) else {
        return;
    };
    for (key, default) in defaults {
        match value.get_mut(key) {
            Some(existing) if existing.is_object() && default.is_object() => {
                merge_missing_settings(existing, default.clone());
            }
            Some(_) => {}
            None => {
                value.insert(key.clone(), default.clone());
            }
        }
    }
}

fn required_text(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| RpcError {
            code: "invalid_params".into(),
            message: format!("{key} is required"),
            details: None,
        })
}

fn ensure_user_message(message: &Value) -> Result<(), RpcError> {
    if message.pointer("/sender/kind").and_then(Value::as_str) == Some("user") {
        return Ok(());
    }
    Err(RpcError {
        code: "forbidden".into(),
        message: "only your own messages can be changed".into(),
        details: None,
    })
}

fn announcement_path_component(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .collect()
}

fn takeover_component(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty() {
        output.push_str("takeover");
    }
    output
}

fn artifact_history_component(value: &str) -> Result<String, RpcError> {
    if is_normal_component(value) {
        Ok(value.to_owned())
    } else {
        Err(rpc_error("invalid_params", "artifact_id is invalid", None))
    }
}

fn artifact_version_log_path(artifact_id: &str) -> Result<String, RpcError> {
    let component = artifact_history_component(artifact_id)?;
    Ok(format!("data/artifact-history/{component}/versions.jsonl"))
}

fn is_normal_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://") || value.contains("://")
}

fn is_sensitive_artifact_path(value: &str) -> bool {
    let Some(name) = Path::new(value).file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let name = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or_default();
    name == ".env"
        || name.starts_with(".env.")
        || matches!(stem, "id_rsa" | "id_ed25519" | "credentials" | "secrets")
        || matches!(
            Path::new(&name)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("pem" | "key" | "p12" | "pfx" | "keystore" | "kdbx")
        )
}

fn resolve_project_artifact_file(project_root: &Path, path: &str) -> Option<std::path::PathBuf> {
    let root = fs::canonicalize(project_root).ok()?;
    let path = Path::new(path);
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let candidate = fs::canonicalize(candidate).ok()?;
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return None;
    }
    Some(candidate)
}

fn main_chat() -> Value {
    json!({"id":"chat_main","kind":"main","title":"主会话","bot_id":null,"project_id":null,"member_bot_ids":["main"],"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":true,"muted":false,"updated_at":now()})
}

fn project_chat(project: &Value) -> Value {
    json!({"id":project["chat_id"],"kind":"project","title":project["name"],"bot_id":null,"project_id":project["id"],"member_bot_ids":project["members"].as_array().map(|items| items.iter().filter_map(|item| item["bot_id"].as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default(),"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":false,"muted":false,"updated_at":project["updated_at"]})
}

fn validate_json_shape(method: &str, value: &Value) -> Result<(), String> {
    macro_rules! parse {
        ($ty:ty, $value:expr) => {{
            serde_json::from_value::<$ty>($value.clone())
                .map(|_| ())
                .map_err(|e| format!("{method}: {e}"))?;
        }};
    }
    match method {
        "ping" => parse!(macbot_protocol::PingResult, value),
        "bootstrap" => {
            parse!(Hello, value["hello"]);
            parse!(Vec<Bot>, value["bots"]);
            parse!(Vec<Chat>, value["chats"]);
            parse!(Vec<Project>, value["projects"]);
            parse!(Settings, value["settings"]);
            parse!(PendingItems, value["pending"]);
        }
        "bot.list" => parse!(Vec<Bot>, value["bots"]),
        "bot.get" | "bot.create" | "bot.update" => parse!(Bot, value["bot"]),
        "bot.duplicate" => parse!(BotDuplicateResult, value),
        "project.list" => parse!(Vec<Project>, value["projects"]),
        "project.get" => {
            parse!(Project, value["project"]);
            parse!(Announcement, value["announcement"]);
        }
        "project.create" => {
            parse!(Project, value["project"]);
            parse!(Chat, value["chat"]);
        }
        "project.add_member"
        | "project.remove_member"
        | "project.confirm_done"
        | "project.archive"
        | "project.reopen"
        | "project.status"
        | "project_status" => parse!(Project, value["project"]),
        "project.request_changes" => parse!(Message, value["message"]),
        "bot.create_from_template" => {
            parse!(Vec<Bot>, value["bots"]);
            parse!(Vec<Chat>, value["dm_chats"]);
        }
        "assignment.create" | "assign" | "delegate" => parse!(Assignment, value),
        "assignment.get" | "assignment.stop" | "assignment.retry" => {
            parse!(Assignment, value["assignment"])
        }
        "assignment.list" => parse!(Vec<Assignment>, value["items"]),
        "send_msg" => parse!(Message, value),
        "project.request_review" => {
            parse!(Project, value["project"]);
        }
        "propose_bot" => parse!(Question, value["question"]),
        "question.ask" => parse!(Question, value["question"]),
        "chat.send" => parse!(Message, value["message"]),
        "chat.history" => parse!(Vec<Message>, value["messages"]),
        "chat.thread" => {
            parse!(Message, value["root"]);
            parse!(Vec<Message>, value["replies"]);
        }
        "chat.react" => parse!(Message, value["message"]),
        "chat.set_pinned" | "chat.set_muted" => parse!(Chat, value["chat"]),
        "chat.list" => parse!(Vec<Chat>, value["chats"]),
        "chat.get" => parse!(Chat, value["chat"]),
        "settings.get" | "settings.update" => parse!(Settings, value["settings"]),
        "usage.summary" => parse!(UsageSummaryResult, value),
        "usage.heatmap" => parse!(HeatmapResult, value),
        "usage.timeseries" => parse!(UsageTimeseriesResult, value),
        "usage.breakdown" => parse!(UsageBreakdownResult, value),
        "workbench.get" => parse!(macbot_protocol::WorkbenchResult, value),
        "device.register" => parse!(Device, value["device"]),
        "approval.decide" => parse!(Approval, value["approval"]),
        "approval.list" => parse!(Vec<Approval>, value["approvals"]),
        "question.list" => parse!(Vec<Question>, value["questions"]),
        "question.answer" => parse!(Question, value["question"]),
        "routine.create" | "routine.update" | "routine.set_enabled" => {
            parse!(Routine, value["routine"])
        }
        _ => {}
    }
    Ok(())
}

fn validate_result(method: &str, value: &Value) -> Result<(), String> {
    validate_json_shape(method, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_review_card_artifacts_are_deduplicated_on_read() {
        let mut message = json!({
            "blocks":[{
                "type":"review_card",
                "artifacts":[
                    {"artifact_id":"old-1","title":"旧标题","path_or_url":"runs/report.md"},
                    {"artifact_id":"other","title":"另一份","path_or_url":"runs/other.md"},
                    {"artifact_id":"new-1","title":"最新标题","path_or_url":"runs/report.md"}
                ]
            }]
        });
        deduplicate_review_card_artifacts(&mut message);
        assert_eq!(
            message["blocks"][0]["artifacts"].as_array().unwrap().len(),
            2
        );
        assert_eq!(message["blocks"][0]["artifacts"][0]["artifact_id"], "new-1");
        assert_eq!(message["blocks"][0]["artifacts"][0]["title"], "最新标题");
        assert_eq!(
            message["blocks"][0]["artifacts"][1]["path_or_url"],
            "runs/other.md"
        );
    }
    use crate::{Gateway, GatewayConfig};
    use chrono::Duration;
    use macbot_browser::{BrowserMode, SessionConfig};
    use macbot_protocol::{
        Assignment as WireAssignment, Bot as WireBot, Chat as WireChat, HeatmapResult,
        Message as WireMessage, Project as WireProject, Provider as WireProvider,
        ProviderResult as WireProviderResult, UsageBreakdownResult, UsageSummaryResult,
        UsageTimeseriesResult,
    };
    use macbot_usage::{Totals, UsageRecord};
    use tempfile::tempdir;

    #[tokio::test]
    async fn loaded_skill_updates_source_message_history_and_replay_once_per_bot_assignment() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let sent = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":"chat_main",
                    "text":"/review-check apply these rules",
                    "mentions":[],
                    "client_request_id":"loaded-skill-source-message"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let source = sent["message"]["id"].as_str().unwrap();
        let mut live_events = gateway.state.events.subscribe();
        for (bot_id, assignment_id) in [
            ("bot_main", Some("asg_review_main")),
            ("bot_review", Some("asg_review_worker")),
        ] {
            backend
                .record_loaded_skill(
                    &gateway.state,
                    "chat_main",
                    source,
                    json!({
                        "name":"review-check",
                        "bot_id":bot_id,
                        "assignment_id":assignment_id,
                        "loaded_at":"2026-10-12T01:02:03.000Z"
                    }),
                )
                .await
                .unwrap();
        }
        // Retrying the same successful load must not append another record or
        // create another durable update event.
        backend
            .record_loaded_skill(
                &gateway.state,
                "chat_main",
                source,
                json!({
                    "name":"review-check",
                    "bot_id":"bot_main",
                    "assignment_id":"asg_review_main",
                    "loaded_at":"2026-10-12T01:02:03.000Z"
                }),
            )
            .await
            .unwrap();

        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let restored = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == source)
            .unwrap();
        assert_eq!(restored["loaded_skills"].as_array().unwrap().len(), 2);
        let wire_messages: Vec<macbot_protocol::Message> =
            serde_json::from_value(history["messages"].clone()).unwrap();
        let typed = wire_messages
            .iter()
            .find(|message| message.id == source)
            .unwrap();
        assert_eq!(typed.loaded_skills.len(), 2);
        let events = backend.store.events_since(0).unwrap();
        let updates = events
            .iter()
            .filter(|event| {
                event.event == "message.updated" && event.data["message"]["id"] == source
            })
            .collect::<Vec<_>>();
        assert_eq!(updates.len(), 2);
        assert_eq!(
            updates.last().unwrap().data["message"]["loaded_skills"],
            restored["loaded_skills"]
        );
        let mut live_updated = 0;
        while let Ok(event) = live_events.try_recv() {
            if event["event"] == "message.updated" && event["data"]["message"]["id"] == source {
                live_updated += 1;
            }
        }
        assert_eq!(live_updated, 2);
    }

    #[test]
    fn project_create_result_and_events_use_the_protocol_shapes() {
        let project = json!({
            "id":"project-shape",
            "chat_id":"chat_project-shape",
            "name":"Shape",
            "slug":"shape",
            "goal":"check wire shape",
            "flow":[],
            "deadline":null,
            "home_path":"~/MacBot/projects/shape/",
            "status":"active",
            "lead_bot_id":"main",
            "members":[{"bot_id":"main","role_note":"","joined_at":"2026-10-09T00:00:00Z"}],
            "created_by":{"kind":"user"},
            "created_at":"2026-10-09T00:00:00Z",
            "updated_at":"2026-10-09T00:00:00Z",
            "done_at":null
        });
        let result = json!({"project":project,"chat":project_chat(&project)});

        validate_json_shape("project.create", &result).unwrap();
        assert_eq!(
            event_data("project.create", &json!({}), &result),
            json!({"project":project})
        );

        let mut incomplete = result.clone();
        incomplete.as_object_mut().unwrap().remove("chat");
        assert!(validate_json_shape("project.create", &incomplete).is_err());
    }

    #[tokio::test]
    async fn production_ping_returns_protocol_server_time_without_persistence() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let _writer = backend.write_lock.lock().await;
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            backend.call("ping", json!({}), &gateway.state),
        )
        .await
        .expect("ping must not wait for the mutation writer")
        .unwrap();
        validate_json_shape("ping", &result).unwrap();
        let timestamp = result["server_time"].as_str().unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(timestamp).is_ok());
        assert!(backend.store.events_since(0).unwrap().is_empty());
        assert!(backend
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn usage_queries_do_not_wait_for_mutation_writer() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let _writer = backend.write_lock.lock().await;
        let methods = [
            ("usage.summary", json!({})),
            (
                "usage.heatmap",
                json!({"mode":"calendar","metric":"tokens"}),
            ),
            (
                "usage.timeseries",
                json!({"granularity":"hour","dimension":"model","metric":"tokens"}),
            ),
            ("usage.breakdown", json!({"dimension":"bot"})),
        ];
        for (method, mut params) in methods {
            params["from"] = json!("2026-01-01T00:00:00Z");
            params["to"] = json!("2026-01-02T00:00:00Z");
            tokio::time::timeout(
                std::time::Duration::from_millis(200),
                backend.call(method, params, &gateway.state),
            )
            .await
            .unwrap_or_else(|_| panic!("{method} waited for mutation writer"))
            .unwrap();
        }
    }

    #[tokio::test]
    async fn takeover_projection_updates_private_null_scope_and_repairs_missing_event() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let group = json!({
            "id":"msg_takeover_run-1", "chat_id":"group-1", "seq":1,
            "sender":{"kind":"bot","bot_id":"bot-1"}, "assignment_id":null,
            "blocks":[{"type":"takeover_request","bot_id":"bot-1","state":"pending"}]
        });
        let private = json!({
            "id":"msg_takeover_question_msg_takeover_run-1", "chat_id":"dm-bot-1", "seq":1,
            "sender":{"kind":"bot","bot_id":"bot-1"}, "assignment_id":"dm_group-1",
            "blocks":[
                {"type":"question","question_id":"question-1"},
                {"type":"takeover_request","bot_id":"bot-1","state":"pending"}
            ]
        });
        backend
            .store
            .append_jsonl("data/chats/group-1/messages.jsonl", &group)
            .unwrap();
        backend
            .store
            .append_jsonl("data/chats/dm-bot-1/messages.jsonl", &private)
            .unwrap();
        let request = json!({
            "message_id":"msg_takeover_run-1", "run_id":"run-1",
            "group_chat_id":"group-1", "chat_id":"dm-bot-1",
            "bot_id":"bot-1", "assignment_id":"dm_group-1", "state":"active"
        });
        backend
            .project_takeover_message_state(&gateway.state, &request)
            .await
            .unwrap();
        let events = backend.store.events_since(0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "message.updated")
                .count(),
            2
        );
        let group_rows = backend
            .store
            .read_jsonl::<Value>("data/chats/group-1/messages.jsonl")
            .unwrap();
        assert_eq!(group_rows.last().unwrap()["blocks"][0]["state"], "active");
        let private_rows = backend
            .store
            .read_jsonl::<Value>("data/chats/dm-bot-1/messages.jsonl")
            .unwrap();
        assert_eq!(private_rows.last().unwrap()["blocks"][1]["state"], "active");

        let mut recovered = group_rows.last().unwrap().clone();
        recovered["blocks"][0]["state"] = json!("done");
        backend
            .store
            .append_jsonl("data/chats/group-1/messages.jsonl", &recovered)
            .unwrap();
        backend.repair_persisted_message_events().unwrap();
        let repaired = backend.store.events_since(0).unwrap();
        assert!(repaired.iter().any(|event| {
            event.event == "message.updated"
                && event.data["message"]["id"] == "msg_takeover_run-1"
                && event.data["message"]["blocks"][0]["state"] == "done"
        }));
    }

    #[tokio::test]
    async fn execution_takeover_request_creates_private_scope_record_without_assignment() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let result = backend
            .execution_takeover_request(
                &gateway.state,
                json!({
                    "bot_id":"main",
                    "assignment_id":null,
                    "chat_id":"chat_main",
                    "message_id":"msg_takeover_run-1",
                    "run_id":"run-1",
                    "reason":"private takeover"
                }),
            )
            .await
            .unwrap();
        assert_eq!(result["question"]["assignment_id"], "dm_chat_main");
        assert_eq!(result["takeover_request"]["assignment_id"], "dm_chat_main");
        let saved = backend
            .store
            .read_snapshot::<Value>("data/takeovers/dm_chat_main.json")
            .unwrap()
            .unwrap();
        assert_eq!(saved["message_id"], "msg_takeover_run-1");
        assert_eq!(saved["run_id"], "run-1");
    }

    #[test]
    fn legacy_takeover_approval_ref_repair_requires_exact_record() {
        let home = tempdir().unwrap();
        let backend = ProductionBackend::open(home.path()).unwrap();
        backend
            .store
            .write_snapshot(
                "data/takeovers/dm_group-1.json",
                &json!({
                    "message_id":"msg_takeover_run-1", "run_id":"run-1",
                    "group_chat_id":"group-1", "question_id":"question-1",
                    "bot_id":"bot-1", "assignment_id":"dm_group-1", "state":"done"
                }),
            )
            .unwrap();
        let message = json!({
            "id":"msg_takeover_run-1", "chat_id":"group-1",
            "blocks":[
                {"type":"approval_ref","approval_id":"question-1"},
                {"type":"approval_ref","approval_id":"real-approval"}
            ]
        });
        backend
            .store
            .append_jsonl("data/chats/group-1/messages.jsonl", &message)
            .unwrap();
        backend.repair_persisted_message_events().unwrap();
        let repaired = backend
            .store
            .read_jsonl::<Value>("data/chats/group-1/messages.jsonl")
            .unwrap()
            .pop()
            .unwrap();
        assert!(repaired["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| block["approval_id"] != "question-1"));
        assert!(repaired["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| block["approval_id"] == "real-approval"));
    }

    #[tokio::test]
    async fn private_takeover_answers_only_its_exact_question_scope() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let result = backend
            .execution_takeover_request(
                &gateway.state,
                json!({
                    "bot_id":"main",
                    "assignment_id":null,
                    "chat_id":"chat_main",
                    "message_id":"msg_takeover_run-2",
                    "run_id":"run-2",
                    "reason":"private takeover"
                }),
            )
            .await
            .unwrap();
        let request = result["takeover_request"].clone();
        let question_id = request["question_id"].as_str().unwrap().to_owned();
        backend
            .answer_takeover_question(&gateway.state, &request)
            .await
            .unwrap();
        let snapshot = backend.orchestrator.snapshot().unwrap();
        assert_eq!(
            snapshot["questions"][question_id.clone()]["state"],
            "answered"
        );
        assert!(backend
            .store
            .events_since(0)
            .unwrap()
            .iter()
            .any(|event| event.event == "question.answered"
                && event.data["question"]["id"] == question_id));
    }

    #[tokio::test]
    async fn waiting_private_takeover_recovery_requires_exact_job_and_card() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let message = json!({
            "id":"msg_takeover_run-1", "chat_id":"chat_main", "seq":1,
            "sender":{"kind":"bot","bot_id":"main"}, "assignment_id":null,
            "created_at":"2026-10-10T13:00:00Z",
            "blocks":[{"type":"takeover_request","bot_id":"main","reason":"recover me","state":"pending"}]
        });
        backend
            .store
            .append_jsonl("data/chats/chat_main/messages.jsonl", &message)
            .unwrap();
        backend
            .store
            .write_snapshot(
                "data/waiting/msg_takeover_run-1.json",
                &json!({
                    "kind":"takeover", "run_id":"run-1", "assignment_id":null,
                    "chat_id":"chat_main"
                }),
            )
            .unwrap();
        backend
            .store
            .write_snapshot(
                "data/run_requests/run-1.json",
                &json!({
                    "run_id":"run-1", "bot_id":"main", "chat_id":"chat_main",
                    "assignment_id":null
                }),
            )
            .unwrap();
        backend
            .store
            .write_snapshot(
                "data/jobs/job-1.json",
                &json!({
                    "id":"job-1", "owner":"main", "kind":"chat",
                    "status":"waiting", "unsafe_replay":true,
                    "updated_at":0, "commit_seq":1,
                    "checkpoint":{
                        "run_id":"run-1",
                        "pending_tool":{"name":"request_takeover","call_id":"call-1","args":{"reason":"recover me"}}
                    }
                }),
            )
            .unwrap();
        let (assignment_id, request) = backend
            .takeover_for_action(&json!({"bot_id":"main"}), "pending")
            .unwrap();
        assert_eq!(assignment_id, "dm_chat_main");
        assert_eq!(request["message_id"], "msg_takeover_run-1");
        assert_eq!(request["assignment_id"], "dm_chat_main");
        assert_eq!(request["run_id"], "run-1");
        assert_eq!(request["state"], "pending");
        drop(gateway);
    }

    #[tokio::test]
    async fn takeover_start_and_release_preserve_scope_and_remaining_driver() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        gateway
            .state
            .browser
            .lock()
            .await
            .set_bot_config(
                "bot-1",
                SessionConfig {
                    executable: "/usr/bin/true".into(),
                    mode: BrowserMode::Attach,
                    chrome_profile: None,
                    idle_timeout_secs: 900,
                    profile_source: None,
                    isolated_profile_root: None,
                    state_path: None,
                },
            )
            .unwrap();
        let takeovers = home.path().join("data/takeovers");
        std::fs::create_dir_all(&takeovers).unwrap();

        let older_active = json!({
            "bot_id":"bot-1",
            "assignment_id":"dm_old",
            "chat_id":"dm-bot-1",
            "state":"active",
            "created_at":"2026-10-10T15:00:00Z",
            "run_id":"run-old",
            "message_id":"msg-old"
        });
        let newer_active = json!({
            "bot_id":"bot-1",
            "assignment_id":"browser_takeover_bot-1",
            "chat_id":"dm-bot-1",
            "state":"active",
            "created_at":"2026-10-10T16:00:00Z",
            "run_id":"run-new",
            "message_id":"msg-new",
            "started_at":"2026-10-10T16:01:00Z"
        });
        let pending = json!({
            "bot_id":"bot-1",
            "assignment_id":"dm_pending",
            "chat_id":"dm-bot-1",
            "state":"pending",
            "created_at":"2026-10-10T17:00:00Z",
            "run_id":"run-pending",
            "message_id":"msg-pending"
        });
        for (name, record) in [
            ("dm_old", older_active),
            ("browser_takeover_bot-1", newer_active),
            ("dm_pending", pending),
        ] {
            backend
                .store
                .write_snapshot(format!("data/takeovers/{name}.json"), &record)
                .unwrap();
        }

        let start = backend
            .takeover_start(&gateway.state, &json!({"bot_id":"bot-1"}))
            .await
            .unwrap();
        assert_eq!(
            start["takeover_request"]["assignment_id"],
            "browser_takeover_bot-1"
        );
        assert_eq!(start["takeover_request"]["run_id"], "run-new");
        assert_eq!(
            start["takeover_request"]["started_at"],
            "2026-10-10T16:01:00Z"
        );
        let records = std::fs::read_dir(&takeovers).unwrap().count();
        assert_eq!(records, 3, "active start must not create a sibling scope");
        assert!(
            gateway
                .state
                .browser
                .lock()
                .await
                .state("bot-1")
                .unwrap()
                .takeover
        );

        let release = backend
            .takeover_release(
                &gateway.state,
                &json!({"bot_id":"bot-1","assignment_id":"browser_takeover_bot-1"}),
            )
            .await
            .unwrap();
        assert_eq!(release["takeover_request"]["state"], "done");
        assert!(
            gateway
                .state
                .browser
                .lock()
                .await
                .state("bot-1")
                .unwrap()
                .takeover,
            "remaining active scope must retain user driver"
        );

        let old_release = backend
            .takeover_release(
                &gateway.state,
                &json!({"bot_id":"bot-1","assignment_id":"dm_old"}),
            )
            .await
            .unwrap();
        assert_eq!(old_release["takeover_request"]["state"], "done");
        assert!(
            !gateway
                .state
                .browser
                .lock()
                .await
                .state("bot-1")
                .unwrap()
                .takeover
        );

        let hinted_start = backend
            .takeover_start(
                &gateway.state,
                &json!({"bot_id":"bot-1","assignment_id":"dm_pending"}),
            )
            .await
            .unwrap();
        assert_eq!(
            hinted_start["takeover_request"]["assignment_id"],
            "dm_pending"
        );
        assert_eq!(hinted_start["takeover_request"]["run_id"], "run-pending");
        assert!(
            gateway
                .state
                .browser
                .lock()
                .await
                .state("bot-1")
                .unwrap()
                .takeover
        );
        let hinted_release = backend
            .takeover_release(
                &gateway.state,
                &json!({"bot_id":"bot-1","assignment_id":"dm_pending"}),
            )
            .await
            .unwrap();
        assert_eq!(hinted_release["takeover_request"]["state"], "done");
        assert!(
            !gateway
                .state
                .browser
                .lock()
                .await
                .state("bot-1")
                .unwrap()
                .takeover
        );
        drop(gateway);
    }

    #[tokio::test]
    async fn takeover_start_retries_exact_pending_question_for_active_scope() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        gateway
            .state
            .browser
            .lock()
            .await
            .set_bot_config(
                "main",
                SessionConfig {
                    executable: "/usr/bin/true".into(),
                    mode: BrowserMode::Attach,
                    chrome_profile: None,
                    idle_timeout_secs: 900,
                    profile_source: None,
                    isolated_profile_root: None,
                    state_path: None,
                },
            )
            .unwrap();

        let created = backend
            .execution_takeover_request(
                &gateway.state,
                json!({
                    "bot_id":"main",
                    "assignment_id":null,
                    "chat_id":"chat_main",
                    "message_id":"msg-takeover-retry",
                    "run_id":"run-takeover-retry",
                    "reason":"retry exact question"
                }),
            )
            .await
            .unwrap();
        let mut active = created["takeover_request"].clone();
        active["state"] = json!("active");
        let assignment_id = active["assignment_id"].as_str().unwrap().to_owned();
        let started_at = "2026-10-10T16:01:00Z";
        active["started_at"] = json!(started_at);
        backend
            .store
            .write_snapshot(format!("data/takeovers/{assignment_id}.json"), &active)
            .unwrap();

        let started = backend
            .takeover_start(&gateway.state, &json!({"bot_id":"main"}))
            .await
            .unwrap();
        assert_eq!(started["takeover_request"]["assignment_id"], assignment_id);
        assert_eq!(started["takeover_request"]["started_at"], started_at);
        let question_id = active["question_id"].as_str().unwrap();
        let snapshot = backend.orchestrator.snapshot().unwrap();
        assert_eq!(snapshot["questions"][question_id]["state"], "answered");
        assert!(backend
            .store
            .events_since(0)
            .unwrap()
            .iter()
            .any(|event| event.event == "question.answered"
                && event.data["question"]["id"] == question_id));
        drop(gateway);
    }

    #[tokio::test]
    async fn execution_takeover_retries_are_idempotent_across_restart_and_scope_changes() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"takeover-idempotency"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let browser_config = || SessionConfig {
            executable: "/usr/bin/true".into(),
            mode: BrowserMode::Attach,
            chrome_profile: None,
            idle_timeout_secs: 900,
            profile_source: None,
            isolated_profile_root: None,
            state_path: None,
        };
        gateway
            .state
            .browser
            .lock()
            .await
            .set_bot_config(&bot_id, browser_config())
            .unwrap();

        let start_params = json!({
            "bot_id":bot_id,
            "client_request_id":"takeover-start-retry"
        });
        let first_start = backend
            .execution_takeover_start(&gateway.state, &start_params)
            .await
            .unwrap();
        let start_assignment = first_start["takeover_request"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let start_path = format!("data/takeovers/{start_assignment}.json");
        let mut completed_start = backend
            .store
            .read_snapshot::<Value>(&start_path)
            .unwrap()
            .unwrap();
        completed_start["state"] = json!("done");
        backend
            .store
            .write_snapshot(&start_path, &completed_start)
            .unwrap();
        drop(backend);
        drop(gateway);

        let restarted_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        restarted_gateway
            .state
            .browser
            .lock()
            .await
            .set_bot_config(&bot_id, browser_config())
            .unwrap();
        let retry_start = restarted
            .execution_takeover_start(&restarted_gateway.state, &start_params)
            .await
            .unwrap();
        assert_eq!(retry_start, json!({}));
        assert_eq!(
            restarted
                .store
                .read_snapshot::<Value>(&start_path)
                .unwrap()
                .unwrap()["state"],
            "done"
        );

        let older = json!({
            "bot_id":bot_id,
            "assignment_id":"dm-older",
            "chat_id":"dm-test",
            "state":"active",
            "created_at":"2026-10-10T15:00:00Z"
        });
        let newer = json!({
            "bot_id":bot_id,
            "assignment_id":"dm-newer",
            "chat_id":"dm-test",
            "state":"active",
            "created_at":"2026-10-10T16:00:00Z"
        });
        restarted
            .store
            .write_snapshot("data/takeovers/dm-older.json", &older)
            .unwrap();
        restarted
            .store
            .write_snapshot("data/takeovers/dm-newer.json", &newer)
            .unwrap();
        let release_params = json!({
            "bot_id":bot_id,
            "client_request_id":"takeover-release-retry"
        });
        let first_release = restarted
            .execution_takeover_release(&restarted_gateway.state, &release_params)
            .await
            .unwrap();
        assert_eq!(
            first_release["takeover_request"]["assignment_id"],
            "dm-newer"
        );
        drop(restarted);
        drop(restarted_gateway);

        let release_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let release_retry_backend = ProductionBackend::open(home.path()).unwrap();
        release_gateway
            .state
            .browser
            .lock()
            .await
            .set_bot_config(&bot_id, browser_config())
            .unwrap();
        let retry_release = release_retry_backend
            .execution_takeover_release(&release_gateway.state, &release_params)
            .await
            .unwrap();
        assert_eq!(retry_release, json!({}));
        assert_eq!(
            release_retry_backend
                .store
                .read_snapshot::<Value>("data/takeovers/dm-older.json")
                .unwrap()
                .unwrap()["state"],
            "active"
        );
        let operations = release_retry_backend
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .unwrap();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation["client_request_id"] == "takeover-release-retry")
                .count(),
            1
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation["client_request_id"] == "takeover-start-retry")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn takeover_projection_publishes_group_before_private_storage_failure() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let group = json!({
            "id":"msg_takeover_run-1", "chat_id":"group-1", "seq":1,
            "sender":{"kind":"bot","bot_id":"bot-1"}, "assignment_id":null,
            "blocks":[{"type":"takeover_request","bot_id":"bot-1","state":"pending"}]
        });
        backend
            .store
            .append_jsonl("data/chats/group-1/messages.jsonl", &group)
            .unwrap();
        std::fs::create_dir_all(home.path().join("data/chats/dm-bot-1/messages.jsonl")).unwrap();
        let request = json!({
            "message_id":"msg_takeover_run-1", "run_id":"run-1",
            "group_chat_id":"group-1", "chat_id":"dm-bot-1",
            "bot_id":"bot-1", "assignment_id":"dm_group-1", "state":"active"
        });
        let mut live = gateway.state.events.subscribe();
        assert!(backend
            .project_takeover_message_state(&gateway.state, &request)
            .await
            .is_err());
        let published = tokio::time::timeout(std::time::Duration::from_millis(200), live.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(published["event"], "message.updated");
        assert_eq!(published["data"]["message"]["id"], "msg_takeover_run-1");
        let events = backend.store.events_since(0).unwrap();
        assert!(events.iter().any(|event| {
            event.event == "message.updated" && event.data["message"]["id"] == "msg_takeover_run-1"
        }));
        let rows = backend
            .store
            .read_jsonl::<Value>("data/chats/group-1/messages.jsonl")
            .unwrap();
        assert_eq!(rows.last().unwrap()["blocks"][0]["state"], "active");
    }

    #[tokio::test]
    async fn production_chat_mark_read_persists_overlay_and_event() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        backend
            .call(
                "chat.mark_read",
                json!({"chat_id":"chat_main","seq":7}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat = backend
            .call("chat.get", json!({"chat_id":"chat_main"}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(chat["chat"]["last_read_seq"], 7);
        let event = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .find(|event| event.event == "read.updated")
            .unwrap();
        assert_eq!(event.data["chat_id"], "chat_main");
        assert_eq!(event.data["last_read_seq"], 7);
        backend
            .call(
                "chat.mark_read",
                json!({"chat_id":"chat_main","seq":3}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat = backend
            .call("chat.get", json!({"chat_id":"chat_main"}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(chat["chat"]["last_read_seq"], 7);
    }

    #[tokio::test]
    async fn production_pending_approval_without_assignment_is_visible() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let approval = backend.call("approval.request", json!({"bot_id":"main","assignment_id":null,"chat_id":"chat_main","tool":"browser.act","risk":"exec","summary":"standalone","detail":"test"}), &gateway.state).await.unwrap();
        let id = approval["id"].as_str().unwrap().to_owned();
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(bots["bots"][0]["status"]["summary"], "waiting_user");
        let workbench = backend
            .call("workbench.get", json!({}), &gateway.state)
            .await
            .unwrap();
        let typed: macbot_protocol::WorkbenchResult = serde_json::from_value(workbench).unwrap();
        assert!(typed.workbench.waiting.iter().any(|item| matches!(item, macbot_protocol::WorkbenchWaiting::Approval { approval } if approval.id == id)));
        backend
            .call(
                "approval.decide",
                json!({"approval_id":id,"decision":"allow_once"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(bots["bots"][0]["status"]["waiting"], 0);
    }

    #[tokio::test]
    async fn execution_question_is_wrapped_and_emits_typed_event() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "origin_chat_id":"chat_main",
                    "bot_id":"main",
                    "title":"question",
                    "instruction":"wait for user",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap();
        let result = backend
            .call(
                "question.ask",
                json!({
                    "bot_id":"main",
                    "assignment_id":assignment_id,
                    "chat_id":"chat_main",
                    "text":"继续吗？",
                    "options":["继续","停止"],
                    "allow_free_text":false
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        serde_json::from_value::<macbot_protocol::Question>(result["question"].clone()).unwrap();
        let event = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.event == "question.asked")
            .expect("question event");
        assert_eq!(
            event.data.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["question"]
        );
        serde_json::from_value::<macbot_protocol::Question>(event.data["question"].clone())
            .unwrap();
        macbot_protocol::EventData::decode(&macbot_protocol::EventName::QuestionAsked, event.data)
            .unwrap();

        let private = backend
            .call(
                "question.ask",
                json!({
                    "bot_id":"main",
                    "assignment_id":null,
                    "chat_id":"chat_main",
                    "text":"私聊继续吗？",
                    "options":["继续"],
                    "allow_free_text":false
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(private["question"]["assignment_id"], "dm_chat_main");
        backend
            .call(
                "question.answer",
                json!({"question_id":private["question"]["id"],"option_index":0}),
                &gateway.state,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn private_durable_jobs_are_reflected_in_bot_status() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        fs::create_dir_all(home.path().join("data/run_requests")).unwrap();
        let worker = backend
            .call("bot.create", json!({"name":"状态 worker"}), &gateway.state)
            .await
            .unwrap();
        let worker_id = worker["bot"]["id"].as_str().unwrap().to_owned();
        fs::write(
            home.path().join("data/run_requests/private-status.json"),
            json!({
                "run_id":"private-status",
                "assignment_id":null,
                "chat_id":"chat_main",
                "bot_id":worker_id,
                "model":"mock/model",
                "instruction":"private status",
                "private":true
            })
            .to_string(),
        )
        .unwrap();
        let running = {
            let mut durable = backend.durable.lock().await;
            let queued = durable
                .create_job(
                    "private-status",
                    "model",
                    json!({"run_id":"private-status"}),
                )
                .unwrap();
            durable
                .commit(
                    &queued.id,
                    macbot_durable::JobStatus::Running,
                    queued.checkpoint,
                    false,
                )
                .unwrap()
        };
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        let main = bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bot| bot["id"] == worker_id)
            .unwrap();
        assert_eq!(main["status"]["summary"], "working");
        assert_eq!(main["status"]["active"], 1);

        let workbench = backend
            .call("workbench.get", json!({}), &gateway.state)
            .await
            .unwrap();
        let workbench_bot = workbench["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bot| bot["bot_id"] == worker_id)
            .unwrap();
        assert_eq!(workbench_bot["active"], 1);

        let job_path = home.path().join(format!("data/jobs/{}.json", running.id));
        let mut external_job = serde_json::to_value(&running).unwrap();
        external_job["status"] = json!("waiting");
        // Exercise the disk fallback with a large checkpoint while preserving
        // the complete durable Job representation.
        external_job["checkpoint"]["messages"] = json!({
            "transcript": "x".repeat(128 * 1024)
        });
        fs::write(&job_path, external_job.to_string()).unwrap();
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        let main = bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bot| bot["id"] == worker_id)
            .unwrap();
        assert_eq!(main["status"]["summary"], "waiting_user");
        assert_eq!(main["status"]["waiting"], 1);
        let workbench = backend
            .call("workbench.get", json!({}), &gateway.state)
            .await
            .unwrap();
        let workbench_bot = workbench["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|bot| bot["bot_id"] == worker_id)
            .unwrap();
        assert_eq!(workbench_bot["active"], 0);

        // Durable checkpoints are arbitrary JSON.  Legacy jobs without an
        // object-shaped run_id must remain ignorable during disk fallback.
        for checkpoint in [Value::Null, json!([]), json!("legacy-checkpoint")] {
            external_job["checkpoint"] = checkpoint;
            fs::write(&job_path, external_job.to_string()).unwrap();
            let bots = backend
                .call("bot.list", json!({}), &gateway.state)
                .await
                .unwrap();
            let main = bots["bots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|bot| bot["id"] == worker_id)
                .unwrap();
            assert_eq!(main["status"]["waiting"], 0);
        }
    }

    #[tokio::test]
    async fn chat_sequences_are_shared_across_senders_updates_and_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let user_one = backend
            .call(
                "chat.send",
                json!({"chat_id":"chat_main","text":"user-1","mentions":[]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_one = backend
            .call(
                "send_msg",
                json!({"bot_id":"main","chat_id":"chat_main","text":"bot-1","intent":"ack"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let mut placeholder = bot_one.clone();
        placeholder["blocks"] = json!([{"type":"text","markdown":"placeholder"}]);
        placeholder["fallback_text"] = json!("placeholder");
        backend
            .store
            .append_jsonl("data/chats/chat_main/messages.jsonl", &placeholder)
            .unwrap();
        let mut final_update = bot_one.clone();
        // Legacy stream writers sometimes saved an empty text block while the
        // fallback already contained the completed answer.
        final_update["blocks"] = json!([{"type":"text","markdown":""}]);
        final_update["fallback_text"] = json!("final");
        backend
            .store
            .append_jsonl("data/chats/chat_main/messages.jsonl", &final_update)
            .unwrap();
        let user_two = backend
            .call(
                "chat.send",
                json!({"chat_id":"chat_main","text":"user-2","mentions":[]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_two = backend
            .call(
                "send_msg",
                json!({"bot_id":"main","chat_id":"chat_main","text":"bot-2","intent":"ack"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let first = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        let first_items = first["messages"].as_array().unwrap();
        assert_eq!(first_items.len(), 4);
        assert_eq!(
            first_items
                .iter()
                .map(|item| item["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(first_items[1]["id"], bot_one["id"]);
        assert_eq!(first_items[1]["fallback_text"], "final");
        assert_eq!(first_items[1]["blocks"][0]["type"], "text");
        assert_eq!(first_items[1]["blocks"][0]["markdown"], "final");
        for item in first_items {
            serde_json::from_value::<Message>(item.clone()).unwrap();
        }
        let after_two = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","after_seq":2,"limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            after_two["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(user_one["message"]["seq"], 1);
        assert_eq!(user_two["message"]["seq"], 3);
        assert_eq!(bot_two["seq"], 4);
        drop(backend);
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let after_restart = restarted
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","after_seq":2,"limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            after_restart["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        let all_after_restart = restarted
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        let restarted_bot = all_after_restart["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == bot_one["id"])
            .unwrap();
        assert_eq!(restarted_bot["blocks"][0]["markdown"], "final");
        for item in all_after_restart["messages"].as_array().unwrap() {
            serde_json::from_value::<Message>(item.clone()).unwrap();
        }
        assert_eq!(restarted.store.last_chat_sequence("chat_main").unwrap(), 4);
    }

    #[tokio::test]
    async fn send_msg_projects_add_member_checked_mentioned_cards() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"Project card worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"Mentioned group","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let sent = backend
            .call(
                "send_msg",
                json!({
                    "bot_id":bot["bot"]["id"],
                    "chat_id":project["chat"]["id"],
                    "text":"我会参考这个群的结论",
                    "intent":"progress",
                    "projects":[{"project_id":project["project"]["id"],"note":"负责登录实现"}]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(sent["blocks"].as_array().unwrap().iter().any(|block| {
            block["type"] == "project_card"
                && block["project_id"] == project["project"]["id"]
                && block["context"] == "mentioned"
                && block["note"] == "负责登录实现"
        }));
        serde_json::from_value::<Message>(sent).unwrap();

        let outsider = backend
            .call("bot.create", json!({"name":"Outsider"}), &gateway.state)
            .await
            .unwrap();
        let rejected = backend
            .call(
                "send_msg",
                json!({
                    "bot_id":outsider["bot"]["id"],
                    "chat_id":project["chat"]["id"],
                    "text":"无权引用这个群",
                    "intent":"progress",
                    "projects":[{"project_id":project["project"]["id"]}]
                }),
                &gateway.state,
            )
            .await;
        assert_eq!(rejected.unwrap_err().code, "forbidden");
    }

    #[tokio::test]
    async fn project_create_persists_one_typed_main_project_card_across_retry_and_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"项目卡测试 Bot","client_request_id":"project-card-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let params = json!({
            "name":"项目卡测试",
            "goal":"验证主会话项目卡",
            "member_bot_ids":[bot_id],
            "flow":["build"],
            "client_request_id":"project-card-create"
        });
        let created = backend
            .call("project.create", params.clone(), &gateway.state)
            .await
            .unwrap();
        assert!(created.get("chat").is_some());
        let project_id = created["project"]["id"].as_str().unwrap().to_owned();
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let cards = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "project_card" && block["project_id"] == project_id
                    })
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(cards.len(), 1);
        serde_json::from_value::<Message>(cards[0].clone()).unwrap();
        assert_eq!(cards[0]["sender"]["kind"], "bot");
        assert_eq!(cards[0]["sender"]["bot_id"], "main");
        assert!(!cards[0]["fallback_text"].as_str().unwrap_or("").is_empty());
        let card_id = cards[0]["id"].as_str().unwrap().to_owned();
        let after_card = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","after_seq":cards[0]["seq"].as_u64().unwrap().saturating_sub(1),"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(after_card["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == card_id));
        let event_count = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .filter(|event| {
                event.event == "message.created" && event.data["message"]["id"] == card_id
            })
            .count();
        assert_eq!(event_count, 1);
        let event_path = home.path().join("data/events/events.jsonl");
        let retained = backend
            .store
            .read_jsonl::<Value>("data/events/events.jsonl")
            .unwrap()
            .into_iter()
            .filter(|event| {
                !(event["event"] == "message.created" && event["data"]["message"]["id"] == card_id)
            })
            .map(|event| serde_json::to_string(&event).unwrap())
            .collect::<Vec<_>>();
        fs::write(
            event_path,
            retained.join("\n") + if retained.is_empty() { "" } else { "\n" },
        )
        .unwrap();
        let retried = backend
            .call("project.create", params, &gateway.state)
            .await
            .unwrap();
        assert_eq!(retried["project"]["id"], project_id);
        assert_eq!(
            backend
                .store
                .events_since(0)
                .unwrap()
                .into_iter()
                .filter(|event| {
                    event.event == "message.created" && event.data["message"]["id"] == card_id
                })
                .count(),
            1
        );
        let retried_history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            retried_history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["id"] == card_id)
                .count(),
            1
        );
        drop(backend);
        let restarted_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let after_restart = restarted
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &restarted_gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            after_restart["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["id"] == card_id)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn production_rpc_returns_complete_protocol_objects_and_durable_event() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot_value = backend
            .call("bot.create", json!({"name":"编码"}), &gateway.state)
            .await
            .unwrap();
        let bot: WireBot = serde_json::from_value(bot_value["bot"].clone()).unwrap();
        assert_eq!(bot.name, "编码");
        let project_value = backend
            .call(
                "project.create",
                json!({"name":"登录","goal":"邮箱","member_bot_ids":[bot.id]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project: WireProject =
            serde_json::from_value(project_value["project"].clone()).unwrap();
        let assignment_value = backend.call("assignment.create", json!({"project_id":project.id,"origin_chat_id":project.chat_id,"bot_id":bot.id,"title":"实现","instruction":"实现","from":"main"}), &gateway.state).await.unwrap();
        let assignment: WireAssignment = serde_json::from_value(assignment_value.clone()).unwrap();
        let message_value = backend.call("send_msg", json!({"bot_id":bot.id,"chat_id":project.chat_id,"assignment_id":assignment.id,"text":"收到","intent":"ack"}), &gateway.state).await.unwrap();
        let _: WireMessage = serde_json::from_value(message_value).unwrap();
        let events = backend.store.events_since(0).unwrap();
        let message_event = events
            .iter()
            .rev()
            .find(|event| event.event == "message.created")
            .unwrap();
        let live = gateway.state.inner.read().await.events.back().cloned();
        let last_seq = backend.store.last_event_seq().unwrap();
        assert_eq!(
            live.as_ref().and_then(|event| event["seq"].as_u64()),
            Some(last_seq)
        );
        assert!(message_event.seq <= last_seq);
        assert!(message_event.data.get("message").is_some());
        let provider_value = backend
            .call("provider.create", json!({"name":"Mock","api_kind":"openai-completions","base_url":"https://example.com","client_request_id":"provider-1"}), &gateway.state)
            .await
            .unwrap();
        let _: WireProviderResult = serde_json::from_value(provider_value).unwrap();
        let provider_list = backend
            .call("provider.list", json!({}), &gateway.state)
            .await
            .unwrap();
        let providers: Vec<WireProvider> =
            serde_json::from_value(provider_list["providers"].clone()).unwrap();
        assert_eq!(providers.len(), 1);
        let chat = backend
            .call(
                "chat.send",
                json!({"chat_id":project.chat_id,"text":"用户消息","mentions":[],"client_request_id":"chat-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let _: WireMessage = serde_json::from_value(chat["message"].clone()).unwrap();
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":project.chat_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        let _: Vec<WireMessage> = serde_json::from_value(history["messages"].clone()).unwrap();
        let settings = backend
            .call("settings.get", json!({}), &gateway.state)
            .await
            .unwrap();
        serde_json::from_value::<Settings>(settings["settings"].clone()).unwrap();
        let updated = backend
            .call(
                "settings.update",
                json!({"patch":{"currency":"USD"},"client_request_id":"settings-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(updated["settings"]["currency"], "USD");
        let device = backend
            .call("device.register", json!({"device_id":"dev-1","platform":"macos","app_version":"1","device_name":"Test","push_token":null}), &gateway.state)
            .await
            .unwrap();
        serde_json::from_value::<macbot_protocol::Device>(device["device"].clone()).unwrap();
        let bootstrap = backend
            .call("bootstrap", json!({}), &gateway.state)
            .await
            .unwrap();
        serde_json::from_value::<Settings>(bootstrap["settings"].clone()).unwrap();
        assert_eq!(
            bootstrap["hello"]["node_id"],
            gateway.state.node_id.read().await.as_str()
        );
    }

    #[tokio::test]
    async fn production_bot_dm_ids_and_chat_kinds_are_consistent_across_reads() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let created = backend
            .call(
                "bot.create",
                json!({"name":"只读回归外的 Bot","client_request_id":"dm-consistency"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot: WireBot = serde_json::from_value(created["bot"].clone()).unwrap();
        let created_chat: WireChat = serde_json::from_value(created["dm_chat"].clone()).unwrap();
        assert_eq!(bot.dm_chat_id, created_chat.id);
        assert_eq!(created_chat.kind, macbot_protocol::ChatKind::Direct);

        let template = backend
            .call(
                "bot.create_from_template",
                json!({"template_id":"product-code-test"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let template_bots = template["bots"].as_array().unwrap();
        let template_chats = template["dm_chats"].as_array().unwrap();
        assert_eq!(template_bots.len(), template_chats.len());
        for (bot, chat) in template_bots.iter().zip(template_chats) {
            assert_eq!(bot["dm_chat_id"], chat["id"]);
            assert_eq!(chat["kind"], "direct");
        }

        let list = backend
            .call("chat.list", json!({}), &gateway.state)
            .await
            .unwrap();
        let chats = list["chats"].as_array().unwrap();
        let worker_chat = chats
            .iter()
            .find(|chat| chat["id"] == bot.dm_chat_id)
            .expect("worker direct chat in chat.list");
        assert_eq!(worker_chat["kind"], "direct");
        assert_eq!(worker_chat["bot_id"], bot.id);
        assert_eq!(
            chats
                .iter()
                .filter(|chat| chat["id"] == "chat_main")
                .count(),
            1,
            "main chat must not be duplicated as a worker direct chat"
        );
        let main_bot = backend
            .call("bot.get", json!({"bot_id":"main"}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(main_bot["bot"]["dm_chat_id"], "chat_main");
        assert_eq!(
            chats.iter().filter(|chat| chat["bot_id"] == "main").count(),
            0
        );

        let fetched = backend
            .call(
                "chat.get",
                json!({"chat_id":bot.dm_chat_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(fetched["chat"]["id"], bot.dm_chat_id);
        assert_eq!(fetched["chat"]["kind"], "direct");
        let sent = backend
            .call(
                "chat.send",
                json!({"chat_id":bot.dm_chat_id,"text":"私聊回归","mentions":[],"client_request_id":"dm-message"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(sent["message"]["chat_id"], bot.dm_chat_id);
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":bot.dm_chat_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(history["messages"].as_array().unwrap().len(), 1);

        let bootstrap = backend
            .call("bootstrap", json!({}), &gateway.state)
            .await
            .unwrap();
        let bootstrap_bot = bootstrap["bots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == bot.id)
            .unwrap();
        assert_eq!(bootstrap_bot["dm_chat_id"], bot.dm_chat_id);
        let bootstrap_chat = bootstrap["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == bot.dm_chat_id)
            .unwrap();
        assert_eq!(bootstrap_chat["kind"], "direct");
        assert_eq!(bootstrap["hello"]["timezone"], "Asia/Shanghai");
    }

    #[tokio::test]
    async fn production_usage_queries_and_csv_use_the_durable_ledger() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let now = Utc::now();
        let from = (now - Duration::hours(2)).to_rfc3339();
        let to = (now + Duration::hours(1)).to_rfc3339();
        {
            let mut ledger = backend.usage.lock().await;
            for (request_id, cost) in [("usage-a", Some(0.25)), ("usage-b", None)] {
                ledger
                    .record(UsageRecord {
                        request_id: request_id.into(),
                        ts: now,
                        bot_id: "bot_usage".into(),
                        project_id: Some("project_usage".into()),
                        chat_id: "chat_usage".into(),
                        assignment_id: Some(request_id.into()),
                        run_id: request_id.into(),
                        phase: "work".into(),
                        provider_id: "provider_usage".into(),
                        model_id: "model_usage".into(),
                        routine: false,
                        usage: Totals {
                            input_tokens: 100,
                            output_tokens: 40,
                            cache_read_tokens: 20,
                            cache_write_tokens: 10,
                            requests: 1,
                            cost,
                        },
                        task_done: true,
                    })
                    .unwrap();
            }
        }
        let params = json!({"from":from,"to":to});
        let summary = backend
            .call("usage.summary", params.clone(), &gateway.state)
            .await
            .unwrap();
        let summary: UsageSummaryResult = serde_json::from_value(summary).unwrap();
        assert_eq!(summary.current.usage.requests, 2);
        assert_eq!(summary.current.usage.input_tokens, 200);
        assert_eq!(summary.current.tasks_done, 2);
        assert!(summary.current.usage.cost.is_none());

        let heatmap = backend
            .call(
                "usage.heatmap",
                json!({"from":from,"to":to,"mode":"calendar","metric":"tokens"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let _: HeatmapResult = serde_json::from_value(heatmap).unwrap();
        let timeseries = backend
            .call(
                "usage.timeseries",
                json!({"from":from,"to":to,"granularity":"hour","dimension":"model","metric":"tokens","split_io":true}),
                &gateway.state,
            )
            .await
            .unwrap();
        let timeseries: UsageTimeseriesResult = serde_json::from_value(timeseries).unwrap();
        assert!(!timeseries.series.is_empty());
        assert!(timeseries.series[0].input_values.is_some());
        let breakdown = backend
            .call(
                "usage.breakdown",
                json!({"from":from,"to":to,"dimension":"bot","drill":{"bot_id":"bot_usage"}}),
                &gateway.state,
            )
            .await
            .unwrap();
        let breakdown: UsageBreakdownResult = serde_json::from_value(breakdown).unwrap();
        assert_eq!(breakdown.rows.len(), 1);
        assert_eq!(breakdown.rows[0].usage.requests, 2);
        let csv = backend
            .usage
            .lock()
            .await
            .export_csv(
                &json!({"from":from,"to":to,"dimension":"bot"}),
                "Asia/Shanghai",
            )
            .unwrap();
        assert!(csv.lines().count() >= 2, "{csv}");
        assert!(csv.contains("bot_usage"), "{csv}");
    }

    #[test]
    fn settings_secret_params_are_separate_and_redacted_from_operations() {
        let params = json!({
            "patch": {"web_search": {"provider": "brave", "endpoint": "https://search.example"}},
            "web_search_key": "do-not-log-this",
            "client_request_id": "settings-secret-1"
        });
        validate_settings_secret_params(&params).unwrap();
        let redacted = redact_settings_params(&params);
        assert!(!redacted.to_string().contains("do-not-log-this"));
        assert_eq!(redacted["web_search_key"], "<redacted>");

        let embedded = json!({"patch":{"web_search":{"api_key":"forbidden"}}});
        assert!(validate_settings_secret_params(&embedded).is_err());
        let readonly = json!({"patch":{"web_search":{"has_key":true}}});
        assert!(validate_settings_secret_params(&readonly).is_err());
        let readonly_push = json!({"patch":{"push":{"apns_configured":true}}});
        assert!(validate_settings_secret_params(&readonly_push).is_err());
    }

    #[tokio::test]
    async fn corrupt_snapshot_recovers_last_committed_operation_snapshot() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let created = backend
            .call(
                "bot.create",
                json!({"name":"可恢复","client_request_id":"recover-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let id = created["bot"]["id"].as_str().unwrap().to_owned();
        std::fs::write(home.path().join("data/orchestrator/state.json"), b"{broken").unwrap();
        drop(backend);
        let reopened = ProductionBackend::open(home.path()).unwrap();
        let recovered = reopened
            .call("bot.get", json!({"bot_id":id}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(recovered["bot"]["name"], "可恢复");
    }

    #[tokio::test]
    async fn loop_resolve_updates_pause_message_and_emits_new_assignment() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let from_bot = backend
            .call("bot.create", json!({"name":"loop-from"}), &gateway.state)
            .await
            .unwrap();
        let from = from_bot["bot"]["id"].as_str().unwrap().to_owned();
        let from_dm = from_bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let to = backend
            .call("bot.create", json!({"name":"loop-to"}), &gateway.state)
            .await
            .unwrap()["bot"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let assignment = backend.call("assignment.create", json!({"origin_chat_id":"chat_main","bot_id":from,"title":"loop root","instruction":"handoff","from":"main","root_message_id":"root-loop","loop_hops":8}), &gateway.state).await.unwrap();
        let assignment_id = assignment["id"].as_str().unwrap();
        let sent = backend.call("send_msg", json!({"bot_id":from,"chat_id":from_dm,"assignment_id":assignment_id,"text":"继续交接","intent":"done","mentions":[{"kind":"bot","bot_id":to,"instruction":"下一跳"}]}), &gateway.state).await.unwrap();
        let sent_message: Message = serde_json::from_value(sent.clone()).unwrap();
        assert_eq!(sent["blocks"][0]["type"], "loop_paused");
        let resolved = backend
            .call(
                "loop.resolve",
                json!({"root_message_id":"root-loop","action":"continue"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(resolved, json!({}));
        let events = backend.store.events_since(0).unwrap();
        let update = events
            .iter()
            .find(|event| event.event == "message.updated")
            .expect("message.updated");
        let updated: Message = serde_json::from_value(update.data["message"].clone()).unwrap();
        assert_eq!(updated.id, sent_message.id);
        assert_eq!(updated.seq, sent_message.seq);
        assert!(matches!(
            updated.blocks[0],
            macbot_protocol::Block::LoopPaused {
                state: macbot_protocol::LoopState::Continued,
                ..
            }
        ));
        let created = events
            .iter()
            .find(|event| {
                event.event == "assignment.created"
                    && event.data["assignment"]["parent_assignment_id"] == assignment_id
            })
            .expect("continued assignment");
        let _: Assignment = serde_json::from_value(created.data["assignment"].clone()).unwrap();
        assert_eq!(created.data["assignment"]["loop_hops"], 0);
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":from_dm,"limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        let history_message: Message = serde_json::from_value(
            history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == sent_message.id)
                .cloned()
                .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            history_message.blocks[0],
            macbot_protocol::Block::LoopPaused {
                state: macbot_protocol::LoopState::Continued,
                ..
            }
        ));
        let main_history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["assignment_id"] == assignment_id
                    && item["sender"]["kind"] == "system"
                    && item["blocks"][0]["type"] == "completion"
            }));
        drop(backend);
        drop(gateway);
        let restarted_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let replay = restarted
            .call(
                "chat.history",
                json!({"chat_id":from_dm,"limit":20}),
                &restarted_gateway.state,
            )
            .await
            .unwrap();
        let replay_message: Message = serde_json::from_value(
            replay["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == sent_message.id)
                .cloned()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(replay_message.seq, sent_message.seq);
        assert!(matches!(
            replay_message.blocks[0],
            macbot_protocol::Block::LoopPaused {
                state: macbot_protocol::LoopState::Continued,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn durable_wait_rebuilds_missing_legacy_questions_without_running_tools() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"legacy decisions"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let mut expected = Vec::new();
        for index in 0..4 {
            let project = backend.call("project.create", json!({"name":format!("legacy {index}"),"goal":"choose login","member_bot_ids":[bot_id]}), &gateway.state).await.unwrap();
            let chat_id = project["project"]["chat_id"].as_str().unwrap();
            let assignment = backend.call("assignment.create", json!({"bot_id":bot_id,"project_id":project["project"]["id"],"origin_chat_id":chat_id,"title":"decision","instruction":"choose login"}), &gateway.state).await.unwrap();
            let assignment_id = assignment["id"].as_str().unwrap();
            let message = backend.execution_send_msg(&gateway.state, json!({"receipt":{"run_id":format!("legacy-{index}"),"call_id":"decision"},"message":{"chat_id":chat_id,"bot_id":bot_id,"assignment_id":assignment_id,"text":"Choose login","intent":"decision","options":["Email","Phone"],"mentions":["user"]}})).await.unwrap();
            let run_id = format!("legacy-run-{index}");
            let mut checkpoint = json!({"run_id":run_id,"waiting_reason":"decision","waiting_message_id":message["id"],"waiting_message":true,"pending_tools":[]});
            if index == 2 {
                checkpoint["pending_tools"] = json!([{"name":"write","call_id":"unsafe"}]);
            }
            let mut durable = backend.durable.lock().await;
            let job = durable
                .create_job(bot_id, "execution", checkpoint.clone())
                .unwrap();
            durable
                .commit(
                    &job.id,
                    if index == 3 {
                        macbot_durable::JobStatus::Suspended
                    } else {
                        macbot_durable::JobStatus::Waiting
                    },
                    checkpoint,
                    index == 3,
                )
                .unwrap();
            drop(durable);
            backend.store.write_snapshot(format!("data/run_requests/{run_id}.json"), &json!({"run_id":run_id,"assignment_id":assignment_id,"bot_id":bot_id,"chat_id":chat_id,"model":"mock-model","instruction":"choose login"})).unwrap();
            expected.push((assignment_id.to_owned(), chat_id.to_owned(), message));
        }
        let mut legacy = backend.orchestrator.snapshot().unwrap();
        legacy["questions"] = json!({});
        legacy["question_created_at"] = json!({});
        legacy["question_scopes"] = json!({});
        for (assignment_id, chat_id, message) in &expected {
            legacy["messages"][message["id"].as_str().unwrap()]
                .as_object_mut()
                .unwrap()
                .remove("question_id");
            legacy["assignments"][assignment_id]["status"] = json!("working");
            legacy["assignments"][assignment_id]["wait"] = Value::Null;
            let mut old_wire = message.clone();
            old_wire["blocks"] = json!([{"type":"text","markdown":"Choose login"}]);
            backend
                .store
                .append_jsonl(format!("data/chats/{chat_id}/messages.jsonl"), &old_wire)
                .unwrap();
        }
        backend
            .store
            .append_jsonl(
                "data/orchestrator/operations.jsonl",
                &json!({"method":"legacy.fixture","status":"done","snapshot":legacy}),
            )
            .unwrap();
        let baseline_events = backend.store.events_since(0).unwrap().len() as u64;
        let job_commits = backend
            .store
            .read_jsonl::<Value>("data/jobs/commits.jsonl")
            .unwrap();
        drop(backend);
        let mut previous_events = None;
        for _ in 0..2 {
            let restarted = ProductionBackend::open(home.path()).unwrap();
            let bootstrap = restarted
                .call("bootstrap", json!({}), &gateway.state)
                .await
                .unwrap();
            assert_eq!(
                bootstrap["pending"]["questions"].as_array().unwrap().len(),
                2
            );
            let workbench = restarted
                .call("workbench.get", json!({}), &gateway.state)
                .await
                .unwrap();
            assert_eq!(
                workbench["waiting"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| item.get("question").is_some())
                    .count(),
                2
            );
            let disk: Value = restarted
                .store
                .read_snapshot("data/orchestrator/state.json")
                .unwrap()
                .unwrap();
            for (index, (assignment_id, chat_id, original)) in expected.iter().enumerate() {
                let history = restarted
                    .call(
                        "chat.history",
                        json!({"chat_id":chat_id,"limit":100}),
                        &gateway.state,
                    )
                    .await
                    .unwrap();
                let current = history["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["id"] == original["id"])
                    .unwrap();
                for field in ["id", "seq", "created_at"] {
                    assert_eq!(current[field], original[field]);
                }
                if index < 2 {
                    let question_id = format!("decision:{}", original["id"].as_str().unwrap());
                    assert_eq!(
                        current["blocks"],
                        json!([{"type":"question","question_id":question_id}])
                    );
                    assert_eq!(
                        disk["questions"][&question_id]["options"],
                        json!(["Email", "Phone"])
                    );
                    assert_eq!(
                        disk["assignments"][assignment_id]["wait"]["message_id"],
                        original["id"]
                    );
                    assert_eq!(disk["assignments"][assignment_id]["status"], "waiting_user");
                } else {
                    assert_eq!(current["blocks"][0]["type"], "text");
                    assert_eq!(disk["assignments"][assignment_id]["status"], "working");
                    assert!(disk["assignments"][assignment_id]["wait"].is_null());
                }
            }
            if let Some(cursor) = previous_events {
                assert!(restarted.store.events_since(cursor).unwrap().is_empty());
            }
            previous_events = Some(
                restarted
                    .store
                    .events_since(0)
                    .unwrap()
                    .last()
                    .map(|event| event.seq)
                    .unwrap_or(0),
            );
            let new_events = restarted.store.events_since(baseline_events).unwrap();
            assert_eq!(
                new_events
                    .iter()
                    .filter(|event| event.event == "question.asked")
                    .count(),
                2
            );
            assert_eq!(
                new_events
                    .iter()
                    .filter(|event| event.event == "message.updated"
                        && event.data["message"]["blocks"][0]["type"] == "question"
                        && expected
                            .iter()
                            .any(|(_, _, message)| event.data["message"]["id"] == message["id"]))
                    .count(),
                2
            );
            for (index, (assignment_id, _, _)) in expected.iter().enumerate() {
                assert_eq!(
                    new_events
                        .iter()
                        .filter(|event| event.event == "assignment.updated"
                            && event.data["assignment"]["id"] == *assignment_id)
                        .count(),
                    usize::from(index < 2)
                );
            }
            assert_eq!(
                restarted
                    .store
                    .read_jsonl::<Value>("data/jobs/commits.jsonl")
                    .unwrap(),
                job_commits
            );
        }
    }

    #[tokio::test]
    async fn decision_question_is_visible_idempotent_and_repaired_on_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"decision worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = &bot["bot"]["id"];
        let project = backend
            .call(
                "project.create",
                json!({"name":"decision project","goal":"choose login","member_bot_ids":[bot_id]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat_id = &project["project"]["chat_id"];
        let assignment = backend.call("assignment.create", json!({"bot_id":bot_id,"project_id":project["project"]["id"],"origin_chat_id":chat_id,"title":"decision","instruction":"choose login"}), &gateway.state).await.unwrap();
        let envelope = json!({"receipt":{"run_id":"decision-run","call_id":"decision-call"},"message":{"chat_id":chat_id,"bot_id":bot_id,"assignment_id":assignment["id"],"text":"Choose login","intent":"decision","options":["Email","Phone"],"mentions":["user"]}});
        let first = backend
            .execution_send_msg(&gateway.state, envelope.clone())
            .await
            .unwrap();
        let repeated = backend
            .execution_send_msg(&gateway.state, envelope)
            .await
            .unwrap();
        assert_eq!(first, repeated);
        let question_id = first["blocks"][0]["question_id"].as_str().unwrap();
        assert_eq!(first["blocks"][0]["type"], "question");
        serde_json::from_value::<Message>(first.clone()).unwrap();
        let bootstrap = backend
            .call("bootstrap", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(bootstrap["pending"]["questions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q["id"] == question_id && q["options"] == json!(["Email", "Phone"])));
        let workbench = backend
            .call("workbench.get", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(workbench["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["question"]["id"] == question_id));
        let events = backend.store.events_since(0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "question.asked"
                    && event.data["question"]["id"] == question_id)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "message.created"
                    && event.data["message"]["id"] == first["id"])
                .count(),
            1
        );

        // Model an old release with a valid Question but no message reference
        // and a text-only persisted wire row. Restoration must preserve ids,
        // timestamps and cursors, rather than replaying the model/tool.
        let mut old_snapshot = backend.orchestrator.snapshot().unwrap();
        old_snapshot["messages"][first["id"].as_str().unwrap()]
            .as_object_mut()
            .unwrap()
            .remove("question_id");
        backend
            .store
            .append_jsonl(
                "data/orchestrator/operations.jsonl",
                &json!({"method":"legacy.fixture","status":"done","snapshot":old_snapshot}),
            )
            .unwrap();
        let mut old_wire = first.clone();
        old_wire["blocks"] = json!([{"type":"text","markdown":"Choose login"}]);
        backend
            .store
            .append_jsonl(
                format!("data/chats/{}/messages.jsonl", chat_id.as_str().unwrap()),
                &old_wire,
            )
            .unwrap();
        drop(backend);
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let history = restarted
            .call(
                "chat.history",
                json!({"chat_id":chat_id,"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let repaired = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == first["id"])
            .unwrap();
        assert_eq!(repaired["blocks"], first["blocks"]);
        for field in ["id", "seq", "created_at"] {
            assert_eq!(repaired[field], first[field]);
        }
        let events = restarted.store.events_since(0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "question.asked"
                    && event.data["question"]["id"] == question_id)
                .count(),
            1
        );
        assert!(events.iter().any(|event| event.event == "message.updated"
            && event.data["message"]["id"] == first["id"]
            && event.data["message"]["blocks"] == first["blocks"]));
    }

    #[tokio::test]
    async fn unknown_send_target_has_no_rpc_message_assignment_or_event_side_effects() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"Target worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"Target project", "goal":"Validate explicit routing",
                    "member_bot_ids":[bot_id]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let chat_id = project["project"]["chat_id"].as_str().unwrap();
        let before = backend.orchestrator.snapshot().unwrap();
        let events = backend.store.events_since(0).unwrap();
        let operations = backend
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .unwrap();
        let params = json!({"chat_id":project_id,"bot_id":"main","intent":"progress",
            "text":"Do not create an orphan handoff", "mentions":[bot_id]});
        // A private incarnation may address a Bot DM; a group incarnation
        // remains locked to its project chat.
        backend
            .execution_validate_send_msg_target(&json!({
                "chat_id":"chat_main","bot_id":"main","to":{"bot":bot_id}
            }))
            .unwrap();
        assert!(backend
            .execution_validate_send_msg_target(&json!({
                "chat_id":chat_id,"bot_id":"main","to":{"bot":bot_id}
            }))
            .is_err());
        for invalid_source in [json!(project_id), json!(42), Value::Null, json!("")] {
            let invalid = json!({"chat_id":invalid_source,"bot_id":"main",
                "to":{"bot":bot_id},"intent":"progress","text":"No invalid source ref"});
            assert!(backend
                .execution_validate_send_msg_target(&invalid)
                .is_err());
            assert!(backend
                .call("send_msg", invalid, &gateway.state)
                .await
                .is_err());
        }
        assert!(backend.execution_validate_send_msg_target(&params).is_err());
        assert!(backend
            .call("send_msg", params.clone(), &gateway.state)
            .await
            .is_err());
        assert!(backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"invalid-target","call_id":"invalid-call"},"message":params
                })
            )
            .await
            .is_err());
        assert_eq!(backend.orchestrator.snapshot().unwrap(), before);
        assert_eq!(backend.store.events_since(0).unwrap().len(), events.len());
        assert_eq!(
            backend
                .store
                .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
                .unwrap(),
            operations
        );
        assert!(!home
            .path()
            .join(format!("data/chats/{project_id}"))
            .exists());

        let corrected = backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"invalid-target","call_id":"corrected-call"},
                    "message":{"chat_id":chat_id,"bot_id":"main","intent":"progress",
                        "text":"Explicit corrected target", "mentions":[bot_id]}
                }),
            )
            .await
            .unwrap();
        assert_eq!(corrected["chat_id"], chat_id);
        let snapshot = backend.orchestrator.snapshot().unwrap();
        let handoff = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .find(|assignment| assignment["trigger_message_id"] == corrected["id"])
            .unwrap();
        assert_eq!(handoff["project_id"], project_id);
        assert_eq!(handoff["origin_chat_id"], chat_id);
        backend
            .execution_validate_send_msg_target(&json!({
                "bot_id":"main","chat_id":"chat_main"
            }))
            .unwrap();
        assert!(backend
            .execution_validate_send_msg_target(&json!({
                "bot_id":"main","chat_id":bot["bot"]["dm_chat_id"]
            }))
            .is_err());
    }

    #[tokio::test]
    async fn send_msg_to_current_bot_stays_in_private_or_group_assignment_scope() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"Self-route worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let dm_chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap();
        let private_assignment = backend
            .call(
                "assignment.create",
                json!({
                    "bot_id":bot_id,
                    "origin_chat_id":dm_chat_id,
                    "title":"Private incarnation",
                    "instruction":"Report in the private task chat",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let private_sent = backend
            .call(
                "send_msg",
                json!({
                    "bot_id":bot_id,
                    "assignment_id":private_assignment["id"],
                    "chat_id":dm_chat_id,
                    "to":bot_id,
                    "intent":"done",
                    "text":"私聊分身仍发送到自己的 DM"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(private_sent["chat_id"], dm_chat_id);

        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"Self-route project",
                    "goal":"Keep a group incarnation bound to its group",
                    "member_bot_ids":[bot_id]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let group_chat_id = project["project"]["chat_id"].as_str().unwrap();
        let group_assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":group_chat_id,
                    "bot_id":bot_id,
                    "title":"Group incarnation",
                    "instruction":"Report only in the project group",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let group_sent = backend
            .call(
                "send_msg",
                json!({
                    "bot_id":bot_id,
                    "assignment_id":group_assignment["id"],
                    "project_id":project_id,
                    "chat_id":group_chat_id,
                    "to":{"bot":bot_id},
                    "intent":"done",
                    "text":"群分身不能因自身 bot_id 逃离群"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(group_sent["chat_id"], group_chat_id);
        let snapshot = backend.orchestrator.snapshot().unwrap();
        assert!(!snapshot
            .get("chats")
            .and_then(Value::as_object)
            .is_some_and(|chats| chats.keys().any(|chat_id| chat_id.starts_with("bot_dm_"))));
    }

    #[tokio::test]
    async fn execution_send_msg_preserves_receipt_idempotency() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let envelope = json!({
            "receipt":{"run_id":"run-1","call_id":"call-1","message_id":"msg-1","intent":"progress"},
            "message":{"message_id":"msg-1","chat_id":"chat_main","bot_id":"main","text":"进展","intent":"progress"}
        });
        let first = backend
            .execution_send_msg(&gateway.state, envelope.clone())
            .await
            .unwrap();
        let second = backend
            .execution_send_msg(&gateway.state, envelope)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            backend
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| event.event == "message.created")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn assignment_retry_returns_child_and_is_idempotent_after_card_persistence() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call("bot.create", json!({"name":"重试测试 Bot"}), &gateway.state)
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"重试测试项目",
                    "goal":"验证 assignment.retry",
                    "member_bot_ids":[bot_id]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let source = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot_id,
                    "title":"首次执行",
                    "instruction":"失败后重试",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let source_id = source["id"].as_str().unwrap().to_owned();
        backend
            .orchestrator
            .finish_assignment(&source_id, "failed")
            .unwrap();
        let params = json!({
            "assignment_id":source_id,
            "client_request_id":"qa-retry-same-request"
        });
        let first = backend
            .call("assignment.retry", params.clone(), &gateway.state)
            .await
            .unwrap();
        let retry_id = first["assignment"]["id"].as_str().unwrap().to_owned();
        assert_eq!(first["assignment"]["parent_assignment_id"], source_id);
        let second = backend
            .call("assignment.retry", params, &gateway.state)
            .await
            .unwrap();
        assert_eq!(second["assignment"]["id"], retry_id);

        let snapshot = backend.orchestrator.snapshot().unwrap();
        let retry_children = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .filter(|assignment| assignment["parent_assignment_id"] == source_id)
            .count();
        assert_eq!(retry_children, 1);
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":project["project"]["chat_id"],"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let child_cards = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "task_card" && block["assignment_id"] == retry_id
                    })
                })
            })
            .count();
        assert_eq!(child_cards, 1);
    }

    #[tokio::test]
    async fn assignment_cards_are_typed_sequenced_and_idempotent() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"卡片测试 Bot","client_request_id":"card-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"任务卡测试项目",
                    "goal":"验证 task_card",
                    "member_bot_ids":[bot_id],
                    "client_request_id":"card-project"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let project_chat = project["project"]["chat_id"].as_str().unwrap();
        let assignment_params = json!({
            "project_id":project_id,
            "origin_chat_id":project_chat,
            "bot_id":bot_id,
            "title":"实现卡片测试",
            "instruction":"完成卡片测试",
            "from":"main",
            "client_request_id":"card-assignment"
        });
        let assignment = backend
            .call(
                "assignment.create",
                assignment_params.clone(),
                &gateway.state,
            )
            .await
            .unwrap();
        let _retry = backend
            .call("assignment.create", assignment_params, &gateway.state)
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap();
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":project_chat,"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let task_cards = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "task_card" && block["assignment_id"] == assignment_id
                    })
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(task_cards.len(), 1);
        let task_card_wire = task_cards[0].clone();
        let task_message: Message = serde_json::from_value(task_cards[0].clone()).unwrap();
        assert_eq!(task_message.chat_id, project_chat);
        assert!(task_message.seq > 0);
        assert_eq!(task_message.assignment_id.as_deref(), Some(assignment_id));

        // Historical attention repair must reuse the per-chat durable cache
        // when the card and its event are already canonical.  In particular,
        // it must not rewrite the complete chat JSONL merely because the
        // assignment is still referenced by an old attention notice.
        let task_message_id = task_message.id.clone();
        let rows_before = backend
            .store
            .read_jsonl::<Value>(format!("data/chats/{project_chat}/messages.jsonl"))
            .unwrap();
        let events_before = backend.store.events_since(0).unwrap().len();
        let mut cached_index = AssignmentCardEventIndex::default();
        cached_index
            .message_rows
            .insert(task_message_id.clone(), task_card_wire.clone());
        let sequences_before = backend
            .store
            .read_jsonl::<Value>(format!("data/chats/{project_chat}/sequences.jsonl"))
            .unwrap();
        backend
            .ensure_assignment_card_with_event_index(
                &gateway.state,
                &assignment,
                false,
                Some(&mut cached_index),
            )
            .await
            .unwrap();
        let rows_after = backend
            .store
            .read_jsonl::<Value>(format!("data/chats/{project_chat}/messages.jsonl"))
            .unwrap();
        assert_eq!(rows_before, rows_after);
        assert_eq!(events_before, backend.store.events_since(0).unwrap().len());
        assert_eq!(
            sequences_before,
            backend
                .store
                .read_jsonl::<Value>(format!("data/chats/{project_chat}/sequences.jsonl"))
                .unwrap()
        );
        assert_eq!(cached_index.chat_messages.len(), 1);

        // A real card content change still sequences the cached chat once.
        let mut changed_assignment = assignment.clone();
        changed_assignment["title"] = json!("实现卡片测试（更新）");
        let mut changed_index = AssignmentCardEventIndex::default();
        changed_index
            .message_rows
            .insert(task_message_id, task_card_wire);
        let events_before_change = backend.store.events_since(0).unwrap().len();
        backend
            .ensure_assignment_card_with_event_index(
                &gateway.state,
                &changed_assignment,
                false,
                Some(&mut changed_index),
            )
            .await
            .unwrap();
        let events_after_change = backend.store.events_since(0).unwrap();
        assert_eq!(events_after_change.len(), events_before_change + 1);
        let changed_card = events_after_change
            .last()
            .and_then(|event| event.data.get("message"))
            .unwrap();
        assert_eq!(changed_card["id"], task_message.id);
        assert_eq!(changed_card["fallback_text"], "任务：实现卡片测试（更新）");

        let delegated = backend
            .call(
                "delegate",
                json!({
                    "origin_chat_id":"chat_main",
                    "bot_id":bot_id,
                    "title":"主 Bot 委派",
                    "instruction":"执行委派任务",
                    "from":"main",
                    "client_request_id":"card-delegate"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let delegated_id = delegated["id"].as_str().unwrap();
        let main_history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let delegation = main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "delegation" && block["assignment_id"] == delegated_id
                    })
                })
            })
            .unwrap();
        let delegation_message: Message = serde_json::from_value(delegation.clone()).unwrap();
        assert!(delegation_message.seq > 0);
        assert_eq!(
            delegation_message.assignment_id.as_deref(),
            Some(delegated_id)
        );
    }

    #[tokio::test]
    async fn private_worker_messages_stay_in_dm_and_results_return_to_main() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let worker = backend
            .call("bot.create", json!({"name":"委派 worker"}), &gateway.state)
            .await
            .unwrap();
        let peer = backend
            .call("bot.create", json!({"name":"交付复核 Bot"}), &gateway.state)
            .await
            .unwrap();
        let worker_id = worker["bot"]["id"].as_str().unwrap();
        let worker_dm = worker["bot"]["dm_chat_id"].as_str().unwrap();
        let peer_id = peer["bot"]["id"].as_str().unwrap();
        let delegated = backend
            .call(
                "delegate",
                json!({
                    "origin_chat_id":"chat_main",
                    "bot_id":worker_id,
                    "title":"整理资料",
                    "instruction":"整理公开资料并汇报",
                    "from":"main",
                    "client_request_id":"main-dm-delegate-worker-messages"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = delegated["id"].as_str().unwrap();

        let ack = backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"delegate-worker-run","call_id":"ack"},
                    "message":{
                        "bot_id":worker_id,
                        "chat_id":worker_dm,
                        "assignment_id":assignment_id,
                        "text":"已开始整理",
                        "intent":"ack",
                        "mentions":[]
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(ack["sender"]["kind"], "bot");
        assert_eq!(ack["sender"]["bot_id"], worker_id);
        assert_eq!(ack["intent"], "ack");

        let queued = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":"chat_main",
                    "text":"补充一项资料",
                    "mentions":[]
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        assert_eq!(queued["delivery"][0]["assignment_id"], assignment_id);
        assert_eq!(queued["delivery"][0]["state"], "queued");
        let reviewed = backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"delegate-worker-run","call_id":"done-review"},
                    "message":{
                        "bot_id":worker_id,
                        "chat_id":worker_dm,
                        "assignment_id":assignment_id,
                        "text":"检查待处理补充",
                        "intent":"done",
                        "mentions":[]
                    }
                }),
            )
            .await
            .unwrap();
        assert!(reviewed["_queue_review"].is_object());
        assert_eq!(
            reviewed["_queue_review"]["messages"][0]["delivery"][0]["state"],
            "read"
        );

        let completion = backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"delegate-worker-run","call_id":"done"},
                    "message":{
                        "bot_id":worker_id,
                        "chat_id":worker_dm,
                        "assignment_id":assignment_id,
                        "text":"整理完成：找到三份可用资料",
                        "intent":"done",
                        "mentions":[{"kind":"bot","bot_id":peer_id,"instruction":"复核交付报告"}],
                        "artifacts":[{"title":"交付报告","path_or_url":"reports/summary.md"}]
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(completion["sender"]["kind"], "bot");
        assert_eq!(completion["sender"]["bot_id"], worker_id);
        assert_eq!(completion["intent"], "done");

        let assignment = backend
            .call(
                "assignment.get",
                json!({"assignment_id":assignment_id}),
                &gateway.state,
            )
            .await
            .unwrap()["assignment"]
            .clone();
        assert_eq!(assignment["status"], "done");
        assert_eq!(assignment["result_message_id"], completion["id"]);

        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let messages = history["messages"].as_array().unwrap();
        let result_card = messages
            .iter()
            .find(|message| {
                message["assignment_id"] == assignment_id
                    && message["sender"]["kind"] == "system"
                    && message["blocks"][0]["type"] == "completion"
            })
            .expect("main receives a server-generated completion summary");
        assert_ne!(result_card["id"], completion["id"]);
        assert_eq!(
            result_card["blocks"][0]["summary"],
            "整理完成：找到三份可用资料"
        );
        assert_eq!(result_card["assignment_id"], assignment_id);
        assert_ne!(result_card["id"], assignment["result_message_id"]);
        assert_eq!(
            result_card["blocks"][0]["artifacts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            result_card["blocks"][0]["artifacts"][0]["title"],
            "交付报告"
        );
        assert_eq!(
            result_card["blocks"][0]["artifacts"][0]["path_or_url"],
            "reports/summary.md"
        );
        assert_eq!(
            result_card["blocks"][0]["next"],
            json!([{
                "bot_id":peer_id,
                "instruction":"复核交付报告"
            }])
        );
        assert!(messages.iter().any(|message| {
            message["blocks"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block["type"] == "delegation" && block["assignment_id"] == assignment_id
                })
            })
        }));
        assert!(!messages.iter().any(|message| {
            message["assignment_id"] == assignment_id
                && message["sender"]["kind"] == "bot"
                && message["sender"]["bot_id"] == worker_id
        }));
        let worker_history = backend
            .call(
                "chat.history",
                json!({"chat_id":worker_dm,"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(worker_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["id"] == completion["id"]
                    && message["sender"]["kind"] == "bot"
                    && message["sender"]["bot_id"] == worker_id
            }));

        let completion_replay = backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"delegate-worker-run","call_id":"done"},
                    "message":{
                        "bot_id":worker_id,
                        "chat_id":worker_dm,
                        "assignment_id":assignment_id,
                        "text":"整理完成：找到三份可用资料",
                        "intent":"done",
                        "mentions":[{"kind":"bot","bot_id":peer_id,"instruction":"复核交付报告"}],
                        "artifacts":[{"title":"交付报告","path_or_url":"reports/summary.md"}]
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(completion_replay["id"], completion["id"]);
        let main_after_replay = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            main_after_replay["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["id"] == result_card["id"])
                .count(),
            1,
            "replaying completion must not duplicate its result card"
        );

        // An explicit user message in the main chat routes execution to the
        // worker DM, while its result is returned to the source chat.
        let direct_message = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":"chat_main",
                    "text":"用户直接交办",
                    "mentions":[{"kind":"bot","bot_id":worker_id}]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let direct_id = direct_message["message"]["delivery"][0]["assignment_id"]
            .as_str()
            .unwrap();
        let direct_assignment = backend
            .call(
                "assignment.get",
                json!({"assignment_id":direct_id}),
                &gateway.state,
            )
            .await
            .unwrap()["assignment"]
            .clone();
        assert_eq!(direct_assignment["origin_chat_id"], worker_dm);
        assert_eq!(
            backend
                .orchestrator
                .resolve_assignment_return_chat(direct_id)
                .unwrap(),
            "chat_main"
        );
        let direct_reply = backend
            .call(
                "send_msg",
                json!({
                    "bot_id":worker_id,
                    "chat_id":worker_dm,
                    "assignment_id":direct_id,
                    "text":"这是用户直接交办的答复",
                    "intent":"done",
                    "mentions":[]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(direct_reply["sender"]["kind"], "bot");
        assert_eq!(direct_reply["sender"]["bot_id"], worker_id);
        let main_after_direct = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(main_after_direct["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["assignment_id"] == direct_id
                    && message["sender"]["kind"] == "system"
                    && message["blocks"][0]["type"] == "completion"
            }));

        // Release the durable store lock before reopening the same data root.
        drop(backend);
        drop(gateway);
        let reopened_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let reopened = ProductionBackend::open(home.path()).unwrap();
        let restored_main = reopened
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &reopened_gateway.state,
            )
            .await
            .unwrap();
        let restored_messages = restored_main["messages"].as_array().unwrap();
        let restored_result_cards = restored_messages
            .iter()
            .filter(|message| {
                message["assignment_id"] == assignment_id
                    && message["sender"]["kind"] == "system"
                    && message["blocks"][0]["type"] == "completion"
            })
            .collect::<Vec<_>>();
        assert_eq!(restored_result_cards.len(), 1);
        assert_eq!(restored_result_cards[0]["id"], result_card["id"]);
        assert_eq!(
            restored_result_cards[0]["blocks"][0]["artifacts"],
            result_card["blocks"][0]["artifacts"]
        );
        assert_eq!(
            restored_result_cards[0]["blocks"][0]["next"],
            result_card["blocks"][0]["next"]
        );
        assert_eq!(restored_result_cards[0]["assignment_id"], assignment_id);
        let restored_assignment = reopened
            .call(
                "assignment.get",
                json!({"assignment_id":assignment_id}),
                &reopened_gateway.state,
            )
            .await
            .unwrap()["assignment"]
            .clone();
        assert_eq!(restored_assignment["result_message_id"], completion["id"]);
        let restored_worker = reopened
            .call(
                "chat.history",
                json!({"chat_id":worker_dm,"limit":100}),
                &reopened_gateway.state,
            )
            .await
            .unwrap();
        assert!(restored_worker["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["id"] == completion["id"] && message["assignment_id"] == assignment_id
            }));
        let replay_after_restart = reopened
            .execution_send_msg(
                &reopened_gateway.state,
                json!({
                    "receipt":{"run_id":"delegate-worker-run","call_id":"done"},
                    "message":{
                        "bot_id":worker_id,
                        "chat_id":worker_dm,
                        "assignment_id":assignment_id,
                        "text":"整理完成：找到三份可用资料",
                        "intent":"done",
                        "mentions":[{"kind":"bot","bot_id":peer_id,"instruction":"复核交付报告"}],
                        "artifacts":[{"title":"交付报告","path_or_url":"reports/summary.md"}]
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(replay_after_restart["id"], completion["id"]);
        let after_restart = reopened
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &reopened_gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            after_restart["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["id"] == result_card["id"])
                .count(),
            1,
            "retry after restart must preserve one assignment-linked summary"
        );
        assert!(restored_messages.iter().any(|message| {
            message["id"] == queued["id"] && message["delivery"][0]["state"] == "read"
        }));
    }

    #[test]
    fn assignment_result_payload_respects_return_chat_permissions() {
        let snapshot = json!({
            "projects":{
                "project-a":{"id":"project-a","chat_id":"group-a","members":[{"bot_id":"peer"}]},
                "project-b":{"id":"project-b","chat_id":"group-b","members":[]}
            },
            "bots":{
                "worker":{"id":"worker","dm_chat_id":"dm_worker"},
                "peer":{"id":"peer","dm_chat_id":"dm_peer"},
                "outsider":{"id":"outsider","dm_chat_id":"dm_outsider"}
            },
            "artifacts":{
                "group-a-artifact":{"id":"group-a-artifact","assignment_id":"assignment","project_id":"project-a","bot_id":"worker","title":"A","path_or_url":"a.md","kind":"file"},
                "group-b-artifact":{"id":"group-b-artifact","assignment_id":"assignment","project_id":"project-b","bot_id":"worker","title":"B","path_or_url":"b.md","kind":"file"},
                "private-artifact":{"id":"private-artifact","assignment_id":"assignment","project_id":null,"bot_id":"worker","title":"Private","path_or_url":"private.md","kind":"file"}
            }
        });
        let canonical = json!({
            "artifacts":[
                {"title":"A","path_or_url":"a.md"},
                {"title":"B","path_or_url":"b.md"},
                {"title":"Private","path_or_url":"private.md"}
            ],
            "mentions":[
                {"kind":"bot","bot_id":"peer","instruction":"review"},
                {"kind":"bot","bot_id":"outsider","instruction":"review"}
            ]
        });

        let group_artifacts =
            assignment_result_artifacts(&snapshot, &canonical, "assignment", "group-a");
        assert_eq!(group_artifacts.len(), 1);
        assert_eq!(group_artifacts[0]["artifact_id"], "group-a-artifact");
        let other_group_artifacts =
            assignment_result_artifacts(&snapshot, &canonical, "assignment", "group-b");
        assert_eq!(other_group_artifacts.len(), 1);
        assert_eq!(other_group_artifacts[0]["artifact_id"], "group-b-artifact");
        let main_artifacts =
            assignment_result_artifacts(&snapshot, &canonical, "assignment", "chat_main");
        assert_eq!(main_artifacts.len(), 1);
        assert_eq!(main_artifacts[0]["artifact_id"], "private-artifact");
        assert_eq!(
            assignment_result_artifacts(&snapshot, &canonical, "assignment", "dm_peer"),
            Vec::<Value>::new()
        );

        assert_eq!(
            assignment_result_next(&snapshot, &canonical, "summary", "group-a"),
            vec![json!({"bot_id":"peer","instruction":"review"})]
        );
        assert_eq!(
            assignment_result_next(&snapshot, &canonical, "summary", "group-b"),
            Vec::<Value>::new()
        );
        assert_eq!(
            assignment_result_next(&snapshot, &canonical, "summary", "dm_peer"),
            vec![json!({"bot_id":"peer","instruction":"review"})]
        );
    }

    #[tokio::test]
    async fn cross_scope_blocked_result_preserves_blocked_reason_only() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let worker = backend
            .call(
                "bot.create",
                json!({"name":"阻塞结果 worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let worker_id = worker["bot"]["id"].as_str().unwrap();
        let worker_dm = worker["bot"]["dm_chat_id"].as_str().unwrap();
        let delegated = backend
            .call(
                "delegate",
                json!({
                    "origin_chat_id":"chat_main",
                    "bot_id":worker_id,
                    "title":"可能阻塞的工作",
                    "instruction":"尝试完成",
                    "from":"main",
                    "client_request_id":"cross-scope-blocked-delegate"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = delegated["id"].as_str().unwrap();
        let blocked = backend
            .execution_send_msg(
                &gateway.state,
                json!({
                    "receipt":{"run_id":"cross-scope-blocked-run","call_id":"blocked"},
                    "message":{
                        "bot_id":worker_id,
                        "chat_id":worker_dm,
                        "assignment_id":assignment_id,
                        "text":"缺少外部授权，无法继续",
                        "intent":"blocked",
                        "mentions":[],
                        "artifacts":[{"title":"临时证据","path_or_url":"private/evidence.txt"}]
                    }
                }),
            )
            .await
            .unwrap();
        assert_eq!(blocked["intent"], "blocked");
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let result = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["assignment_id"] == assignment_id
                    && message["sender"]["kind"] == "system"
                    && message["fallback_text"] == "任务受阻：缺少外部授权，无法继续"
            })
            .expect("blocked result should be linked to its assignment");
        assert_eq!(
            result["blocks"],
            json!([{
                "type":"blocked",
                "reason":"缺少外部授权，无法继续"
            }])
        );
        assert_eq!(result["assignment_id"], assignment_id);
        assert!(result["blocks"][0].get("artifacts").is_none());
        assert!(result["blocks"][0].get("next").is_none());
        let worker_history = backend
            .call(
                "chat.history",
                json!({"chat_id":worker_dm,"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(worker_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["id"] == blocked["id"]
                    && message["chat_id"] == worker_dm
                    && message["sender"]["kind"] == "bot"
            }));
    }

    #[tokio::test]
    async fn routine_tick_returns_execution_dispatch_and_assignment_event() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        backend.call("routine.create", json!({"bot_id":"main","name":"巡检","instructions":"检查状态","schedules":[{"cron":"*/5 * * * *","label":"每五分钟"}],"timezone":"Asia/Shanghai"}), &gateway.state).await.unwrap();
        let result = backend
            .tick_routines(&gateway.state, Utc::now() + chrono::Duration::minutes(6))
            .await
            .unwrap();
        assert_eq!(result["runs"].as_array().unwrap().len(), 1);
        assert_eq!(result["dispatch"][0]["instruction"], "检查状态");
        let event = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .find(|event| event.event == "assignment.created")
            .unwrap();
        let _: macbot_protocol::Assignment =
            serde_json::from_value(event.data["assignment"].clone()).unwrap();
        let assignment_id = event.data["assignment"]["id"].as_str().unwrap();
        let main_history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let task_card = main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "task_card" && block["assignment_id"] == assignment_id
                    })
                })
            })
            .unwrap();
        let _: Message = serde_json::from_value(task_card.clone()).unwrap();
        let routine_id = result["runs"][0]["routine_id"].as_str().unwrap();
        backend
            .call(
                "routine.test_run",
                json!({"routine_id":routine_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        for event in backend.store.events_since(0).unwrap() {
            if event.event == "routine.run" {
                assert_eq!(event.data.as_object().unwrap().len(), 1);
                assert!(event.data.get("run").is_some());
                let _: macbot_protocol::RoutineRun =
                    serde_json::from_value(event.data["run"].clone()).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn project_attention_refresh_emits_typed_system_notice_once() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call("bot.create", json!({"name":"阻塞 Bot"}), &gateway.state)
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"关注测试","goal":"attention","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot["bot"]["id"],
                    "title":"阻塞任务",
                    "instruction":"等待",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .orchestrator
            .finish_assignment(assignment["id"].as_str().unwrap(), "blocked")
            .unwrap();
        let refreshed = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        let notice: Message = serde_json::from_value(refreshed["notices"][0].clone()).unwrap();
        assert_eq!(notice.sender, macbot_protocol::Sender::System);
        assert!(matches!(
            notice.blocks[0],
            macbot_protocol::Block::System {
                code: macbot_protocol::SystemCode::Info,
                ..
            }
        ));
        let second = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        assert!(second["notices"].as_array().unwrap().is_empty());

        // A new blocked task must still emit while the same poll repairs and
        // deduplicates historical notices and their Main follow-up tasks.
        let another = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot["bot"]["id"],
                    "title":"Second blocked task",
                    "instruction":"Wait",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .orchestrator
            .finish_assignment(another["id"].as_str().unwrap(), "blocked")
            .unwrap();
        let next = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        assert_eq!(next["notices"].as_array().unwrap().len(), 1);
        assert_ne!(next["notices"][0]["id"], refreshed["notices"][0]["id"]);
        let seq = backend.store.last_event_seq().unwrap();
        backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        assert_eq!(backend.store.last_event_seq().unwrap(), seq);
        let events = backend.store.events_since(0).unwrap();
        for message in [&refreshed["notices"][0], &next["notices"][0]] {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event == "message.created"
                        && event.data["message"]["id"] == message["id"])
                    .count(),
                1
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "assignment.created"
                    && event.data["assignment"]["trigger_message_id"]
                        == refreshed["notices"][0]["id"])
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "assignment.created"
                    && event.data["assignment"]["trigger_message_id"] == next["notices"][0]["id"])
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn project_attention_does_not_requeue_main_no_report() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call("bot.create", json!({"name":"关注 worker"}), &gateway.state)
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"main no-report","goal":"attention","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let chat_id = project["project"]["chat_id"].as_str().unwrap();
        let main_assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":"main",
                    "title":"main coordination",
                    "instruction":"coordinate",
                    "from":"system"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let main_assignment_id = main_assignment["id"].as_str().unwrap();
        backend
            .orchestrator
            .finish_assignment(main_assignment_id, "done")
            .unwrap();

        // Simulate a durable main-owned attention marker recovered after a
        // restart. The marker must remain visible, but it must not create a
        // second main assignment for the same coordination work.
        let attention_id = format!("task_attention:task_no_report:{main_assignment_id}");
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["messages"][&attention_id] = json!({
            "id":attention_id,
            "chat_id":chat_id,
            "sender":"system",
            "created_at":Utc::now().to_rfc3339(),
            "text":"主 Bot 的任务结束但没有提交完成报告",
            "intent":null,
            "assignment_id":main_assignment_id,
            "mentions":[],
            "artifacts":[],
            "options":[],
            "question_id":null,
            "delivery":[],
            "fallback_text":"主 Bot 的任务结束但没有提交完成报告"
        });
        backend.orchestrator.restore(snapshot).unwrap();

        let refreshed = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        assert_eq!(refreshed["notices"].as_array().unwrap().len(), 1);
        assert_eq!(refreshed["notices"][0]["id"], attention_id);
        let snapshot = backend.orchestrator.snapshot().unwrap();
        let assignments = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .filter(|assignment| assignment["trigger_message_id"] == attention_id)
            .count();
        assert_eq!(assignments, 0);

        // A worker no-report marker still wakes main and creates exactly one
        // main follow-up, preserving the normal worker attention path.
        let worker_assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":bot["bot"]["id"],
                    "title":"worker no-report",
                    "instruction":"work",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let worker_assignment_id = worker_assignment["id"].as_str().unwrap();
        backend
            .orchestrator
            .finish_assignment(worker_assignment_id, "done")
            .unwrap();
        let worker_refresh = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        let worker_notice = worker_refresh["notices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|notice| {
                notice["id"] == format!("task_attention:task_no_report:{worker_assignment_id}")
            })
            .expect("worker no-report notice");
        let worker_notice_id = worker_notice["id"].as_str().unwrap();
        let worker_followups = backend.orchestrator.snapshot().unwrap()["assignments"]
            .as_object()
            .unwrap()
            .values()
            .filter(|assignment| assignment["trigger_message_id"] == worker_notice_id)
            .count();
        assert_eq!(worker_followups, 1);
    }

    #[tokio::test]
    async fn project_attention_does_not_derive_main_siblings_for_multiple_worker_notices() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let mut workers = Vec::new();
        for index in 1..=3 {
            workers.push(
                backend
                    .call(
                        "bot.create",
                        json!({"name":format!("attention worker {index}")}),
                        &gateway.state,
                    )
                    .await
                    .unwrap()["bot"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"active main attention",
                    "goal":"reuse one main coordination",
                    "member_bot_ids":workers
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let chat_id = project["project"]["chat_id"].as_str().unwrap();
        let main_working = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":"main",
                    "title":"working main",
                    "instruction":"coordinate",
                    "from":"system"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let main_queued = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":"main",
                    "title":"queued main",
                    "instruction":"wait for the first coordinator",
                    "from":"system"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(main_working["status"], "working");
        assert_eq!(main_queued["status"], "queued");
        let main_waiting = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":"main",
                    "title":"waiting main",
                    "instruction":"wait for a decision",
                    "from":"system"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["assignments"][main_waiting["id"].as_str().unwrap()]["status"] =
            json!("waiting_user");
        snapshot["assignments"][main_waiting["id"].as_str().unwrap()]["wait"] =
            json!({"reason":"question","message_id":null});
        backend.orchestrator.restore(snapshot).unwrap();

        let mut worker_assignment_ids = Vec::new();
        for worker in &workers {
            let assignment = backend
                .call(
                    "assignment.create",
                    json!({
                        "project_id":project_id,
                        "origin_chat_id":chat_id,
                        "bot_id":worker,
                        "title":"worker no-report",
                        "instruction":"work",
                        "from":"main"
                    }),
                    &gateway.state,
                )
                .await
                .unwrap();
            worker_assignment_ids.push(assignment["id"].as_str().unwrap().to_owned());
        }
        for assignment_id in &worker_assignment_ids {
            backend
                .orchestrator
                .finish_assignment(assignment_id, "done")
                .unwrap();
        }

        let refreshed = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        let notices = refreshed["notices"].as_array().unwrap();
        assert_eq!(notices.len(), workers.len());

        let snapshot = backend.orchestrator.snapshot().unwrap();
        let assignments = snapshot["assignments"]
            .as_object()
            .unwrap()
            .values()
            .filter(|assignment| {
                assignment["project_id"] == project_id
                    && assignment["bot_id"] == "main"
                    && matches!(
                        assignment["status"].as_str(),
                        Some("queued" | "working" | "waiting_user" | "waiting_bot" | "blocked")
                    )
            })
            .collect::<Vec<_>>();
        assert_eq!(assignments.len(), 3);
        for notice in notices {
            let notice_id = notice["id"].as_str().unwrap();
            assert_eq!(
                assignments
                    .iter()
                    .filter(|assignment| assignment["trigger_message_id"] == notice_id)
                    .count(),
                0
            );
        }
    }

    #[tokio::test]
    async fn project_attention_restores_snapshot_notice_when_chat_row_is_missing() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call("bot.create", json!({"name":"缺失 row Bot"}), &gateway.state)
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"缺失 row 关注","goal":"row recovery","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot["bot"]["id"],
                    "title":"缺失 row 任务",
                    "instruction":"等待",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .orchestrator
            .finish_assignment(assignment["id"].as_str().unwrap(), "blocked")
            .unwrap();
        let first = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        let notice_id = first["notices"][0]["id"].as_str().unwrap().to_owned();
        let chat_id = project["project"]["chat_id"].as_str().unwrap();
        let path = format!("data/chats/{}/messages.jsonl", takeover_component(chat_id));
        let rows = backend.store.read_jsonl::<Value>(&path).unwrap();
        assert!(rows.iter().any(|row| row["id"] == notice_id));
        let retained = rows
            .into_iter()
            .filter(|row| row["id"] != notice_id)
            .collect::<Vec<_>>();
        fs::write(
            home.path().join(&path),
            retained
                .iter()
                .map(|row| serde_json::to_string(row).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + if retained.is_empty() { "" } else { "\n" },
        )
        .unwrap();
        let event_seq = backend.store.last_event_seq().unwrap();

        let repaired = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        assert!(repaired["notices"].as_array().unwrap().is_empty());
        let restored = backend
            .store
            .read_jsonl::<Value>(&path)
            .unwrap()
            .into_iter()
            .find(|row| row["id"] == notice_id)
            .expect("snapshot-only attention message should be persisted");
        assert!(restored["seq"].as_u64().unwrap_or(0) > 0);
        assert_eq!(backend.store.last_event_seq().unwrap(), event_seq);
        assert_eq!(
            backend
                .store
                .events_since(0)
                .unwrap()
                .into_iter()
                .filter(|event| {
                    event.event == "message.created" && event.data["message"]["id"] == notice_id
                })
                .count(),
            1
        );
        // A durable row with a missing/zero sequence is also incomplete even
        // when its payload matches the orchestrator snapshot.
        let mut zero_seq_rows = backend.store.read_jsonl::<Value>(&path).unwrap();
        zero_seq_rows
            .iter_mut()
            .find(|row| row["id"] == notice_id)
            .unwrap()["seq"] = json!(0);
        fs::write(
            home.path().join(&path),
            zero_seq_rows
                .iter()
                .map(|row| serde_json::to_string(row).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        let resequenced = backend
            .store
            .read_jsonl::<Value>(&path)
            .unwrap()
            .into_iter()
            .rfind(|row| row["id"] == notice_id)
            .unwrap();
        assert!(resequenced["seq"].as_u64().unwrap_or(0) > 0);
    }

    #[tokio::test]
    async fn project_attention_repairs_missing_events_after_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call("bot.create", json!({"name":"崩溃恢复 Bot"}), &gateway.state)
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"恢复关注","goal":"attention recovery","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot["bot"]["id"],
                    "title":"恢复任务",
                    "instruction":"等待",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .orchestrator
            .finish_assignment(assignment["id"].as_str().unwrap(), "blocked")
            .unwrap();
        let first = backend
            .refresh_project_attention(&gateway.state, Utc::now())
            .await
            .unwrap();
        let notice_id = first["notices"][0]["id"].as_str().unwrap().to_owned();
        let main_assignment_id = backend.orchestrator.snapshot().unwrap()["assignments"]
            .as_object()
            .unwrap()
            .values()
            .find(|item| item["trigger_message_id"] == notice_id)
            .and_then(|item| item["id"].as_str())
            .unwrap()
            .to_owned();
        let retained = backend
            .store
            .read_jsonl::<Value>("data/events/events.jsonl")
            .unwrap()
            .into_iter()
            .filter(|event| {
                !((event["event"] == "message.created"
                    && event["data"]["message"]["id"] == notice_id)
                    || (event["event"] == "assignment.created"
                        && event["data"]["assignment"]["id"] == main_assignment_id))
            })
            .map(|event| serde_json::to_string(&event).unwrap())
            .collect::<Vec<_>>();
        fs::write(
            home.path().join("data/events/events.jsonl"),
            retained.join("\n") + if retained.is_empty() { "" } else { "\n" },
        )
        .unwrap();
        drop(backend);
        drop(gateway);
        let restarted_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let events_before_poll = restarted.store.events_since(0).unwrap();
        assert_eq!(
            events_before_poll
                .iter()
                .filter(|event| event.event == "message.created"
                    && event.data["message"]["id"] == notice_id)
                .count(),
            1
        );
        assert_eq!(
            events_before_poll
                .iter()
                .filter(|event| event.event == "assignment.created"
                    && event.data["assignment"]["id"] == main_assignment_id)
                .count(),
            1
        );
        let seq_before_poll = restarted.store.last_event_seq().unwrap();
        let repaired = restarted
            .refresh_project_attention(&restarted_gateway.state, Utc::now())
            .await
            .unwrap();
        assert!(repaired["notices"].as_array().unwrap().is_empty());
        let events = restarted.store.events_since(0).unwrap();
        assert_eq!(restarted.store.last_event_seq().unwrap(), seq_before_poll);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "message.created"
                    && event.data["message"]["id"] == notice_id)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "assignment.created"
                    && event.data["assignment"]["id"] == main_assignment_id)
                .count(),
            1
        );
        let second = restarted
            .refresh_project_attention(&restarted_gateway.state, Utc::now())
            .await
            .unwrap();
        assert!(second["notices"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn bot_dm_route_creates_read_only_chat_and_group_reference() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let from = backend
            .call("bot.create", json!({"name":"发送者"}), &gateway.state)
            .await
            .unwrap();
        let to = backend
            .call("bot.create", json!({"name":"接收者"}), &gateway.state)
            .await
            .unwrap();
        let from_id = from["bot"]["id"].as_str().unwrap();
        let to_id = to["bot"]["id"].as_str().unwrap();
        let sent = backend
            .call(
                "send_msg",
                json!({
                    "bot_id":from_id,
                    "chat_id":from["bot"]["dm_chat_id"],
                    "to":{"bot":to_id},
                    "text":"私信",
                    "intent":"ack",
                    "mentions":[]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let sent_message: Message = serde_json::from_value(sent.clone()).unwrap();
        assert!(sent_message.chat_id.starts_with("bot_dm_"));
        let chats = backend
            .call("chat.list", json!({}), &gateway.state)
            .await
            .unwrap();
        let bot_dm = chats["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|chat| chat["id"] == sent_message.chat_id)
            .unwrap();
        let _: Chat = serde_json::from_value(bot_dm.clone()).unwrap();
        assert_eq!(bot_dm["kind"], "bot_dm");
        let sender_history = backend
            .call(
                "chat.history",
                json!({"chat_id":from["bot"]["dm_chat_id"],"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(sender_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "bot_dm_ref"
                            && block["chat_id"] == sent_message.chat_id
                            && block["count"] == 1
                    })
                })
            }));
        let denied = backend
            .call(
                "chat.send",
                json!({"chat_id":sent_message.chat_id,"text":"禁止","mentions":[]}),
                &gateway.state,
            )
            .await;
        assert_eq!(denied.unwrap_err().code, "forbidden");
    }

    #[tokio::test]
    async fn bot_lifecycle_events_have_single_typed_bot_and_chat_payloads() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let created = backend
            .call("bot.create", json!({"name":"事件 Bot"}), &gateway.state)
            .await
            .unwrap();
        let events = backend.store.events_since(0).unwrap();
        let bot_created = events
            .iter()
            .filter(|event| event.event == "bot.created")
            .collect::<Vec<_>>();
        let chat_created = events
            .iter()
            .filter(|event| event.event == "chat.created")
            .collect::<Vec<_>>();
        assert_eq!(bot_created.len(), 1);
        assert_eq!(chat_created.len(), 1);
        let _: Bot = serde_json::from_value(bot_created[0].data["bot"].clone()).unwrap();
        let _: Chat = serde_json::from_value(chat_created[0].data["chat"].clone()).unwrap();
        assert_eq!(chat_created[0].data["chat"]["id"], created["dm_chat"]["id"]);

        let before = events.len();
        let template = backend
            .call(
                "bot.create_from_template",
                json!({"template_id":"product-code-test"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let events = backend.store.events_since(0).unwrap();
        let delta = &events[before..];
        let template_bots = template["bots"].as_array().unwrap().len();
        let template_chats = template["dm_chats"].as_array().unwrap().len();
        assert_eq!(
            delta
                .iter()
                .filter(|event| event.event == "bot.created")
                .count(),
            template_bots
        );
        assert_eq!(
            delta
                .iter()
                .filter(|event| event.event == "chat.created")
                .count(),
            template_chats
        );
        for event in delta.iter().filter(|event| event.event == "bot.created") {
            let _: Bot = serde_json::from_value(event.data["bot"].clone()).unwrap();
        }
        for event in delta.iter().filter(|event| event.event == "chat.created") {
            let _: Chat = serde_json::from_value(event.data["chat"].clone()).unwrap();
        }
    }

    #[tokio::test]
    async fn production_duplicate_uses_preallocated_target_and_rolls_back_skills() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let source = backend
            .call(
                "bot.create",
                json!({"name":"复制源","client_request_id":"duplicate-source"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let source_id = source["bot"]["id"].as_str().unwrap();
        let feature_service = FeatureService::with_store(
            backend.store.clone(),
            backend.store.root().to_path_buf(),
            Vec::<std::path::PathBuf>::new(),
        )
        .unwrap();
        feature_service
            .skill_rpc(
                "skill.create",
                json!({"name":"duplicate-skill","content":"---\nname: duplicate-skill\ndescription: duplicate test\n---\nUse this skill."}),
            )
            .unwrap();
        feature_service
            .skill_rpc(
                "skill.set_enabled",
                json!({"name":"duplicate-skill","enabled":false,"bot_id":source_id}),
            )
            .unwrap();
        backend
            .call(
                "routine.create",
                json!({
                    "bot_id":source_id,
                    "name":"复制巡检",
                    "instructions":"检查复制",
                    "schedules":[{"cron":"*/5 * * * *","label":"每五分钟"}],
                    "timezone":"Asia/Shanghai"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let target_id = "duplicate-target-01";
        let duplicated = backend
            .duplicate_bot_with_feature_service(
                &json!({"bot_id":source_id,"name":"复制目标","target_bot_id":target_id}),
                Arc::new(feature_service.clone()),
            )
            .await
            .unwrap();
        let _: BotDuplicateResult = serde_json::from_value(duplicated.clone()).unwrap();
        assert_eq!(duplicated["bot"]["id"], target_id);
        assert_eq!(duplicated["dm_chat"]["id"], format!("dm_{target_id}"));
        let routines = backend
            .call("routine.list", json!({"bot_id":target_id}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(routines["routines"].as_array().unwrap().len(), 1);
        assert_eq!(routines["routines"][0]["enabled"], true);
        assert!(!feature_service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("duplicate-skill", Some(target_id))
            .unwrap());

        let existing = backend
            .call(
                "bot.create",
                json!({"name":"已存在","client_request_id":"duplicate-existing"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let existing_id = existing["bot"]["id"].as_str().unwrap();
        let failed = backend
            .duplicate_bot_with_feature_service(
                &json!({"bot_id":source_id,"name":"不会创建","target_bot_id":existing_id}),
                Arc::new(feature_service.clone()),
            )
            .await;
        assert!(failed.is_err());
        assert!(feature_service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("duplicate-skill", Some(existing_id))
            .unwrap());
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(!bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"].as_str() == Some("不会创建")));
    }

    #[tokio::test]
    async fn composed_duplicate_rolls_back_when_persist_snapshot_fails() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let source = backend
            .call(
                "bot.create",
                json!({"name":"持久化失败源","client_request_id":"dup-io-source"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let source_id = source["bot"]["id"].as_str().unwrap();
        let feature_service = Arc::new(
            FeatureService::with_store(
                backend.store.clone(),
                backend.store.root().to_path_buf(),
                Vec::<std::path::PathBuf>::new(),
            )
            .unwrap(),
        );
        feature_service
            .skill_rpc(
                "skill.create",
                json!({"name":"dup-io-skill","content":"---\nname: dup-io-skill\ndescription: test\n---\nUse."}),
            )
            .unwrap();
        feature_service
            .skill_rpc(
                "skill.set_enabled",
                json!({"name":"dup-io-skill","enabled":false,"bot_id":source_id}),
            )
            .unwrap();
        backend
            .call(
                "routine.create",
                json!({
                    "bot_id":source_id,
                    "name":"复制失败例程",
                    "instructions":"test",
                    "schedules":[{"cron":"*/5 * * * *","label":"测试"}],
                    "timezone":"Asia/Shanghai"
                }),
                &gateway.state,
            )
            .await
            .unwrap();

        let state_path = home.path().join("data/orchestrator/state.json");
        fs::remove_file(&state_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        let target_id = "dup-io-target";
        let failed = backend
            .duplicate_bot_with_feature_service_and_state(
                &gateway.state,
                &json!({"bot_id":source_id,"name":"不会留下","target_bot_id":target_id}),
                feature_service.clone(),
            )
            .await;
        assert!(failed.is_err());

        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(!bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bot| bot["id"].as_str() == Some(target_id)));
        let routines = backend
            .call("routine.list", json!({"bot_id":target_id}), &gateway.state)
            .await
            .unwrap();
        assert!(routines["routines"].as_array().unwrap().is_empty());
        assert!(feature_service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("dup-io-skill", Some(target_id))
            .unwrap());
        assert!(!backend
            .store
            .events_since(0)
            .unwrap()
            .iter()
            .any(|event| event.event == "bot.created"
                && event.data["bot"]["id"].as_str() == Some(target_id)));

        // The failed write left state.json as a directory; remove the injected
        // fault and verify restart selects the compensating WAL snapshot.
        fs::remove_dir(&state_path).unwrap();
        drop(feature_service);
        drop(backend);
        drop(gateway);
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(!bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bot| bot["id"].as_str() == Some(target_id)));
        let routines = backend
            .call("routine.list", json!({"bot_id":target_id}), &gateway.state)
            .await
            .unwrap();
        assert!(routines["routines"].as_array().unwrap().is_empty());
        let feature_service = Arc::new(
            FeatureService::with_store(
                backend.store.clone(),
                backend.store.root().to_path_buf(),
                Vec::<std::path::PathBuf>::new(),
            )
            .unwrap(),
        );
        assert!(feature_service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("dup-io-skill", Some(target_id))
            .unwrap());

        // The public adapter.call path must use the same state-aware route.
        let snapshot = backend.orchestrator.snapshot().unwrap();
        backend
            .store
            .write_snapshot("data/orchestrator/state.json", &snapshot)
            .unwrap();
        fs::remove_file(&state_path).unwrap();
        fs::create_dir(&state_path).unwrap();
        let target_id_2 = "dup-io-target-call";
        let failed = backend
            .call(
                "bot.duplicate",
                json!({"bot_id":source_id,"name":"call不会留下","target_bot_id":target_id_2,"client_request_id":"dup-io-failed"}),
                &gateway.state,
            )
            .await;
        assert!(failed.is_err());
        let operations = backend
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .unwrap();
        let rollback = operations
            .iter()
            .rfind(|operation| {
                operation.get("client_request_id").and_then(Value::as_str) == Some("dup-io-failed")
            })
            .unwrap();
        assert_eq!(rollback["status"], "rolled_back");
        fs::remove_dir(&state_path).unwrap();
        drop(feature_service);
        drop(backend);
        drop(gateway);
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bots = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(!bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bot| bot["id"].as_str() == Some(target_id_2)));
        let routines = backend
            .call(
                "routine.list",
                json!({"bot_id":target_id_2}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(routines["routines"].as_array().unwrap().is_empty());
        let feature_service = FeatureService::with_store(
            backend.store.clone(),
            backend.store.root().to_path_buf(),
            Vec::<std::path::PathBuf>::new(),
        )
        .unwrap();
        assert!(feature_service
            .skill_registry
            .read()
            .unwrap()
            .is_enabled_for("dup-io-skill", Some(target_id_2))
            .unwrap());
    }

    #[tokio::test]
    async fn assignment_stop_emits_one_protocol_system_message_and_is_repeat_safe() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "origin_chat_id":"chat_main",
                    "bot_id":"main",
                    "title":"停止测试",
                    "instruction":"停止",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap();
        let stopped = backend
            .call(
                "assignment.stop",
                json!({"assignment_id":assignment_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(stopped["assignment"]["status"], "cancelled");
        let events = backend.store.events_since(0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "message.created")
                .count(),
            2
        );
        let message = events
            .iter()
            .find(|event| {
                event.event == "message.created"
                    && event.data["message"]["blocks"][0]["code"] == "task_stopped"
            })
            .unwrap();
        let _: Message = serde_json::from_value(message.data["message"].clone()).unwrap();
        assert_eq!(message.data["message"]["blocks"][0]["type"], "system");
        assert_eq!(message.data["message"]["blocks"][0]["code"], "task_stopped");
        assert_eq!(
            message.data["message"]["id"],
            format!("task_stopped:{assignment_id}")
        );
        assert!(message.data["message"]["seq"].as_u64().unwrap_or(0) > 0);
        backend
            .call(
                "assignment.stop",
                json!({"assignment_id":assignment_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            backend
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| event.event == "message.created")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn cancelled_approvals_expire_and_repair_durable_events_once() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "origin_chat_id":"chat_main", "bot_id":"main",
                    "title":"cancel approval", "instruction":"wait", "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let id = assignment["id"].as_str().unwrap();
        let approval = backend
            .orchestrator
            .rpc(
                "approval.request",
                json!({
                    "bot_id":"main", "assignment_id":id, "chat_id":"chat_main",
                    "tool":"write", "risk":"write", "summary":"test write", "detail":"{}"
                }),
            )
            .await
            .unwrap();
        let approval_id = approval["id"].as_str().unwrap();
        let standalone = backend
            .orchestrator
            .rpc(
                "approval.request",
                json!({
                    "bot_id":"main", "assignment_id":null, "chat_id":"chat_main",
                    "tool":"write", "risk":"write", "summary":"DM write", "detail":"{}"
                }),
            )
            .await
            .unwrap();
        backend
            .persist_orchestrator(json!({"method":"test.setup", "result":{},"status":"done"}))
            .await
            .unwrap();
        backend
            .call(
                "assignment.stop",
                json!({"assignment_id":id}),
                &gateway.state,
            )
            .await
            .unwrap();
        let current = backend
            .call("approval.list", json!({}), &gateway.state)
            .await
            .unwrap();
        let expired = current["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == approval_id)
            .unwrap()
            .clone();
        assert_eq!(expired["state"], "expired");
        assert!(expired["decided_at"].is_string());
        let pending = backend
            .call(
                "approval.list",
                json!({"state":["pending"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(pending["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == standalone["id"]));
        assert!(!pending["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == approval_id));
        let workbench = backend
            .call("workbench.get", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(!workbench["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["approval"]["id"] == approval_id));
        for decision in ["allow_once", "always_allow", "deny"] {
            assert!(backend
                .call(
                    "approval.decide",
                    json!({"approval_id":approval_id,"decision":decision}),
                    &gateway.state
                )
                .await
                .is_err());
        }
        backend
            .call(
                "assignment.stop",
                json!({"assignment_id":id}),
                &gateway.state,
            )
            .await
            .unwrap();
        let resolved_count = |backend: &ProductionBackend| {
            backend
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| {
                    event.event == "approval.resolved"
                        && event.data["approval"]["id"] == approval_id
                })
                .count()
        };
        assert_eq!(resolved_count(&backend), 1);
        let event = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .find(|event| {
                event.event == "approval.resolved" && event.data["approval"]["id"] == approval_id
            })
            .unwrap();
        macbot_protocol::EventData::decode(
            &macbot_protocol::EventName::ApprovalResolved,
            event.data,
        )
        .unwrap();

        // Simulate an old writer with a terminal assignment and pending approval.
        let mut old = backend.orchestrator.snapshot().unwrap();
        old["approvals"][approval_id]["state"] = json!("pending");
        old["approvals"][approval_id]["decided_at"] = Value::Null;
        backend
            .store
            .append_jsonl(
                "data/orchestrator/operations.jsonl",
                &json!({"method":"test.legacy", "status":"done", "snapshot":old, "result":{}}),
            )
            .unwrap();
        drop(backend);
        let restored = ProductionBackend::open(home.path()).unwrap();
        let snapshot = restored.orchestrator.snapshot().unwrap();
        assert_eq!(snapshot["approvals"][approval_id]["state"], "expired");
        let decided_at = snapshot["approvals"][approval_id]["decided_at"].clone();
        assert_eq!(decided_at, snapshot["assignments"][id]["finished_at"]);
        let disk: Value = restored
            .store
            .read_snapshot("data/orchestrator/state.json")
            .unwrap()
            .unwrap();
        assert_eq!(
            disk["approvals"][approval_id],
            snapshot["approvals"][approval_id]
        );
        assert_eq!(resolved_count(&restored), 1);
        drop(restored);
        let twice = ProductionBackend::open(home.path()).unwrap();
        assert_eq!(
            twice.orchestrator.snapshot().unwrap()["approvals"][approval_id]["decided_at"],
            decided_at
        );
        assert_eq!(resolved_count(&twice), 1);
    }

    #[tokio::test]
    async fn project_confirm_done_accepts_active_project() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"active-done-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"active-done-project",
                    "goal":"finish active project",
                    "member_bot_ids":[bot["bot"]["id"]]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let done = backend
            .call(
                "project.confirm_done",
                json!({"project_id":project["project"]["id"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(done["project"]["status"], "done");
    }

    #[tokio::test]
    async fn project_request_changes_returns_canonical_message_and_survives_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"changes-worker","client_request_id":"changes-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"changes-project",
                    "goal":"verify request changes",
                    "member_bot_ids":[bot_id]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap();
        let chat_id = project["project"]["chat_id"].as_str().unwrap();
        backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":bot_id,
                    "title":"review",
                    "instruction":"prepare review",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .call(
                "project.request_review",
                json!({"project_id":project_id,"summary":"ready for review"}),
                &gateway.state,
            )
            .await
            .unwrap();

        let response = backend
            .call(
                "project.request_changes",
                json!({
                    "project_id":project_id,
                    "text":"请补充回归测试",
                    "client_request_id":"changes-1"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(response.as_object().unwrap().len(), 1);
        let message: WireMessage = serde_json::from_value(response["message"].clone()).unwrap();
        assert_eq!(message.chat_id, chat_id);
        assert_eq!(message.fallback_text, "请补充回归测试");
        assert!(message
            .mentions
            .iter()
            .any(|mention| { matches!(mention, macbot_protocol::Mention::Main) }));
        assert!(message.seq > 0);
        let main_history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|card| {
                card["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "review_card"
                            && block["project_id"] == project_id
                            && block["state"] == "changes_requested"
                    })
                })
            }));
        let message_id = message.id.clone();
        let message_seq = message.seq;

        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":chat_id,"after_seq":message_seq - 1,"limit":20}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["id"] == message_id)
                .count(),
            1
        );
        assert_eq!(history["messages"][0]["seq"], message_seq);
        assert_eq!(history["messages"][0]["fallback_text"], "请补充回归测试");

        let events = backend.store.events_since(0).unwrap();
        let project_event = events
            .iter()
            .rev()
            .find(|event| {
                event.event == "project.updated"
                    && event.data["project"]["id"] == project_id
                    && event.data["project"]["status"] == "active"
            })
            .unwrap();
        let updated_project: WireProject =
            serde_json::from_value(project_event.data["project"].clone()).unwrap();
        assert_eq!(
            updated_project.status,
            macbot_protocol::ProjectStatus::Active
        );
        let message_events = events
            .iter()
            .filter(|event| {
                event.event == "message.created" && event.data["message"]["id"] == message_id
            })
            .collect::<Vec<_>>();
        assert_eq!(message_events.len(), 1);
        let event_message: WireMessage =
            serde_json::from_value(message_events[0].data["message"].clone()).unwrap();
        assert_eq!(event_message.seq, message_seq);
        assert!(event_message
            .mentions
            .iter()
            .any(|mention| { matches!(mention, macbot_protocol::Mention::Main) }));

        drop(backend);
        drop(gateway);
        let restarted_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let replay = restarted
            .call(
                "chat.history",
                json!({"chat_id":chat_id,"after_seq":message_seq - 1,"limit":20}),
                &restarted_gateway.state,
            )
            .await
            .unwrap();
        let replay_messages = replay["messages"].as_array().unwrap();
        assert_eq!(
            replay_messages
                .iter()
                .filter(|item| item["id"] == message_id)
                .count(),
            1
        );
        assert_eq!(replay_messages[0]["seq"], message_seq);
        assert_eq!(
            restarted.store.last_chat_sequence(chat_id).unwrap(),
            message_seq
        );
    }

    #[tokio::test]
    async fn project_review_requires_confirmation_and_propose_returns_question() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"worker","client_request_id":"review-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"review","goal":"ship","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_chat_id = project["project"]["chat_id"].as_str().unwrap().to_owned();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot["bot"]["id"],
                    "title":"ship",
                    "instruction":"ship",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .call(
                "send_msg",
                json!({
                    "bot_id":bot["bot"]["id"],
                    "chat_id":project["project"]["chat_id"],
                    "assignment_id":assignment["id"],
                    "text":"产物已生成",
                    "intent":"ack",
                    "artifacts":[{"title":"报告","path_or_url":"runs/report.md"}]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let review = backend
            .call(
                "project.request_review",
                json!({"project_id":project["project"]["id"],"summary":"ready","client_request_id":"review-card-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(review["project"]["status"], "review");
        let bootstrap = backend
            .call("bootstrap", json!({}), &gateway.state)
            .await
            .unwrap();
        assert!(bootstrap["pending"]["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &project["project"]["id"]));
        let retry = backend
            .call(
                "project.request_review",
                json!({"project_id":project["project"]["id"],"summary":"ready","client_request_id":"review-card-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(retry["project"]["id"], project["project"]["id"]);
        let main_history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let review_message: Message = main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "review_card"
                            && block["project_id"] == project["project"]["id"]
                            && block["state"] == "pending"
                    })
                })
            })
            .cloned()
            .map(|message| serde_json::from_value(message).unwrap())
            .unwrap();
        assert!(review_message.seq > 0);
        let review_block = &review_message.blocks[0];
        let review_block = serde_json::to_value(review_block).unwrap();
        assert_eq!(review_block["type"], "review_card");
        assert_eq!(review_block["artifacts"][0]["title"], "报告");
        assert_eq!(
            review_block["artifacts"][0]["path_or_url"],
            "runs/report.md"
        );
        assert!(review_block["artifacts"][0]["artifact_id"]
            .as_str()
            .is_some());
        let events = backend.store.events_since(0).unwrap();
        let project_event = events
            .iter()
            .rev()
            .find(|event| event.event == "project.updated")
            .unwrap();
        assert_eq!(
            project_event
                .data
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["project"]
        );
        serde_json::from_value::<Project>(project_event.data["project"].clone()).unwrap();
        let message_event = events
            .iter()
            .rev()
            .find(|event| {
                event.event == "message.created" && event.data["message"]["id"] == review_message.id
            })
            .unwrap();
        serde_json::from_value::<Message>(message_event.data["message"].clone()).unwrap();
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","after_seq":review_message.seq - 1}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == review_message.id
                && message["seq"] == review_message.seq));
        backend
            .call(
                "project.request_changes",
                json!({"project_id":project["project"]["id"],"text":"请补充证据","client_request_id":"review-changes-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .call(
                "project.request_review",
                json!({"project_id":project["project"]["id"],"summary":"再次 ready","client_request_id":"review-card-2"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let review_again = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let review_again_card = review_again["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == review_message.id)
            .unwrap();
        assert_eq!(review_again_card["seq"], review_message.seq);
        assert_eq!(review_again_card["blocks"][0]["state"], "pending");
        let done = backend
            .call(
                "project.confirm_done",
                json!({"project_id":project["project"]["id"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(done["project"]["status"], "done");
        let confirmed = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let confirmed_card = confirmed["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == review_message.id)
            .unwrap();
        assert_eq!(confirmed_card["seq"], review_message.seq);
        assert_eq!(confirmed_card["blocks"][0]["state"], "confirmed");
        let project_history = backend
            .call(
                "chat.history",
                json!({"chat_id":project_chat_id,"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let completion = project_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "completion"
                            && block["summary"]
                                .as_str()
                                .is_some_and(|text| !text.is_empty())
                    })
                })
            })
            .unwrap();
        assert_eq!(completion["chat_id"], project_chat_id);
        let _: Message = serde_json::from_value(completion.clone()).unwrap();
        let denied = backend
            .call(
                "project.confirm_done",
                json!({"project_id":project["project"]["id"]}),
                &gateway.state,
            )
            .await;
        assert!(denied.is_err());
        let proposal = backend
            .call(
                "propose_bot",
                json!({"name":"审阅者","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        serde_json::from_value::<Question>(proposal["question"].clone()).unwrap();
        assert_eq!(proposal["proposal"]["state"], "pending_user");
        assert_eq!(assignment["bot_id"], bot["bot"]["id"]);
        let proposal_event = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| {
                event.event == "question.asked"
                    && event.data["question"]["id"] == proposal["question"]["id"]
            })
            .unwrap();
        assert_eq!(
            proposal_event
                .data
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["question"]
        );
        let proposal_assignment_id = proposal["question"]["assignment_id"].as_str().unwrap();
        backend
            .call(
                "question.answer",
                json!({"question_id":proposal["question"]["id"],"option_index":1}),
                &gateway.state,
            )
            .await
            .unwrap();
        let proposal_assignment = backend
            .call(
                "assignment.get",
                json!({"assignment_id":proposal_assignment_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(proposal_assignment["assignment"]["status"], "cancelled");
    }
    #[tokio::test]
    async fn user_chat_steer_preserves_canonical_message_and_independent_delivery() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let first = backend
            .call("bot.create", json!({"name":"Coder one"}), &gateway.state)
            .await
            .unwrap();
        let second = backend
            .call("bot.create", json!({"name":"Coder two"}), &gateway.state)
            .await
            .unwrap();
        let project = backend.call("project.create", json!({"name":"Steer regression", "goal":"delivery", "member_bot_ids":[first["bot"]["id"],second["bot"]["id"]]}), &gateway.state).await.unwrap();
        for bot in [&first, &second] {
            backend.call("assignment.create", json!({"bot_id":bot["bot"]["id"],"project_id":project["project"]["id"],"origin_chat_id":project["chat"]["id"],"title":"working", "instruction":"keep working", "from":"main"}), &gateway.state).await.unwrap();
        }
        let sent = backend.call("chat.send", json!({"chat_id":project["chat"]["id"],"text":"/delivery-check 邮箱登录优先", "mentions":[{"kind":"bot","bot_id":first["bot"]["id"],"instruction":null},{"kind":"bot","bot_id":second["bot"]["id"],"instruction":null}],"client_request_id":"steer-users"}), &gateway.state).await.unwrap();
        let canonical = sent["message"].clone();
        assert_eq!(canonical["delivery"].as_array().unwrap().len(), 2);
        assert!(canonical["delivery"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["state"] == "queued"));
        for delivery in canonical["delivery"].as_array().unwrap() {
            backend
                .record_loaded_skill(
                    &gateway.state,
                    project["chat"]["id"].as_str().unwrap(),
                    canonical["id"].as_str().unwrap(),
                    json!({
                        "name":"delivery-check",
                        "bot_id":delivery["bot_id"],
                        "assignment_id":delivery["assignment_id"],
                        "loaded_at":"2026-10-12T01:02:03.000Z"
                    }),
                )
                .await
                .unwrap();
        }
        for delivery in canonical["delivery"].as_array().unwrap() {
            let mut transition = delivery.clone();
            for status in ["delivered", "read", "delivered"] {
                transition["state"] = json!(status);
                backend
                    .execution_update_steer_delivery(
                        &gateway.state,
                        canonical["id"].as_str().unwrap(),
                        &transition,
                    )
                    .await
                    .unwrap();
            }
        }
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":project["chat"]["id"],"after_seq":0}),
                &gateway.state,
            )
            .await
            .unwrap();
        let current = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == canonical["id"])
            .unwrap();
        for field in [
            "id",
            "seq",
            "created_at",
            "mentions",
            "reply_to",
            "blocks",
            "fallback_text",
        ] {
            assert_eq!(current[field], canonical[field], "{field}");
        }
        assert!(current["delivery"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["state"] == "read"));
        assert_eq!(current["loaded_skills"].as_array().unwrap().len(), 2);
        assert_eq!(current["loaded_skills"][0]["name"], "delivery-check");
        assert_eq!(current["loaded_skills"][1]["name"], "delivery-check");
        let snapshots = backend.orchestrator.snapshot().unwrap();
        assert_eq!(
            snapshots["messages"][canonical["id"].as_str().unwrap()]["delivery"],
            current["delivery"]
        );
        for event in backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            .filter(|e| e.event == "message.updated" && e.data["message"]["id"] == canonical["id"])
        {
            assert_eq!(event.data["message"]["seq"], canonical["seq"]);
            macbot_protocol::EventData::decode(
                &macbot_protocol::EventName::MessageUpdated,
                event.data,
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn pending_bot_proposal_question_has_a_durable_card_and_legacy_backfill() {
        async fn question_card_count(
            backend: &ProductionBackend,
            state: &GatewayState,
            question_id: &str,
        ) -> usize {
            let message_id = format!("msg_question_proposal_{}", takeover_component(question_id));
            backend
                .call(
                    "chat.history",
                    json!({"chat_id":"chat_main","limit":100}),
                    state,
                )
                .await
                .unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| {
                    message["id"] == message_id
                        && message["blocks"].as_array().is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block["type"] == "question" && block["question_id"] == question_id
                            })
                        })
                })
                .count()
        }

        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let proposal = backend
            .call(
                "propose_bot",
                json!({"name":"Proposal worker","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let question = proposal["question"].clone();
        let question_id = question["id"].as_str().unwrap();
        let message_id = format!("msg_question_proposal_{}", takeover_component(question_id));

        assert_eq!(
            question_card_count(&backend, &gateway.state, question_id).await,
            1
        );
        assert!(backend
            .ensure_proposal_question_message(&question)
            .unwrap()
            .is_none());
        assert_eq!(
            question_card_count(&backend, &gateway.state, question_id).await,
            1
        );

        // Simulate the durable files left by the old implementation: the
        // proposal and pending Question exist, while no Question message or
        // message.created event was ever written.
        let messages_path = home.path().join("data/chats/chat_main/messages.jsonl");
        let messages = std::fs::read_to_string(&messages_path).unwrap();
        let retained_messages = messages
            .lines()
            .filter(|line| {
                serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|message| {
                        message.get("id").and_then(Value::as_str).map(str::to_owned)
                    })
                    .as_deref()
                    != Some(message_id.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&messages_path, format!("{retained_messages}\n")).unwrap();

        let events_path = home.path().join("data/events/events.jsonl");
        let events = std::fs::read_to_string(&events_path).unwrap();
        let retained_events = events
            .lines()
            .filter(|line| {
                serde_json::from_str::<Value>(line)
                    .ok()
                    .is_none_or(|event| {
                        event["event"] != "message.created"
                            || event.pointer("/data/message/id").and_then(Value::as_str)
                                != Some(message_id.as_str())
                    })
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&events_path, format!("{retained_events}\n")).unwrap();
        drop(backend);

        let restored = ProductionBackend::open(home.path()).unwrap();
        assert_eq!(
            question_card_count(&restored, &gateway.state, question_id).await,
            1
        );
        assert_eq!(
            restored
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| event.event == "message.created"
                    && event.data["message"]["id"] == message_id)
                .count(),
            1
        );

        let answered = restored
            .call(
                "question.answer",
                json!({"question_id":question_id,"option_index":1}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(answered["question"]["state"], "answered");
        let listed = restored
            .call(
                "question.list",
                json!({"chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(listed["questions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == question_id && item["state"] == "answered"));
        assert_eq!(
            question_card_count(&restored, &gateway.state, question_id).await,
            1
        );
        assert_eq!(
            restored
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| event.event == "question.answered"
                    && event.data["question"]["id"] == question_id)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn proposal_answers_are_targeted_idempotent_and_preflight_duplicate_names() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let first = backend
            .call(
                "propose_bot",
                json!({"name":"Shared proposal name","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let second = backend
            .call(
                "propose_bot",
                json!({"name":"Shared proposal name","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let first_id = first["question"]["id"].as_str().unwrap().to_owned();
        let second_id = second["question"]["id"].as_str().unwrap().to_owned();
        let first_assignment = first["question"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second_assignment = second["question"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(first_id, second_id);
        assert_ne!(first_assignment, second_assignment);

        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        for question_id in [&first_id, &second_id] {
            let cards = history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| {
                    message["blocks"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block["type"] == "question"
                                && block["question_id"] == question_id.as_str()
                        })
                    })
                })
                .count();
            assert_eq!(cards, 1, "each proposal card must target only its Question");
        }

        let approved = backend
            .call(
                "question.answer",
                json!({"question_id":second_id,"option_index":0,"client_request_id":"proposal-answer-second"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(approved["question"]["id"], second["question"]["id"]);
        assert_eq!(approved["question"]["state"], "answered");
        assert_eq!(
            backend.read_proposal(&second_id).unwrap().unwrap()["state"],
            "approved"
        );
        assert_eq!(
            backend
                .call(
                    "assignment.get",
                    json!({"assignment_id":second_assignment}),
                    &gateway.state,
                )
                .await
                .unwrap()["assignment"]["status"],
            "done"
        );
        let bots_after_approval = backend
            .call("bot.list", json!({}), &gateway.state)
            .await
            .unwrap()["bots"]
            .as_array()
            .unwrap()
            .len();

        // Replaying the same answer must reuse the canonical Question and
        // complete no Bot or proposal action a second time.
        let replay = backend
            .call(
                "question.answer",
                json!({"question_id":second_id,"option_indexes":[0],"client_request_id":"proposal-answer-second-retry"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(replay["question"], approved["question"]);
        assert_eq!(
            backend
                .call("bot.list", json!({}), &gateway.state)
                .await
                .unwrap()["bots"]
                .as_array()
                .unwrap()
                .len(),
            bots_after_approval
        );
        assert_eq!(
            backend
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| {
                    event.event == "question.answered" && event.data["question"]["id"] == second_id
                })
                .count(),
            1
        );

        // Approving the other pending Question with the now occupied name is
        // rejected before question.answer mutates its Question or assignment.
        let before = backend.orchestrator.snapshot().unwrap();
        let duplicate = backend
            .call(
                "question.answer",
                json!({"question_id":first_id,"option_index":0}),
                &gateway.state,
            )
            .await
            .unwrap_err();
        assert_eq!(duplicate.code, "conflict");
        let after = backend.orchestrator.snapshot().unwrap();
        assert_eq!(
            after["questions"][&first_id],
            before["questions"][&first_id]
        );
        assert_eq!(after["questions"][&second_id]["state"], "answered");
        assert_eq!(
            backend.read_proposal(&first_id).unwrap().unwrap()["state"],
            "pending"
        );
        assert_eq!(
            backend.read_proposal(&second_id).unwrap().unwrap()["state"],
            "approved"
        );
        assert_eq!(
            after["assignments"][&first_assignment],
            before["assignments"][&first_assignment]
        );
        assert_eq!(
            backend
                .call("bot.list", json!({}), &gateway.state)
                .await
                .unwrap()["bots"]
                .as_array()
                .unwrap()
                .len(),
            bots_after_approval
        );

        // The user can still explicitly reject that exact pending proposal;
        // only its own question and assignment are finalized.
        backend
            .call(
                "question.answer",
                json!({"question_id":first_id,"option_index":1}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            backend.read_proposal(&first_id).unwrap().unwrap()["state"],
            "rejected"
        );
        assert_eq!(
            backend
                .call(
                    "assignment.get",
                    json!({"assignment_id":first_assignment}),
                    &gateway.state,
                )
                .await
                .unwrap()["assignment"]["status"],
            "cancelled"
        );
        assert_eq!(
            backend.read_proposal(&second_id).unwrap().unwrap()["state"],
            "approved"
        );
    }

    #[tokio::test]
    async fn startup_reconciles_answered_proposal_left_pending_after_name_conflict() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let stale = backend
            .call(
                "propose_bot",
                json!({"name":"Existing worker","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let winner = backend
            .call(
                "propose_bot",
                json!({"name":"Existing worker","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let stale_id = stale["question"]["id"].as_str().unwrap().to_owned();
        let stale_assignment = stale["question"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let winner_id = winner["question"]["id"].as_str().unwrap().to_owned();
        backend
            .call(
                "question.answer",
                json!({"question_id":winner_id,"option_index":0}),
                &gateway.state,
            )
            .await
            .unwrap();

        // Recreate the inconsistent durable state left by the old server:
        // an approval answer was snapshotted, but its same-name Bot creation
        // failed before the proposal record could leave `pending`.
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["questions"][&stale_id]["state"] = json!("answered");
        snapshot["questions"][&stale_id]["answer"] = json!({
            "option_index":0,
            "option_indexes":[0],
            "text":null,
            "at":"2026-10-13T14:41:28.607Z"
        });
        snapshot["assignments"][&stale_assignment]["status"] = json!("done");
        snapshot["assignments"][&stale_assignment]["wait"] = Value::Null;
        backend.orchestrator.restore(snapshot).unwrap();
        backend
            .persist_orchestrator(json!({
                "method":"test.install_interrupted_proposal_state",
                "params":{},
                "result":{},
                "status":"done"
            }))
            .await
            .unwrap();
        assert_eq!(
            backend.read_proposal(&stale_id).unwrap().unwrap()["state"],
            "pending"
        );
        drop(backend);

        let reopened = ProductionBackend::open(home.path()).unwrap();
        let repaired = reopened.read_proposal(&stale_id).unwrap().unwrap();
        assert_eq!(
            repaired["state"],
            "failed",
            "question state after reopen: {}",
            reopened.orchestrator.snapshot().unwrap()["questions"][&stale_id]["state"]
        );
        assert_eq!(repaired["failure_code"], "bot_name_conflict");
        assert_eq!(
            reopened.read_proposal(&winner_id).unwrap().unwrap()["state"],
            "approved"
        );
        let repaired_state = reopened.orchestrator.snapshot().unwrap();
        assert_eq!(repaired_state["questions"][&stale_id]["state"], "answered");
        assert_eq!(
            repaired_state["assignments"][&stale_assignment]["status"],
            "done"
        );
        assert_eq!(
            reopened
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| {
                    event.event == "question.answered" && event.data["question"]["id"] == stale_id
                })
                .count(),
            1
        );
        assert_eq!(
            reopened
                .call("bot.list", json!({}), &gateway.state)
                .await
                .unwrap()["bots"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn retrying_an_already_answered_stale_proposal_finalizes_only_that_proposal() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let stale = backend
            .call(
                "propose_bot",
                json!({"name":"Existing worker","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let winner = backend
            .call(
                "propose_bot",
                json!({"name":"Existing worker","chat_id":"chat_main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let stale_id = stale["question"]["id"].as_str().unwrap().to_owned();
        let stale_assignment = stale["question"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let winner_id = winner["question"]["id"].as_str().unwrap().to_owned();
        backend
            .call(
                "question.answer",
                json!({"question_id":winner_id,"option_index":0}),
                &gateway.state,
            )
            .await
            .unwrap();

        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["questions"][&stale_id]["state"] = json!("answered");
        snapshot["questions"][&stale_id]["answer"] = json!({
            "option_index":0,
            "option_indexes":[0],
            "text":null,
            "at":"2026-10-13T14:41:28.607Z"
        });
        backend.orchestrator.restore(snapshot).unwrap();

        let answer = json!({
            "question_id":stale_id,
            "option_index":0,
            "client_request_id":"stale-proposal-answer"
        });
        assert_eq!(
            backend
                .call("question.answer", answer.clone(), &gateway.state)
                .await
                .unwrap_err()
                .code,
            "conflict"
        );
        assert_eq!(
            backend.read_proposal(&stale_id).unwrap().unwrap()["state"],
            "failed"
        );
        assert_eq!(
            backend.read_proposal(&winner_id).unwrap().unwrap()["state"],
            "approved"
        );
        assert_eq!(
            backend
                .call(
                    "assignment.get",
                    json!({"assignment_id":stale_assignment}),
                    &gateway.state,
                )
                .await
                .unwrap()["assignment"]["status"],
            "failed"
        );
        assert!(backend
            .call("question.answer", answer, &gateway.state)
            .await
            .is_err());
        assert_eq!(
            backend
                .store
                .events_since(0)
                .unwrap()
                .iter()
                .filter(|event| {
                    event.event == "question.answered" && event.data["question"]["id"] == stale_id
                })
                .count(),
            1
        );
        assert_eq!(
            backend
                .call("bot.list", json!({}), &gateway.state)
                .await
                .unwrap()["bots"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn queued_message_can_be_steered_after_chat_is_marked_read() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"Queued worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"Read cursor steer","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "bot_id":bot["bot"]["id"],
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["chat"]["id"],
                    "title":"Working task",
                    "instruction":"Keep working",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap().to_owned();
        let queued = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":project["chat"]["id"],
                    "text":"请把这条消息作为插话处理",
                    "mentions":[],
                    "client_request_id":"read-cursor-queued-send"
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        assert_eq!(queued["delivery"][0]["state"], "queued");

        backend
            .call(
                "chat.mark_read",
                json!({"chat_id":project["chat"]["id"],"seq":queued["seq"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let promoted = backend
            .call(
                "message.steer",
                json!({
                    "message_id":queued["id"],
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id,
                    "client_request_id":"read-cursor-steer"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(promoted["message"]["id"], queued["id"]);
        assert_eq!(promoted["message"]["delivery"][0]["state"], "queued");
        assert_eq!(
            promoted["assignment"]["steers"][0]["message_id"],
            queued["id"]
        );

        backend
            .execution_update_steer_delivery(
                &gateway.state,
                queued["id"].as_str().unwrap(),
                &json!({
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id,
                    "state":"read"
                }),
            )
            .await
            .unwrap();
        let replay = backend
            .call(
                "message.steer",
                json!({
                    "message_id":queued["id"],
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(replay["message"]["delivery"][0]["state"], "read");
        assert_eq!(replay["assignment"]["steers"].as_array().unwrap().len(), 1);

        // A different queued message whose assignment delivery was already
        // consumed must still be rejected, regardless of the chat read cursor.
        let consumed = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":project["chat"]["id"],
                    "text":"这条已被消费",
                    "mentions":[],
                    "client_request_id":"read-cursor-consumed-send"
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["messages"][consumed["id"].as_str().unwrap()]["delivery"][0]["state"] =
            json!("read");
        backend.orchestrator.restore(snapshot).unwrap();
        let error = backend
            .call(
                "message.steer",
                json!({
                    "message_id":consumed["id"],
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id,
                    "client_request_id":"read-cursor-consumed-steer"
                }),
                &gateway.state,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert_eq!(
            error.message,
            "conflict: message is not waiting for this Bot"
        );

        let deleted = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":project["chat"]["id"],
                    "text":"这条消息被删除",
                    "mentions":[],
                    "client_request_id":"read-cursor-deleted-send"
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        backend
            .call(
                "message.delete",
                json!({"message_id":deleted["id"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let error = backend
            .call(
                "message.steer",
                json!({
                    "message_id":deleted["id"],
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id
                }),
                &gateway.state,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");

        backend
            .orchestrator
            .finish_assignment(&assignment_id, "done")
            .unwrap();
        let completed_retry = backend
            .call(
                "message.steer",
                json!({
                    "message_id":queued["id"],
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(completed_retry["assignment"]["status"], "done");
        assert_eq!(completed_retry["message"]["delivery"][0]["state"], "read");

        let mut cancelled_snapshot = backend.orchestrator.snapshot().unwrap();
        cancelled_snapshot["assignments"][&assignment_id]["status"] = json!("cancelled");
        backend.orchestrator.restore(cancelled_snapshot).unwrap();
        let cancelled_retry = backend
            .call(
                "message.steer",
                json!({
                    "message_id":queued["id"],
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment_id
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(cancelled_retry["assignment"]["status"], "cancelled");
        assert_eq!(cancelled_retry["message"]["delivery"][0]["state"], "read");
    }

    #[tokio::test]
    async fn startup_restores_deleted_queue_tombstone_from_legacy_chat_log() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"Legacy tombstone worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"Legacy tombstone","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "bot_id":bot["bot"]["id"],
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["chat"]["id"],
                    "title":"Working task",
                    "instruction":"Keep working",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let queued = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":project["chat"]["id"],
                    "text":"deleted before upgrade",
                    "mentions":[],
                    "client_request_id":"legacy-tombstone-queued"
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        backend
            .call(
                "message.delete",
                json!({"message_id":queued["id"]}),
                &gateway.state,
            )
            .await
            .unwrap();

        // Emulate a pre-migration orchestrator snapshot: it has the redacted
        // message but no `deleted` bit, while the chat log retains its tombstone.
        let message_id = queued["id"].as_str().unwrap();
        let mut legacy_snapshot = backend.orchestrator.snapshot().unwrap();
        legacy_snapshot["messages"][message_id]
            .as_object_mut()
            .unwrap()
            .remove("deleted");
        let mut unrelated_bot_message = legacy_snapshot["messages"][message_id].clone();
        unrelated_bot_message["id"] = json!("msg-unrelated-bot");
        unrelated_bot_message["chat_id"] = project["chat"]["id"].clone();
        unrelated_bot_message["sender"] = json!("bot");
        unrelated_bot_message["deleted"] = json!(false);
        unrelated_bot_message["delivery"] = json!([]);
        unrelated_bot_message["assignment_id"] = Value::Null;
        legacy_snapshot["messages"]["msg-unrelated-bot"] = unrelated_bot_message;
        let mut unrelated_user_message = legacy_snapshot["messages"][message_id].clone();
        unrelated_user_message["id"] = json!("msg-unrelated-user");
        unrelated_user_message["chat_id"] = json!("chat_unrelated");
        unrelated_user_message["deleted"] = json!(false);
        unrelated_user_message["delivery"] = json!([]);
        unrelated_user_message["assignment_id"] = Value::Null;
        legacy_snapshot["messages"]["msg-unrelated-user"] = unrelated_user_message;
        backend
            .store
            .write_snapshot("data/orchestrator/state.json", &legacy_snapshot)
            .unwrap();
        let unrelated_chat_log = home.path().join("data/chats/chat_unrelated/messages.jsonl");
        fs::create_dir_all(unrelated_chat_log.parent().unwrap()).unwrap();
        let mut chat_log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(unrelated_chat_log)
            .unwrap();
        use std::io::Write as _;
        writeln!(
            chat_log_file,
            "{}",
            json!({
                "sender":{"kind":"user","user_id":"user"},
                "deleted":true
            })
        )
        .unwrap();
        writeln!(
            chat_log_file,
            "{}",
            json!({
                "id":"msg-unrelated-bot",
                "chat_id":project["chat"]["id"],
                "sender":{"kind":"user","user_id":"user"},
                "deleted":true
            })
        )
        .unwrap();
        writeln!(
            chat_log_file,
            "{}",
            json!({
                "id":"msg-unrelated-user",
                "chat_id":project["chat"]["id"],
                "sender":{"kind":"user","user_id":"user"},
                "deleted":true
            })
        )
        .unwrap();
        chat_log_file.flush().unwrap();
        fs::remove_file(home.path().join("data/orchestrator/operations.jsonl")).unwrap();
        drop(backend);

        let reopened = ProductionBackend::open(home.path()).unwrap();
        assert_eq!(
            reopened.orchestrator.snapshot().unwrap()["messages"][message_id]["deleted"],
            true
        );
        assert_eq!(
            reopened.orchestrator.snapshot().unwrap()["messages"]["msg-unrelated-bot"]["deleted"],
            false,
            "a tombstone with a mismatched chat and non-user snapshot row must be ignored"
        );
        assert_eq!(
            reopened.orchestrator.snapshot().unwrap()["messages"]["msg-unrelated-user"]["deleted"],
            false,
            "a tombstone must match both chat and message IDs"
        );
        assert_eq!(
            reopened.orchestrator.snapshot().unwrap()["assignments"]
                [assignment["id"].as_str().unwrap()]["pending_messages"],
            0,
            "restoring a legacy tombstone must exclude it from pending count"
        );
        let error = reopened
            .call(
                "message.steer",
                json!({
                    "message_id":message_id,
                    "bot_id":bot["bot"]["id"],
                    "assignment_id":assignment["id"]
                }),
                &gateway.state,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
    }

    #[test]
    fn startup_fails_when_legacy_chat_log_has_io_error_during_tombstone_migration() {
        let home = tempdir().unwrap();
        let unreadable_log = home
            .path()
            .join("data/chats/chat_with_io_error/messages.jsonl");
        // A directory at the JSONL path deterministically causes open or line
        // reading to fail with an I/O error, including with elevated
        // filesystem permissions.
        fs::create_dir_all(&unreadable_log).unwrap();

        let error = match ProductionBackend::open(home.path()) {
            Ok(_) => panic!("startup unexpectedly accepted an unreadable chat log"),
            Err(error) => error,
        };
        assert!(
            matches!(error, AdapterError::Store(macbot_store::StoreError::Io(_))),
            "expected startup to fail closed on a chat-log I/O error, got {error:?}"
        );
    }

    #[tokio::test]
    async fn send_msg_done_reviews_queued_messages_before_creating_delivery() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"Review worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({"name":"Review queue","member_bot_ids":[bot["bot"]["id"]]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "bot_id":bot["bot"]["id"],
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["chat"]["id"],
                    "title":"Review queued messages",
                    "instruction":"finish after review",
                    "from":"main"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let queued = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":project["chat"]["id"],
                    "text":"请把交付改成适配手机屏幕",
                    "mentions":[]
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        assert_eq!(queued["delivery"][0]["state"], "queued");

        let deleted = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":project["chat"]["id"],
                    "text":"删除后不应进入 queue review",
                    "mentions":[]
                }),
                &gateway.state,
            )
            .await
            .unwrap()["message"]
            .clone();
        backend
            .call(
                "message.delete",
                json!({"message_id":deleted["id"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            backend.orchestrator.snapshot().unwrap()["assignments"]
                [assignment["id"].as_str().unwrap()]["pending_messages"],
            1,
            "deleting one of two queued messages decrements pending count"
        );

        let envelope = json!({
            "receipt":{"run_id":"queue-review-run","call_id":"queue-review-call"},
            "message":{
                "bot_id":bot["bot"]["id"],
                "chat_id":project["chat"]["id"],
                "assignment_id":assignment["id"],
                "message_id":"msg-should-not-be-created",
                "intent":"done",
                "text":"已完成"
            }
        });
        let review = backend
            .execution_send_msg(&gateway.state, envelope.clone())
            .await
            .unwrap();
        assert_eq!(review["_queue_review"]["message_ids"][0], queued["id"]);
        assert_eq!(
            review["_queue_review"]["message_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_ne!(review["_queue_review"]["message_ids"][0], deleted["id"]);
        assert_eq!(
            review["_queue_review"]["messages"][0]["delivery"][0]["state"],
            "read"
        );
        let retry = backend
            .execution_send_msg(&gateway.state, envelope)
            .await
            .unwrap();
        assert_eq!(retry, review);

        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":project["chat"]["id"],"after_seq":0}),
                &gateway.state,
            )
            .await
            .unwrap();
        let messages = history["messages"].as_array().unwrap();
        assert!(messages.iter().any(|message| message["id"] == queued["id"]));
        assert!(messages
            .iter()
            .all(|message| message["id"] != "msg-should-not-be-created"));
        let reviewed = messages
            .iter()
            .find(|message| message["id"] == queued["id"])
            .unwrap();
        assert_eq!(reviewed["delivery"][0]["state"], "read");
        assert_eq!(
            backend.orchestrator.snapshot().unwrap()["assignments"]
                [assignment["id"].as_str().unwrap()]["pending_messages"],
            0
        );
    }

    #[tokio::test]
    async fn production_chat_send_persists_reply_and_attachments_through_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        fs::create_dir_all(home.path().join("uploads")).unwrap();
        fs::create_dir_all(home.path().join("data/uploads")).unwrap();
        fs::write(home.path().join("uploads/upload-chat-1"), b"attachment").unwrap();
        fs::write(
            home.path().join("data/uploads/upload-chat-1.json"),
            json!({"file":{"root":"upload","root_id":"upload-chat-1","path":"","name":"note.txt","size":10,"mime":"text/plain"}}).to_string(),
        )
        .unwrap();
        let created = backend
            .call("bot.create", json!({"name":"附件回归 Bot"}), &gateway.state)
            .await
            .unwrap();
        let chat_id = created["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let root = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":chat_id,
                    "text":"被引用的原消息",
                    "mentions":[],
                    "client_request_id":"chat-attachment-root"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let root_id = root["message"]["id"].as_str().unwrap().to_owned();
        let sent = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":chat_id,
                    "text":"带附件的回复",
                    "mentions":[],
                    "quote":root_id,
                    "attachments":["upload-chat-1"],
                    "client_request_id":"chat-attachment-1"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(sent["message"]["quote"]["message_id"], root_id);
        assert_eq!(sent["message"]["reply_to"], Value::Null);
        assert_eq!(sent["message"]["blocks"][1]["type"], "file");
        assert_eq!(
            sent["message"]["blocks"][1]["file"]["root_id"],
            "upload-chat-1"
        );
        assert_eq!(sent["message"]["blocks"][1]["file"]["name"], "note.txt");

        let attachment_only = backend
            .call(
                "chat.send",
                json!({
                    "chat_id":chat_id,
                    "text":"",
                    "mentions":[],
                    "attachments":["upload-chat-1"],
                    "client_request_id":"chat-attachment-only"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(attachment_only["message"]["fallback_text"], "note.txt");
        assert_eq!(
            attachment_only["message"]["blocks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(attachment_only["message"]["blocks"][0]["type"], "file");

        let history = backend
            .call("chat.history", json!({"chat_id":chat_id}), &gateway.state)
            .await
            .unwrap();
        let message = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == sent["message"]["id"])
            .unwrap();
        assert_eq!(message["quote"]["message_id"], root_id);
        assert_eq!(message["reply_to"], Value::Null);
        assert_eq!(message["blocks"][1]["file"]["root_id"], "upload-chat-1");
        let attachment_only_history = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == attachment_only["message"]["id"])
            .unwrap();
        assert_eq!(attachment_only_history["fallback_text"], "note.txt");
        assert_eq!(
            attachment_only_history["blocks"].as_array().unwrap().len(),
            1
        );
        assert_eq!(attachment_only_history["blocks"][0]["type"], "file");
        drop(backend);
        drop(gateway);

        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let history = backend
            .call("chat.history", json!({"chat_id":chat_id}), &gateway.state)
            .await
            .unwrap();
        let message = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == sent["message"]["id"])
            .unwrap();
        assert_eq!(message["quote"]["message_id"], root_id);
        assert_eq!(message["reply_to"], Value::Null);
        assert_eq!(message["blocks"][1]["file"]["root_id"], "upload-chat-1");
        let attachment_only_history = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == attachment_only["message"]["id"])
            .unwrap();
        assert_eq!(attachment_only_history["fallback_text"], "note.txt");
        assert_eq!(
            attachment_only_history["blocks"].as_array().unwrap().len(),
            1
        );
        assert_eq!(attachment_only_history["blocks"][0]["type"], "file");
    }

    #[tokio::test]
    async fn production_chat_thread_reactions_read_flags_and_cursor_survive_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let created = backend
            .call(
                "bot.create",
                json!({"name":"聊天方法回归 Bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat_id = created["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let root = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"根消息","mentions":[],"client_request_id":"chat-root"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let root_id = root["message"]["id"].as_str().unwrap().to_owned();
        let reply = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"回复","mentions":[],"quote":root_id,"client_request_id":"chat-reply"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let reply_id = reply["message"]["id"].as_str().unwrap().to_owned();
        assert_eq!(reply["message"]["quote"]["message_id"], root_id);
        assert_eq!(reply["message"]["reply_to"], Value::Null);
        let history = backend
            .call("chat.history", json!({"chat_id":chat_id}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(history["messages"].as_array().unwrap().len(), 2);
        assert_eq!(history["messages"][0]["id"], root_id);
        assert_eq!(history["messages"][1]["id"], reply_id);
        assert_eq!(history["messages"][1]["quote"]["message_id"], root_id);

        let reacted = backend
            .call(
                "chat.react",
                json!({"message_id":root_id,"emoji":"👍","on":true,"client_request_id":"react-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(reacted["message"]["id"], root_id);
        assert_eq!(reacted["message"]["reactions"][0]["emoji"], "👍");
        backend
            .call(
                "chat.mark_read",
                json!({"chat_id":chat_id,"seq":2,"client_request_id":"read-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let pinned = backend
            .call(
                "chat.set_pinned",
                json!({"chat_id":chat_id,"pinned":true,"client_request_id":"pin-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(pinned["chat"]["pinned"], true);
        let muted = backend
            .call(
                "chat.set_muted",
                json!({"chat_id":chat_id,"muted":true,"client_request_id":"mute-1"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(muted["chat"]["muted"], true);
        let before = backend
            .call(
                "chat.history",
                json!({"chat_id":chat_id,"before_seq":2}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(before["messages"].as_array().unwrap().len(), 1);
        assert_eq!(before["messages"][0]["id"], root_id);
        fs::create_dir_all(home.path().join("data/chats/bot_dm_read_only")).unwrap();
        fs::write(
            home.path()
                .join("data/chats/bot_dm_read_only/metadata.json"),
            json!({"kind":"bot_dm"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            backend
                .call(
                    "chat.send",
                    json!({"chat_id":"bot_dm_read_only","text":"拒绝","mentions":[]}),
                    &gateway.state
                )
                .await
                .unwrap_err()
                .code,
            "forbidden"
        );
        // The prefix does not imply read-only, but this must be an actual
        // registered direct chat rather than arbitrary metadata for a new ID.
        let named_bot = backend
            .call(
                "bot.create",
                json!({"name":"Named direct chat"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["bots"][named_bot["bot"]["id"].as_str().unwrap()]["dm_chat_id"] =
            json!("bot_dm_named");
        backend.orchestrator.restore(snapshot).unwrap();
        fs::create_dir_all(home.path().join("data/chats/bot_dm_named")).unwrap();
        fs::write(
            home.path().join("data/chats/bot_dm_named/metadata.json"),
            json!({"kind":"direct"}).to_string(),
        )
        .unwrap();
        assert!(backend
            .call(
                "chat.send",
                json!({"chat_id":"bot_dm_named","text":"名称不代表类型","mentions":[]}),
                &gateway.state
            )
            .await
            .is_ok());

        drop(backend);
        drop(gateway);
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let chat = backend
            .call("chat.get", json!({"chat_id":chat_id}), &gateway.state)
            .await
            .unwrap();
        assert_eq!(chat["chat"]["last_read_seq"], 2);
        assert_eq!(chat["chat"]["pinned"], true);
        assert_eq!(chat["chat"]["muted"], true);
        let history = backend
            .call("chat.history", json!({"chat_id":chat_id}), &gateway.state)
            .await
            .unwrap();
        let restored_root = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == root_id)
            .unwrap();
        assert_eq!(restored_root["reactions"][0]["emoji"], "👍");
        let restored_reply = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == reply_id)
            .unwrap();
        assert_eq!(restored_reply["quote"]["message_id"], root_id);
        assert_eq!(restored_reply["reply_to"], Value::Null);
    }

    #[tokio::test]
    async fn completed_operations_repair_missing_events_after_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"事件恢复 Bot","client_request_id":"repair-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"事件恢复项目",
                    "goal":"恢复缺失事件",
                    "member_bot_ids":[bot_id],
                    "client_request_id":"repair-project"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":project["project"]["chat_id"],
                    "bot_id":bot_id,
                    "title":"恢复任务",
                    "instruction":"恢复",
                    "from":"main",
                    "client_request_id":"repair-assignment"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .call(
                "send_msg",
                json!({
                    "bot_id":bot_id,
                    "chat_id":project["project"]["chat_id"],
                    "assignment_id":assignment["id"],
                    "text":"恢复消息",
                    "intent":"ack",
                    "client_request_id":"repair-message"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        drop(backend);
        drop(gateway);
        fs::write(home.path().join("data/events/events.jsonl"), b"").unwrap();

        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let events = backend.store.events_since(0).unwrap();
        assert!(events
            .iter()
            .any(|event| { event.event == "bot.created" && event.data["bot"]["id"] == bot_id }));
        assert!(events.iter().any(|event| {
            event.event == "project.created" && event.data["project"]["id"] == project_id
        }));
        assert!(events.iter().any(|event| {
            event.event == "assignment.created"
                && event.data["assignment"]["id"] == assignment["id"]
        }));
        assert!(events.iter().any(|event| {
            matches!(event.event.as_str(), "message.created" | "message.updated")
                && event.data["message"]["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("msg_task_card_"))
        }));
        let repaired_count = events.len();
        drop(backend);
        drop(gateway);
        let _backend = ProductionBackend::open(home.path()).unwrap();
        assert_eq!(
            _backend.store.events_since(0).unwrap().len(),
            repaired_count
        );
    }

    #[test]
    fn repaired_events_deduplicate_new_keys_and_legacy_payloads() {
        let home = tempdir().unwrap();
        let backend = ProductionBackend::open(home.path()).unwrap();
        let fresh = json!({"message":{"id":"fresh"}});
        assert!(backend
            .append_repaired_event("repair:fresh", "message.created", fresh.clone())
            .unwrap()
            .is_some());
        assert!(backend.store.has_event_key("repair:fresh"));
        assert!(backend
            .append_repaired_event("repair:fresh", "message.created", fresh)
            .unwrap()
            .is_none());

        let legacy = json!({"message":{"id":"legacy"}});
        backend
            .store
            .append_event("message.created", legacy.clone())
            .unwrap();
        let before = backend.store.events_since(0).unwrap().len();
        assert!(backend
            .append_repaired_event("repair:legacy", "message.created", legacy)
            .unwrap()
            .is_none());
        assert_eq!(backend.store.events_since(0).unwrap().len(), before);

        let cached = backend.store.events_since(0).unwrap();
        assert!(backend
            .append_repaired_event_with_events(
                "repair:legacy-cached",
                "message.created",
                json!({"message":{"id":"legacy"}}),
                &cached,
            )
            .unwrap()
            .is_none());
        assert_eq!(backend.store.events_since(0).unwrap().len(), before);
    }

    #[tokio::test]
    async fn deleted_entities_are_not_resurrected_by_startup_repair() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let created = backend
            .call(
                "bot.create",
                json!({"name":"待删除","client_request_id":"delete-repair-create"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = created["bot"]["id"].as_str().unwrap().to_owned();
        backend
            .call(
                "bot.delete",
                json!({"bot_id":bot_id,"client_request_id":"delete-repair-delete"}),
                &gateway.state,
            )
            .await
            .unwrap();
        drop(backend);
        drop(gateway);
        fs::write(home.path().join("data/events/events.jsonl"), b"").unwrap();
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let events = restarted.store.events_since(0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "bot.created" && event.data["bot"]["id"] == bot_id)
                .count(),
            0
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "bot.deleted" && event.data["bot_id"] == bot_id)
                .count(),
            1
        );
        let bots = restarted
            .orchestrator
            .rpc("bot.list", json!({}))
            .await
            .unwrap();
        assert!(!bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bot| bot["id"] == bot_id));
    }

    #[tokio::test]
    async fn confirmed_review_retry_keeps_one_card_after_restart() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = ProductionBackend::open(home.path()).unwrap();
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"确认卡 Bot","client_request_id":"confirmed-card-bot"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"确认卡项目",
                    "goal":"确认",
                    "member_bot_ids":[bot["bot"]["id"]],
                    "client_request_id":"confirmed-card-project"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let project_chat_id = project["project"]["chat_id"].as_str().unwrap().to_owned();
        let review_params = json!({
            "project_id":project_id,
            "summary":"ready",
            "client_request_id":"confirmed-card-review"
        });
        backend
            .call(
                "project.request_review",
                review_params.clone(),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .call(
                "project.confirm_done",
                json!({"project_id":project_id}),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .call("project.request_review", review_params, &gateway.state)
            .await
            .unwrap();
        let history = backend
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let review_cards = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "review_card" && block["project_id"] == project_id
                    })
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(review_cards.len(), 1);
        assert_eq!(review_cards[0]["blocks"][0]["state"], "confirmed");
        let project_history = backend
            .call(
                "chat.history",
                json!({"chat_id":project_chat_id,"limit":100}),
                &gateway.state,
            )
            .await
            .unwrap();
        let completion_count = project_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "completion"
                            && block["summary"]
                                .as_str()
                                .is_some_and(|text| !text.is_empty())
                    })
                })
            })
            .count();
        assert_eq!(completion_count, 1);
        drop(backend);
        drop(gateway);
        let restarted_gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let restarted = ProductionBackend::open(home.path()).unwrap();
        let history = restarted
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &restarted_gateway.state,
            )
            .await
            .unwrap();
        let restarted_project_history = restarted
            .call(
                "chat.history",
                json!({"chat_id":project_chat_id,"limit":100}),
                &restarted_gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(
            history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| {
                    message["blocks"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block["type"] == "review_card"
                                && block["project_id"] == project_id
                                && block["state"] == "confirmed"
                        })
                    })
                })
                .count(),
            1
        );
        assert_eq!(
            restarted_project_history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| {
                    message["blocks"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|block| block["type"] == "completion")
                    })
                })
                .count(),
            1
        );
    }
}
