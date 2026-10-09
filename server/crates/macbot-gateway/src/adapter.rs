//! Production RPC adapter.
//!
//! The gateway boundary is protocol JSON.  The orchestration crate keeps a
//! compact internal state model, so this module is the single place that
//! expands it to the complete `macbot-protocol` wire objects and validates the
//! result before it is exposed to a client.  Mutations are write-ahead:
//! operation JSONL is synced, then the global event log is synced, then the
//! event is published to connected clients.

use crate::{features::FeatureService, now, GatewayState, RpcBackend, RpcError, RpcResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use macbot_browser::BrowserError;
use macbot_durable::DurableRuntime;
use macbot_orchestrator::{Orchestrator, UsageTotals};
use macbot_protocol::{
    Announcement, Approval, Assignment, Bot, BotDuplicateResult, Chat, Device, HeatmapResult,
    Hello, Message, PendingItems, Project, Question, Routine, Settings, UsageBreakdownResult,
    UsageSummaryResult, UsageTimeseriesResult,
};
use macbot_providers::registry::{ProviderRegistry, RegistryError};
use macbot_skills::BotSkillSettingsSnapshot;
use macbot_store::Store;
use macbot_usage::UsageLedger;
use serde_json::{json, Map, Value};
use std::{collections::HashMap, fs, path::Path, sync::Arc};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

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

#[derive(Clone)]
pub struct ProductionBackend {
    pub orchestrator: Orchestrator,
    pub store: Store,
    pub durable: Arc<Mutex<DurableRuntime>>,
    pub usage: Arc<Mutex<UsageLedger>>,
    pub providers: Arc<Mutex<ProviderRegistry>>,
    write_lock: Arc<Mutex<()>>,
    persist_lock: Arc<Mutex<()>>,
    idempotency: Arc<Mutex<HashMap<String, Value>>>,
}

impl ProductionBackend {
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
        let operations = store.read_jsonl::<Value>("data/orchestrator/operations.jsonl")?;
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
        if let Some(snapshot) = snapshot {
            orchestrator
                .restore(snapshot)
                .map_err(|error| AdapterError::OrchestratorSnapshot(error.to_string()))?;
        }
        Self::migrate_legacy_chat_sequences(&store, &orchestrator)?;
        let mut idempotency = HashMap::new();
        for op in operations {
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
        Ok(Self {
            orchestrator,
            store,
            durable: Arc::new(Mutex::new(durable)),
            usage: Arc::new(Mutex::new(usage)),
            providers: Arc::new(Mutex::new(registry)),
            write_lock: Arc::new(Mutex::new(())),
            persist_lock: Arc::new(Mutex::new(())),
            idempotency: Arc::new(Mutex::new(idempotency)),
        })
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
                let messages =
                    store.read_jsonl::<Value>(path.strip_prefix(store.root()).unwrap_or(&path))?;
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

    /// Internal runtime action.  The returned snapshot is intentionally kept
    /// out of the public RPC response; the composed runtime backend uses it
    /// only to resume the waiting run after the user releases the browser.
    pub async fn execution_takeover_start(
        &self,
        state: &GatewayState,
        params: &Value,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
        let result = self.takeover_start(state, params).await?;
        self.persist(state, "takeover.start", params, &json!({}))
            .await?;
        Ok(result)
    }

    /// Internal runtime action.  See [`Self::execution_takeover_start`].
    pub async fn execution_takeover_release(
        &self,
        state: &GatewayState,
        params: &Value,
    ) -> RpcResult {
        let _guard = self.write_lock.lock().await;
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

    fn event_name(method: &str) -> Option<&'static str> {
        Some(match method {
            "bot.create" | "bot.duplicate" => "bot.created",
            "bot.update" => "bot.updated",
            "bot.delete" => "bot.deleted",
            "project.create" => "project.created",
            "project.update"
            | "project.add_member"
            | "project.remove_member"
            | "project.confirm_done"
            | "project.request_review"
            | "project.request_changes"
            | "project.archive"
            | "project.reopen" => "project.updated",
            "assignment.create" | "assign" | "delegate" => "assignment.created",
            "assignment.stop" | "assignment.steer" | "steer" => "assignment.updated",
            "send_msg" | "chat.send" => "message.created",
            "approval.request" => "approval.requested",
            "approval.decide" => "approval.resolved",
            "question.ask" => "question.asked",
            "propose_bot" => "question.asked",
            "question.answer" => "question.answered",
            "routine.create" | "routine.update" | "routine.set_enabled" => "routine.updated",
            "routine.delete" => "routine.deleted",
            "routine.test_run" => "routine.run",
            "routine.execution" => "routine.run",
            "chat.mark_read" => "read.updated",
            "loop.resolve" => "assignment.updated",
            "settings.update" => "settings.updated",
            _ => return None,
        })
    }

    /// Persist one orchestrator operation while serializing the fresh snapshot
    /// with every runtime writer. This lock is intentionally independent from
    /// `write_lock`: model/runtime callbacks may call this entry point without
    /// holding the RPC request lock.
    pub async fn persist_orchestrator(&self, mut operation: Value) -> RpcResult {
        let _guard = self.persist_lock.lock().await;
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let (result, request_id, status, snapshot) = {
            let object = operation.as_object_mut().ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "orchestrator operation must be an object".into(),
                details: None,
            })?;
            object.insert("snapshot".into(), snapshot);
            object.entry("status").or_insert_with(|| json!("done"));
            object.entry("at").or_insert_with(|| json!(now()));
            (
                object.get("result").cloned().unwrap_or(Value::Null),
                object
                    .get("client_request_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                object
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("done")
                    .to_owned(),
                object.get("snapshot").cloned().unwrap_or(Value::Null),
            )
        };
        self.store
            .append_jsonl("data/orchestrator/operations.jsonl", &operation)
            .map_err(store_error)?;
        if status == "rolled_back" {
            if let Some(request_id) = request_id.as_ref() {
                self.idempotency.lock().await.remove(request_id);
            }
        }
        self.store
            .write_snapshot("data/orchestrator/state.json", &snapshot)
            .map_err(store_error)?;
        if status == "done" {
            if let Some(request_id) = request_id {
                self.idempotency
                    .lock()
                    .await
                    .insert(request_id, result.clone());
            }
        }
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
        let audit_params = if method == "settings.update" {
            redact_settings_params(params)
        } else {
            params.clone()
        };
        self.persist_orchestrator(json!({
            "method":method,
            "params":audit_params,
            "client_request_id":request_id,
            "result":result,
            "status":"done",
            "at":now()
        }))
        .await?;
        if let Some(event_name) = Self::event_name(method) {
            let data = event_data(method, params, result);
            let event = self
                .store
                .append_event(event_name, data.clone())
                .map_err(store_error)?;
            state.publish_event(event.seq, &event.event, data).await;
        }
        if matches!(method, "bot.create" | "bot.duplicate") {
            if let Some(chat) = result.get("dm_chat").filter(|chat| chat.is_object()) {
                let data = json!({"chat":chat});
                let event = self
                    .store
                    .append_event("chat.created", data.clone())
                    .map_err(store_error)?;
                state.publish_event(event.seq, &event.event, data).await;
            }
        } else if method == "bot.create_from_template" {
            let bots = result
                .get("bots")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let chats = result
                .get("dm_chats")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for index in 0..bots.len().max(chats.len()) {
                if let Some(bot) = bots.get(index) {
                    let data = json!({"bot":bot});
                    let event = self
                        .store
                        .append_event("bot.created", data.clone())
                        .map_err(store_error)?;
                    state.publish_event(event.seq, &event.event, data).await;
                }
                if let Some(chat) = chats.get(index) {
                    let data = json!({"chat":chat});
                    let event = self
                        .store
                        .append_event("chat.created", data.clone())
                        .map_err(store_error)?;
                    state.publish_event(event.seq, &event.event, data).await;
                }
            }
        }
        Ok(result.clone())
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
        self.call("send_msg", params, state).await
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
        let _guard = self.write_lock.lock().await;
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
                return Ok(value);
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
            "chat.mark_read" => self.chat_mark_read(&params).await?,
            "chat.list" => self.chat_list().await?,
            "chat.get" => self.chat_get(&params).await?,
            "settings.get" => json!({"settings": self.settings(state).await?}),
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
            "project.request_review" => project_request_review(self, &params).await?,
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
                let question = self
                    .orchestrator
                    .rpc(
                        "question.ask",
                        json!({
                            "bot_id": params.get("bot_id").and_then(Value::as_str).unwrap_or("main"),
                            "assignment_id": proposal_id,
                            "chat_id": params.get("chat_id").and_then(Value::as_str).unwrap_or("chat_main"),
                            "text": format!("批准创建 Bot「{}」？", proposal.get("name").and_then(Value::as_str).unwrap_or("新 Bot")),
                            "options": ["批准", "拒绝"],
                            "allow_free_text": true
                        }),
                    )
                    .await
                    .map_err(Self::error)?;
                json!({"proposal":proposal,"question":question})
            }
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
                self.orchestrator
                    .rpc(method, params.clone())
                    .await
                    .map_err(Self::error)?
            }
            _ => self
                .orchestrator
                .rpc(method, params.clone())
                .await
                .map_err(Self::error)?,
        };
        let result = normalize_result(method, result).map_err(|message| RpcError {
            code: "internal".into(),
            message,
            details: None,
        })?;
        let mut result = result;
        if method == "send_msg" {
            result = self.persist_client_message(&result)?;
        } else if method == "chat.send" {
            let canonical = self.persist_client_message(&result["message"])?;
            result["message"] = canonical;
        }
        self.enrich_bot_status(&mut result)
            .map_err(|message| RpcError {
                code: "internal".into(),
                message,
                details: None,
            })?;
        validate_result(method, &result).map_err(|message| RpcError {
            code: "internal".into(),
            message,
            details: None,
        })?;
        if Self::is_mutation(method) {
            self.persist(state, method, &params, &result).await
        } else {
            Ok(result)
        }
    }
}

