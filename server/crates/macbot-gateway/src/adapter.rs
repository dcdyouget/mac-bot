//! Production RPC adapter.
//!
//! The gateway boundary is protocol JSON.  The orchestration crate keeps a
//! compact internal state model, so this module is the single place that
//! expands it to the complete `macbot-protocol` wire objects and validates the
//! result before it is exposed to a client.  Mutations are write-ahead:
//! operation JSONL is synced, then the global event log is synced, then the
//! event is published to connected clients.

use crate::{now, GatewayState, RpcBackend, RpcError, RpcResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use macbot_browser::BrowserError;
use macbot_durable::DurableRuntime;
use macbot_orchestrator::{Orchestrator, UsageTotals};
use macbot_protocol::{
    Announcement, Approval, Assignment, Bot, Chat, Device, HeatmapResult, Hello, Message,
    PendingItems, Project, Question, Routine, Settings, UsageBreakdownResult, UsageSummaryResult,
    UsageTimeseriesResult,
};
use macbot_providers::registry::{ProviderRegistry, RegistryError};
use macbot_store::Store;
use macbot_usage::UsageLedger;
use serde_json::{json, Map, Value};
use std::{collections::HashMap, fs, path::Path, sync::Arc};
use thiserror::Error;
use tokio::sync::Mutex;

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
    idempotency: Arc<Mutex<HashMap<String, Value>>>,
}

