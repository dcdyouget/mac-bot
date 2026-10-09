//! Runtime composition for real model runs.
//!
//! The RPC adapter owns method normalization and persistence. This module owns
//! the process-wide execution engine and the bridges that connect it to the
//! durable event stream and orchestrator group handoff.

use crate::{
    adapter::ProductionBackend,
    collaboration_tools::{
        CollaborationIdentity, CollaborationTools, ProductionBrowserBridge,
        ProductionCoordinationRpc, ReqwestWebBridge, SubagentDispatchBridge,
        SubagentDispatchRequest, WebCredentialStore, WebSearchConfig,
    },
    execution::{
        ExecutionEngine, ExecutionError, ExecutionEvent, ExecutionOutcome, ExecutionRequest,
        ExecutionSink, ExecutionState, GatewayStateSink, GroupMessageBridge, ProviderResolver,
    },
    features::{
        FeatureService, MaintenanceUsageContext, MaintenanceUsageSink, ModelMaintenanceAdapter,
        SharedFeatureService,
    },
    memory_tools::{FeatureExecutionSink, FeatureRunContext, FeatureToolRuntime},
    GatewayState, RpcBackend,
};
use async_trait::async_trait;
use chrono::Utc;
use macbot_memory::{MemoryAccess, MemoryTarget, ProjectRecord, SessionMessage};
use macbot_orchestrator::Orchestrator;
use macbot_providers::{
    registry::ProviderRegistry, Completion, ModelEvent, ModelProvider, ModelRequest, TokenUsage,
};
use macbot_store::Store;
use macbot_tools::{
    BashJobManager, BashJobTool, BashTool, EditTool, FileMutationQueue, FindTool, GrepTool, LsTool,
    Part, ReadTool, Tool, ToolContext, ToolResult, WriteTool,
};
use macbot_usage::{Price, Totals, UsageLedger, UsageRecord};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("execution: {0}")]
    Execution(#[from] ExecutionError),
    #[error("orchestrator: {0}")]
    Orchestrator(String),
    #[error("provider: {0}")]
    Provider(String),
}

/// Process-wide execution runtime. Clone this handle into RPC/task schedulers;
/// all runs share durable jobs, provider resolution and usage deduplication.
#[derive(Clone)]
pub struct RuntimeExecution {
    engine: Arc<ExecutionEngine>,
    state: Arc<ExecutionState>,
    store: Store,
    home: PathBuf,
    usage: Arc<Mutex<UsageLedger>>,
    provider_resolver: Arc<RegistryResolver>,
    base_tools: Vec<Arc<dyn Tool>>,
    base_sink: Arc<dyn ExecutionSink>,
    feature_service: Arc<FeatureService>,
    backend: Arc<ProductionBackend>,
    gateway_state: GatewayState,
}

enum WaitingContinuation {
    Question(String),
    Takeover(Option<String>),
}

/// Composes the transport/orchestrator adapter with durable model execution.
/// The adapter remains the owner of RPC normalization and WAL commits; this
/// wrapper only schedules a run after a successful `chat.send`.
#[derive(Clone)]
pub struct ComposedBackend {
    inner: Arc<ProductionBackend>,
    runtime: RuntimeExecution,
    state: GatewayState,
    feature_service: SharedFeatureService,
    scheduled: Arc<Mutex<HashSet<String>>>,
    active_runs: Arc<std::sync::Mutex<HashSet<String>>>,
    waiting_runs: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl ComposedBackend {
    pub fn open(
        inner: Arc<ProductionBackend>,
        state: GatewayState,
        home: impl Into<std::path::PathBuf>,
    ) -> Result<Self, RuntimeError> {
        let home = home.into();
        let feature_service = Arc::new(
            FeatureService::with_store(inner.store.clone(), home.clone(), Vec::<PathBuf>::new())
                .map_err(|error| RuntimeError::Provider(error.to_string()))?,
        );
        let runtime = RuntimeExecution::open(
            home,
            inner.clone(),
            inner.usage.clone(),
            inner.providers.clone(),
            state.clone(),
            feature_service.clone(),
        )?;
        let backend = Self {
            inner,
            runtime,
            state,
            feature_service,
            scheduled: Arc::new(Mutex::new(HashSet::new())),
            active_runs: Arc::new(std::sync::Mutex::new(HashSet::new())),
            waiting_runs: Arc::new(std::sync::Mutex::new(HashSet::new())),
        };
        // Recovery must happen before the first client request.  A suspended
        // unsafe job is left for approval; ordinary working jobs resume from
        // their durable checkpoint with the original run id.
        let recovery = backend.runtime.clone();
        let recovery_backend = backend.clone();
        tokio::spawn(async move {
            if let Err(error) = recovery.configure_feature_runtime().await {
                tracing::warn!(%error, "feature runtime configuration unavailable during recovery");
            }
            match recovery.recover().await {
                Ok(jobs) => {
                    for job in jobs {
                        if !matches!(
                            job.status,
                            macbot_durable::JobStatus::Queued | macbot_durable::JobStatus::Running
                        ) {
                            continue;
                        }
                        let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str)
                        else {
                            continue;
                        };
                        let request = recovery.store.read_snapshot::<ExecutionRequest>(format!(
                            "data/run_requests/{run_id}.json"
                        ));
                        match request {
                            Ok(Some(request)) => recovery_backend.spawn_request(request),
                            Ok(None) => {
                                tracing::warn!(%run_id, "durable job has no execution request")
                            }
                            Err(error) => {
                                tracing::error!(%error, %run_id, "cannot load recovered execution request")
                            }
                        }
                    }
                }
                Err(error) => tracing::error!(%error, "durable execution recovery failed"),
            }
            recovery_backend.dispatch_ready_assignments().await;
        });
        Ok(backend)
    }

    /// Advance due routines and dispatch each newly-created run through the
    /// same durable execution path as `chat.send`.  `ProductionBackend` owns
    /// routine state transitions and event persistence; this layer owns the
    /// model/tool execution that follows those transitions.
    pub async fn tick_routines(&self, at: chrono::DateTime<chrono::Utc>) -> crate::RpcResult {
        let result = self.inner.tick_routines(&self.state, at).await?;
        let Some(dispatches) = result.get("dispatch").and_then(Value::as_array) else {
            return Ok(result);
        };
        self.dispatch_routines(dispatches).await;
        Ok(result)
    }

    /// Start routine assignments produced by either the periodic tick or
    /// `routine.test_run`.  Keeping this path shared makes manual runs obey
    /// the same idempotency and recovery rules as scheduled runs.
    async fn dispatch_routines(&self, dispatches: &[Value]) {
        let snapshot = self.inner.orchestrator.snapshot().ok();
        for dispatch in dispatches {
            let Some(run_id) = dispatch.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let bot_id = dispatch
                .get("bot_id")
                .and_then(Value::as_str)
                .unwrap_or("main")
                .to_owned();
            let model = dispatch
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    snapshot
                        .as_ref()
                        .and_then(|value| self.configured_model(value, &bot_id))
                })
                .filter(|value| !value.is_empty());
            let Some(model) = model else {
                let reason = "no model configured for routine Bot";
                tracing::error!(%run_id, %bot_id, "{reason}");
                if let Some(assignment_id) = dispatch.get("assignment_id").and_then(Value::as_str) {
                    block_missing_model(
                        self.inner.clone(),
                        self.state.clone(),
                        bot_id.clone(),
                        dispatch
                            .get("chat_id")
                            .and_then(Value::as_str)
                            .unwrap_or("chat_main")
                            .to_owned(),
                        assignment_id.to_owned(),
                        run_id,
                    )
                    .await;
                }
                self.mark_routine_failed(run_id, reason).await;
                continue;
            };
            // A scheduler tick may be retried after a transport failure.  The
            // routine run id is the stable execution key for that retry. Do
            // this after model validation so a missing model can be fixed and
            // retried on a later tick.
            if !self.scheduled.lock().await.insert(run_id.to_owned()) {
                continue;
            }
            let provider_id = model
                .split_once('/')
                .map(|(provider, _)| provider)
                .unwrap_or("")
                .to_owned();
            let assignment_id = dispatch
                .get("assignment_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let chat_id = dispatch
                .get("chat_id")
                .and_then(Value::as_str)
                .unwrap_or("chat_main")
                .to_owned();
            let instruction = dispatch
                .get("instruction")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let bot = snapshot.as_ref().and_then(|value| {
                value
                    .get("bots")
                    .and_then(Value::as_object)
                    .and_then(|bots| bots.get(&bot_id))
            });
            let is_main = bot
                .and_then(|value| value.get("is_main"))
                .and_then(Value::as_bool)
                .unwrap_or(bot_id == "main");
            let project_id = dispatch
                .get("project_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    assignment_id.as_ref().and_then(|id| {
                        snapshot
                            .as_ref()
                            .and_then(|value| value.get("assignments"))
                            .and_then(|value| value.get(id))
                            .and_then(|value| value.get("project_id"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                });
            let price = self.runtime.price_for_model(&model);
            let cwd = self.runtime.cwd_for(project_id.as_deref(), &bot_id);
            let request = ExecutionRequest {
                run_id: run_id.to_owned(),
                assignment_id,
                chat_id,
                bot_id,
                model,
                provider_id,
                project_id,
                instruction: instruction.clone(),
                messages: vec![json!({"role":"user","content":instruction})],
                max_turns: 16,
                private: false,
                allow_unsafe: false,
                cwd: Some(cwd),
                routine: true,
                price,
                resume_approved: false,
                subagent: false,
                tools: bot_tool_allowlist(bot, is_main, false, self.web_search_configured()),
                phase: Some(if is_main { "coordinate" } else { "work" }.into()),
                parent_run_id: None,
                subagent_task: None,
                save_full_requests: false,
                resume_message: None,
            };
            self.spawn_request(request);
        }
    }

    /// Resolve a Bot's effective model from the live durable settings.  The
    /// orchestrator snapshot intentionally contains only scheduling settings;
    /// provider/model defaults belong to `data/settings.json` and are read
    /// through the already-open Store handle so settings updates take effect
    /// without reopening the process lock.
    fn configured_model(&self, snapshot: &Value, bot_id: &str) -> Option<String> {
        let settings = self
            .inner
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .unwrap_or(Value::Null);
        resolve_model(snapshot, &settings, bot_id, ModelRole::Bot)
    }

    async fn mark_routine_failed(&self, run_id: &str, reason: &str) {
        if let Err(error) = self
            .inner
            .execution_finish_routine_run(&self.state, run_id, "failed", Some(reason.to_owned()))
            .await
        {
            tracing::warn!(%error, %run_id, "failed to persist routine failure");
        }
    }

    fn web_search_configured(&self) -> bool {
        self.inner
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .and_then(|settings| settings.get("web_search").cloned())
            .is_some_and(|web| {
                web.get("provider")
                    .and_then(Value::as_str)
                    .is_some_and(|provider| !provider.is_empty())
                    && web
                        .get("endpoint")
                        .and_then(Value::as_str)
                        .is_some_and(|endpoint| !endpoint.is_empty())
            })
    }

