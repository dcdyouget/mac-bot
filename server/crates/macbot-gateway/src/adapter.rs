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
use macbot_orchestrator::Orchestrator;
use macbot_protocol::{
    Announcement, Approval, Assignment, Bot, Chat, Device, Hello, Message, PendingItems, Project,
    Question, Routine, Settings,
};
use macbot_providers::registry::{ProviderRegistry, RegistryError};
use macbot_providers::SecretStore;
use macbot_store::Store;
use macbot_usage::UsageLedger;
use serde_json::{json, Map, Value};
use std::{collections::HashMap, path::Path, sync::Arc};
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
        #[cfg(target_os = "macos")]
        let secrets: Arc<dyn SecretStore> = Arc::new(macbot_providers::KeychainSecrets);
        #[cfg(not(target_os = "macos"))]
        let secrets: Arc<dyn SecretStore> = Arc::new(macbot_providers::MemorySecrets::default());
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
        )
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
            | "project.request_changes"
            | "project.archive"
            | "project.reopen" => "project.updated",
            "assignment.create" | "assign" | "delegate" => "assignment.created",
            "assignment.stop" | "assignment.steer" | "steer" => "assignment.updated",
            "send_msg" | "chat.send" => "message.created",
            "approval.request" => "approval.requested",
            "approval.decide" => "approval.resolved",
            "question.ask" => "question.asked",
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
        let operation = json!({ "method": method, "params": params, "client_request_id": request_id, "result": result, "snapshot": snapshot, "status": "done", "at": now() });
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
}

#[async_trait]
impl RpcBackend for ProductionBackend {
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
        if Self::is_mutation(method) {
            let start = json!({ "method": method, "params": params, "client_request_id": params.get("client_request_id").and_then(Value::as_str), "status": "started", "at": now() });
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
            "device.register" => self.device_register(&params).await?,
            "takeover.start" => {
                let bot_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
                state
                    .browser
                    .lock()
                    .await
                    .takeover_start(bot_id)
                    .map_err(browser_error)?;
                json!({})
            }
            "takeover.release" => {
                let bot_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
                state
                    .browser
                    .lock()
                    .await
                    .takeover_release(bot_id)
                    .map_err(browser_error)?;
                json!({})
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
        self.store
            .write_snapshot("data/settings.json", &value)
            .map_err(store_error)?;
        *state.host_name.write().await = value
            .get("host_name")
            .and_then(Value::as_str)
            .unwrap_or("Mac Bot")
            .into();
        Ok(json!({"settings": value}))
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
            .map_err(Self::error)?;
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
        "chat.send" => parse!(Message, value["message"]),
        "chat.history" => parse!(Vec<Message>, value["messages"]),
        "chat.list" => parse!(Vec<Chat>, value["chats"]),
        "chat.get" => parse!(Chat, value["chat"]),
        "settings.get" | "settings.update" => parse!(Settings, value["settings"]),
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
    use macbot_protocol::{
        Assignment as WireAssignment, Bot as WireBot, Message as WireMessage,
        Project as WireProject, Provider as WireProvider, ProviderResult as WireProviderResult,
    };
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
}