impl ProductionBackend {
    pub(crate) fn update_assignment_usage(
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
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        self.store
            .append_jsonl(
                "data/orchestrator/operations.jsonl",
                &json!({"method":"assignment.usage","params":{"assignment_id":assignment_id,"usage":usage},"result":{"assignment":assignment},"snapshot":snapshot,"status":"done","at":now()}),
            )
            .map_err(store_error)?;
        self.store
            .write_snapshot("data/orchestrator/state.json", &snapshot)
            .map_err(store_error)?;
        Ok(json!({"assignment":assignment}))
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
        let mut idempotency = HashMap::new();
        for op in operations {
            if op.get("status").and_then(Value::as_str) == Some("done") {
                if let (Some(id), Some(result)) = (
                    op.get("client_request_id").and_then(Value::as_str),
                    op.get("result"),
                ) {
                    idempotency.insert(id.into(), result.clone());
                }
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
            idempotency: Arc::new(Mutex::new(idempotency)),
        })
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
            "bot.create" | "bot.create_from_template" => "bot.created",
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
            "loop.resolve" => "assignment.updated",
            "settings.update" => "settings.updated",
            _ => return None,
        })
    }

    async fn persist(
        &self,
        state: &GatewayState,
        method: &str,
        params: &Value,
        result: &Value,
    ) -> RpcResult {
        let request_id = params.get("client_request_id").and_then(Value::as_str);
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let audit_params = if method == "settings.update" {
            redact_settings_params(params)
        } else {
            params.clone()
        };
        let operation = json!({ "method": method, "params": audit_params, "client_request_id": request_id, "result": result, "snapshot": snapshot, "status": "done", "at": now() });
        self.store
            .append_jsonl("data/orchestrator/operations.jsonl", &operation)
            .map_err(store_error)?;
        self.store
            .write_snapshot("data/orchestrator/state.json", &snapshot)
            .map_err(store_error)?;
        if let Some(request_id) = request_id {
            self.idempotency
                .lock()
                .await
                .insert(request_id.into(), result.clone());
        }
        if let Some(event_name) = Self::event_name(method) {
            let data = event_data(method, params, result);
            let event = self
                .store
                .append_event(event_name, data.clone())
                .map_err(store_error)?;
            state.publish_event(event.seq, &event.event, data).await;
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
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        self.store
            .write_snapshot("data/orchestrator/state.json", &snapshot)
            .map_err(store_error)?;
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
            "chat.send" => self.chat_send(params.clone()).await?,
            "chat.history" => self.chat_history(&params).await?,
            "chat.list" => self.chat_list().await?,
            "chat.get" => self.chat_get(&params).await?,
            "settings.get" => json!({"settings": self.settings(state).await?}),
            "settings.update" => self.settings_update(state, &params).await?,
            "usage.summary" | "usage.heatmap" | "usage.timeseries" | "usage.breakdown" => {
                self.usage_query(state, method, &params).await?
            }
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
                if current["project"]["status"].as_str() != Some("review") {
                    return Err(RpcError {
                        code: "conflict".into(),
                        message: "project can be finished only after user review confirmation"
                            .into(),
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
        let (assignment_id, mut request) = self.takeover_for_action(params, "pending")?;
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
        let next_seq = self
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("messages")
                    .and_then(Value::as_object)
                    .map(|items| {
                        items
                            .values()
                            .filter(|item| {
                                item.get("chat_id").and_then(Value::as_str) == Some(chat_id)
                            })
                            .count() as u64
                    })
            })
            .unwrap_or(1);
        let mut result = json!({"message": message});
        result["message"]["seq"] = json!(next_seq);
        if let Some(reply_to) = params.get("reply_to") {
            result["message"]["reply_to"] = reply_to.clone();
        }
        Ok(result)
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
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let mut messages = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|items| items.values())
            .filter(|item| item.get("chat_id").and_then(Value::as_str) == Some(chat_id))
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by(|a, b| {
            a.get("created_at")
                .and_then(Value::as_str)
                .cmp(&b.get("created_at").and_then(Value::as_str))
        });
        for (index, message) in messages.iter_mut().enumerate() {
            message["seq"] = json!((index + 1) as u64);
        }
        let after = params.get("after_seq").and_then(Value::as_u64).unwrap_or(0);
        messages.retain(|item| item.get("seq").and_then(Value::as_u64).unwrap_or(0) > after);
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .min(100) as usize;
        let has_more = messages.len() > limit;
        messages.truncate(limit);
        Ok(json!({"messages":messages,"has_more":has_more}))
    }

    async fn chat_list(&self) -> RpcResult {
        let snapshot = self.orchestrator.snapshot().map_err(Self::error)?;
        let mut chats = vec![main_chat()];
        if let Some(bots) = snapshot.get("bots").and_then(Value::as_object) {
            for bot in bots
                .values()
                .filter(|bot| bot.get("hidden").and_then(Value::as_bool) != Some(true))
            {
                chats.push(json!({"id":bot["dm_chat_id"],"kind":"bot_dm","title":bot["name"],"bot_id":bot["id"],"project_id":null,"member_bot_ids":[bot["id"]],"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":bot["pinned"],"muted":false,"updated_at":bot["updated_at"]}));
            }
        }
        if let Some(projects) = snapshot.get("projects").and_then(Value::as_object) {
            for project in projects.values() {
                chats.push(project_chat(project));
            }
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
        let seq = self.store.last_event_seq().map_err(store_error)?;
        let host_name = state.host_name.read().await.clone();
        let node_id = state.node_id.read().await.clone();
        let hello = json!({"protocol":1,"server_version":"0.1.0","node_id":node_id,"host_name":host_name,"server_time":now(),"last_seq":seq,"timezone":settings["timezone"],"currency":settings["currency"],"features":["browser"]});
        Ok(
            json!({"seq":seq,"hello":hello,"bots":bots,"chats":chats,"projects":projects,"settings":settings,"pending":{"approvals":pending["approvals"],"questions":questions,"reviews":[]}}),
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
        let Some(assignments) = self
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?
            .get("assignments")
            .and_then(Value::as_object)
            .cloned()
        else {
            return Ok(());
        };
        let mut counts: HashMap<String, (u32, u32, u32, bool)> = HashMap::new();
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
        "routine.delete" => {
            json!({ "routine_id": params.get("routine_id").cloned().unwrap_or(Value::Null) })
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
        "bot.get" | "bot.create" | "bot.update" => {
            if let Some(item) = result.get_mut("bot") {
                normalize_bot(item);
            }
        }
        "bot.create_from_template" => {
            if let Some(items) = result.get_mut("bots").and_then(Value::as_array_mut) {
                for item in items {
                    normalize_bot(item);
                }
            }
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
    if o.get("dm_chat_id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        o.insert("dm_chat_id".into(), json!(format!("dm_{id}")));
    }
    o.entry("status")
        .or_insert_with(|| json!({"summary":"idle","active":0,"queued":0,"waiting":0}));
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
        .unwrap_or("")
        .to_owned();
    let block = match intent {
        "progress" => json!({"type":"progress","text":text}),
        "blocked" => json!({"type":"blocked","reason":text}),
        "done" => {
            json!({"type":"completion","summary":text,"artifacts":[],"next":[],"notify_main":true})
        }
        _ => json!({"type":"text","markdown":text}),
    };
    o.insert("blocks".into(), json!([block]));
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
        Assignment as WireAssignment, Bot as WireBot, HeatmapResult, Message as WireMessage,
        Project as WireProject, Provider as WireProvider, ProviderResult as WireProviderResult,
        UsageBreakdownResult, UsageSummaryResult, UsageTimeseriesResult,
    };
    use macbot_usage::{Totals, UsageRecord};
    use tempfile::tempdir;

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