    /// Run the process-owned feature maintenance pass. Memory maintenance is
    /// deliberately driven by the gateway scheduler so there is one writer
    /// for the shared Store and one usage flush per process.
    pub async fn tick_features(&self) -> crate::RpcResult {
        let configure_error = self
            .runtime
            .configure_feature_runtime()
            .await
            .err()
            .map(|error| error.to_string());
        let snapshot = self
            .inner
            .orchestrator
            .snapshot()
            .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
        let mut active_bots = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|assignments| assignments.values())
            .filter(|assignment| {
                matches!(
                    assignment.get("status").and_then(Value::as_str),
                    Some("working" | "queued" | "waiting_user" | "waiting_bot" | "blocked")
                )
            })
            .filter_map(|assignment| assignment.get("bot_id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        {
            let durable = self.runtime.state.durable.lock().await;
            for job in durable.jobs() {
                if !matches!(
                    job.status,
                    macbot_durable::JobStatus::Queued | macbot_durable::JobStatus::Running
                ) {
                    continue;
                }
                let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                    continue;
                };
                if let Ok(Some(request)) = self
                    .runtime
                    .store
                    .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                {
                    active_bots.insert(request.bot_id);
                }
            }
        }
        let targets = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|bots| bots.values())
            .filter_map(|bot| bot.get("id").and_then(Value::as_str))
            .filter(|bot_id| !active_bots.contains(*bot_id))
            .map(MemoryTarget::bot)
            .collect::<Vec<_>>();
        let (committed, maintenance_error) = if let Some(error) = configure_error {
            (0, Some(error))
        } else {
            match self.feature_service.maintenance_tick(&targets).await {
                Ok(entries) => (entries.len(), None),
                Err(error) => (0, Some(error.to_string())),
            }
        };
        let flush_error = self
            .runtime
            .usage
            .lock()
            .await
            .flush_minute()
            .err()
            .map(|error| error.to_string());
        if let Some(error) = flush_error {
            return Err(crate::rpc_error("internal", &error, None));
        }
        match self
            .state
            .browser
            .lock()
            .await
            .close_idle(SystemTime::now())
        {
            Ok(closed) if !closed.is_empty() => {
                tracing::info!(count = closed.len(), "closed idle browser sessions")
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "failed to close idle browser sessions"),
        }
        Ok(
            json!({"maintenance_commits":committed,"maintenance_error":maintenance_error,"usage_flushed":true}),
        )
    }

    async fn request_for_chat(&self, params: &Value, result: &Value) -> Option<ExecutionRequest> {
        let chat_id = params.get("chat_id")?.as_str()?.to_owned();
        let instruction = params.get("text")?.as_str()?.to_owned();
        let requested_bot = params
            .get("mentions")
            .and_then(Value::as_array)
            .and_then(|mentions| {
                mentions.iter().find_map(|mention| {
                    (mention.get("kind").and_then(Value::as_str) == Some("bot"))
                        .then(|| mention.get("bot_id").and_then(Value::as_str))
                        .flatten()
                })
            })
            .unwrap_or("main");
        let snapshot = self.inner.orchestrator.snapshot().ok()?;
        let chat_bot = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| {
                bots.values().find(|bot| {
                    bot.get("dm_chat_id").and_then(Value::as_str) == Some(chat_id.as_str())
                })
            })
            .and_then(|bot| bot.get("id").and_then(Value::as_str));
        let bot_id = if chat_bot.is_some() && requested_bot == "main" {
            chat_bot.unwrap_or("main")
        } else if requested_bot == "main" {
            snapshot
                .get("bots")
                .and_then(Value::as_object)
                .and_then(|bots| {
                    bots.values()
                        .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                })
                .and_then(|bot| bot.get("id").and_then(Value::as_str))
                .unwrap_or("main")
        } else {
            requested_bot
        }
        .to_owned();
        let bot = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(&bot_id));
        let bot_is_main = bot
            .and_then(|value| value.get("is_main").and_then(Value::as_bool))
            .unwrap_or(bot_id == "main");
        let model = self.configured_model(&snapshot, &bot_id)?;
        let provider_id = model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or("")
            .to_owned();
        let message_id = result
            .get("message")
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str);
        let assignment = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .and_then(|assignments| {
                assignments.values().find(|assignment| {
                    message_id.is_some()
                        && assignment.get("trigger_message_id").and_then(Value::as_str)
                            == message_id
                        && assignment.get("bot_id").and_then(Value::as_str) == Some(&bot_id)
                })
            });
        let assignment_id = assignment
            .and_then(|assignment| assignment.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let chat = snapshot
            .get("projects")
            .and_then(Value::as_object)
            .and_then(|projects| {
                projects.values().find(|project| {
                    project.get("chat_id").and_then(Value::as_str) == Some(chat_id.as_str())
                })
            });
        let project_id = chat
            .and_then(|project| project.get("id").and_then(Value::as_str))
            .or_else(|| {
                assignment
                    .and_then(|assignment| assignment.get("project_id"))
                    .and_then(Value::as_str)
            })
            .map(str::to_owned);
        let chat_kind = if chat_id == "chat_main" {
            "main"
        } else if chat.is_some() {
            "project"
        } else {
            "direct"
        };
        let private = chat_kind != "project";
        let tools = bot_tool_allowlist(bot, bot_is_main, private, self.web_search_configured());
        let run_id = if private {
            message_id
                .map(|id| {
                    format!(
                        "run_chat_{}",
                        id.chars()
                            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                            .collect::<String>()
                    )
                })
                .unwrap_or_else(|| format!("run_{}", Uuid::now_v7()))
        } else {
            assignment_id
                .clone()
                .map(|id| format!("run_{id}"))
                .unwrap_or_else(|| format!("run_{}", Uuid::now_v7()))
        };
        let history = self
            .inner
            .call(
                "chat.history",
                json!({"chat_id": chat_id, "tail": 100, "limit": 100}),
                &self.state,
            )
            .await
            .ok();
        let mut messages =
            self.chat_model_messages(&snapshot, &chat_id, message_id, history.as_ref());
        messages.push(json!({"role":"user","content":instruction}));
        let price = self.runtime.price_for_model(&model);
        let cwd = self.runtime.cwd_for(project_id.as_deref(), &bot_id);
        Some(ExecutionRequest {
            run_id,
            assignment_id,
            chat_id,
            bot_id,
            model,
            provider_id,
            project_id,
            instruction,
            messages,
            max_turns: 16,
            private,
            allow_unsafe: false,
            cwd: Some(cwd),
            routine: false,
            price,
            resume_approved: false,
            subagent: false,
            tools,
            phase: None,
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
        })
    }

    /// Rebuild the provider conversation from the durable chat log. The
    /// orchestrator snapshot is a HashMap and does not carry the wire seq, so
    /// iterating values directly can put an old turn after the current user
    /// request. Merge the snapshot with the append-only chat log, deduplicate
    /// by message id, and use a stable `(created_at, seq, id)` ordering.
    ///
    /// `current_message_id` is excluded because `chat.send` has already
    /// persisted that user message; the caller appends it exactly once below.
    fn chat_model_messages(
        &self,
        snapshot: &Value,
        chat_id: &str,
        current_message_id: Option<&str>,
        history: Option<&Value>,
    ) -> Vec<Value> {
        let mut by_id = std::collections::BTreeMap::<String, Value>::new();
        if let Some(messages) = history
            .and_then(|value| value.get("messages"))
            .and_then(Value::as_array)
        {
            for message in messages {
                let id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !id.is_empty() {
                    by_id.insert(id, message.clone());
                }
            }
        } else if let Some(messages) = snapshot.get("messages").and_then(Value::as_object) {
            for message in messages
                .values()
                .filter(|message| message.get("chat_id").and_then(Value::as_str) == Some(chat_id))
            {
                let id = message
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !id.is_empty() {
                    by_id.insert(id, message.clone());
                }
            }
        }
        let mut messages = by_id
            .into_values()
            .filter(|message| {
                current_message_id.is_none_or(|current| {
                    message.get("id").and_then(Value::as_str) != Some(current)
                })
            })
            .filter_map(Self::chat_message_to_model)
            .collect::<Vec<_>>();
        messages.sort_by(|left, right| {
            let left_created = left
                .get("_created_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let right_created = right
                .get("_created_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            left_created
                .cmp(right_created)
                .then_with(|| {
                    left.get("_seq")
                        .and_then(Value::as_u64)
                        .cmp(&right.get("_seq").and_then(Value::as_u64))
                })
                .then_with(|| {
                    left.get("_id")
                        .and_then(Value::as_str)
                        .cmp(&right.get("_id").and_then(Value::as_str))
                })
        });
        for message in &mut messages {
            if let Some(object) = message.as_object_mut() {
                object.remove("_created_at");
                object.remove("_seq");
                object.remove("_id");
            }
        }
        messages
    }

    fn chat_message_to_model(message: Value) -> Option<Value> {
        let id = message.get("id").and_then(Value::as_str)?.to_owned();
        let created_at = message
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let seq = message.get("seq").and_then(Value::as_u64);
        let sender_kind = message
            .get("sender")
            .and_then(|sender| {
                sender
                    .as_str()
                    .or_else(|| sender.get("kind").and_then(Value::as_str))
            })
            .unwrap_or("bot");
        let role =
            message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or(if sender_kind == "user" {
                    "user"
                } else {
                    "assistant"
                });
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .or_else(|| message.get("text").and_then(Value::as_str))
            .or_else(|| message.get("fallback_text").and_then(Value::as_str))
            .unwrap_or_default();
        if content.is_empty() && message.get("tool_calls").is_none() {
            return None;
        }
        let mut model = json!({
            "role": role,
            "content": content,
            "_created_at": created_at,
            "_seq": seq,
            "_id": id,
        });
        for key in [
            "tool_calls",
            "tool_call_id",
            "name",
            "is_error",
            "assistant_content",
        ] {
            if let Some(value) = message.get(key) {
                model[key] = value.clone();
            }
        }
        Some(model)
    }

    /// Build a durable request for an assignment admitted by the orchestrator.
    /// Assignment creation and queue promotion are separate from execution;
    /// only assignments already marked `working` may reach this function's
    /// caller.  This is also the path used for Bot-to-Bot handoffs, whose
    /// originating RPC is an internal `send_msg` rather than `chat.send`.
    fn request_for_assignment(&self, assignment: &Value) -> Option<ExecutionRequest> {
        if assignment.get("status").and_then(Value::as_str) != Some("working") {
            return None;
        }
        let assignment_id = assignment.get("id").and_then(Value::as_str)?.to_owned();
        let chat_id = assignment
            .get("origin_chat_id")
            .and_then(Value::as_str)
            .unwrap_or("chat_main")
            .to_owned();
        let bot_id = assignment.get("bot_id").and_then(Value::as_str)?.to_owned();
        let instruction = assignment
            .get("instruction")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let snapshot = self.inner.orchestrator.snapshot().ok()?;
        let bot = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(&bot_id));
        let is_main = bot
            .and_then(|value| value.get("is_main"))
            .and_then(Value::as_bool)
            .unwrap_or(bot_id == "main");
        let model = assignment
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| self.configured_model(&snapshot, &bot_id))?;
        let provider_id = model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or("")
            .to_owned();
        let project_id = assignment
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let private = chat_id == "chat_main" || chat_id.starts_with("dm_");
        let mut messages = self.chat_model_messages(&snapshot, &chat_id, None, None);
        messages.push(json!({"role":"user","content":instruction}));
        let run_id = format!("run_{assignment_id}");
        let price = self.runtime.price_for_model(&model);
        let cwd = self.runtime.cwd_for(project_id.as_deref(), &bot_id);
        Some(ExecutionRequest {
            run_id,
            assignment_id: Some(assignment_id),
            chat_id,
            bot_id,
            model,
            provider_id,
            project_id,
            instruction,
            messages,
            max_turns: 16,
            private,
            allow_unsafe: false,
            cwd: Some(cwd),
            routine: false,
            price,
            resume_approved: false,
            subagent: false,
            tools: bot_tool_allowlist(bot, is_main, private, self.web_search_configured()),
            phase: Some(if is_main { "coordinate" } else { "work" }.into()),
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
        })
    }

    /// Dispatch all currently admitted assignments.  The orchestrator remains
    /// the source of truth for concurrency and queue promotion; this method
    /// only starts `working` assignments and uses `active_runs` to prevent a
    /// duplicate spawn during overlapping RPC/event callbacks.
    async fn dispatch_ready_assignments(&self) {
        let assignments = match self.inner.orchestrator.snapshot() {
            Ok(snapshot) => snapshot
                .get("assignments")
                .and_then(Value::as_object)
                .map(|items| items.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default(),
            Err(error) => {
                tracing::warn!(%error, "cannot inspect assignments for dispatch");
                return;
            }
        };
        for assignment in assignments {
            let Some(request) = self.request_for_assignment(&assignment) else {
                if assignment.get("status").and_then(Value::as_str) == Some("working") {
                    if let Some(assignment_id) = assignment.get("id").and_then(Value::as_str) {
                        tracing::error!(%assignment_id, "working assignment has no configured model; failing it");
                        block_missing_model(
                            self.inner.clone(),
                            self.state.clone(),
                            assignment
                                .get("bot_id")
                                .and_then(Value::as_str)
                                .unwrap_or("main")
                                .to_owned(),
                            assignment
                                .get("origin_chat_id")
                                .and_then(Value::as_str)
                                .unwrap_or("chat_main")
                                .to_owned(),
                            assignment_id.to_owned(),
                            &format!("model-missing-{assignment_id}"),
                        )
                        .await;
                    }
                }
                continue;
            };
            let key = request.assignment_id.as_deref().unwrap_or(&request.run_id);
            if self
                .waiting_runs
                .lock()
                .map(|waiting| waiting.contains(key))
                .unwrap_or(false)
            {
                continue;
            }
            self.spawn_request(request);
        }
    }

    fn mark_waiting(&self, assignment_id: Option<&str>, run_id: &str, waiting: bool) {
        let key = assignment_id.unwrap_or(run_id);
        if let Ok(mut runs) = self.waiting_runs.lock() {
            if waiting {
                runs.insert(key.to_owned());
            } else {
                runs.remove(key);
            }
        }
    }

    fn assignment_is_working(&self, assignment_id: &str) -> bool {
        self.inner
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("assignments")
                    .and_then(Value::as_object)
                    .and_then(|items| items.get(assignment_id))
                    .and_then(|assignment| assignment.get("status"))
                    .and_then(Value::as_str)
                    .map(|status| status == "working")
            })
            .unwrap_or(false)
    }

    fn trace_history(&self, params: &Value) -> crate::RpcResult {
        let scope = params
            .get("assignment_id")
            .or_else(|| params.get("chat_id"))
            .and_then(Value::as_str)
            .filter(|value| is_safe_component(value))
            .ok_or_else(|| {
                crate::rpc_error(
                    "invalid_params",
                    "assignment_id or chat_id is required",
                    None,
                )
            })?;
        let mut items = self
            .inner
            .store
            .read_jsonl::<Value>(format!("data/traces/{scope}.jsonl"))
            .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
        let before = params.get("before_aseq").and_then(Value::as_u64);
        let after = params.get("after_aseq").and_then(Value::as_u64);
        items.retain(|item| {
            let aseq = item.get("aseq").and_then(Value::as_u64).unwrap_or(0);
            before.is_none_or(|value| aseq < value) && after.is_none_or(|value| aseq > value)
        });
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .clamp(1, 500) as usize;
        let has_more = items.len() > limit;
        if params.get("tail").and_then(Value::as_bool) == Some(true) {
            if items.len() > limit {
                items = items.split_off(items.len() - limit);
            }
        } else {
            items.truncate(limit);
        }
        let first_aseq = items.first().and_then(|item| item.get("aseq")).cloned();
        let last_aseq = items.last().and_then(|item| item.get("aseq")).cloned();
        Ok(json!({
            "items": items,
            "first_aseq": first_aseq,
            "last_aseq": last_aseq,
            "has_more_before": has_more,
            "live": false
        }))
    }

    fn spawn_request(&self, request: ExecutionRequest) {
        let run_id = request.run_id.clone();
        let active_key = request
            .assignment_id
            .clone()
            .unwrap_or_else(|| run_id.clone());
        let active_runs = self.active_runs.clone();
        let Ok(mut active) = active_runs.lock() else {
            tracing::warn!(%run_id, "execution scheduler lock poisoned");
            return;
        };
        if !active.insert(active_key.clone()) {
            return;
        }
        drop(active);
        let runtime = self.runtime.clone();
        let inner = self.inner.clone();
        let state = self.state.clone();
        let scheduler = self.clone();
        tokio::spawn(async move {
            let assignment_id = request.assignment_id.clone();
            if assignment_id
                .as_deref()
                .is_some_and(|id| !scheduler.assignment_is_working(id))
            {
                if let Ok(mut active) = active_runs.lock() {
                    active.remove(&active_key);
                }
                scheduler.dispatch_ready_assignments().await;
                return;
            }
            if let Err(error) = runtime.configure_browser_for_request(&request).await {
                tracing::warn!(bot_id = %request.bot_id, %error, "failed to apply Bot browser configuration");
            }
            match runtime.run(request).await {
                Ok(outcome) if outcome.status == "done" => {
                    scheduler.mark_waiting(assignment_id.as_deref(), &run_id, false);
                    if let Some(assignment_id) = assignment_id {
                        reconcile_steers(&runtime, &inner, &state, &assignment_id).await;
                        finish_assignment(inner, state, assignment_id).await;
                    }
                }
                Ok(outcome) if matches!(outcome.status.as_str(), "failed" | "cancelled") => {
                    scheduler.mark_waiting(assignment_id.as_deref(), &run_id, false);
                    if let Some(assignment_id) = assignment_id {
                        fail_assignment(inner, state, assignment_id, &outcome.status).await;
                    }
                }
                Ok(outcome) => {
                    scheduler.mark_waiting(
                        assignment_id.as_deref(),
                        &outcome.run_id,
                        matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended"),
                    );
                }
                Err(error) => {
                    tracing::error!(%error, "chat execution failed");
                    scheduler.mark_waiting(assignment_id.as_deref(), &run_id, false);
                    if let Some(assignment_id) = assignment_id {
                        fail_assignment(inner, state, assignment_id, "failed").await;
                    }
                }
            }
            if let Ok(mut active) = active_runs.lock() {
                active.remove(&active_key);
            }
            scheduler.dispatch_ready_assignments().await;
        });
    }
}