/// Turn a worker's done handoff into the durable project review state and
/// one user-visible message.  The orchestrator deliberately keeps this
/// composition out of its compact RPC model; the gateway adapter owns the
/// wire-level project/message event shape.
async fn project_request_review(backend: &ProductionBackend, params: &Value) -> RpcResult {
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
    let project_result = backend
        .orchestrator
        .rpc("project.get", json!({"project_id":project_id}))
        .await
        .map_err(ProductionBackend::error)?;
    let project = project_result
        .get("project")
        .cloned()
        .unwrap_or(Value::Null);
    let chat_id = project
        .get("chat_id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError {
            code: "internal".into(),
            message: "project has no chat_id".into(),
            details: None,
        })?;
    let assignments = backend
        .orchestrator
        .rpc("assignment.list", json!({}))
        .await
        .map_err(ProductionBackend::error)?;
    let assignment = assignments
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                item.get("project_id").and_then(Value::as_str) == Some(project_id)
                    && matches!(
                        item.get("status").and_then(Value::as_str),
                        Some("working") | Some("waiting_bot") | Some("done")
                    )
            })
        })
        .cloned()
        .ok_or_else(|| RpcError {
            code: "conflict".into(),
            message: "project has no worker assignment ready for review".into(),
            details: None,
        })?;
    let message = backend
        .orchestrator
        .rpc(
            "send_msg",
            json!({
                "bot_id": assignment.get("bot_id").cloned().unwrap_or_else(|| json!("main")),
                "chat_id": chat_id,
                "assignment_id": assignment.get("id"),
                "text": summary,
                "intent": "done",
                "mentions": ["main"]
            }),
        )
        .await
        .map_err(ProductionBackend::error)?;
    let updated = backend
        .orchestrator
        .rpc("project.get", json!({"project_id":project_id}))
        .await
        .map_err(ProductionBackend::error)?;
    Ok(json!({
        "project": updated.get("project").cloned().unwrap_or(project),
        "message": message
    }))
}
impl ProductionBackend {
    async fn settings(&self, state: &GatewayState) -> Result<Value, RpcError> {
        let path = "data/settings.json";
        let host_name = state.host_name.read().await.clone();
        let value = self
            .store
            .read_snapshot::<Value>(path)
            .map_err(store_error)?
            .unwrap_or_else(|| default_settings(&host_name));
        serde_json::from_value::<Settings>(value.clone()).map_err(|error| RpcError {
            code: "internal".into(),
            message: error.to_string(),
            details: None,
        })?;
        let mut value = value;
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
        let mut value = self.settings(state).await?;
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
        *state.host_name.write().await = value
            .get("host_name")
            .and_then(Value::as_str)
            .unwrap_or("Mac Bot")
            .into();
        Ok(json!({"settings": self.settings(state).await?}))
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
        let assignment_id = required_text(params, "assignment_id")?;
        let group_chat_id = required_text(params, "chat_id")?;
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
        let question = self
            .orchestrator
            .rpc(
                "question.ask",
                json!({
                    "bot_id": bot_id,
                    "assignment_id": assignment_id,
                    "chat_id": dm_chat_id,
                    "text": format!("{reason}。是否接管浏览器？"),
                    "options": ["接管", "取消"],
                    "allow_free_text": false
                }),
            )
            .await
            .map_err(Self::error)?;
        let request = json!({
            "bot_id": bot_id,
            "assignment_id": assignment_id,
            "chat_id": dm_chat_id,
            "group_chat_id": group_chat_id,
            "reason": reason,
            "question_id": question.get("id").cloned().unwrap_or(Value::Null),
            "state": "pending",
            "created_at": now()
        });
        self.store
            .write_snapshot(
                format!("data/takeovers/{}.json", takeover_component(&assignment_id)),
                &request,
            )
            .map_err(store_error)?;
        Ok(json!({
            "takeover_request": request,
            "question": question,
            // `approval_ref` is the protocol's group-side pointer.  The
            // private question is the approval object users act on, so its
            // id is exposed under the protocol field name `approval_id`.
            "approval_ref": {
                "approval_id": request["question_id"],
                "chat_id": group_chat_id,
                "question_id": request["question_id"]
            }
        }))
    }