async fn enqueue_steer_runtime(runtime: RuntimeExecution, result: &Value, params: &Value) {
    let message_id = result
        .get("steer")
        .and_then(|value| value.get("message_id"))
        .and_then(Value::as_str)
        .or_else(|| result.get("message_id").and_then(Value::as_str))
        .or_else(|| params.get("message_id").and_then(Value::as_str))
        .map(str::to_owned);
    let assignment_id = result
        .get("steer")
        .and_then(|value| value.get("assignment_id"))
        .and_then(Value::as_str)
        .or_else(|| result.get("assignment_id").and_then(Value::as_str))
        .map(str::to_owned);
    let Some((message_id, assignment_id)) = message_id.zip(assignment_id) else {
        return;
    };
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    for _ in 0..40 {
        if runtime
            .enqueue_steer_for_assignment(&assignment_id, &message_id, &text)
            .await
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tracing::warn!(%assignment_id, %message_id, "steer could not find a durable execution job");
}

async fn block_missing_model(
    inner: Arc<ProductionBackend>,
    state: GatewayState,
    bot_id: String,
    chat_id: String,
    assignment_id: String,
    run_id: &str,
) {
    let chat_id = routable_missing_model_chat(&inner, &bot_id, &chat_id, &assignment_id);
    let message = json!({
        "bot_id": bot_id,
        "chat_id": chat_id,
        "assignment_id": assignment_id,
        "text": "未配置默认模型",
        "intent": "blocked",
        "mentions": []
    });
    let envelope = json!({
        "message": message,
        "receipt": {"run_id": run_id, "call_id": "model-missing"}
    });
    if let Err(error) = inner.execution_send_msg(&state, envelope).await {
        tracing::warn!(%error, %run_id, "failed to persist missing-model notification");
    }
}

fn missing_model_bot_from_snapshot(
    snapshot: &Value,
    chat_id: &str,
    requested_bot: Option<&str>,
) -> String {
    if let Some(bot_id) = snapshot
        .get("bots")
        .and_then(Value::as_object)
        .and_then(|bots| {
            bots.values()
                .find(|bot| bot.get("dm_chat_id").and_then(Value::as_str) == Some(chat_id))
        })
        .and_then(|bot| bot.get("id").and_then(Value::as_str))
    {
        return bot_id.to_owned();
    }
    if chat_id == "chat_main" {
        if let Some(bot_id) = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| {
                bots.values()
                    .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
            })
            .and_then(|bot| bot.get("id").and_then(Value::as_str))
        {
            return bot_id.to_owned();
        }
    }
    requested_bot
        .filter(|bot_id| !bot_id.is_empty() && *bot_id != "main")
        .or_else(|| {
            snapshot
                .get("bots")
                .and_then(Value::as_object)
                .and_then(|bots| {
                    bots.values()
                        .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                })
                .and_then(|bot| bot.get("id").and_then(Value::as_str))
        })
        .unwrap_or("main")
        .to_owned()
}

async fn notify_missing_model_chat(
    inner: Arc<ProductionBackend>,
    state: GatewayState,
    bot_id: String,
    chat_id: String,
    user_message_id: &str,
) {
    let receipt_id = format!("model-missing-{user_message_id}");
    let message = json!({
        "bot_id": bot_id,
        "chat_id": chat_id,
        "assignment_id": null,
        "text": "未配置默认模型",
        "intent": "blocked",
        "mentions": []
    });
    let envelope = json!({
        "message": message,
        "receipt": {"run_id": receipt_id, "call_id": "model-missing"}
    });
    if let Err(error) = inner.execution_send_msg(&state, envelope).await {
        tracing::warn!(%error, %user_message_id, "failed to persist missing-model DM notification");
    }
}

/// Routine dispatch normally resolves to a real project chat or Bot DM.
/// A synthetic `routine:<id>` value can remain only as a legacy defensive
/// fallback; missing-model notices should otherwise land in a real chat so a
/// user can see the terminal blocked state.
fn routable_missing_model_chat(
    inner: &ProductionBackend,
    bot_id: &str,
    requested_chat_id: &str,
    assignment_id: &str,
) -> String {
    let Ok(snapshot) = inner.orchestrator.snapshot() else {
        return requested_chat_id.to_owned();
    };
    routable_missing_model_chat_from_snapshot(&snapshot, bot_id, requested_chat_id, assignment_id)
}

fn routable_missing_model_chat_from_snapshot(
    snapshot: &Value,
    bot_id: &str,
    requested_chat_id: &str,
    assignment_id: &str,
) -> String {
    let is_real_chat = requested_chat_id == "chat_main"
        || snapshot
            .get("bots")
            .and_then(Value::as_object)
            .is_some_and(|bots| {
                bots.values().any(|bot| {
                    bot.get("dm_chat_id").and_then(Value::as_str) == Some(requested_chat_id)
                })
            })
        || snapshot
            .get("projects")
            .and_then(Value::as_object)
            .is_some_and(|projects| {
                projects.values().any(|project| {
                    project.get("chat_id").and_then(Value::as_str) == Some(requested_chat_id)
                })
            });
    if is_real_chat {
        return requested_chat_id.to_owned();
    }
    let project_chat = snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| assignments.get(assignment_id))
        .and_then(|assignment| assignment.get("project_id"))
        .and_then(Value::as_str)
        .and_then(|project_id| snapshot.get("projects")?.get(project_id))
        .and_then(|project| project.get("chat_id"))
        .and_then(Value::as_str);
    if let Some(chat_id) = project_chat {
        return chat_id.to_owned();
    }
    snapshot
        .get("bots")
        .and_then(Value::as_object)
        .and_then(|bots| bots.get(bot_id))
        .and_then(|bot| bot.get("dm_chat_id"))
        .and_then(Value::as_str)
        .or_else(|| {
            snapshot
                .get("bots")
                .and_then(Value::as_object)
                .and_then(|bots| {
                    bots.values()
                        .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                })
                .and_then(|bot| bot.get("dm_chat_id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("chat_main")
        .to_owned()
}

async fn finish_assignment(
    inner: Arc<ProductionBackend>,
    state: GatewayState,
    assignment_id: String,
) {
    finish_assignment_with_status(inner, state, assignment_id, "done").await;
}

async fn fail_assignment(
    inner: Arc<ProductionBackend>,
    state: GatewayState,
    assignment_id: String,
    status: &str,
) {
    finish_assignment_with_status(inner, state, assignment_id, status).await;
}

async fn finish_assignment_with_status(
    inner: Arc<ProductionBackend>,
    state: GatewayState,
    assignment_id: String,
    status: &str,
) {
    if let Ok(assignment) = inner.orchestrator.finish_assignment(&assignment_id, status) {
        if let Ok(snapshot) = inner.orchestrator.snapshot() {
            let _ = inner.store.append_jsonl(
                "data/orchestrator/operations.jsonl",
                &json!({"method":"execution.finish","params":{"assignment_id":assignment_id},"result":{"assignment":assignment},"snapshot":snapshot,"status":status,"at":crate::now()}),
            );
            let _ = inner
                .store
                .write_snapshot("data/orchestrator/state.json", &snapshot);
        }
        if let Ok(event) = inner
            .store
            .append_event("assignment.updated", json!({"assignment":assignment}))
        {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        mark_routine_run_for_assignment(&inner, &state, &assignment_id, status).await;
    }
}

async fn mark_routine_run_for_assignment(
    inner: &Arc<ProductionBackend>,
    state: &GatewayState,
    assignment_id: &str,
    assignment_status: &str,
) {
    let status = if assignment_status == "done" {
        "done"
    } else {
        "failed"
    };
    let error = (status == "failed").then(|| "execution failed".to_owned());
    if let Err(error) = inner
        .execution_finish_routine_run(state, assignment_id, status, error)
        .await
    {
        tracing::warn!(%error, %assignment_id, "failed to persist routine run completion");
    }
}

async fn reconcile_steers(
    runtime: &RuntimeExecution,
    inner: &Arc<ProductionBackend>,
    state: &GatewayState,
    assignment_id: &str,
) {
    let applied = runtime.applied_steers(assignment_id).await;
    if applied.is_empty() {
        return;
    }
    for message_id in &applied {
        let _ = inner.orchestrator.mark_steer_read(message_id);
    }
    if let Ok(snapshot) = inner.orchestrator.snapshot() {
        let _ = inner.store.append_jsonl(
            "data/orchestrator/operations.jsonl",
            &json!({"method":"execution.steer.read","params":{"assignment_id":assignment_id},"result":{"assignment_id":assignment_id,"message_ids":applied},"snapshot":snapshot,"status":"done","at":crate::now()}),
        );
        let _ = inner
            .store
            .write_snapshot("data/orchestrator/state.json", &snapshot);
        if let Some(assignment) = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .and_then(|items| items.get(assignment_id))
        {
            if let Ok(event) = inner
                .store
                .append_event("assignment.updated", json!({"assignment":assignment}))
            {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
        }
    }
}

/// Stop all non-terminal assignments affected by a project lifecycle
/// mutation before the orchestrator changes the project/member state.
/// Queued assignments have no engine job; working and waiting assignments
/// must cancel their durable run first so a late model result cannot
/// publish after the project has been closed or the Bot removed.
async fn cancel_project_assignments(
    backend: &ComposedBackend,
    project_id: &str,
    bot_id: Option<&str>,
) -> crate::RpcResult {
    let snapshot = backend
        .inner
        .orchestrator
        .snapshot()
        .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
    let assignments = project_assignment_targets(&snapshot, project_id, bot_id);

    for (assignment_id, _initial_status) in assignments {
        // Stopping queued work can pump the scheduler and promote another
        // assignment. Re-read the durable status before cancelling so that a
        // target captured as queued is cancelled if it is now running.
        let status = match assignment_status(backend, &assignment_id) {
            Ok(status)
                if matches!(
                    status.as_str(),
                    "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
                ) =>
            {
                status
            }
            Ok(_) => continue,
            Err(error) if error.code.as_str() == "not_found" => continue,
            Err(error) => return Err(error),
        };
        cancel_assignment_engine(backend, &assignment_id, &status).await?;
        let stop_params = json!({
            "assignment_id": assignment_id,
            "client_request_id": format!("project-stop:{project_id}:{assignment_id}")
        });
        backend
            .inner
            .call("assignment.stop", stop_params, &backend.state)
            .await?;
    }
    Ok(json!({}))
}

/// Cancel the engine job for one assignment before the orchestrator mutates
/// its durable assignment state.  Queued work has no engine job and must go
/// straight through the orchestrator; terminal work is already settled.
async fn cancel_assignment_engine(
    backend: &ComposedBackend,
    assignment_id: &str,
    status: &str,
) -> crate::RpcResult {
    if status == "queued" || matches!(status, "done" | "failed" | "cancelled") {
        return Ok(json!({}));
    }
    let outcome = backend
        .runtime
        .cancel_assignment(assignment_id)
        .await
        .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
    if outcome.is_none() && status != "blocked" {
        return Err(crate::rpc_error(
            "conflict",
            "assignment has no running execution to cancel",
            Some(json!({"assignment_id": assignment_id})),
        ));
    }
    if outcome.is_none() && status == "blocked" {
        // A blocked assignment may be a persisted user-facing decision that
        // never acquired an engine job; assignment.stop still settles it.
        return Ok(json!({}));
    }
    Ok(json!({}))
}

fn project_assignment_targets(
    snapshot: &Value,
    project_id: &str,
    bot_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut targets = snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|items| items.values())
        .filter(|assignment| {
            assignment.get("project_id").and_then(Value::as_str) == Some(project_id)
                && bot_id
                    .is_none_or(|id| assignment.get("bot_id").and_then(Value::as_str) == Some(id))
                && matches!(
                    assignment.get("status").and_then(Value::as_str),
                    Some("queued")
                        | Some("working")
                        | Some("waiting_user")
                        | Some("waiting_bot")
                        | Some("blocked")
                )
        })
        .filter_map(|assignment| {
            Some((
                assignment.get("id")?.as_str()?.to_owned(),
                assignment.get("status")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    targets.sort_by(|(left_id, left_status), (right_id, right_status)| {
        cancellation_priority(left_status)
            .cmp(&cancellation_priority(right_status))
            .then_with(|| left_id.cmp(right_id))
    });
    targets
}

fn cancellation_priority(status: &str) -> u8 {
    match status {
        "queued" => 0,
        "working" => 1,
        "waiting_user" | "waiting_bot" => 2,
        "blocked" => 3,
        _ => 4,
    }
}

fn snapshot_value(backend: &ComposedBackend) -> Result<Value, crate::RpcError> {
    backend
        .inner
        .orchestrator
        .snapshot()
        .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))
}

fn required_nonempty_param(params: &Value, key: &str) -> Result<String, crate::RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| crate::rpc_error("invalid_params", &format!("{key} is required"), None))
}

fn validate_project_lifecycle(
    backend: &ComposedBackend,
    method: &str,
    params: &Value,
) -> Result<(String, Option<String>), crate::RpcError> {
    let project_id = required_nonempty_param(params, "project_id")?;
    let snapshot = snapshot_value(backend)?;
    let project = snapshot
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| projects.get(&project_id))
        .ok_or_else(|| {
            crate::rpc_error(
                "not_found",
                "project not found",
                Some(json!({"project_id": project_id})),
            )
        })?;
    match method {
        "project.remove_member" => {
            let bot_id = required_nonempty_param(params, "bot_id")?;
            let member = project
                .get("members")
                .and_then(Value::as_array)
                .is_some_and(|members| {
                    members.iter().any(|member| {
                        member.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str())
                    })
                });
            if !member {
                return Err(crate::rpc_error(
                    "conflict",
                    "bot is not a project member",
                    Some(json!({"project_id": project_id, "bot_id": bot_id})),
                ));
            }
            Ok((project_id, Some(bot_id)))
        }
        "project.confirm_done" => {
            let status = project.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(status, "active" | "review") {
                return Err(crate::rpc_error(
                    "conflict",
                    "project must be active or in review",
                    Some(json!({"project_id": project_id, "status": status})),
                ));
            }
            Ok((project_id, None))
        }
        _ => Err(crate::rpc_error(
            "internal",
            "invalid project lifecycle method",
            None,
        )),
    }
}

fn assignment_status(
    backend: &ComposedBackend,
    assignment_id: &str,
) -> Result<String, crate::RpcError> {
    let snapshot = snapshot_value(backend)?;
    snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| assignments.get(assignment_id))
        .and_then(|assignment| assignment.get("status"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            crate::rpc_error(
                "not_found",
                "assignment not found",
                Some(json!({"assignment_id": assignment_id})),
            )
        })
}

fn approval_assignment_id(
    backend: &ComposedBackend,
    approval_id: &str,
) -> Result<Option<String>, crate::RpcError> {
    let snapshot = snapshot_value(backend)?;
    Ok(snapshot
        .get("approvals")
        .and_then(Value::as_object)
        .and_then(|approvals| approvals.get(approval_id))
        .and_then(|approval| approval.get("assignment_id"))
        .and_then(Value::as_str)
        .map(str::to_owned))
}

#[async_trait]
impl crate::RpcBackend for ComposedBackend {
    async fn export_usage_csv(
        &self,
        params: &Value,
        timezone: &str,
    ) -> Result<String, crate::RpcError> {
        self.inner.export_usage_csv(params, timezone).await
    }

    async fn call(&self, method: &str, params: Value, state: &GatewayState) -> crate::RpcResult {
        if method == "trace.history" {
            return self.trace_history(&params);
        }
        if method.starts_with("skill.") {
            if method == "skill.create_draft" {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| crate::RpcError {
                        code: "invalid_params".into(),
                        message: "name is required".into(),
                        details: None,
                    })?;
                let content = params
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| crate::RpcError {
                        code: "invalid_params".into(),
                        message: "content is required".into(),
                        details: None,
                    })?;
                let skill = self
                    .feature_service
                    .create_skill_draft(name, content)
                    .map_err(|error| crate::RpcError {
                        code: error.code().into(),
                        message: error.to_string(),
                        details: None,
                    })?;
                return Ok(json!({"skill": skill}));
            }
            let response = self
                .feature_service
                .skill_rpc(method, params)
                .map_err(|error| crate::RpcError {
                    code: error.code().into(),
                    message: error.to_string(),
                    details: None,
                })?;
            for event in response.events {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
            return Ok(response.result);
        }
        if method == "search" {
            return self
                .feature_service
                .search_rpc(params)
                .map_err(|error| crate::RpcError {
                    code: error.code().into(),
                    message: error.to_string(),
                    details: None,
                });
        }
        if method == "chat.send" {
            self.runtime
                .configure_feature_runtime()
                .await
                .map_err(|error| crate::RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
        }
        // Public takeover actions have an empty protocol result.  The
        // runtime still needs the durable request snapshot to resume the
        // waiting run, so use the typed internal bridge for those two calls
        // and strip the private payload before returning to the client.
        let private_takeover_action = matches!(method, "takeover.start" | "takeover.release");
        if method == "assignment.stop" {
            let assignment_id = required_nonempty_param(&params, "assignment_id")?;
            let status = assignment_status(self, &assignment_id)?;
            if matches!(status.as_str(), "done" | "failed" | "cancelled") {
                let snapshot = snapshot_value(self)?;
                let assignment = snapshot
                    .get("assignments")
                    .and_then(Value::as_object)
                    .and_then(|assignments| assignments.get(&assignment_id))
                    .cloned()
                    .unwrap_or(Value::Null);
                return Ok(json!({"assignment": assignment}));
            }
            cancel_assignment_engine(self, &assignment_id, &status).await?;
        }
        if method == "project.remove_member" {
            let (project_id, bot_id) = validate_project_lifecycle(self, method, &params)?;
            cancel_project_assignments(self, &project_id, bot_id.as_deref()).await?;
        } else if method == "project.confirm_done" {
            let (project_id, _) = validate_project_lifecycle(self, method, &params)?;
            cancel_project_assignments(self, &project_id, None).await?;
        }
        if method == "approval.decide"
            && matches!(
                params.get("decision").and_then(Value::as_str),
                Some("deny" | "reject" | "decline")
            )
        {
            if let Some(approval_id) = params.get("approval_id").and_then(Value::as_str) {
                if let Some(assignment_id) = approval_assignment_id(self, approval_id)? {
                    let status = assignment_status(self, &assignment_id)?;
                    if !matches!(status.as_str(), "done" | "failed" | "cancelled") {
                        cancel_assignment_engine(self, &assignment_id, &status).await?;
                        self.inner
                            .call(
                                "assignment.stop",
                                json!({
                                    "assignment_id": assignment_id,
                                    "client_request_id": format!("approval-deny:{approval_id}")
                                }),
                                &self.state,
                            )
                            .await?;
                    }
                }
            }
        }
        let inner_result = if method == "takeover.start" {
            self.inner.execution_takeover_start(state, &params).await
        } else if method == "takeover.release" {
            self.inner.execution_takeover_release(state, &params).await
        } else {
            self.inner.call(method, params.clone(), state).await
        };
        let mut result = match inner_result {
            Ok(result) => result,
            Err(error) if method == "question.answer" && error.code == "not_found" => {
                let question_id = params
                    .get("question_id")
                    .and_then(Value::as_str)
                    .ok_or(error.clone())?;
                let marker = self
                    .inner
                    .store
                    .read_snapshot::<Value>(format!(
                        "data/waiting/{}.json",
                        safe_component(question_id)
                    ))
                    .map_err(|store_error| crate::RpcError {
                        code: "internal".into(),
                        message: store_error.to_string(),
                        details: None,
                    })?
                    .filter(|marker| marker.get("kind").and_then(Value::as_str) == Some("question"))
                    .ok_or(error)?;
                let answer_text = params.get("text").cloned().unwrap_or(Value::Null);
                let answer_option = params.get("option_index").cloned().unwrap_or(Value::Null);
                json!({
                    "question": {
                        "id": question_id,
                        "assignment_id": marker.get("assignment_id").cloned().unwrap_or(Value::Null),
                        "chat_id": marker.get("chat_id").cloned().unwrap_or(Value::Null),
                        "text": marker.get("text").cloned().unwrap_or(Value::Null),
                        "options": [],
                        "allow_free_text": true,
                        "state": "answered",
                        "answer": {"text": answer_text, "option_index": answer_option}
                    }
                })
            }
            Err(error) => return Err(error),
        };
        if method == "routine.test_run" {
            let dispatches = result
                .get("dispatch")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            self.dispatch_routines(&dispatches).await;
            if let Some(object) = result.as_object_mut() {
                object.remove("dispatch");
            }
        }
        if method == "settings.update" {
            self.runtime
                .configure_feature_runtime()
                .await
                .map_err(|error| crate::RpcError {
                    code: "internal".into(),
                    message: error.to_string(),
                    details: None,
                })?;
        }
        let mut resumed_waiting_message = false;
        if method == "chat.send" {
            let reply_to = params
                .get("reply_to")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    result
                        .get("message")
                        .and_then(|message| message.get("reply_to"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let assignment_id = params
                .get("assignment_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    reply_to.as_deref().and_then(|reply_to| {
                        self.inner
                            .orchestrator
                            .snapshot()
                            .ok()
                            .and_then(|snapshot| {
                                snapshot
                                    .get("assignments")
                                    .and_then(Value::as_object)
                                    .and_then(|assignments| {
                                        assignments.values().find_map(|assignment| {
                                            (assignment
                                                .get("result_message_id")
                                                .and_then(Value::as_str)
                                                == Some(reply_to)
                                                && matches!(
                                                    assignment
                                                        .get("status")
                                                        .and_then(Value::as_str),
                                                    Some(
                                                        "waiting_user" | "waiting_bot" | "blocked"
                                                    )
                                                ))
                                            .then(|| {
                                                assignment
                                                    .get("id")
                                                    .and_then(Value::as_str)
                                                    .map(str::to_owned)
                                            })
                                            .flatten()
                                        })
                                    })
                            })
                    })
                });
            if let (Some(assignment_id), Some(text)) = (
                assignment_id,
                params
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) {
                let runtime = self.runtime.clone();
                let inner = self.inner.clone();
                let state = self.state.clone();
                let scheduler = self.clone();
                self.mark_waiting(Some(&assignment_id), "", false);
                resumed_waiting_message = runtime
                    .resume_message(&assignment_id, text)
                    .await
                    .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?
                    .map(|(assignment, outcome)| {
                        scheduler.mark_waiting(
                            assignment.as_deref(),
                            &outcome.run_id,
                            matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended"),
                        );
                        if outcome.status == "done" {
                            if let Some(assignment) = assignment {
                                let inner = inner.clone();
                                let state = state.clone();
                                tokio::spawn(async move {
                                    finish_assignment(inner, state, assignment).await;
                                    scheduler.dispatch_ready_assignments().await;
                                });
                            }
                        }
                        true
                    })
                    .unwrap_or(false);
            }
            let schedule_key = params
                .get("client_request_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    result
                        .get("message")
                        .and_then(|message| message.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let fresh = if let Some(key) = schedule_key {
                self.scheduled.lock().await.insert(key)
            } else {
                true
            };
            if fresh && !resumed_waiting_message {
                if let Some(request) = self.request_for_chat(&params, &result).await {
                    if request
                        .assignment_id
                        .as_deref()
                        .is_none_or(|id| self.assignment_is_working(id))
                    {
                        self.spawn_request(request);
                    }
                } else if let Some(message_id) = result
                    .get("message")
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str)
                {
                    // chat.send has already admitted the assignment.  Do not
                    // leave it permanently working when settings contain no
                    // model for the selected Bot.
                    let snapshot = self.inner.orchestrator.snapshot().ok();
                    let assignment = snapshot.as_ref().and_then(|snapshot| {
                        snapshot
                            .get("assignments")
                            .and_then(Value::as_object)
                            .and_then(|assignments| {
                                assignments.values().find(|assignment| {
                                    assignment.get("trigger_message_id").and_then(Value::as_str)
                                        == Some(message_id)
                                        && assignment.get("status").and_then(Value::as_str)
                                            == Some("working")
                                })
                            })
                    });
                    if let Some(assignment) = assignment {
                        let assignment_id = assignment
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let assignment_bot_id = assignment
                            .get("bot_id")
                            .and_then(Value::as_str)
                            .unwrap_or("main")
                            .to_owned();
                        let assignment_chat_id = assignment
                            .get("origin_chat_id")
                            .and_then(Value::as_str)
                            .unwrap_or("chat_main")
                            .to_owned();
                        tracing::error!(%assignment_id, "chat assignment has no configured model; failing it");
                        block_missing_model(
                            self.inner.clone(),
                            self.state.clone(),
                            assignment_bot_id,
                            assignment_chat_id,
                            assignment_id,
                            &format!("model-missing-{message_id}"),
                        )
                        .await;
                    } else {
                        let chat_id = params
                            .get("chat_id")
                            .and_then(Value::as_str)
                            .unwrap_or("chat_main")
                            .to_owned();
                        let requested_bot = params.get("bot_id").and_then(Value::as_str);
                        let bot_id = snapshot
                            .as_ref()
                            .map(|snapshot| {
                                missing_model_bot_from_snapshot(snapshot, &chat_id, requested_bot)
                            })
                            .unwrap_or_else(|| requested_bot.unwrap_or("main").to_owned());
                        tracing::error!(%message_id, %bot_id, "chat request has no configured model");
                        notify_missing_model_chat(
                            self.inner.clone(),
                            self.state.clone(),
                            bot_id,
                            chat_id,
                            message_id,
                        )
                        .await;
                    }
                }
            }
        }
        if matches!(method, "assignment.steer" | "steer") {
            let runtime = self.runtime.clone();
            let params_for_task = params.clone();
            let result_for_task = result.clone();
            tokio::spawn(async move {
                enqueue_steer_runtime(runtime, &result_for_task, &params_for_task).await;
            });
        }
        if method == "approval.decide"
            && matches!(
                params.get("decision").and_then(Value::as_str),
                Some("allow_once" | "always_allow")
            )
        {
            if let Some(approval_id) = params
                .get("approval_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let runtime = self.runtime.clone();
                let inner = self.inner.clone();
                let state = self.state.clone();
                let scheduler = self.clone();
                tokio::spawn(async move {
                    match runtime.resume_approved(&approval_id).await {
                        Ok(Some((assignment_id, outcome))) => {
                            scheduler.mark_waiting(
                                assignment_id.as_deref(),
                                &outcome.run_id,
                                matches!(
                                    outcome.status.as_str(),
                                    "waiting" | "blocked" | "suspended"
                                ),
                            );
                            if outcome.status == "done" {
                                if let Some(assignment_id) = assignment_id {
                                    finish_assignment(inner, state, assignment_id).await;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => tracing::error!(%error, "approval continuation failed"),
                    }
                    scheduler.dispatch_ready_assignments().await;
                });
            }
        }
        if method == "question.answer" {
            if let Some(question) = result.get("question") {
                let question_id = question
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let answer = question
                    .get("answer")
                    .and_then(|answer| answer.get("text"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        question
                            .get("answer")
                            .and_then(|answer| answer.get("option_index"))
                            .and_then(Value::as_u64)
                            .map(|index| format!("选项 {}", index + 1))
                    });
                if let (Some(question_id), Some(answer)) = (question_id, answer) {
                    let runtime = self.runtime.clone();
                    let inner = self.inner.clone();
                    let state = self.state.clone();
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        match runtime.resume_question(&question_id, answer).await {
                            Ok(Some((assignment_id, outcome))) => {
                                scheduler.mark_waiting(
                                    assignment_id.as_deref(),
                                    &outcome.run_id,
                                    matches!(
                                        outcome.status.as_str(),
                                        "waiting" | "blocked" | "suspended"
                                    ),
                                );
                                if outcome.status == "done" {
                                    if let Some(assignment_id) = assignment_id {
                                        finish_assignment(inner, state, assignment_id).await;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(error) => tracing::error!(%error, "question continuation failed"),
                        }
                        scheduler.dispatch_ready_assignments().await;
                    });
                }
            }
        }
        if method == "takeover.release" {
            if let Some(request) = result.get("takeover_request") {
                let assignment_id = request
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let note = request
                    .get("note")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(assignment_id) = assignment_id {
                    let runtime = self.runtime.clone();
                    let inner = self.inner.clone();
                    let state = self.state.clone();
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        match runtime.resume_takeover(&assignment_id, note).await {
                            Ok(Some((assignment_result, outcome))) => {
                                scheduler.mark_waiting(
                                    assignment_result
                                        .as_deref()
                                        .or(Some(assignment_id.as_str())),
                                    &outcome.run_id,
                                    matches!(
                                        outcome.status.as_str(),
                                        "waiting" | "blocked" | "suspended"
                                    ),
                                );
                                if outcome.status == "done" {
                                    if let Some(assignment_id) = assignment_result {
                                        finish_assignment(inner, state, assignment_id).await;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(error) => tracing::error!(%error, "takeover continuation failed"),
                        }
                        scheduler.dispatch_ready_assignments().await;
                    });
                }
            }
        }
        if private_takeover_action {
            result = json!({});
        }
        // Assignment creation and queue promotion can happen inside the
        // ProductionBackend (including the internal send_msg bridge), so
        // dispatch after all synchronous continuation handling above.
        if !resumed_waiting_message
            && !matches!(
                method,
                "approval.decide" | "question.answer" | "takeover.release"
            )
        {
            self.dispatch_ready_assignments().await;
        }
        Ok(result)
    }
}

fn is_safe_component(value: &str) -> bool {
    let mut components = std::path::Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn normalize_call_id(value: &str) -> &str {
    value.strip_prefix("apr_").unwrap_or(value)
}

fn bot_tool_allowlist(
    bot: Option<&Value>,
    is_main: bool,
    private: bool,
    web_search_configured: bool,
) -> Option<Vec<String>> {
    let config = bot.and_then(|value| value.get("tools").and_then(Value::as_object));
    let mut names = vec![
        "skill",
        "memory",
        "memory_search",
        "session_search",
        "project_find",
        "chat_history",
        "question",
    ];
    let files_enabled = config
        .and_then(|value| value.get("files"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !is_main && files_enabled {
        names.extend(["read", "ls", "find", "grep", "write", "edit"]);
    }
    if !is_main
        && config
            .and_then(|value| value.get("bash"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
    {
        names.extend(["bash", "bash_job"]);
    }
    if web_search_configured
        && config
            .and_then(|value| value.get("web"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        names.extend(["web_fetch", "web_search"]);
    }
    if config
        .and_then(|value| value.get("browser"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        names.extend([
            "browser_open",
            "browser_snapshot",
            "browser_act",
            "browser_get",
            "browser_wait",
            "browser_screenshot",
            "browser_tabs",
            "browser_nav",
            "browser_eval",
            "request_takeover",
        ]);
    }
    if config
        .and_then(|value| value.get("subagent"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !is_main
    {
        names.push("subagent");
    }
    if is_main {
        names.extend([
            "list_bots",
            "create_project",
            "project_create",
            "assign",
            "delegate",
            "project_status",
            "get_status",
            "request_review",
            "finish_project",
            "propose_bot",
            "notify_user",
            "remind",
            "routine",
        ]);
    }
    // A non-main Bot still needs the collaboration bridge in a direct chat
    // so it can report to the main Bot.  Subagents receive an explicit
    // read-only allowlist and never pass through this helper.
    if !private || !is_main {
        names.push("send_msg");
    }
    Some(names.into_iter().map(str::to_owned).collect())
}

/// Private Bots may search projects they belong to, while the generic feature
/// tool intentionally limits non-main runs to the current project.  This
/// runtime-owned bridge supplies the broader, still membership-filtered view
/// without weakening the feature service's write authorization.
struct ProjectFindToolBridge {
    service: Arc<FeatureService>,
    visible_project_ids: Vec<String>,
}

#[async_trait]
impl Tool for ProjectFindToolBridge {
    fn name(&self) -> &str {
        "project_find"
    }

    fn description(&self) -> &str {
        "Find projects this Bot belongs to."
    }

    fn schema(&self) -> Value {
        json!({
            "type":"object",
            "required":["query"],
            "properties":{"query":{"type":"string"}}
        })
    }

    fn risk(&self, _: &Value) -> macbot_tools::Risk {
        macbot_tools::Risk::Read
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .filter(|query| !query.is_empty());
        let Some(query) = query else {
            return ToolResult::error("query is required");
        };
        match self.service.project_find(query) {
            Ok(projects) => {
                let projects = projects
                    .into_iter()
                    .filter(|project| self.visible_project_ids.iter().any(|id| id == &project.id))
                    .collect::<Vec<_>>();
                let details = json!({"projects": projects});
                ToolResult {
                    content: vec![Part::Text {
                        text: details.to_string(),
                    }],
                    details,
                    is_error: false,
                }
            }
            Err(error) => ToolResult::error(error),
        }
    }
}

impl RuntimeExecution {
    pub fn open(
        home: impl Into<std::path::PathBuf>,
        backend: Arc<ProductionBackend>,
        usage: Arc<Mutex<UsageLedger>>,
        providers: Arc<Mutex<ProviderRegistry>>,
        state: GatewayState,
        feature_service: SharedFeatureService,
    ) -> Result<Self, RuntimeError> {
        let home = home.into();
        let gateway_state = state.clone();
        let store = backend.store.clone();
        let orchestrator = backend.orchestrator.clone();
        let jobs = BashJobManager::default();
        let bridge = Arc::new(OrchestratorGroupBridge {
            backend: backend.clone(),
            state: state.clone(),
            jobs: jobs.clone(),
        });
        let gateway_sink = Arc::new(GatewayStateSink::new(state, store.clone(), bridge));
        let sink: Arc<dyn ExecutionSink> = Arc::new(OrchestratorSink {
            inner: gateway_sink,
            orchestrator: orchestrator.clone(),
            store: store.clone(),
            backend: backend.clone(),
            state: gateway_state.clone(),
        });
        let provider = Arc::new(UnavailableProvider);
        let resolver = Arc::new(RegistryResolver { providers });
        let mutations = FileMutationQueue::new();
        let base_tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ReadTool),
            Arc::new(WriteTool::with_queue(mutations.clone())),
            Arc::new(EditTool::with_queue(mutations)),
            Arc::new(LsTool),
            Arc::new(FindTool),
            Arc::new(GrepTool),
            Arc::new(BashTool::with_jobs(jobs.clone())),
            Arc::new(BashJobTool { jobs }),
        ];
        let state = ExecutionState::from_store(store.clone())?;
        let base_sink: Arc<dyn ExecutionSink> = sink;
        let engine = ExecutionEngine::new_with_usage_and_state(
            store.clone(),
            provider,
            base_tools.clone(),
            base_sink.clone(),
            home.clone(),
            usage.clone(),
            state.clone(),
        )?
        .with_provider_resolver(resolver.clone());
        Ok(Self {
            engine: Arc::new(engine),
            state,
            store,
            home,
            usage,
            provider_resolver: resolver,
            base_tools,
            base_sink,
            feature_service,
            backend,
            gateway_state,
        })
    }

    /// Refresh the shared feature runtime from durable settings. This is
    /// intentionally called before model runs and after settings mutations so
    /// skill roots, maintenance provider selection, context compaction and
    /// memory commits all observe the same live service handle.
    pub async fn configure_feature_runtime(&self) -> Result<(), RuntimeError> {
        let settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .map_err(|error| RuntimeError::Provider(error.to_string()))?
            .unwrap_or_else(|| json!({}));
        let extra_dirs = settings
            .pointer("/skills/extra_dirs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let snapshot = self.backend.orchestrator.snapshot().ok();
        let maintenance_model = snapshot.as_ref().and_then(|snapshot| {
            resolve_model(snapshot, &settings, "main", ModelRole::Maintenance)
        });
        let provider = if let Some(model_ref) = maintenance_model {
            let providers = self.provider_resolver.providers.lock().await;
            let (catalog, provider) = providers
                .resolve_model(&model_ref)
                .map_err(|error| RuntimeError::Provider(error.to_string()))?;
            let maintenance_bot = self
                .backend
                .orchestrator
                .snapshot()
                .ok()
                .and_then(|snapshot| {
                    snapshot
                        .get("bots")
                        .and_then(Value::as_object)
                        .and_then(|bots| {
                            bots.values()
                                .find(|bot| {
                                    bot.get("is_main").and_then(Value::as_bool) == Some(true)
                                })
                                .and_then(|bot| bot.get("id").and_then(Value::as_str))
                        })
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "system".into());
            let context = MaintenanceUsageContext {
                request_id: format!("maintenance-{}", Uuid::now_v7()),
                bot_id: maintenance_bot,
                project_id: None,
                chat_id: "maintenance".into(),
                run_id: format!("maintenance-{}", Uuid::now_v7()),
                provider_id: catalog.provider_id.clone(),
                model_id: catalog.model_id.clone(),
                routine: false,
            };
            Some(Arc::new(
                ModelMaintenanceAdapter::new(provider, catalog.model_id)
                    .with_usage_sink(Arc::new(RuntimeMaintenanceUsage {
                        usage: self.usage.clone(),
                    }))
                    .with_usage_context(context),
            )
                as Arc<dyn macbot_memory::AsyncMaintenanceProvider>)
        } else {
            None
        };
        self.feature_service
            .configure_runtime(extra_dirs, provider)
            .map_err(|error| RuntimeError::Provider(error.to_string()))
    }

    pub async fn run(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, RuntimeError> {
        self.configure_feature_runtime().await?;
        self.persist_request(&request)?;
        // Start the staged memory transaction before model/tool execution.
        // A duplicate durable run may already own the transaction after a
        // crash, which is safe to leave untouched.
        let _ = self.feature_service.begin_memory_run(&request.run_id);
        Ok(self.engine_for(&request)?.run(request).await?)
    }

    /// Cancel the durable execution owned by an assignment.  Assignment.stop
    /// is a public orchestrator mutation, but cancellation must also reach the
    /// engine so foreground tools receive the run-scoped cancellation signal
    /// and background process groups are cleaned up.
    pub async fn cancel_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<Option<ExecutionOutcome>, RuntimeError> {
        let Some(request) =
            self.find_request(|request| request.assignment_id.as_deref() == Some(assignment_id))
        else {
            return Ok(None);
        };
        Ok(self.engine_for(&request)?.cancel(&request).await?)
    }

    /// Continue the durable job whose pending unsafe call owns `approval_id`.
    /// The request is persisted separately because the durable checkpoint only
    /// contains the conversation and pending tool call.
    pub async fn resume_approved(
        &self,
        approval_id: &str,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.configure_feature_runtime().await?;
        let mapped_call_id = self
            .store
            .read_snapshot::<Value>(format!(
                "data/approval-map/{}.json",
                safe_component(approval_id)
            ))
            .map_err(|error| RuntimeError::Execution(error.into()))?
            .and_then(|value| {
                value
                    .get("call_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let jobs_dir = self.home.join("data/jobs");
        let entries = match fs::read_dir(jobs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RuntimeError::Execution(ExecutionError::Durable(
                    error.into(),
                )));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let file = fs::File::open(entry.path())
                .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
            let job: macbot_durable::Job = serde_json::from_reader(file).map_err(|error| {
                RuntimeError::Execution(ExecutionError::Durable(
                    macbot_durable::DurableError::Invalid(error.to_string()),
                ))
            })?;
            let Some(call_id) = job
                .checkpoint
                .get("pending_tool")
                .and_then(|value| value.get("call_id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if normalize_call_id(&format!("apr_{}", safe_component(call_id)))
                != normalize_call_id(approval_id)
                && mapped_call_id.as_deref().map(normalize_call_id)
                    != Some(normalize_call_id(call_id))
            {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                tracing::warn!(%run_id, "approval has no persisted execution request");
                return Ok(None);
            };
            let assignment_id = request.assignment_id.clone();
            let _ = self.feature_service.begin_memory_run(&request.run_id);
            let outcome = self
                .engine_for(&request)?
                .continue_approved(request)
                .await?;
            return Ok(Some((assignment_id, outcome)));
        }
        Ok(None)
    }

    /// Resume the waiting model turn owned by an orchestrator assignment.
    /// Questions and browser takeover are represented by the same durable
    /// waiting job, but only their matching pending tool may be continued.
    /// This keeps a question answer from accidentally approving an unrelated
    /// unsafe side effect after a restart.
    pub async fn resume_question(
        &self,
        question_id: &str,
        answer: String,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.resume_waiting_assignment(
            question_id,
            &["ask_user", "question"],
            WaitingContinuation::Question(answer),
        )
        .await
    }

    /// Resume a browser takeover after the driver has released control. The
    /// release RPC is the only user action that grants this continuation;
    /// takeover.start only changes the browser driver's state.
    pub async fn resume_takeover(
        &self,
        assignment_id: &str,
        note: Option<String>,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.resume_waiting_assignment(
            assignment_id,
            &["request_takeover"],
            WaitingContinuation::Takeover(note),
        )
        .await
    }

    pub async fn resume_message(
        &self,
        assignment_id: &str,
        message: String,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.configure_feature_runtime().await?;
        let entries = match fs::read_dir(self.home.join("data/jobs")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RuntimeError::Execution(ExecutionError::Durable(
                    error.into(),
                )));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let file = fs::File::open(entry.path())
                .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
            let job: macbot_durable::Job = serde_json::from_reader(file).map_err(|error| {
                RuntimeError::Execution(ExecutionError::Durable(
                    macbot_durable::DurableError::Invalid(error.to_string()),
                ))
            })?;
            if !matches!(
                job.status,
                macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
            ) || job
                .checkpoint
                .get("waiting_reason")
                .and_then(Value::as_str)
                .is_none()
            {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                continue;
            };
            if request.assignment_id.as_deref() != Some(assignment_id) {
                continue;
            }
            let assignment = request.assignment_id.clone();
            let outcome = self
                .engine_for(&request)?
                .continue_message(request, message)
                .await?;
            return Ok(Some((assignment, outcome)));
        }
        Ok(None)
    }

    async fn resume_waiting_assignment(
        &self,
        marker_or_assignment: &str,
        pending_tools: &[&str],
        continuation: WaitingContinuation,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.configure_feature_runtime().await?;
        let jobs_dir = self.home.join("data/jobs");
        // The execution card initially carries the provider call id, while
        // the orchestrator assigns the public question id. Resolve that id
        // back to its assignment so older cards still continue the same job.
        let question_assignment = self
            .backend
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("questions")
                    .and_then(Value::as_object)
                    .and_then(|questions| questions.get(marker_or_assignment))
                    .and_then(|question| question.get("assignment_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let entries = match fs::read_dir(jobs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RuntimeError::Execution(ExecutionError::Durable(
                    error.into(),
                )));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let file = fs::File::open(entry.path())
                .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
            let job: macbot_durable::Job = serde_json::from_reader(file).map_err(|error| {
                RuntimeError::Execution(ExecutionError::Durable(
                    macbot_durable::DurableError::Invalid(error.to_string()),
                ))
            })?;
            if !matches!(
                job.status,
                macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
            ) {
                continue;
            }
            let Some(call_name) = job
                .checkpoint
                .get("pending_tool")
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !pending_tools.contains(&call_name) {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                continue;
            };
            let assignment_matches = request.assignment_id.as_deref() == Some(marker_or_assignment)
                || question_assignment.as_deref() == request.assignment_id.as_deref()
                || (request.assignment_id.is_none()
                    && format!("dm_{}", safe_component(&request.chat_id)) == marker_or_assignment);
            let marker_matches = self
                .store
                .read_snapshot::<Value>(format!(
                    "data/waiting/{}.json",
                    safe_component(marker_or_assignment)
                ))
                .ok()
                .flatten()
                .is_some_and(|marker| {
                    marker.get("run_id").and_then(Value::as_str) == Some(run_id)
                        || marker.get("assignment_id").and_then(Value::as_str)
                            == request.assignment_id.as_deref()
                });
            if !assignment_matches && !marker_matches {
                continue;
            }
            let assignment = request.assignment_id.clone();
            let _ = self.feature_service.begin_memory_run(&request.run_id);
            let outcome = match continuation {
                WaitingContinuation::Question(answer) => {
                    self.engine_for(&request)?
                        .continue_question(request, answer)
                        .await?
                }
                WaitingContinuation::Takeover(note) => {
                    let mut request = request;
                    if let Some(note) = note {
                        request.instruction = note;
                    }
                    self.engine_for(&request)?
                        .continue_takeover(request)
                        .await?
                }
            };
            return Ok(Some((assignment, outcome)));
        }
        Ok(None)
    }

    fn persist_request(&self, request: &ExecutionRequest) -> Result<(), RuntimeError> {
        if !is_safe_component(&request.run_id) {
            return Err(RuntimeError::Execution(ExecutionError::Durable(
                macbot_durable::DurableError::Invalid("unsafe run id".into()),
            )));
        }
        self.store
            .write_snapshot(
                format!("data/run_requests/{}.json", request.run_id),
                request,
            )
            .map_err(|error| RuntimeError::Execution(error.into()))?;
        Ok(())
    }

    async fn configure_browser_for_request(
        &self,
        request: &ExecutionRequest,
    ) -> Result<(), String> {
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        let bot = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(&request.bot_id));
        let mode = match bot
            .and_then(|bot| bot.get("browser_mode"))
            .and_then(Value::as_str)
            .unwrap_or("headless")
        {
            "attach" => macbot_protocol::BrowserMode::Attach,
            "headless_profile" => macbot_protocol::BrowserMode::HeadlessProfile,
            _ => macbot_protocol::BrowserMode::Headless,
        };
        let chrome_profile = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .and_then(|settings| settings.pointer("/browser/chrome_profile").cloned())
            .and_then(|value| value.as_str().map(str::to_owned));
        ProductionBrowserBridge::new(self.gateway_state.clone())
            .configure_bot(&request.bot_id, mode, chrome_profile.as_deref())
            .await
    }

    pub async fn recover(&self) -> Result<Vec<macbot_durable::Job>, RuntimeError> {
        Ok(self.engine.recover().await?)
    }

    fn cwd_for(&self, project_id: Option<&str>, bot_id: &str) -> PathBuf {
        let path = match project_id.filter(|id| !id.is_empty()) {
            Some(project_id) => {
                let slug = self
                    .backend
                    .orchestrator
                    .snapshot()
                    .ok()
                    .and_then(|snapshot| {
                        snapshot
                            .get("projects")
                            .and_then(Value::as_object)
                            .and_then(|projects| projects.get(project_id))
                            .and_then(|project| project.get("slug").and_then(Value::as_str))
                            .filter(|slug| !slug.is_empty())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| project_id.to_owned());
                self.home.join("projects").join(safe_component(&slug))
            }
            None => self.home.join("bots").join(safe_component(bot_id)),
        };
        if let Err(error) = fs::create_dir_all(&path) {
            tracing::warn!(path = %path.display(), %error, "failed to create run cwd");
        }
        path
    }

    fn model_context_window(&self, model_ref: &str) -> usize {
        self.provider_resolver
            .providers
            .try_lock()
            .ok()
            .and_then(|providers| {
                providers
                    .models()
                    .find(|model| model.r#ref == model_ref)
                    .map(|model| model.context_window as usize)
            })
            .filter(|window| *window > 0)
            .unwrap_or(128_000)
    }

    fn context_metadata(&self, request: &ExecutionRequest) -> (usize, String, String, Vec<String>) {
        let snapshot = self.backend.orchestrator.snapshot().unwrap_or_default();
        let bot = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(&request.bot_id));
        let identity = bot
            .map(|bot| {
                [
                    bot.get("name").and_then(Value::as_str),
                    bot.get("label").and_then(Value::as_str),
                    bot.get("description").and_then(Value::as_str),
                ]
                .into_iter()
                .flatten()
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("Bot id: {}", request.bot_id));
        let project = request.project_id.as_ref().and_then(|project_id| {
            snapshot
                .get("projects")
                .and_then(Value::as_object)
                .and_then(|projects| projects.get(project_id))
        });
        let mut announcement = project
            .and_then(|project| {
                project
                    .get("announcement")
                    .and_then(Value::as_str)
                    .or_else(|| project.get("goal").and_then(Value::as_str))
            })
            .unwrap_or_default()
            .to_owned();
        if let (Some(project_id), Some(project)) = (&request.project_id, project) {
            let members = project
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|member| {
                    let bot_id = member.get("bot_id").and_then(Value::as_str).unwrap_or("");
                    let role = member
                        .get("role_note")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("member");
                    format!("{bot_id}: {role}")
                })
                .filter(|member| !member.starts_with(':'))
                .collect::<Vec<_>>();
            let status = project
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("active");
            let artifacts = snapshot
                .get("artifacts")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|items| items.values())
                .filter(|artifact| {
                    artifact.get("project_id").and_then(Value::as_str) == Some(project_id)
                })
                .filter_map(|artifact| {
                    let title = artifact.get("title").and_then(Value::as_str)?;
                    let path = artifact
                        .get("path_or_url")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Some(format!("{title} {path}").trim().to_owned())
                })
                .collect::<Vec<_>>();
            let highlights = snapshot
                .get("highlights")
                .and_then(Value::as_object)
                .and_then(|items| items.get(project_id))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|highlight| highlight.get("text").and_then(Value::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            announcement = format!(
                "公告/目标: {announcement}\n项目状态: {status}\n成员: {}\n产物: {}\n重点: {}",
                if members.is_empty() {
                    "(none)".into()
                } else {
                    members.join(", ")
                },
                if artifacts.is_empty() {
                    "(none)".into()
                } else {
                    artifacts.join(", ")
                },
                if highlights.is_empty() {
                    "(none)".into()
                } else {
                    highlights.join(" | ")
                }
            );
        }
        let mut references = Vec::new();
        if let Some(project_id) = &request.project_id {
            references.push(format!("project:{project_id}"));
            if let Some(project) = project {
                if let Some(home) = project.get("home_path").and_then(Value::as_str) {
                    references.push(format!("home:{home}"));
                }
            }
        }
        (
            self.model_context_window(&request.model),
            identity,
            announcement,
            references,
        )
    }

    fn feature_access(&self, request: &ExecutionRequest) -> MemoryAccess {
        let Some(project_id) = request.project_id.as_deref() else {
            return MemoryAccess::user("user");
        };
        let members = self
            .backend
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("projects")
                    .and_then(Value::as_object)
                    .and_then(|projects| projects.get(project_id))
                    .and_then(|project| project.get("members"))
                    .and_then(Value::as_array)
                    .map(|members| {
                        members
                            .iter()
                            .filter_map(|member| {
                                member
                                    .get("bot_id")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                            .collect::<Vec<_>>()
                    })
            })
            .unwrap_or_default();
        MemoryAccess::group(project_id.to_owned(), members)
    }

    fn visible_project_ids(&self, request: &ExecutionRequest, is_main: bool) -> Vec<String> {
        let Ok(snapshot) = self.backend.orchestrator.snapshot() else {
            return Vec::new();
        };
        let Some(projects) = snapshot.get("projects").and_then(Value::as_object) else {
            return Vec::new();
        };
        if is_main {
            return projects.keys().cloned().collect();
        }
        projects
            .iter()
            .filter(|(id, project)| {
                request.project_id.as_deref() == Some(id.as_str())
                    || project
                        .get("members")
                        .and_then(Value::as_array)
                        .is_some_and(|members| {
                            members.iter().any(|member| {
                                member.get("bot_id").and_then(Value::as_str)
                                    == Some(request.bot_id.as_str())
                            })
                        })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn sync_feature_projects(&self) {
        let Ok(snapshot) = self.backend.orchestrator.snapshot() else {
            return;
        };
        let Some(projects) = snapshot.get("projects").and_then(Value::as_object) else {
            return;
        };
        for (id, project) in projects {
            let Some(name) = project.get("name").and_then(Value::as_str) else {
                continue;
            };
            let goal = project
                .get("goal")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status = project
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("active");
            let members = project
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|member| member.get("bot_id").and_then(Value::as_str))
                .collect::<Vec<_>>();
            let content = format!(
                "status={status}; members={}; home={}",
                members.join(","),
                project
                    .get("home_path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            if let Err(error) = self.feature_service.add_project_record(ProjectRecord {
                id: id.clone(),
                name: name.to_owned(),
                goal: goal.to_owned(),
                content,
            }) {
                tracing::warn!(project_id = %id, %error, "failed to sync project feature index");
            }
        }
    }

    fn sync_feature_messages(&self, request: &ExecutionRequest) {
        let existing = self
            .feature_service
            .chat_history(&request.chat_id, 100)
            .unwrap_or_default();
        for (index, message) in request.messages.iter().enumerate() {
            let Some(content) = message.get("content").and_then(Value::as_str) else {
                continue;
            };
            let id = format!("ctx_{}_{}", safe_component(&request.chat_id), index);
            if existing.iter().any(|item| item.id == id) {
                continue;
            }
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            if let Err(error) = self.feature_service.add_session_message(SessionMessage {
                id,
                session_id: request.chat_id.clone(),
                chat_id: request.chat_id.clone(),
                project_id: request.project_id.clone(),
                role: role.to_owned(),
                content: content.to_owned(),
                at: Utc::now(),
            }) {
                tracing::warn!(chat_id = %request.chat_id, %error, "failed to sync recent feature message");
            }
        }
    }

    fn engine_for(&self, request: &ExecutionRequest) -> Result<ExecutionEngine, RuntimeError> {
        let is_main = self
            .backend
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("bots")
                    .and_then(Value::as_object)
                    .and_then(|bots| bots.get(&request.bot_id))
                    .and_then(|bot| bot.get("is_main").and_then(Value::as_bool))
            })
            .unwrap_or(request.bot_id == "main");
        let context_metadata = self.context_metadata(request);
        let mut context = if is_main {
            FeatureRunContext::main(request.bot_id.clone(), "user")
        } else {
            FeatureRunContext::bot(request.bot_id.clone(), "user")
        };
        context = context.with_context_metadata(
            context_metadata.0,
            context_metadata.1,
            context_metadata.2,
            context_metadata.3,
        );
        context.run_id = Some(request.run_id.clone());
        context.private = request.private;
        context.project_id = request.project_id.clone();
        context.session_id = Some(request.chat_id.clone());
        context.chat_id = Some(request.chat_id.clone());
        context.access = self.feature_access(request);
        context.recent_context = request
            .messages
            .iter()
            .filter_map(|message| message.get("content").and_then(Value::as_str))
            .rev()
            .take(30)
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        self.sync_feature_projects();
        self.sync_feature_messages(request);
        let mut explicit_skill = None;
        if let Ok(skills) = self.feature_service.skill_registry.read() {
            explicit_skill = match skills.load_for_text(&request.instruction, Some(&request.bot_id))
            {
                Ok(Some((detail, instruction))) => {
                    Some((detail.content, instruction, detail.skill.name))
                }
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(bot_id = %request.bot_id, %error, "explicit skill invocation could not be loaded");
                    None
                }
            };
            let skill_catalog = skills
                .list()
                .into_iter()
                .filter(|skill| {
                    skill.enabled
                        && !skill
                            .disabled_bot_ids
                            .iter()
                            .any(|id| id == &request.bot_id)
                })
                .map(|skill| format!("/{0}: {1} ({2})", skill.name, skill.description, skill.path))
                .collect::<Vec<_>>();
            let tool_catalog = request.tools.clone().unwrap_or_default().join(", ");
            if let Some(identity) = context.bot_identity.as_mut() {
                let skill_text = if skill_catalog.is_empty() {
                    "(none)".to_owned()
                } else {
                    skill_catalog.join("\n")
                };
                identity.push_str("\n可用技能目录（显式调用 /skill-name）：\n");
                identity.push_str(&skill_text);
                identity.push_str("\n可用工具：");
                identity.push_str(if tool_catalog.is_empty() {
                    "(none)"
                } else {
                    &tool_catalog
                });
            }
        }
        if let Some((content, instruction, name)) = explicit_skill {
            if let Err(error) = self
                .feature_service
                .record_skill_invocation(&name, Some(&request.bot_id))
            {
                tracing::warn!(bot_id = %request.bot_id, skill = %name, %error, "failed to record explicit skill invocation");
            }
            if let Some(identity) = context.bot_identity.as_mut() {
                identity.push_str("\n用户显式调用的技能正文（已按需加载）：\n");
                identity.push_str(&content);
            }
            context
                .recent_context
                .push(format!("显式技能指令: {instruction}"));
        }
        let feature_runtime = FeatureToolRuntime::new(self.feature_service.clone(), context);
        let mut tools = self.base_tools.clone();
        tools.extend(feature_runtime.tools());
        if !is_main {
            tools.push(Arc::new(ProjectFindToolBridge {
                service: self.feature_service.clone(),
                visible_project_ids: self.visible_project_ids(request, is_main),
            }));
        }
        let identity = CollaborationIdentity {
            bot_id: request.bot_id.clone(),
            chat_id: request.chat_id.clone(),
            assignment_id: request.assignment_id.clone(),
            project_id: request.project_id.clone(),
            is_main,
        };
        let rpc = Arc::new(ProductionCoordinationRpc::new(
            self.backend.clone(),
            self.gateway_state.clone(),
        ));
        let browser = Arc::new(ProductionBrowserBridge::new(self.gateway_state.clone()));
        let web_settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .and_then(|settings| settings.get("web_search").cloned())
            .unwrap_or_default();
        let web_config = WebSearchConfig {
            provider: web_settings
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_owned),
            endpoint: web_settings
                .get("endpoint")
                .and_then(Value::as_str)
                .map(str::to_owned),
            // Credentials are always fetched by RegistryWebCredentials from
            // the injected Keychain/FileSecrets backend at request time.
            api_key: None,
        };
        let web: Option<Arc<dyn crate::collaboration_tools::WebToolBridge>> =
            ReqwestWebBridge::new(web_config).ok().map(|bridge| {
                Arc::new(bridge.with_credentials(Arc::new(RegistryWebCredentials {
                    providers: self.provider_resolver.providers.clone(),
                }))) as Arc<dyn crate::collaboration_tools::WebToolBridge>
            });
        let subagent = Arc::new(RuntimeSubagentDispatch {
            runtime: self.clone(),
        });
        let coordination = CollaborationTools::new(rpc, identity, Some(browser), web)
            .with_subagent_dispatch(subagent);
        tools.extend(coordination.tools());
        let sink: Arc<dyn ExecutionSink> = Arc::new(FeatureExecutionSink::new(
            self.base_sink.clone(),
            feature_runtime,
        ));
        Ok(ExecutionEngine::new_with_usage_and_state(
            self.store.clone(),
            Arc::new(UnavailableProvider),
            tools,
            sink,
            self.home.clone(),
            self.engine_usage(),
            self.state.clone(),
        )?
        .with_provider_resolver(self.provider_resolver.clone()))
    }

    fn engine_usage(&self) -> Arc<Mutex<UsageLedger>> {
        // The base engine owns the same ledger; execution engines are rebuilt
        // per request only to carry a request-scoped memory/tool context.
        self.engine_usage_ref()
    }

    fn engine_usage_ref(&self) -> Arc<Mutex<UsageLedger>> {
        // ExecutionEngine intentionally exposes no usage accessor.  The
        // process-wide ledger is retained through the base sink/runtime by
        // cloning it at construction time below.
        self.usage.clone()
    }

    fn model_for_subagent(
        &self,
        bot_id: &str,
        parent_model: Option<&str>,
    ) -> Option<(String, String)> {
        let snapshot = self.backend.orchestrator.snapshot().ok()?;
        let settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .unwrap_or(Value::Null);
        let model = resolve_model(
            &snapshot,
            &settings,
            bot_id,
            ModelRole::Subagent { parent_model },
        )?;
        let provider_id = model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or_default()
            .to_owned();
        Some((model, provider_id))
    }

    fn price_for_model(&self, model_ref: &str) -> Option<Price> {
        let providers = self.provider_resolver.providers.try_lock().ok()?;
        let price = providers
            .models()
            .find(|model| model.r#ref == model_ref)
            .and_then(|model| model.price.as_ref())
            .map(|price| Price {
                input_per_mtok: price.input_per_mtok,
                output_per_mtok: price.output_per_mtok,
                cache_read_per_mtok: price.cache_read_per_mtok,
                cache_write_per_mtok: price.cache_write_per_mtok,
            });
        price
    }

    pub async fn enqueue_steer_for_assignment(
        &self,
        assignment_id: &str,
        message_id: &str,
        text: &str,
    ) -> bool {
        let Some(request) =
            self.find_request(|request| request.assignment_id.as_deref() == Some(assignment_id))
        else {
            return false;
        };
        let execution_state = self.engine.state();
        let mut durable = execution_state.durable.lock().await;
        let Some(job) = durable
            .jobs()
            .find(|job| {
                job.checkpoint.get("run_id").and_then(Value::as_str)
                    == Some(request.run_id.as_str())
            })
            .cloned()
        else {
            return false;
        };
        durable.enqueue_steer(&job.id, message_id, text).is_ok()
    }

    async fn applied_steers(&self, assignment_id: &str) -> Vec<String> {
        let Some(request) =
            self.find_request(|request| request.assignment_id.as_deref() == Some(assignment_id))
        else {
            return Vec::new();
        };
        let state = self.engine.state();
        let durable = state.durable.lock().await;
        let Some(job_id) = durable
            .jobs()
            .find(|job| {
                job.checkpoint.get("run_id").and_then(Value::as_str)
                    == Some(request.run_id.as_str())
            })
            .map(|job| job.id.clone())
        else {
            return Vec::new();
        };
        durable
            .inbox(&job_id)
            .filter(|item| item.delivery == macbot_durable::Delivery::Read)
            .map(|item| item.message_id.clone())
            .collect()
    }

    fn find_request<F>(&self, predicate: F) -> Option<ExecutionRequest>
    where
        F: Fn(&ExecutionRequest) -> bool,
    {
        let dir = self.home.join("data/run_requests");
        let entries = fs::read_dir(dir).ok()?;
        entries.filter_map(Result::ok).find_map(|entry| {
            let file = fs::File::open(entry.path()).ok()?;
            let request = serde_json::from_reader::<_, ExecutionRequest>(file).ok()?;
            predicate(&request).then_some(request)
        })
    }
}

struct RuntimeSubagentDispatch {
    runtime: RuntimeExecution,
}

struct RuntimeMaintenanceUsage {
    usage: Arc<Mutex<UsageLedger>>,
}

#[async_trait]
impl MaintenanceUsageSink for RuntimeMaintenanceUsage {
    async fn record(
        &self,
        phase: &str,
        context: &MaintenanceUsageContext,
        usage: &TokenUsage,
    ) -> Result<(), String> {
        let totals = Totals {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            requests: 1,
            cost: None,
        };
        self.usage
            .lock()
            .await
            .record(UsageRecord {
                request_id: format!("{}:{phase}", context.request_id),
                ts: chrono::Utc::now(),
                bot_id: context.bot_id.clone(),
                project_id: context.project_id.clone(),
                chat_id: context.chat_id.clone(),
                assignment_id: None,
                run_id: context.run_id.clone(),
                phase: phase.to_owned(),
                provider_id: context.provider_id.clone(),
                model_id: context.model_id.clone(),
                routine: context.routine,
                usage: totals,
                task_done: false,
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl SubagentDispatchBridge for RuntimeSubagentDispatch {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<Value, String> {
        let parent = self
            .runtime
            .find_request(|candidate| candidate.run_id == request.parent_run_id);
        let (model, provider_id) = self
            .runtime
            .model_for_subagent(
                &request.bot_id,
                parent.as_ref().map(|request| request.model.as_str()),
            )
            .ok_or_else(|| "subagent bot has no configured model".to_owned())?;
        let price = self.runtime.price_for_model(&model);
        let cwd = self
            .runtime
            .find_request(|candidate| candidate.run_id == request.parent_run_id)
            .and_then(|parent| parent.cwd)
            .unwrap_or_else(|| self.runtime.cwd_for(None, &request.bot_id));
        let child = ExecutionRequest {
            run_id: format!("subagent_{}", safe_component(&request.subagent_id)),
            assignment_id: Some(request.assignment_id.clone()),
            chat_id: request.chat_id.clone(),
            bot_id: request.bot_id.clone(),
            model,
            provider_id,
            project_id: None,
            instruction: request.task.clone(),
            messages: vec![
                json!({"role":"user","content":request.task,"metadata":{"subagent":true}}),
            ],
            max_turns: request.max_turns as usize,
            private: false,
            allow_unsafe: false,
            cwd: Some(cwd),
            routine: false,
            price,
            resume_approved: false,
            subagent: true,
            tools: Some(vec![
                "read".into(),
                "ls".into(),
                "find".into(),
                "grep".into(),
                "skill".into(),
            ]),
            phase: Some("subagent".into()),
            parent_run_id: Some(request.parent_run_id.clone()),
            subagent_task: Some(request.task.clone()),
            save_full_requests: false,
            resume_message: None,
        };
        let outcome = self
            .runtime
            .run(child)
            .await
            .map_err(|error| error.to_string())?;
        Ok(
            json!({"subagent_id":request.subagent_id,"run_id":outcome.run_id,"status":outcome.status,"text":outcome.text,"turns":outcome.turns}),
        )
    }
}

enum ModelRole<'a> {
    Bot,
    Subagent { parent_model: Option<&'a str> },
    Maintenance,
}

/// Resolve every execution model from the live settings file.  Keeping the
/// role rules in one function is important: a settings update must affect
/// chat, routines, subagents, and maintenance in the same way.
fn resolve_model(
    snapshot: &Value,
    settings: &Value,
    bot_id: &str,
    role: ModelRole<'_>,
) -> Option<String> {
    let models = settings.get("models");
    match role {
        ModelRole::Bot => {
            let bot = snapshot.get("bots")?.get(bot_id)?;
            let is_main = bot
                .get("is_main")
                .and_then(Value::as_bool)
                .unwrap_or(bot_id == "main");
            bot.get("model")
                .and_then(Value::as_str)
                .filter(|model| !model.trim().is_empty())
                .or_else(|| {
                    models
                        .and_then(|models| models.get(if is_main { "main" } else { "bot_default" }))
                        .and_then(Value::as_str)
                        .filter(|model| !model.trim().is_empty())
                })
                .map(str::to_owned)
        }
        ModelRole::Subagent { parent_model } => models
            .and_then(|models| models.get("subagent"))
            .and_then(Value::as_str)
            .filter(|model| !model.trim().is_empty() && *model != "inherit")
            .map(str::to_owned)
            .or_else(|| {
                parent_model
                    .filter(|model| !model.trim().is_empty())
                    .map(str::to_owned)
            }),
        ModelRole::Maintenance => {
            let configured = models
                .and_then(|models| models.get("maintenance"))
                .and_then(Value::as_str)
                .filter(|model| !model.trim().is_empty());
            if let Some(model) = configured.filter(|model| *model != "inherit") {
                return Some(model.to_owned());
            }
            if configured == Some("inherit") {
                if let Some(main_id) = snapshot
                    .get("bots")
                    .and_then(Value::as_object)
                    .and_then(|bots| {
                        bots.values()
                            .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                    })
                    .and_then(|bot| bot.get("id").and_then(Value::as_str))
                {
                    return resolve_model(snapshot, settings, main_id, ModelRole::Bot);
                }
            }
            // A missing/null maintenance model deliberately follows the
            // worker default, never the main model.
            models
                .and_then(|models| models.get("bot_default"))
                .and_then(Value::as_str)
                .filter(|model| !model.trim().is_empty())
                .map(str::to_owned)
        }
    }
}

struct RegistryResolver {
    providers: Arc<Mutex<ProviderRegistry>>,
}

struct RegistryWebCredentials {
    providers: Arc<Mutex<ProviderRegistry>>,
}

impl WebCredentialStore for RegistryWebCredentials {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        let registry = self
            .providers
            .try_lock()
            .map_err(|_| "provider registry is busy".to_owned())?;
        registry
            .secret_store()
            .get(provider)
            .map_err(|error| error.to_string())
    }
}

struct RegistryModelProvider {
    inner: Arc<dyn ModelProvider>,
    model_id: String,
}

#[async_trait]
impl ModelProvider for RegistryModelProvider {
    async fn stream(
        &self,
        mut request: ModelRequest,
        events: mpsc::Sender<ModelEvent>,
    ) -> macbot_providers::Result<Completion> {
        // The protocol keeps `provider/model` as the stable catalog ref, but
        // OpenAI-compatible servers receive the catalog's exact model_id.
        request.model = self.model_id.clone();
        self.inner.stream(request, events).await
    }
}

#[async_trait]
impl ProviderResolver for RegistryResolver {
    async fn resolve(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        let model_ref = if model.contains('/') {
            model.to_owned()
        } else {
            format!("{provider_id}/{model}")
        };
        let providers = self.providers.lock().await;
        let (catalog, provider) = providers
            .resolve_model(&model_ref)
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(RegistryModelProvider {
            inner: provider,
            model_id: catalog.model_id,
        }))
    }
}

struct OrchestratorGroupBridge {
    backend: Arc<ProductionBackend>,
    state: GatewayState,
    jobs: BashJobManager,
}

/// Adds the orchestrator's durable approval registry around the transport
/// sink. Execution emits an internal `apr_<call_id>` token; the orchestrator
/// owns the client-facing approval id, so the mapping is persisted beside the
/// job checkpoints and survives a daemon restart.
struct OrchestratorSink {
    inner: Arc<GatewayStateSink>,
    orchestrator: Orchestrator,
    store: Store,
    backend: Arc<ProductionBackend>,
    state: GatewayState,
}

impl OrchestratorSink {
    fn canonical_chat_message(&self, chat_id: &str, message: &Value) -> Result<Value, String> {
        self.store
            .sequence_chat_messages(chat_id, std::slice::from_ref(message))
            .map_err(|error| error.to_string())?
            .into_iter()
            .next()
            .ok_or_else(|| "chat message sequencing returned no message".into())
    }

    async fn persist_execution_question(&self, message: &Value, block: &Value) {
        let Some(call_id) = block.get("question_id").and_then(Value::as_str) else {
            tracing::error!("execution emitted question block without question_id");
            return;
        };
        let bot_id = message
            .pointer("/sender/bot_id")
            .and_then(Value::as_str)
            .unwrap_or("main");
        let chat_id = message
            .get("chat_id")
            .and_then(Value::as_str)
            .unwrap_or("chat_main");
        let assignment_id = message
            .get("assignment_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("dm_{}", safe_component(chat_id)));
        let result = self
            .backend
            .persist_execution_question(
                &self.state,
                json!({
                    "bot_id": bot_id,
                    "assignment_id": assignment_id,
                    "chat_id": chat_id,
                    "text": message.get("fallback_text").and_then(Value::as_str).unwrap_or("需要你的回答"),
                    "options": [],
                    "allow_free_text": true,
                    "client_request_id": format!("execution:question:{call_id}")
                }),
            )
            .await;
        let Ok(result) = result else {
            tracing::error!(call_id, "failed to persist execution question");
            return;
        };
        let Some(question) = result.get("question") else {
            tracing::error!(call_id, "execution question RPC returned no question");
            return;
        };
        let Some(question_id) = question.get("id").and_then(Value::as_str) else {
            tracing::error!(call_id, "execution question has no id");
            return;
        };
        let run_id = message
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| id.strip_prefix("msg_question_"))
            .map(str::to_owned)
            .unwrap_or_else(|| call_id.to_owned());
        let _ = self.store.write_snapshot(
            format!("data/waiting/{}.json", safe_component(question_id)),
            &json!({
                "kind": "question",
                "run_id": run_id,
                "assignment_id": message.get("assignment_id").cloned().unwrap_or(Value::Null),
                "chat_id": chat_id,
                "question_id": call_id,
                "text": message.get("fallback_text").cloned().unwrap_or(Value::Null)
            }),
        );
        self.inner
            .emit(ExecutionEvent {
                event: "question.asked".into(),
                data: json!({"question": question}),
                persistent: true,
            })
            .await;
        let mut updated_message = message.clone();
        if let Some(question_block) = updated_message
            .get_mut("blocks")
            .and_then(Value::as_array_mut)
            .and_then(|blocks| {
                blocks
                    .iter_mut()
                    .find(|block| block.get("type").and_then(Value::as_str) == Some("question"))
            })
        {
            question_block["question_id"] = json!(question_id);
        }
        let updated_message = match self.canonical_chat_message(chat_id, &updated_message) {
            Ok(message) => message,
            Err(error) => {
                tracing::error!(%error, chat_id, "failed to sequence question card update");
                return;
            }
        };
        if let Err(error) = self.store.append_jsonl(
            format!("data/chats/{}/messages.jsonl", safe_component(chat_id)),
            &updated_message,
        ) {
            tracing::error!(%error, chat_id, "failed to persist question card update");
        }
        self.inner
            .emit(ExecutionEvent {
                event: "message.updated".into(),
                data: json!({"message": updated_message}),
                persistent: true,
            })
            .await;
    }

    async fn persist_execution_takeover(&self, message: &Value, block: &Value) {
        let Some(bot_id) = block.get("bot_id").and_then(Value::as_str) else {
            tracing::error!("execution emitted takeover block without bot_id");
            return;
        };
        let Some(message_id) = message.get("id").and_then(Value::as_str) else {
            tracing::error!("execution emitted takeover message without id");
            return;
        };
        let chat_id = message
            .get("chat_id")
            .and_then(Value::as_str)
            .unwrap_or("chat_main");
        let assignment_id = message
            .get("assignment_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("dm_{}", safe_component(chat_id)));
        let reason = block
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("用户接管浏览器");
        let result = self
            .backend
            .persist_execution_takeover(
                &self.state,
                json!({
                    "bot_id": bot_id,
                    "assignment_id": assignment_id,
                    "chat_id": chat_id,
                    "reason": reason,
                    "client_request_id": format!("execution:takeover:{message_id}")
                }),
            )
            .await;
        let Ok(result) = result else {
            tracing::error!(message_id, "failed to persist execution takeover");
            return;
        };
        let Some(question) = result.get("question") else {
            tracing::error!(message_id, "takeover RPC returned no question");
            return;
        };
        self.inner
            .emit(ExecutionEvent {
                event: "question.asked".into(),
                data: json!({"question": question}),
                persistent: true,
            })
            .await;
        let Some(request) = result.get("takeover_request") else {
            return;
        };
        let Some(dm_chat_id) = request.get("chat_id").and_then(Value::as_str) else {
            return;
        };
        let dm_id = format!("msg_takeover_question_{}", safe_component(message_id));
        let dm_message = json!({
            "id": dm_id,
            "chat_id": dm_chat_id,
            "seq": 0,
            "sender": {"kind":"bot", "bot_id":bot_id},
            "created_at": crate::now(),
            "edited_at": null,
            "deleted": false,
            "reply_to": null,
            "thread_count": 0,
            "mentions": [],
            "blocks": [{"type":"question","question_id":question["id"]},{"type":"takeover_request","bot_id":bot_id,"reason":reason,"state":"pending"}],
            "fallback_text": question.get("text").cloned().unwrap_or_else(|| json!(reason)),
            "intent": null,
            "assignment_id": message.get("assignment_id").cloned().unwrap_or(Value::Null),
            "streaming": false,
            "delivery": [],
            "reactions": []
        });
        let dm_message = match self.canonical_chat_message(dm_chat_id, &dm_message) {
            Ok(message) => message,
            Err(error) => {
                tracing::error!(%error, chat_id = dm_chat_id, "failed to sequence takeover DM card");
                return;
            }
        };
        if let Err(error) = self.store.append_jsonl(
            format!("data/chats/{}/messages.jsonl", safe_component(dm_chat_id)),
            &dm_message,
        ) {
            tracing::error!(%error, chat_id = dm_chat_id, "failed to persist takeover DM card");
            return;
        }
        self.inner
            .emit(ExecutionEvent {
                event: "message.created".into(),
                data: json!({"message":dm_message}),
                persistent: true,
            })
            .await;
        if let Some(approval_ref) = result.get("approval_ref") {
            let mut group_message = message.clone();
            if let Some(blocks) = group_message
                .get_mut("blocks")
                .and_then(Value::as_array_mut)
            {
                blocks.push(json!({
                    "type": "approval_ref",
                    "approval_id": approval_ref.get("approval_id").cloned().unwrap_or(Value::Null),
                    "chat_id": approval_ref.get("chat_id").cloned().unwrap_or(Value::Null)
                }));
            }
            let group_message = match self.canonical_chat_message(chat_id, &group_message) {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!(%error, chat_id, "failed to sequence takeover group card");
                    return;
                }
            };
            if let Err(error) = self.store.append_jsonl(
                format!("data/chats/{}/messages.jsonl", safe_component(chat_id)),
                &group_message,
            ) {
                tracing::error!(%error, chat_id, "failed to persist takeover group card");
            }
            self.inner
                .emit(ExecutionEvent {
                    event: "message.updated".into(),
                    data: json!({"message":group_message}),
                    persistent: true,
                })
                .await;
        }
    }
}

#[async_trait]
impl ExecutionSink for OrchestratorSink {
    async fn emit(&self, event: ExecutionEvent) {
        if event.event == "message.created" {
            if let Some(message) = event.data.get("message") {
                if let Some(block) =
                    message
                        .get("blocks")
                        .and_then(Value::as_array)
                        .and_then(|blocks| {
                            blocks.iter().find(|block| {
                                block.get("type").and_then(Value::as_str) == Some("question")
                            })
                        })
                {
                    self.persist_execution_question(message, block).await;
                } else if let Some(block) = message
                    .get("blocks")
                    .and_then(Value::as_array)
                    .and_then(|blocks| {
                        blocks.iter().find(|block| {
                            block.get("type").and_then(Value::as_str) == Some("takeover_request")
                        })
                    })
                {
                    self.persist_execution_takeover(message, block).await;
                }
            }
        }
        self.inner.emit(event).await;
    }

    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        self.inner.send_group_message(message).await
    }

    async fn approval_required(&self, data: Value) {
        let Some(approval) = data.get("approval") else {
            tracing::error!("execution emitted malformed approval request");
            return;
        };
        let created = match self
            .orchestrator
            .rpc("approval.request", approval.clone())
            .await
        {
            Ok(value) => value.get("approval").cloned().unwrap_or(value),
            Err(error) => {
                tracing::error!(%error, "failed to persist execution approval");
                return;
            }
        };
        if let Ok(snapshot) = self.orchestrator.snapshot() {
            let _ = self.store.append_jsonl(
                "data/orchestrator/operations.jsonl",
                &json!({"method":"execution.approval.request","params":approval,"result":{"approval":created},"snapshot":snapshot,"status":"done","at":crate::now()}),
            );
            let _ = self
                .store
                .write_snapshot("data/orchestrator/state.json", &snapshot);
        }
        if let Some(call_id) = approval.get("id").and_then(Value::as_str) {
            if let Some(approval_id) = created.get("id").and_then(Value::as_str) {
                let _ = self.store.write_snapshot(
                    format!("data/approval-map/{}.json", safe_component(approval_id)),
                    &json!({"call_id": call_id.strip_prefix("apr_").unwrap_or(call_id)}),
                );
            }
        }
        self.inner
            .emit(ExecutionEvent {
                event: "approval.requested".into(),
                data: json!({"approval": created}),
                persistent: true,
            })
            .await;
    }

    async fn assignment_usage(&self, assignment_id: &str, usage: Value) {
        match self.backend.update_assignment_usage(assignment_id, &usage) {
            Ok(result) => {
                self.inner
                    .emit(ExecutionEvent {
                        event: "assignment.updated".into(),
                        data: result,
                        persistent: true,
                    })
                    .await;
            }
            Err(error) => {
                tracing::warn!(%error, %assignment_id, "failed to persist assignment usage")
            }
        }
    }
}

#[async_trait]
impl GroupMessageBridge for OrchestratorGroupBridge {
    async fn send_msg(&self, message: Value) -> Result<Value, String> {
        let result = self
            .backend
            .execution_send_msg(&self.state, message.clone())
            .await
            .map_err(|error| error.to_string())?;
        let result = result.get("message").cloned().unwrap_or(result);
        if let Some(run_id) = message.pointer("/receipt/run_id").and_then(Value::as_str) {
            for artifact in result
                .get("artifacts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(url) = artifact.get("path_or_url").and_then(Value::as_str) else {
                    continue;
                };
                if !self
                    .jobs
                    .register_service_for(run_id, url)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    tracing::warn!(%run_id, %url, "url artifact did not match a run-owned local service");
                }
            }
        }
        Ok(result)
    }
}

struct UnavailableProvider;

#[async_trait]
impl ModelProvider for UnavailableProvider {
    async fn stream(
        &self,
        _request: ModelRequest,
        _events: mpsc::Sender<ModelEvent>,
    ) -> macbot_providers::Result<Completion> {
        Err(macbot_providers::Error::Response(
            "no enabled model is configured".into(),
        ))
    }
}

#[cfg(test)]
mod model_resolution_tests {
    use super::{
        missing_model_bot_from_snapshot, project_assignment_targets, resolve_model,
        routable_missing_model_chat_from_snapshot, ModelRole,
    };
    use serde_json::json;

    #[test]
    fn null_bot_models_use_live_role_defaults() {
        let snapshot = json!({
            "bots": {
                "main": {"id":"main", "is_main":true, "model":null},
                "worker": {"id":"worker", "is_main":false, "model":null}
            }
        });
        let settings = json!({
            "models": {
                "main": "fake/main",
                "bot_default": "fake/worker"
            }
        });
        assert_eq!(
            resolve_model(&snapshot, &settings, "main", ModelRole::Bot).as_deref(),
            Some("fake/main")
        );
        assert_eq!(
            resolve_model(&snapshot, &settings, "worker", ModelRole::Bot).as_deref(),
            Some("fake/worker")
        );
    }

    #[test]
    fn explicit_bot_model_overrides_default_and_missing_is_none() {
        let snapshot = json!({
            "bots": {
                "worker": {"id":"worker", "is_main":false, "model":"fake/special"},
                "empty": {"id":"empty", "is_main":false, "model":null}
            }
        });
        let settings = json!({"models":{"bot_default":null}});
        assert_eq!(
            resolve_model(&snapshot, &settings, "worker", ModelRole::Bot).as_deref(),
            Some("fake/special")
        );
        assert_eq!(
            resolve_model(&snapshot, &settings, "empty", ModelRole::Bot),
            None
        );
    }

    #[test]
    fn maintenance_null_uses_bot_default_and_inherit_uses_main() {
        let snapshot = json!({
            "bots": {"main": {"id":"main", "is_main":true, "model":null}}
        });
        let mut settings = json!({
            "models": {"main":"fake/main", "bot_default":"fake/worker", "maintenance":null}
        });
        assert_eq!(
            resolve_model(&snapshot, &settings, "main", ModelRole::Maintenance).as_deref(),
            Some("fake/worker")
        );
        settings["models"]["maintenance"] = json!("inherit");
        assert_eq!(
            resolve_model(&snapshot, &settings, "main", ModelRole::Maintenance).as_deref(),
            Some("fake/main")
        );
    }

    #[test]
    fn missing_model_dm_resolves_worker_and_main_bots_without_provider() {
        let snapshot = json!({
            "bots": {
                "main": {"id":"main", "is_main":true, "dm_chat_id":"chat_main"},
                "worker": {"id":"worker", "is_main":false, "dm_chat_id":"dm_worker"}
            }
        });
        assert_eq!(
            missing_model_bot_from_snapshot(&snapshot, "dm_worker", None),
            "worker"
        );
        assert_eq!(
            missing_model_bot_from_snapshot(&snapshot, "chat_main", None),
            "main"
        );
    }

    #[test]
    fn project_cancellation_targets_queued_before_running_work() {
        let snapshot = json!({
            "assignments": {
                "working": {"id":"working", "project_id":"project", "bot_id":"worker", "status":"working"},
                "queued": {"id":"queued", "project_id":"project", "bot_id":"worker", "status":"queued"},
                "waiting": {"id":"waiting", "project_id":"project", "bot_id":"worker", "status":"waiting_user"},
                "blocked": {"id":"blocked", "project_id":"project", "bot_id":"worker", "status":"blocked"},
                "done": {"id":"done", "project_id":"project", "bot_id":"worker", "status":"done"},
                "other": {"id":"other", "project_id":"project", "bot_id":"other", "status":"working"}
            }
        });
        let targets = project_assignment_targets(&snapshot, "project", Some("worker"));
        assert_eq!(
            targets,
            vec![
                ("queued".into(), "queued".into()),
                ("working".into(), "working".into()),
                ("waiting".into(), "waiting_user".into()),
                ("blocked".into(), "blocked".into())
            ]
        );
    }

    #[test]
    fn missing_model_notice_uses_project_or_bot_chat() {
        let snapshot = json!({
            "bots": {
                "worker": {"id":"worker", "dm_chat_id":"dm_worker"},
                "main": {"id":"main", "is_main":true, "dm_chat_id":"chat_main"}
            },
            "projects": {
                "project": {"id":"project", "chat_id":"chat_project"}
            },
            "assignments": {
                "assignment": {"id":"assignment", "project_id":"project"}
            }
        });
        assert_eq!(
            routable_missing_model_chat_from_snapshot(
                &snapshot,
                "worker",
                "routine:routine",
                "assignment"
            ),
            "chat_project"
        );
        assert_eq!(
            routable_missing_model_chat_from_snapshot(
                &snapshot,
                "worker",
                "routine:routine",
                "unknown"
            ),
            "dm_worker"
        );
    }
}