    async fn takeover_start(&self, state: &GatewayState, params: &Value) -> RpcResult {
        let bot_id = required_text(params, "bot_id")?;
        let (assignment_id, mut request) = match self.takeover_for_action(params, "pending") {
            Ok(value) => value,
            Err(error) if error.code == "not_found" && params.get("assignment_id").is_none() => {
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
                (
                    assignment_id.clone(),
                    json!({"bot_id":bot_id,"assignment_id":assignment_id,"chat_id":chat_id,"group_chat_id":Value::Null,"reason":"用户接管浏览器","question_id":Value::Null,"state":"pending","created_at":now()}),
                )
            }
            Err(error) => return Err(error),
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
        request["state"] = json!("active");
        request["started_at"] = json!(now());
        self.write_takeover(&assignment_id, &request)?;
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
        state
            .browser
            .lock()
            .await
            .takeover_release(&bot_id)
            .map_err(browser_error)?;
        request["state"] = json!("done");
        request["released_at"] = json!(now());
        if let Some(note) = params.get("note") {
            request["note"] = note.clone();
        }
        self.write_takeover(&assignment_id, &request)?;
        Ok(json!({"takeover_request":request}))
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
        let entries = fs::read_dir(&dir).map_err(|error| RpcError {
            code: "not_found".into(),
            message: error.to_string(),
            details: None,
        })?;
        let mut matches = Vec::new();
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
        matches.sort_by(|left, right| {
            right.1["created_at"]
                .as_str()
                .cmp(&left.1["created_at"].as_str())
        });
        matches.into_iter().next().ok_or_else(|| RpcError {
            code: "not_found".into(),
            message: format!("no {expected_state} takeover request for bot {bot_id}"),
            details: None,
        })
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
            let message = self
                .orchestrator
                .create_task_stopped_message(&assignment_id)
                .map_err(Self::error)?;
            let mut data = serde_json::to_value(message).map_err(|error| RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            })?;
            normalize_message(&mut data);
            let data = self.persist_client_message(&data)?;
            let event = self
                .store
                .append_event("message.created", json!({"message":data.clone()}))
                .map_err(store_error)?;
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(json!({"assignment":assignment}))
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

    async fn chat_send(&self, params: Value) -> RpcResult {
        let chat_id = params
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "chat_id is required".into(),
                details: None,
            })?;
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "text is required".into(),
                details: None,
            })?;
        let mentions = params.get("mentions").and_then(Value::as_array).map(|items| items.iter().filter_map(|item| match item.get("kind").and_then(Value::as_str) { Some("bot") => Some(json!({"bot_id":item.get("bot_id"),"instruction":item.get("instruction")})), Some("main") => Some(json!("main")), Some("user") => Some(json!("user")), _ => None }).collect::<Vec<_>>()).unwrap_or_default();
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
        if let Some(reply_to) = params.get("reply_to") {
            result["message"]["reply_to"] = reply_to.clone();
        }
        Ok(result)
    }

    fn load_chat_messages(&self, chat_id: &str) -> Result<Vec<Value>, RpcError> {
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
        for message in persisted {
            if message.get("chat_id").and_then(Value::as_str) != Some(chat_id) {
                continue;
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
        Ok(messages)
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
        let mut messages = self.load_chat_messages(chat_id)?;
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

    async fn chat_mark_read(&self, params: &Value) -> RpcResult {
        let chat_id = params
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "chat_id is required".into(),
                details: None,
            })?;
        let seq = params
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError {
                code: "invalid_params".into(),
                message: "seq is required".into(),
                details: None,
            })?;
        let _ = self.chat_get(&json!({"chat_id":chat_id})).await?;
        let path = format!("data/chats/{}/metadata.json", takeover_component(chat_id));
        let mut metadata = self
            .store
            .read_snapshot::<Value>(&path)
            .map_err(store_error)?
            .unwrap_or_else(|| json!({}));
        let current = metadata
            .get("last_read_seq")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        metadata["last_read_seq"] = json!(current.max(seq));
        self.store
            .write_snapshot(path, &metadata)
            .map_err(store_error)?;
        Ok(json!({}))
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
        let seq = self.store.last_event_seq().map_err(store_error)?;
        let host_name = state.host_name.read().await.clone();
        let node_id = state.node_id.read().await.clone();
        let hello = json!({"protocol":1,"server_version":"0.1.0","node_id":node_id,"host_name":host_name,"server_time":now(),"last_seq":seq,"timezone":settings["timezone"],"currency":settings["currency"],"features":["browser"]});
        Ok(
            json!({"seq":seq,"hello":hello,"bots":bots,"chats":chats,"projects":projects,"settings":settings,"pending":{"approvals":approvals,"questions":questions,"reviews":[]}}),
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
        let Some(assignments) = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .cloned()
        else {
            return Ok(());
        };
        let mut counts: HashMap<String, (u32, u32, u32, bool)> = HashMap::new();
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
        Ok(())
    }

    fn workbench(&self) -> Result<Value, String> {
        let snapshot = self
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
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
        if let Some(items) = snapshot.get("questions").and_then(Value::as_object) {
            for question in items
                .values()
                .filter(|item| item.get("state").and_then(Value::as_str) == Some("pending"))
            {
                waiting.push(json!({"kind":"question","question":question}));
            }
        }
        let mut counts = HashMap::new();
        for assignment in assignments.values() {
            let Some(bot_id) = assignment.get("bot_id").and_then(Value::as_str) else {
                continue;
            };
            let entry = counts.entry(bot_id.to_owned()).or_insert((0, 0, 0, false));
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
        let assignment_status = assignments
            .iter()
            .filter_map(|(id, item)| {
                item.get("status")
                    .and_then(Value::as_str)
                    .map(|status| (id.clone(), status.to_owned()))
            })
            .collect::<HashMap<_, _>>();
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
        let bots = snapshot.get("bots").and_then(Value::as_object).map(|items| items.values().filter(|bot| !bot_is_main(bot)).map(|bot| {
            let bot_id = bot.get("id").and_then(Value::as_str).unwrap_or("");
            let (active, _, _, _) = counts.get(bot_id).copied().unwrap_or_default();
            let assignments = assignments.values().filter(|item| item.get("bot_id").and_then(Value::as_str) == Some(bot_id)).cloned().map(|mut item| { normalize_assignment(&mut item); item }).collect::<Vec<_>>();
            json!({"bot_id":bot_id,"active":active,"max_parallel":bot.get("max_parallel").and_then(Value::as_u64).unwrap_or(1),"assignments":assignments})
        }).collect::<Vec<_>>()).unwrap_or_default();
        let done_today = assignments
            .values()
            .filter(|item| item.get("status").and_then(Value::as_str) == Some("done"))
            .cloned()
            .map(|mut item| {
                normalize_assignment(&mut item);
                item
            })
            .collect::<Vec<_>>();
        let running = assignments
            .values()
            .filter(|item| item.get("status").and_then(Value::as_str) == Some("working"))
            .count();
        let global_limit = snapshot
            .get("settings")
            .and_then(|settings| settings.get("global_limit"))
            .and_then(Value::as_u64)
            .unwrap_or(8);
        Ok(
            json!({"running":running,"global_limit":global_limit,"subagents_running":0,"waiting":waiting,"bots":bots,"done_today":done_today}),
        )
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

fn event_data(method: &str, params: &Value, result: &Value) -> Value {
    match method {
        "bot.delete" => json!({ "bot_id": params.get("bot_id").cloned().unwrap_or(Value::Null) }),
        "bot.create" | "bot.update" | "bot.duplicate" => {
            json!({ "bot": result.get("bot").cloned().unwrap_or(Value::Null) })
        }
        "project.request_changes" => {
            json!({ "project": result.get("project").cloned().unwrap_or(Value::Null), "message": { "fallback_text": result.get("text").cloned().unwrap_or(Value::Null) } })
        }
        "project.request_review" => {
            json!({
                "project": result.get("project").cloned().unwrap_or(Value::Null),
                "message": result.get("message").cloned().unwrap_or(Value::Null)
            })
        }
        "propose_bot" => json!({
            "question": result.get("question").cloned().unwrap_or(Value::Null),
            "proposal": result.get("proposal").cloned().unwrap_or(Value::Null)
        }),
        "assignment.create" | "assign" | "delegate" | "assignment.stop" | "assignment.steer"
        | "steer" | "loop.resolve" => {
            json!({ "assignment": result.get("assignment").cloned().unwrap_or_else(|| result.clone()) })
        }
        "send_msg" | "chat.send" => {
            json!({ "message": result.get("message").cloned().unwrap_or_else(|| result.clone()) })
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
        "project.list" => {
            if let Some(items) = result.get_mut("projects").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_project(item);
                }
            }
        }
        "project.create"
        | "project.add_member"
        | "project.remove_member"
        | "project.confirm_done"
        | "project.request_changes"
        | "project.archive"
        | "project.reopen"
        | "project.status"
        | "project_status" => {
            if let Some(item) = result.get_mut("project") {
                normalize_project(item);
            }
        }
        "assignment.create" | "assign" | "delegate" => normalize_assignment(&mut result),
        "assignment.get" | "assignment.stop" => {
            if let Some(item) = result.get_mut("assignment") {
                normalize_assignment(item);
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
            if let Some(item) = result.get_mut("message") {
                normalize_message(item);
            }
        }
        "propose_bot" => {
            if let Some(item) = result.get_mut("question") {
                normalize_question(item);
            }
        }
        "chat.send" => {
            if let Some(item) = result.get_mut("message") {
                normalize_message(item);
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
    let _ = obj(value);
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
fn normalize_message(value: &mut Value) {
    let o = obj(value);
    o.entry("seq").or_insert(json!(0));
    if let Some(sender) = o.get_mut("sender") {
        normalize_sender(sender);
    }
    o.entry("edited_at").or_insert(Value::Null);
    o.entry("deleted").or_insert(json!(false));
    o.entry("reply_to").or_insert(Value::Null);
    o.entry("thread_count").or_insert(json!(0));
    o.entry("streaming").or_insert(json!(false));
    o.entry("reactions").or_insert(json!([]));
    let intent = o.get("intent").and_then(Value::as_str).unwrap_or("ack");
    let text = o
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| o.get("fallback_text").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    let block = match intent {
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
    let _ = obj(value);
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

fn default_settings(host_name: &str) -> Value {
    json!({
        "host_name":host_name,"timezone":"Asia/Shanghai","currency":"CNY",
        "concurrency":{"global":8,"bot_default":3,"subagent_per_run":4,"subagent_global":12,"loop_hops":8},
        "models":{"bot_default":null,"main":null,"subagent":"inherit","maintenance":null},
        "main_bot":{"auto_create_project":true},"approvals":{"mode":"require","rules":[]},
        "browser":{"default_mode":"headless","chrome_profile":"Default","stream":{"desktop":{"max_width":1280,"quality":70,"max_fps":15},"mobile":{"max_width":720,"quality":50,"max_fps":10}}},
        "skills":{"extra_dirs":[]},"trace":{"save_full_requests":false},
        "web_search":{"provider":null,"endpoint":null,"has_key":false},"push":{"apns_configured":false}
    })
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
        "project.create"
        | "project.add_member"
        | "project.remove_member"
        | "project.confirm_done"
        | "project.request_changes"
        | "project.archive"
        | "project.reopen"
        | "project.status"
        | "project_status" => parse!(Project, value["project"]),
        "assignment.create" | "assign" | "delegate" => parse!(Assignment, value),
        "assignment.get" | "assignment.stop" => parse!(Assignment, value["assignment"]),
        "assignment.list" => parse!(Vec<Assignment>, value["items"]),
        "send_msg" => parse!(Message, value),
        "project.request_review" => {
            parse!(Project, value["project"]);
            parse!(Message, value["message"]);
        }
        "propose_bot" => parse!(Question, value["question"]),
        "chat.send" => parse!(Message, value["message"]),
        "chat.history" => parse!(Vec<Message>, value["messages"]),
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
    use crate::{Gateway, GatewayConfig};
    use chrono::Duration;
    use macbot_protocol::{
        Assignment as WireAssignment, Bot as WireBot, Chat as WireChat, HeatmapResult,
        Message as WireMessage, Project as WireProject, Provider as WireProvider,
        ProviderResult as WireProviderResult, UsageBreakdownResult, UsageSummaryResult,
        UsageTimeseriesResult,
    };
    use macbot_usage::{Totals, UsageRecord};
    use tempfile::tempdir;

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
            .find(|event| event.event == "message.created")
            .unwrap();
        let live = gateway.state.inner.read().await.events.back().cloned();
        assert_eq!(
            live.as_ref().and_then(|event| event["seq"].as_u64()),
            Some(message_event.seq)
        );
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
            1
        );
        let message = events
            .iter()
            .find(|event| event.event == "message.created")
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
            1
        );
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
        let review = backend
            .call(
                "project.request_review",
                json!({"project_id":project["project"]["id"],"summary":"ready"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(review["project"]["status"], "review");
        serde_json::from_value::<Message>(review["message"].clone()).unwrap();
        let done = backend
            .call(
                "project.confirm_done",
                json!({"project_id":project["project"]["id"]}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(done["project"]["status"], "done");
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
    }
}
