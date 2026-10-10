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
        invalid_memory_target_args, invalid_pending_tool_args, ExecutionEngine, ExecutionError,
        ExecutionEvent, ExecutionOutcome, ExecutionRequest, ExecutionSink, ExecutionState,
        GatewayStateSink, GroupMessageBridge, ProviderResolver,
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
            recovery_backend.recover_invalid_tool_approvals().await;
            recovery_backend.recover_answered_decisions().await;
            recovery_backend.dispatch_ready_assignments().await;
        });
        Ok(backend)
    }

    /// Invalid target arguments cannot authorize a side effect. Retire only
    /// the exactly mapped old approval, then return a tool error to its run.
    async fn recover_invalid_tool_approvals(&self) {
        let Ok(snapshot) = self.inner.orchestrator.snapshot() else {
            return;
        };
        let Some(approvals) = snapshot.get("approvals").and_then(Value::as_object) else {
            return;
        };
        let Ok(entries) = fs::read_dir(self.inner.store.root().join("data/jobs")) else {
            return;
        };
        let jobs = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                (entry.path().extension().and_then(|x| x.to_str()) == Some("json"))
                    .then(|| {
                        fs::File::open(entry.path()).ok().and_then(|file| {
                            serde_json::from_reader::<_, macbot_durable::Job>(file).ok()
                        })
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        if let Err(error) = self.runtime.configure_feature_runtime().await {
            tracing::warn!(%error, "cannot configure invalid memory recovery");
            return;
        }
        let mut claimed_runs = HashSet::new();
        for (approval_id, approval) in approvals {
            if !matches!(
                approval.get("tool").and_then(Value::as_str),
                Some("memory" | "memory_search" | "write" | "edit" | "skill_draft")
            ) || !matches!(
                approval.get("state").and_then(Value::as_str),
                Some("pending" | "expired")
            ) {
                continue;
            }
            let Some(args) = approval
                .get("detail")
                .and_then(Value::as_str)
                .and_then(|detail| serde_json::from_str::<Value>(detail).ok())
            else {
                continue;
            };
            let tool_name = approval["tool"].as_str().unwrap_or_default();
            let syntax_error = invalid_pending_tool_args(tool_name, &args, None);
            let target_error = matches!(tool_name, "memory" | "memory_search")
                .then(|| invalid_memory_target_args(tool_name, &args, &snapshot))
                .flatten();
            if (syntax_error.is_none() && target_error.is_none())
                || (matches!(tool_name, "write" | "edit") && args.get("resolved_path").is_some())
            {
                continue;
            }
            let mapped = self
                .inner
                .store
                .read_snapshot::<Value>(format!(
                    "data/approval-map/{}.json",
                    safe_component(approval_id)
                ))
                .ok()
                .flatten();
            let Some(mapped_call) = mapped
                .as_ref()
                .and_then(|value| value.get("call_id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let mut candidates = Vec::new();
            for job in &jobs {
                if !matches!(
                    job.status,
                    macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
                ) {
                    continue;
                }
                let Some(call) = job.checkpoint.get("pending_tool") else {
                    continue;
                };
                let Some(call_id) = call.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                if normalize_call_id(mapped_call) != normalize_call_id(call_id)
                    || call.get("name") != approval.get("tool")
                    || call.get("args") != Some(&args)
                    || job
                        .checkpoint
                        .get("pending_tools")
                        .and_then(Value::as_array)
                        .is_some_and(|pending| !pending.is_empty() && pending.first() != Some(call))
                {
                    continue;
                }
                let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(Some(request)) = self
                    .inner
                    .store
                    .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                else {
                    continue;
                };
                if request.run_id != run_id
                    || !invalid_tool_approval_request_matches(&snapshot, approval, &request)
                {
                    continue;
                }
                candidates.push((request, call_id.to_owned()));
            }
            if candidates.len() != 1 {
                continue;
            }
            let (request, call_id) = candidates.pop().expect("candidate length checked");
            if !claimed_runs.insert(request.run_id.clone()) {
                continue;
            }
            let Ok(engine) = self.runtime.configured_engine_for(&request).await else {
                continue;
            };
            let Some(active_key) = self.reserve_run(&request) else {
                continue;
            };
            let receipt = json!({"approval_id":approval_id,"run_id":request.run_id,
                "call_id":call_id,"tool":approval["tool"],"args":args,"bot_id":request.bot_id,
                "chat_id":request.chat_id,"assignment_id":request.assignment_id});
            let expired = self
                .inner
                .expire_invalid_tool_approval(&self.state, approval_id, &receipt)
                .await;
            if !matches!(expired, Ok(true)) {
                if let Ok(mut active) = self.active_runs.lock() {
                    active.remove(&active_key);
                }
                continue;
            }
            let assignment_id = request.assignment_id.clone();
            let run_id = request.run_id.clone();
            self.mark_waiting(assignment_id.as_deref(), &request.run_id, true);
            let _ = self.feature_service.begin_memory_run(&request.run_id);
            let continuation = engine.reject_invalid_pending_tool(request, &call_id).await;
            if let Ok(mut active) = self.active_runs.lock() {
                active.remove(&active_key);
            }
            match continuation {
                Ok(Some(outcome)) => {
                    self.mark_waiting(
                        assignment_id.as_deref(),
                        &outcome.run_id,
                        matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended"),
                    );
                    if let Some(assignment_id) = assignment_id {
                        if outcome.status == "done" {
                            self.finish_assignment_and_resume_parent(assignment_id, outcome.text)
                                .await;
                        } else if matches!(outcome.status.as_str(), "failed" | "cancelled") {
                            fail_assignment(
                                self.inner.clone(),
                                self.state.clone(),
                                assignment_id,
                                &outcome.status,
                            )
                            .await;
                        }
                    }
                }
                Ok(None) => {
                    self.mark_waiting(assignment_id.as_deref(), &run_id, false);
                    tracing::warn!(%approval_id, "invalid tool checkpoint changed before recovery");
                }
                Err(error) => {
                    self.mark_waiting(assignment_id.as_deref(), &run_id, false);
                    tracing::warn!(%approval_id, %error, "invalid tool continuation failed");
                    if let Some(assignment_id) = assignment_id {
                        if assignment_status(self, &assignment_id)
                            .ok()
                            .is_some_and(|status| {
                                matches!(status.as_str(), "working" | "waiting_user")
                            })
                        {
                            fail_assignment(
                                self.inner.clone(),
                                self.state.clone(),
                                assignment_id,
                                "failed",
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    /// Continue decisions which were answered before a crash but whose durable
    /// job remained in a safe waiting checkpoint.  This path is deliberately
    /// narrower than normal job recovery: an answered Question, its canonical
    /// message, the exact run request, and a safe decision checkpoint must all
    /// agree before the original run is resumed.
    async fn recover_answered_decisions(&self) {
        let Ok(snapshot) = self.inner.orchestrator.snapshot() else {
            return;
        };
        let Some(questions) = snapshot.get("questions").and_then(Value::as_object) else {
            return;
        };
        let jobs_dir = self.inner.store.root().join("data/jobs");
        let Ok(entries) = fs::read_dir(&jobs_dir) else {
            return;
        };
        let entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        let mut claimed_runs = HashSet::new();
        for (question_id, question) in questions {
            if question
                .get("options")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
            {
                continue;
            }
            let Some(answer) = answered_decision_answer(question) else {
                continue;
            };
            let messages = snapshot
                .get("messages")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|items| items.values())
                .filter(|message| {
                    message.get("question_id").and_then(Value::as_str) == Some(question_id)
                        && message.get("intent").and_then(Value::as_str) == Some("decision")
                        && message.get("options").and_then(Value::as_array)
                            == question.get("options").and_then(Value::as_array)
                })
                .collect::<Vec<_>>();
            if messages.len() != 1 {
                tracing::warn!(%question_id, count = messages.len(), "answered decision has ambiguous canonical message");
                continue;
            }
            let message = messages[0];
            let Some(message_id) = message.get("id").and_then(Value::as_str) else {
                continue;
            };
            let mut candidates = Vec::new();
            for entry in &entries {
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Ok(file) = fs::File::open(entry.path()) else {
                    continue;
                };
                let Ok(job) = serde_json::from_reader::<_, macbot_durable::Job>(file) else {
                    continue;
                };
                let status = match job.status {
                    macbot_durable::JobStatus::Waiting => "waiting",
                    macbot_durable::JobStatus::Suspended => "suspended",
                    _ => continue,
                };
                if !answered_decision_checkpoint_is_safe(
                    &job.checkpoint,
                    job.unsafe_replay,
                    status,
                    message_id,
                ) {
                    continue;
                }
                let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(Some(request)) = self
                    .inner
                    .store
                    .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                else {
                    continue;
                };
                if request.run_id != run_id
                    || !answered_decision_request_matches(&snapshot, question, message, &request)
                {
                    continue;
                }
                candidates.push(request);
            }
            if candidates.len() != 1 {
                tracing::warn!(%question_id, count = candidates.len(), "answered decision has ambiguous durable job");
                continue;
            }
            let request = candidates.pop().expect("candidate length checked");
            if !claimed_runs.insert(request.run_id.clone()) {
                continue;
            }
            let Some(active_key) = self.reserve_run(&request) else {
                continue;
            };
            let assignment_id = request.assignment_id.clone();
            self.mark_waiting(assignment_id.as_deref(), &request.run_id, true);
            let continuation = self.runtime.resume_question(question_id, answer).await;
            if let Ok(mut active) = self.active_runs.lock() {
                active.remove(&active_key);
            }
            match continuation {
                Ok(Some((returned_assignment, outcome))) => {
                    let assignment_id = returned_assignment.or(assignment_id);
                    self.mark_waiting(
                        assignment_id.as_deref(),
                        &outcome.run_id,
                        matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended"),
                    );
                    if outcome.status == "done" {
                        if let Some(assignment_id) = assignment_id {
                            self.finish_assignment_and_resume_parent(assignment_id, outcome.text)
                                .await;
                        }
                    } else if matches!(outcome.status.as_str(), "failed" | "cancelled") {
                        if let Some(assignment_id) = assignment_id {
                            fail_assignment(
                                self.inner.clone(),
                                self.state.clone(),
                                assignment_id,
                                &outcome.status,
                            )
                            .await;
                        }
                    }
                }
                Ok(None) => {
                    tracing::warn!(%question_id, run_id = %request.run_id, "answered decision had no resumable job");
                    self.mark_waiting(assignment_id.as_deref(), &request.run_id, false);
                    // Do not leave this exact active assignment indefinitely
                    // working if its continuation could not be delivered.
                    // A concurrently changed or terminal assignment is left alone.
                    if self
                        .inner
                        .orchestrator
                        .snapshot()
                        .ok()
                        .is_some_and(|current| {
                            answered_decision_request_matches(&current, question, message, &request)
                        })
                    {
                        if let Some(assignment_id) = assignment_id {
                            fail_assignment(
                                self.inner.clone(),
                                self.state.clone(),
                                assignment_id,
                                "failed",
                            )
                            .await;
                        }
                    }
                }
                Err(error) => {
                    tracing::error!(%question_id, run_id = %request.run_id, %error, "answered decision recovery failed");
                    self.mark_waiting(assignment_id.as_deref(), &request.run_id, false);
                    if let Some(assignment_id) = assignment_id {
                        fail_assignment(
                            self.inner.clone(),
                            self.state.clone(),
                            assignment_id,
                            "failed",
                        )
                        .await;
                    }
                }
            }
        }
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
        // Only hold the durable mutex while collecting the small set of
        // active run ids.  Request files contain the full model context and
        // can be megabytes, so parsing them under this Tokio mutex stalls
        // chat.send and other execution paths.
        let active_run_ids = {
            let durable = self.runtime.state.durable.lock().await;
            durable
                .jobs()
                .filter(|job| {
                    matches!(
                        job.status,
                        macbot_durable::JobStatus::Queued | macbot_durable::JobStatus::Running
                    )
                })
                .filter_map(|job| {
                    job.checkpoint
                        .get("run_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        };
        let request_store = self.runtime.store.clone();
        let active_requests = tokio::task::spawn_blocking(move || {
            active_run_ids
                .into_iter()
                .filter_map(|run_id| {
                    request_store
                        .read_snapshot::<ExecutionRequest>(format!(
                            "data/run_requests/{run_id}.json"
                        ))
                        .ok()
                        .flatten()
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        for request in active_requests {
            active_bots.insert(request.bot_id);
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
        if let Err(error) = self
            .inner
            .refresh_project_attention(&self.state, Utc::now())
            .await
        {
            tracing::warn!(%error, "project attention refresh failed during feature tick");
        }
        self.dispatch_ready_assignments().await;
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
            self.chat_model_messages(&snapshot, &chat_id, &bot_id, message_id, history.as_ref());
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
        bot_id: &str,
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
            .filter_map(|message| Self::chat_message_to_model(message, bot_id))
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

    fn chat_message_to_model(message: Value, bot_id: &str) -> Option<Value> {
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
        let sender_bot_id = message
            .get("sender")
            .and_then(|sender| sender.get("bot_id"))
            .and_then(Value::as_str);
        let other_sender =
            sender_kind == "system" || (sender_kind == "bot" && sender_bot_id != Some(bot_id));
        let role = if other_sender {
            "user"
        } else {
            message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or(if sender_kind == "user" {
                    "user"
                } else {
                    "assistant"
                })
        };
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .or_else(|| message.get("text").and_then(Value::as_str))
            .or_else(|| message.get("fallback_text").and_then(Value::as_str))
            .unwrap_or_default();
        if content.is_empty() && message.get("tool_calls").is_none() {
            return None;
        }
        // Another Bot's group message is incoming context, never this Bot's
        // own assistant turn. Preserve attribution to prevent role confusion.
        let content = if other_sender {
            format!(
                "[Incoming chat message; sender={sender_kind}; bot_id={}; message_id={id}]\n{content}",
                sender_bot_id.unwrap_or("none")
            )
        } else {
            content.to_owned()
        };
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
            if let Some(value) = message.get(key).filter(|_| !other_sender) {
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
        let mut messages = self.chat_model_messages(&snapshot, &chat_id, &bot_id, None, None);
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

    /// Queue pumping happens inside the orchestrator mutation that just
    /// finished another assignment.  The promoted assignment therefore has
    /// no adapter RPC of its own to publish `assignment.updated`.  Consult
    /// the durable event tail before emitting that transition so repeated
    /// scheduler ticks and a restart remain idempotent.
    fn assignment_status_event_needed(events: &[macbot_store::Event], assignment: &Value) -> bool {
        let Some(assignment_id) = assignment.get("id").and_then(Value::as_str) else {
            return false;
        };
        let Some(status) = assignment.get("status").and_then(Value::as_str) else {
            return false;
        };
        events
            .iter()
            .rev()
            .find(|event| {
                matches!(
                    event.event.as_str(),
                    "assignment.created" | "assignment.updated"
                ) && event
                    .data
                    .get("assignment")
                    .and_then(|value| value.get("id"))
                    .and_then(Value::as_str)
                    == Some(assignment_id)
            })
            .and_then(|event| {
                event
                    .data
                    .get("assignment")
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
            })
            != Some(status)
    }

    async fn publish_assignment_status(
        inner: &ProductionBackend,
        state: &GatewayState,
        assignment: &Value,
    ) {
        let Ok(event) = inner
            .store
            .append_event("assignment.updated", json!({"assignment": assignment}))
        else {
            tracing::warn!("failed to persist queued assignment promotion event");
            return;
        };
        state
            .publish_event(event.seq, &event.event, event.data)
            .await;
    }

    /// Dispatch all currently admitted assignments.  The orchestrator remains
    /// the source of truth for concurrency and queue promotion; this method
    /// only starts `working` assignments and uses `active_runs` to prevent a
    /// duplicate spawn during overlapping RPC/event callbacks.
    async fn dispatch_ready_assignments(&self) {
        let status_events = self.inner.store.events_since(0).unwrap_or_default();
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
            let Some(active_key) = self.reserve_run(&request) else {
                continue;
            };
            if Self::assignment_status_event_needed(&status_events, &assignment) {
                Self::publish_assignment_status(&self.inner, &self.state, &assignment).await;
            }
            self.spawn_reserved_request(request, active_key);
        }
    }

    async fn finalize_project_before_confirmation(&self, project_id: &str) -> crate::RpcResult {
        self.runtime
            .finalize_project_summary_before_confirmation(project_id)
            .await
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

    async fn finish_assignment_and_resume_parent(&self, assignment_id: String, text: String) {
        // A child can itself be a coordination task.  Walk the durable
        // parent links iteratively so a chain of completed subagents does not
        // require recursive async futures.  The parent must still be waiting
        // on a decision with a concrete message id; we never infer a target
        // from the child text.
        let mut completed_id = assignment_id;
        let mut completed_text = text;
        loop {
            let snapshot = self.inner.orchestrator.snapshot().ok();
            // Work-mode replies are the durable send_msg result. Model text
            // after that tool is not the message delivered to the parent.
            if let Some(text) = snapshot
                .as_ref()
                .and_then(|snapshot| assignment_result_text(snapshot, &completed_id))
            {
                completed_text = text;
            }
            let parent = snapshot
                .as_ref()
                .and_then(|snapshot| waiting_parent_for_child(snapshot, &completed_id));
            let parent_wait_message_id = snapshot.as_ref().and_then(|snapshot| {
                parent
                    .as_deref()
                    .and_then(|parent_id| waiting_message_id_for_assignment(snapshot, parent_id))
            });

            finish_assignment(self.inner.clone(), self.state.clone(), completed_id.clone()).await;
            if completed_text.trim().is_empty() {
                return;
            }
            let Some(parent_id) = parent else {
                return;
            };
            if snapshot.as_ref().is_some_and(|snapshot| {
                matches!(
                    parent_wait_status(snapshot, &parent_id),
                    Some("waiting_bot" | "waiting_user")
                )
            }) {
                self.answer_parent_decision_question(&parent_id, &completed_text)
                    .await;
            }
            match self
                .runtime
                .resume_message_for_decision(
                    &parent_id,
                    parent_wait_message_id.as_deref(),
                    completed_text,
                )
                .await
            {
                Ok(Some((parent_assignment, outcome))) => {
                    self.mark_waiting(
                        parent_assignment.as_deref().or(Some(parent_id.as_str())),
                        &outcome.run_id,
                        matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended"),
                    );
                    let Some(parent_assignment) = parent_assignment else {
                        return;
                    };
                    if outcome.status == "done" {
                        completed_id = parent_assignment;
                        completed_text = outcome.text;
                    } else if matches!(outcome.status.as_str(), "failed" | "cancelled") {
                        fail_assignment(
                            self.inner.clone(),
                            self.state.clone(),
                            parent_assignment,
                            &outcome.status,
                        )
                        .await;
                        return;
                    } else {
                        return;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        %parent_id,
                        %completed_id,
                        "parent decision continuation failed"
                    );
                    return;
                }
            }
        }
    }

    /// A child can finish before its parent's durable checkpoint reaches the
    /// waiting state.  Once the parent checkpoint is committed, sweep only
    /// its explicitly linked, already-done children and feed their canonical
    /// result back through the same resume path.
    async fn resume_completed_children(&self, parent_id: &str) {
        let Some(snapshot) = self.inner.orchestrator.snapshot().ok() else {
            return;
        };
        let children = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|assignments| assignments.values())
            .filter(|assignment| {
                assignment
                    .get("parent_assignment_id")
                    .and_then(Value::as_str)
                    == Some(parent_id)
                    && assignment.get("status").and_then(Value::as_str) == Some("done")
            })
            .filter_map(|assignment| {
                let id = assignment.get("id").and_then(Value::as_str)?;
                let text = assignment_result_text(&snapshot, id)?;
                Some((id.to_owned(), text))
            })
            .collect::<Vec<_>>();
        for (child_id, text) in children {
            self.finish_assignment_and_resume_parent(child_id, text)
                .await;
        }
    }

    async fn answer_parent_decision_question(&self, parent_id: &str, text: &str) {
        let Some(snapshot) = self.inner.orchestrator.snapshot().ok() else {
            return;
        };
        let Some(message_id) = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .and_then(|assignments| assignments.get(parent_id))
            .and_then(|assignment| assignment.get("wait"))
            .and_then(|wait| wait.get("message_id"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let Some(question_id) = pending_decision_question_id(&snapshot, parent_id, message_id)
        else {
            return;
        };
        let answer_result = if parent_wait_status(&snapshot, parent_id) == Some("waiting_bot") {
            self.inner
                .orchestrator
                .answer_decision_for_child(parent_id, text.to_owned())
                .map(|question| {
                    question
                        .map(|question| json!(question))
                        .unwrap_or(Value::Null)
                })
        } else {
            self.inner
                .orchestrator
                .rpc(
                    "question.answer",
                    json!({"question_id":question_id,"text":text}),
                )
                .await
        };
        if let Err(error) = answer_result {
            tracing::warn!(%error, %parent_id, %question_id, "failed to answer child decision question");
            return;
        }
        let result = self
            .inner
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("questions")
                    .and_then(Value::as_object)
                    .and_then(|questions| questions.get(&question_id))
                    .cloned()
            })
            .unwrap_or_else(|| json!({"id":question_id,"state":"answered"}));
        if let Err(error) = self
            .inner
            .persist_orchestrator(json!({
                "method":"question.answer",
                "params":{"question_id":question_id,"text":text,"parent_assignment_id":parent_id},
                "result":{"question":result.clone()},
                "status":"done",
                "at":crate::now()
            }))
            .await
        {
            tracing::warn!(%error, %parent_id, %question_id, "failed to persist child decision answer");
            return;
        }
        if let Ok(event) = self
            .inner
            .store
            .append_event("question.answered", json!({"question":result}))
        {
            self.state
                .publish_event(event.seq, &event.event, event.data)
                .await;
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
        // Determine liveness from the entire scope, before paging. Waiting
        // runs can resume, and an ended child does not finish its parent.
        let mut unfinished_runs = HashSet::new();
        for item in &items {
            let Some(run_id) = item.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            match item.get("type").and_then(Value::as_str) {
                Some("run.start" | "run.resume") => {
                    unfinished_runs.insert(run_id);
                }
                Some("run.end") => {
                    unfinished_runs.remove(run_id);
                }
                _ => {}
            }
        }
        let live = !unfinished_runs.is_empty();
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
            "live": live
        }))
    }

    fn reserve_run(&self, request: &ExecutionRequest) -> Option<String> {
        let active_key = request
            .assignment_id
            .clone()
            .unwrap_or_else(|| request.run_id.clone());
        let active_runs = self.active_runs.clone();
        let Ok(mut active) = active_runs.lock() else {
            tracing::warn!(run_id = %request.run_id, "execution scheduler lock poisoned");
            return None;
        };
        if !active.insert(active_key.clone()) {
            return None;
        }
        Some(active_key)
    }

    fn spawn_request(&self, request: ExecutionRequest) {
        let Some(active_key) = self.reserve_run(&request) else {
            return;
        };
        self.spawn_reserved_request(request, active_key);
    }

    fn spawn_reserved_request(&self, request: ExecutionRequest, active_key: String) {
        let run_id = request.run_id.clone();
        let active_runs = self.active_runs.clone();
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
            match runtime.run(request).await {
                Ok(outcome) if outcome.status == "done" => {
                    scheduler.mark_waiting(assignment_id.as_deref(), &run_id, false);
                    if let Some(assignment_id) = assignment_id {
                        reconcile_steers(&runtime, &inner, &state, &assignment_id).await;
                        scheduler
                            .finish_assignment_and_resume_parent(assignment_id, outcome.text)
                            .await;
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
                    if matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended") {
                        if let Some(parent_id) = assignment_id.as_deref() {
                            scheduler.resume_completed_children(parent_id).await;
                        }
                    }
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
        if let Err(error) = inner
            .persist_orchestrator(json!({
                "method": "execution.finish",
                "params": {"assignment_id": assignment_id},
                "result": {"assignment": assignment},
                "status": status,
                "at": crate::now()
            }))
            .await
        {
            tracing::warn!(%error, %assignment_id, "failed to persist execution finish");
        }
        if let Ok(event) = inner
            .store
            .append_event("assignment.updated", json!({"assignment":assignment}))
        {
            state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        if let Some(project_id) = assignment.project_id.as_deref() {
            if let Err(error) = inner.refresh_project_events(&state, project_id).await {
                tracing::warn!(%error, %project_id, "failed to refresh project announcement after assignment finish");
            }
        }
        if let Err(error) = inner.refresh_project_attention(&state, Utc::now()).await {
            tracing::warn!(%error, %assignment_id, "failed to refresh project attention after assignment finish");
        }
        mark_routine_run_for_assignment(&inner, &state, &assignment_id, status).await;
        publish_live_status(inner.clone(), &state).await;
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
        let _ = inner
            .orchestrator
            .mark_steer_for_assignment(message_id, assignment_id, "read");
    }
    if let Err(error) = inner
        .persist_orchestrator(json!({
            "method": "execution.steer.read",
            "params": {"assignment_id": assignment_id},
            "result": {"assignment_id": assignment_id, "message_ids": applied},
            "status": "done",
            "at": crate::now()
        }))
        .await
    {
        tracing::warn!(%error, %assignment_id, "failed to persist steer reconciliation");
        return;
    }
    if let Ok(snapshot) = inner.orchestrator.snapshot() {
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
    cancel_assignment_engine_with_runtime(&backend.runtime, assignment_id, status).await
}

async fn cancel_assignment_engine_with_runtime(
    runtime: &RuntimeExecution,
    assignment_id: &str,
    status: &str,
) -> crate::RpcResult {
    if status == "queued" || matches!(status, "done" | "failed" | "cancelled") {
        return Ok(json!({}));
    }
    let outcome = runtime
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

fn waiting_assignment_for_message(snapshot: &Value, message_id: &str) -> Option<String> {
    snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| {
            assignments.values().find_map(|assignment| {
                let result_message = assignment.get("result_message_id").and_then(Value::as_str);
                let wait_message = assignment
                    .get("wait")
                    .and_then(|wait| wait.get("message_id"))
                    .and_then(Value::as_str);
                (matches!(
                    assignment.get("status").and_then(Value::as_str),
                    Some("waiting_user" | "waiting_bot" | "blocked")
                ) && (result_message == Some(message_id) || wait_message == Some(message_id)))
                .then(|| {
                    assignment
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .flatten()
            })
        })
}

fn waiting_parent_for_child(snapshot: &Value, child_id: &str) -> Option<String> {
    let assignments = snapshot.get("assignments")?.as_object()?;
    let parent_id = assignments
        .get(child_id)?
        .get("parent_assignment_id")
        .and_then(Value::as_str)?;
    let parent = assignments.get(parent_id)?;
    let wait_message_id = parent
        .get("wait")
        .and_then(|wait| wait.get("message_id"))
        .and_then(Value::as_str)?;
    let trigger_message_id = assignments
        .get(child_id)?
        .get("trigger_message_id")
        .and_then(Value::as_str)?;
    if trigger_message_id != wait_message_id {
        return None;
    }
    (matches!(
        parent.get("status").and_then(Value::as_str),
        Some("waiting_bot" | "waiting_user")
    ) && parent
        .get("wait")
        .and_then(|wait| wait.get("reason"))
        .and_then(Value::as_str)
        == Some("decision")
        && parent
            .get("wait")
            .and_then(|wait| wait.get("message_id"))
            .and_then(Value::as_str)
            .is_some())
    .then(|| parent_id.to_owned())
}

fn waiting_message_id_for_assignment(snapshot: &Value, assignment_id: &str) -> Option<String> {
    snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| assignments.get(assignment_id))
        .and_then(|assignment| assignment.get("wait"))
        .and_then(|wait| wait.get("message_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn waiting_checkpoint_matches_message(checkpoint: &Value, message_id: &str) -> bool {
    matches!(
        checkpoint.get("waiting_reason").and_then(Value::as_str),
        Some("decision" | "blocked")
    ) && checkpoint.get("pending_tool").is_none_or(Value::is_null)
        && checkpoint
            .get("pending_tools")
            .is_none_or(|tools| tools.is_null() || tools.as_array().is_some_and(Vec::is_empty))
        && checkpoint.get("waiting_message_id").and_then(Value::as_str) == Some(message_id)
}

fn parent_wait_status<'a>(snapshot: &'a Value, parent_id: &str) -> Option<&'a str> {
    snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| assignments.get(parent_id))
        .and_then(|parent| parent.get("status"))
        .and_then(Value::as_str)
}

fn pending_decision_question_id(
    snapshot: &Value,
    parent_id: &str,
    message_id: &str,
) -> Option<String> {
    let from_message = snapshot
        .get("messages")
        .and_then(Value::as_object)
        .and_then(|messages| messages.get(message_id))
        .and_then(|message| message.get("question_id"))
        .and_then(Value::as_str)
        .filter(|question_id| {
            snapshot
                .get("questions")
                .and_then(Value::as_object)
                .and_then(|questions| questions.get(*question_id))
                .is_some_and(|question| {
                    question.get("assignment_id").and_then(Value::as_str) == Some(parent_id)
                        && question.get("state").and_then(Value::as_str) == Some("pending")
                })
        })
        .map(str::to_owned);
    from_message.or_else(|| {
        let candidates = snapshot
            .get("questions")
            .and_then(Value::as_object)
            .map(|questions| {
                questions
                    .values()
                    .filter(|question| {
                        question.get("assignment_id").and_then(Value::as_str) == Some(parent_id)
                            && question.get("state").and_then(Value::as_str) == Some("pending")
                    })
                    .filter_map(|question| question.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()
    })
}

fn decision_checkpoint_matches_question(checkpoint: &Value, message_id: &str) -> bool {
    checkpoint.get("waiting_reason").and_then(Value::as_str) == Some("decision")
        && checkpoint.get("pending_tool").is_none_or(Value::is_null)
        && checkpoint
            .get("pending_tools")
            .is_none_or(|tools| tools.is_null() || tools.as_array().is_some_and(Vec::is_empty))
        && checkpoint.get("waiting_message_id").and_then(Value::as_str) == Some(message_id)
}

fn answered_decision_answer(question: &Value) -> Option<String> {
    if question.get("state").and_then(Value::as_str) != Some("answered") {
        return None;
    }
    let answer = question.get("answer")?.as_object()?;
    if let Some(text) = answer
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
    {
        return Some(text.to_owned());
    }
    let index = answer.get("option_index").and_then(Value::as_u64)? as usize;
    question
        .get("options")
        .and_then(Value::as_array)
        .and_then(|options| options.get(index))
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

pub(crate) fn invalid_tool_approval_request_matches(
    snapshot: &Value,
    approval: &Value,
    request: &ExecutionRequest,
) -> bool {
    if approval.get("bot_id").and_then(Value::as_str) != Some(request.bot_id.as_str())
        || approval.get("chat_id").and_then(Value::as_str) != Some(request.chat_id.as_str())
        || approval.get("assignment_id").and_then(Value::as_str) != request.assignment_id.as_deref()
    {
        return false;
    }
    match request.assignment_id.as_deref() {
        Some(id) => snapshot
            .get("assignments")
            .and_then(|items| items.get(id))
            .is_some_and(|assignment| {
                assignment.get("bot_id").and_then(Value::as_str) == Some(request.bot_id.as_str())
                    && assignment.get("origin_chat_id").and_then(Value::as_str)
                        == Some(request.chat_id.as_str())
                    && matches!(
                        assignment.get("status").and_then(Value::as_str),
                        Some("working" | "waiting_user")
                    )
                    && (assignment.get("wait").is_none_or(Value::is_null)
                        || assignment.pointer("/wait/reason").and_then(Value::as_str)
                            == Some("approval"))
            }),
        None => {
            request.private
                && snapshot
                    .get("bots")
                    .and_then(|items| items.get(&request.bot_id))
                    .and_then(|bot| bot.get("dm_chat_id"))
                    .and_then(Value::as_str)
                    == Some(request.chat_id.as_str())
        }
    }
}

fn answered_decision_checkpoint_is_safe(
    checkpoint: &Value,
    unsafe_replay: bool,
    status: &str,
    message_id: &str,
) -> bool {
    matches!(status, "waiting" | "suspended")
        && !unsafe_replay
        && checkpoint.get("waiting_reason").and_then(Value::as_str) == Some("decision")
        && checkpoint.get("pending_tool").is_none_or(Value::is_null)
        && checkpoint
            .get("pending_tools")
            .is_none_or(|value| value.is_null() || value.as_array().is_some_and(Vec::is_empty))
        && checkpoint.get("waiting_message_id").and_then(Value::as_str) == Some(message_id)
}

fn same_runtime_bot_id(left: &str, right: &str) -> bool {
    left == right || (matches!(left, "main" | "bot_main") && matches!(right, "main" | "bot_main"))
}

fn answered_decision_request_matches(
    snapshot: &Value,
    question: &Value,
    message: &Value,
    request: &ExecutionRequest,
) -> bool {
    let Some(question_bot_id) = question.get("bot_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(message_bot_id) = message.get("sender").and_then(Value::as_str) else {
        return false;
    };
    let Some(message_id) = message.get("id").and_then(Value::as_str) else {
        return false;
    };
    let Some(chat_id) = message.get("chat_id").and_then(Value::as_str) else {
        return false;
    };
    if !same_runtime_bot_id(question_bot_id, &request.bot_id)
        || !same_runtime_bot_id(message_bot_id, &request.bot_id)
        || question.get("chat_id").and_then(Value::as_str) != Some(chat_id)
        || request.chat_id != chat_id
    {
        return false;
    }
    let question_assignment_id = question.get("assignment_id").and_then(Value::as_str);
    let assignment_exists = question_assignment_id.is_some_and(|id| {
        snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .is_some_and(|assignments| assignments.contains_key(id))
    });
    if assignment_exists {
        let Some(assignment_id) = question_assignment_id else {
            return false;
        };
        let Some(assignment) = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .and_then(|assignments| assignments.get(assignment_id))
        else {
            return false;
        };
        request.assignment_id.as_deref() == Some(assignment_id)
            && message.get("assignment_id").and_then(Value::as_str) == Some(assignment_id)
            && assignment.get("origin_chat_id").and_then(Value::as_str) == Some(chat_id)
            && same_runtime_bot_id(
                assignment
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                &request.bot_id,
            )
            && matches!(
                assignment.get("status").and_then(Value::as_str),
                Some("working" | "waiting_user" | "waiting_bot")
            )
            && (assignment.get("wait").is_none_or(Value::is_null)
                && assignment.get("status").and_then(Value::as_str) == Some("working")
                || assignment
                    .get("wait")
                    .and_then(Value::as_object)
                    .is_some_and(|wait| {
                        wait.get("reason").and_then(Value::as_str) == Some("decision")
                            && wait.get("message_id").and_then(Value::as_str) == Some(message_id)
                    }))
    } else {
        let Some(question_scope) = question_assignment_id else {
            return false;
        };
        let expected_scope = format!(
            "dm_{}",
            chat_id
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        );
        let bot_dm_matches = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|bots| bots.values())
            .find(|bot| {
                bot.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| same_runtime_bot_id(id, &request.bot_id))
            })
            .and_then(|bot| bot.get("dm_chat_id").and_then(Value::as_str))
            == Some(chat_id);
        question_scope == expected_scope
            && request.assignment_id.is_none()
            && request.private
            && message
                .get("assignment_id")
                .and_then(Value::as_str)
                .is_none()
            && bot_dm_matches
    }
}

fn assignment_result_text(snapshot: &Value, assignment_id: &str) -> Option<String> {
    let message_id = snapshot
        .get("assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| assignments.get(assignment_id))
        .and_then(|assignment| assignment.get("result_message_id"))
        .and_then(Value::as_str)?;
    let message = snapshot
        .get("messages")
        .and_then(Value::as_object)
        .and_then(|messages| messages.get(message_id))?;
    ["text", "content", "fallback_text"]
        .into_iter()
        .filter_map(|key| message.get(key).and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            message
                .get("blocks")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks.iter().find_map(|block| {
                        ["markdown", "text", "fallback_text"]
                            .into_iter()
                            .filter_map(|key| block.get(key).and_then(Value::as_str))
                            .find(|text| !text.trim().is_empty())
                            .map(str::to_owned)
                    })
                })
        })
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
    async fn configure_browser_for_screen(&self, bot_id: &str) -> Result<(), crate::RpcError> {
        self.runtime
            .configure_browser_for_screen(bot_id)
            .await
            .map_err(|error| crate::RpcError {
                code: "internal".into(),
                message: error.to_string(),
                details: None,
            })
    }

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
                // A repeated stop must still reconcile persisted approval
                // events, but never cancel or restart the engine a second time.
                if status == "cancelled" {
                    return self.inner.call(method, params, state).await;
                }
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
                // A late deny must be rejected before touching any active job.
                // An expired receipt can belong to a run now waiting on a new
                // corrected call; it is not permission to cancel that run.
                let snapshot = snapshot_value(self)?;
                let approval = snapshot
                    .get("approvals")
                    .and_then(|items| items.get(approval_id))
                    .ok_or_else(|| crate::rpc_error("not_found", "approval not found", None))?;
                if approval.get("state").and_then(Value::as_str) != Some("pending") {
                    return Err(crate::rpc_error(
                        "conflict",
                        "non-pending approval cannot be decided",
                        None,
                    ));
                }
                if let Some(assignment_id) = approval_assignment_id(self, approval_id)? {
                    let status = assignment_status(self, &assignment_id)?;
                    if !matches!(status.as_str(), "done" | "failed" | "cancelled") {
                        cancel_assignment_engine(self, &assignment_id, &status).await?;
                        // Keep the target approval pending until its explicit
                        // denial is recorded. A preliminary assignment.stop
                        // would expire it before approval.decide can run.
                    }
                }
            }
        }
        if method == "project.confirm_done" {
            let project_id = params
                .get("project_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    crate::rpc_error("invalid_params", "project_id is required", None)
                })?;
            self.finalize_project_before_confirmation(project_id)
                .await?;
        }
        // Keep duplication on the process-wide FeatureService so copied skill
        // metadata and the Bot mutation share one atomic registry instance.
        if method == "bot.duplicate" {
            return self
                .inner
                .duplicate_bot_with_feature_service_and_state(
                    state,
                    &params,
                    self.feature_service.clone(),
                )
                .await;
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
        if matches!(method, "takeover.start" | "takeover.release") {
            if let Some(request) = result.get("takeover_request") {
                self.inner
                    .project_takeover_message_state(&self.state, request)
                    .await?;
            }
        }
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
            let deliveries = result
                .pointer("/message/delivery")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let has_user_deliveries = !deliveries.is_empty();
            let has_queued_deliveries = deliveries
                .iter()
                .any(|delivery| delivery.get("state").and_then(Value::as_str) == Some("queued"));
            let delivery_key = result
                .pointer("/message/id")
                .and_then(Value::as_str)
                .map(|id| format!("user-delivery:{id}"));
            let fresh_delivery = match delivery_key {
                Some(key) => self.scheduled.lock().await.insert(key),
                None => false,
            };
            if fresh_delivery {
                for delivery in deliveries {
                    if delivery.get("state").and_then(Value::as_str) != Some("queued") {
                        continue;
                    }
                    let Some(assignment_id) = delivery
                        .get("assignment_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    else {
                        continue;
                    };
                    let Some(message_id) = result
                        .pointer("/message/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    else {
                        continue;
                    };
                    let text = result
                        .pointer("/message/fallback_text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        match scheduler
                            .runtime
                            .resume_user_steer(&assignment_id, &message_id, text.clone())
                            .await
                        {
                            Ok(Some((assignment, outcome))) => {
                                scheduler.mark_waiting(
                                    assignment.as_deref(),
                                    &outcome.run_id,
                                    matches!(
                                        outcome.status.as_str(),
                                        "waiting" | "blocked" | "suspended"
                                    ),
                                );
                                if outcome.status == "done" {
                                    if let Some(id) = assignment {
                                        scheduler
                                            .finish_assignment_and_resume_parent(id, outcome.text)
                                            .await;
                                    }
                                } else if matches!(outcome.status.as_str(), "failed" | "cancelled")
                                {
                                    if let Some(id) = assignment {
                                        fail_assignment(
                                            scheduler.inner.clone(),
                                            scheduler.state.clone(),
                                            id,
                                            &outcome.status,
                                        )
                                        .await;
                                    }
                                }
                                scheduler.dispatch_ready_assignments().await;
                            }
                            Ok(None) => {
                                enqueue_steer_runtime(scheduler.runtime.clone(),
                                    &json!({"steer":{"message_id":message_id,"assignment_id":assignment_id}}), &json!({"text":text})).await;
                            }
                            Err(error) => {
                                tracing::warn!(%error, %assignment_id, "user steer continuation failed")
                            }
                        }
                    });
                }
            }
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
                                waiting_assignment_for_message(&snapshot, reply_to)
                            })
                    })
                });
            if let (false, Some(assignment_id), Some(text)) = (
                has_user_deliveries,
                assignment_id,
                params
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) {
                let expected_wait_message_id = reply_to.clone().or_else(|| {
                    self.inner
                        .orchestrator
                        .snapshot()
                        .ok()
                        .and_then(|snapshot| {
                            waiting_message_id_for_assignment(&snapshot, &assignment_id)
                        })
                });
                let runtime = self.runtime.clone();
                let scheduler = self.clone();
                self.mark_waiting(Some(&assignment_id), "", false);
                resumed_waiting_message = runtime
                    .resume_message_for_decision(
                        &assignment_id,
                        expected_wait_message_id.as_deref(),
                        text,
                    )
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
                                let scheduler = scheduler.clone();
                                tokio::spawn(async move {
                                    scheduler
                                        .finish_assignment_and_resume_parent(
                                            assignment,
                                            outcome.text,
                                        )
                                        .await;
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
            if fresh && !resumed_waiting_message && !has_queued_deliveries {
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
                let approval_assignment = approval_assignment_id(self, &approval_id).ok().flatten();
                let runtime = self.runtime.clone();
                let inner = self.inner.clone();
                let state = self.state.clone();
                let scheduler = self.clone();
                tokio::spawn(async move {
                    match runtime.resume_approved(&approval_id).await {
                        Ok(Some((assignment_id, outcome))) => {
                            let assignment_id = assignment_id.or(approval_assignment.clone());
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
                                    scheduler
                                        .finish_assignment_and_resume_parent(
                                            assignment_id,
                                            outcome.text,
                                        )
                                        .await;
                                }
                            } else if matches!(outcome.status.as_str(), "failed" | "cancelled") {
                                if let Some(assignment_id) = assignment_id {
                                    fail_assignment(inner, state, assignment_id, &outcome.status)
                                        .await;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::error!(%error, "approval continuation failed");
                            if let Some(assignment_id) = approval_assignment {
                                fail_assignment(inner, state, assignment_id, "failed").await;
                            }
                        }
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
                            .and_then(|index| {
                                question
                                    .get("options")
                                    .and_then(Value::as_array)
                                    .and_then(|options| options.get(index as usize))
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                    });
                if let (Some(question_id), Some(answer)) = (question_id, answer) {
                    let question_assignment = question
                        .get("assignment_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let runtime = self.runtime.clone();
                    let inner = self.inner.clone();
                    let state = self.state.clone();
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        match runtime.resume_question(&question_id, answer).await {
                            Ok(Some((assignment_id, outcome))) => {
                                let assignment_id = assignment_id.or(question_assignment.clone());
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
                                        scheduler
                                            .finish_assignment_and_resume_parent(
                                                assignment_id,
                                                outcome.text,
                                            )
                                            .await;
                                    }
                                } else if matches!(outcome.status.as_str(), "failed" | "cancelled")
                                {
                                    if let Some(assignment_id) = assignment_id {
                                        fail_assignment(
                                            inner,
                                            state,
                                            assignment_id,
                                            &outcome.status,
                                        )
                                        .await;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::error!(%error, "question continuation failed");
                                if let Some(assignment_id) = question_assignment {
                                    fail_assignment(inner, state, assignment_id, "failed").await;
                                }
                            }
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
                                let assignment_result =
                                    assignment_result.or_else(|| Some(assignment_id.clone()));
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
                                        scheduler
                                            .finish_assignment_and_resume_parent(
                                                assignment_id,
                                                outcome.text,
                                            )
                                            .await;
                                    }
                                } else if matches!(outcome.status.as_str(), "failed" | "cancelled")
                                {
                                    if let Some(assignment_id) = assignment_result {
                                        fail_assignment(
                                            inner,
                                            state,
                                            assignment_id,
                                            &outcome.status,
                                        )
                                        .await;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::error!(%error, "takeover continuation failed");
                                fail_assignment(inner, state, assignment_id, "failed").await;
                            }
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
        if method == "loop.resolve" {
            // loop.resolve creates the promoted assignment inside the
            // orchestrator RPC; dispatch it through the same execution path
            // before returning the empty protocol result.
            self.dispatch_ready_assignments().await;
        } else if !resumed_waiting_message
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

fn project_summary(project: &Value, announcement: &Value) -> String {
    let name = project
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("未命名项目");
    let goal = project.get("goal").and_then(Value::as_str).unwrap_or("");
    let mut summary = format!("项目：{name}");
    if !goal.trim().is_empty() {
        summary.push_str(&format!("\n目标：{goal}"));
    }
    if let Some(artifacts) = announcement.get("artifacts").and_then(Value::as_array) {
        let mut items = artifacts
            .iter()
            .filter_map(|artifact| {
                let title = artifact.get("title").and_then(Value::as_str).unwrap_or("");
                let path = artifact
                    .get("path_or_url")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                (!title.is_empty() || !path.is_empty()).then(|| {
                    if path.is_empty() {
                        title.to_owned()
                    } else if title.is_empty() {
                        path.to_owned()
                    } else {
                        format!("{title} ({path})")
                    }
                })
            })
            .collect::<Vec<_>>();
        items.sort();
        items.dedup();
        if !items.is_empty() {
            summary.push_str(&format!("\n产物：{}", items.join("、")));
        }
    }
    if let Some(highlights) = announcement.get("highlights").and_then(Value::as_array) {
        let items = highlights
            .iter()
            .filter_map(|highlight| highlight.get("text").and_then(Value::as_str))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>();
        if !items.is_empty() {
            summary.push_str(&format!("\n进展：{}", items.join("；")));
        }
    }
    summary
}

fn normalize_call_id(value: &str) -> &str {
    value.strip_prefix("apr_").unwrap_or(value)
}

fn bot_tool_allowlist(
    bot: Option<&Value>,
    is_main: bool,
    _private: bool,
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
    if !is_main {
        names.push("skill_draft");
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
    if !is_main
        && config
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
    // Coordination may begin in the main DM and report into the newly
    // created project. Subagents use their own restricted tool list.
    names.push("send_msg");
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
        let state = ExecutionState::with_durable(store.clone(), backend.durable.clone());
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
        Ok(self
            .configured_engine_for(&request)
            .await?
            .run(request)
            .await?)
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

    pub(crate) async fn finalize_project_summary_before_confirmation(
        &self,
        project_id: &str,
    ) -> crate::RpcResult {
        let project_result = self
            .backend
            .orchestrator
            .rpc("project.get", json!({"project_id": project_id}))
            .await
            .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
        let project = project_result
            .get("project")
            .cloned()
            .unwrap_or(Value::Null);
        let announcement = project_result
            .get("announcement")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let bot_id = project
            .get("lead_bot_id")
            .and_then(Value::as_str)
            .or_else(|| {
                project
                    .get("members")
                    .and_then(Value::as_array)
                    .and_then(|members| members.first())
                    .and_then(|member| member.get("bot_id"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("main");
        let summary = project_summary(&project, &announcement);
        self.feature_service
            .finalize_project_summary(project_id, bot_id, &summary)
            .map(|_| json!({}))
            .map_err(|error| {
                tracing::error!(%error, %project_id, "failed to finalize project summary");
                crate::rpc_error(
                    "internal",
                    &format!("项目确认前总结写入失败：{error}"),
                    Some(json!({"project_id":project_id,"summary":summary})),
                )
            })
    }

    /// Complete a model-side finish_project through the same durable gateway
    /// path as a client confirmation. The coordination assignment is marked
    /// done first so the orchestrator's project transition cannot cancel the
    /// run that is currently executing this tool; every other live assignment
    /// is cancelled at the engine before its assignment is stopped.
    pub(crate) async fn confirm_project_for_coordination(
        &self,
        params: &Value,
        coordination_assignment_id: Option<&str>,
    ) -> crate::RpcResult {
        let project_id = params
            .get("project_id")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::rpc_error("invalid_params", "project_id is required", None))?;
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
        let project = snapshot
            .get("projects")
            .and_then(Value::as_object)
            .and_then(|projects| projects.get(project_id))
            .cloned()
            .ok_or_else(|| crate::rpc_error("not_found", "project not found", None))?;
        if !matches!(
            project.get("status").and_then(Value::as_str),
            Some("active" | "review")
        ) {
            return Err(crate::rpc_error(
                "conflict",
                "only active or review projects can be finished",
                Some(json!({"project_id":project_id})),
            ));
        }
        for (assignment_id, _initial_status) in
            project_assignment_targets(&snapshot, project_id, None)
        {
            if coordination_assignment_id == Some(assignment_id.as_str()) {
                continue;
            }
            let current = self
                .backend
                .orchestrator
                .snapshot()
                .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
            let status = current
                .get("assignments")
                .and_then(Value::as_object)
                .and_then(|assignments| assignments.get(&assignment_id))
                .and_then(|assignment| assignment.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("done");
            if !matches!(
                status,
                "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
            ) {
                continue;
            }
            cancel_assignment_engine_with_runtime(self, &assignment_id, status).await?;
            self.backend
                .call(
                    "assignment.stop",
                    json!({
                        "assignment_id": assignment_id,
                        "client_request_id": format!("project-stop:{project_id}:{assignment_id}")
                    }),
                    &self.gateway_state,
                )
                .await?;
        }

        self.finalize_project_summary_before_confirmation(project_id)
            .await?;

        if let Some(assignment_id) = coordination_assignment_id {
            let current = self
                .backend
                .orchestrator
                .snapshot()
                .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
            let belongs = current
                .get("assignments")
                .and_then(Value::as_object)
                .and_then(|assignments| assignments.get(assignment_id))
                .is_some_and(|assignment| {
                    assignment.get("project_id").and_then(Value::as_str) == Some(project_id)
                        && matches!(
                            assignment.get("status").and_then(Value::as_str),
                            Some("queued" | "working" | "waiting_user" | "waiting_bot" | "blocked")
                        )
                });
            if belongs {
                finish_assignment_with_status(
                    self.backend.clone(),
                    self.gateway_state.clone(),
                    assignment_id.to_owned(),
                    "done",
                )
                .await;
            }
        }
        let result = self
            .backend
            .call("project.confirm_done", params.clone(), &self.gateway_state)
            .await?;
        self.backend
            .refresh_project_events(&self.gateway_state, project_id)
            .await?;
        self.backend
            .refresh_project_attention(&self.gateway_state, Utc::now())
            .await?;
        Ok(result)
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
                .configured_engine_for(&request)
                .await?
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
        if let Some(result) = self.resume_decision_question(question_id, &answer).await? {
            return Ok(Some(result));
        }
        self.resume_waiting_assignment(
            question_id,
            &["ask_user", "question"],
            WaitingContinuation::Question(answer),
        )
        .await
    }

    async fn resume_decision_question(
        &self,
        question_id: &str,
        answer: &str,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.configure_feature_runtime().await?;
        let Some(snapshot) = self.backend.orchestrator.snapshot().ok() else {
            return Ok(None);
        };
        let Some(question) = snapshot
            .get("questions")
            .and_then(Value::as_object)
            .and_then(|questions| questions.get(question_id))
        else {
            return Ok(None);
        };
        let question_assignment_id = question.get("assignment_id").and_then(Value::as_str);
        let Some((message_id, chat_id)) = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .and_then(|messages| {
                messages.values().find_map(|message| {
                    (message.get("question_id").and_then(Value::as_str) == Some(question_id))
                        .then(|| {
                            Some((
                                message.get("id").and_then(Value::as_str)?.to_owned(),
                                message.get("chat_id").and_then(Value::as_str)?.to_owned(),
                            ))
                        })
                        .flatten()
                })
            })
        else {
            return Ok(None);
        };
        let has_assignment = question_assignment_id.is_some_and(|id| {
            snapshot
                .get("assignments")
                .and_then(Value::as_object)
                .is_some_and(|assignments| assignments.contains_key(id))
        });
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
            ) || job.unsafe_replay
                || !decision_checkpoint_matches_question(&job.checkpoint, &message_id)
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
            let matches_request = if has_assignment {
                request.assignment_id.as_deref() == question_assignment_id
            } else {
                request.assignment_id.is_none() && request.chat_id == chat_id
            };
            if !matches_request
                || !question
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| same_runtime_bot_id(id, &request.bot_id))
            {
                continue;
            }
            let assignment = request.assignment_id.clone();
            let _ = self.feature_service.begin_memory_run(&request.run_id);
            let outcome = self
                .configured_engine_for(&request)
                .await?
                .continue_message(request, answer.to_owned())
                .await?;
            return Ok(Some((assignment, outcome)));
        }
        Ok(None)
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

    async fn resume_user_steer(
        &self,
        assignment_id: &str,
        message_id: &str,
        text: String,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        let Some(request) =
            self.find_request(|request| request.assignment_id.as_deref() == Some(assignment_id))
        else {
            return Ok(None);
        };
        let expected = {
            let durable = self.state.durable.lock().await;
            let message_id = durable
                .jobs()
                .find(|job| {
                    job.checkpoint.get("run_id").and_then(Value::as_str) == Some(&request.run_id)
                        && matches!(
                            job.status,
                            macbot_durable::JobStatus::Waiting
                                | macbot_durable::JobStatus::Suspended
                        )
                        && !job.unsafe_replay
                        && matches!(
                            job.checkpoint.get("waiting_reason").and_then(Value::as_str),
                            Some("decision" | "blocked")
                        )
                })
                .and_then(|job| {
                    job.checkpoint
                        .get("waiting_message_id")
                        .and_then(Value::as_str)
                })
                .map(str::to_owned);
            message_id
        };
        self.resume_message_for_decision_inner(
            assignment_id,
            expected.as_deref(),
            text,
            Some(message_id),
        )
        .await
    }

    pub async fn resume_message(
        &self,
        assignment_id: &str,
        message: String,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        let expected_message_id = self
            .backend
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| waiting_message_id_for_assignment(&snapshot, assignment_id));
        self.resume_message_for_decision(assignment_id, expected_message_id.as_deref(), message)
            .await
    }

    pub async fn resume_message_for_decision(
        &self,
        assignment_id: &str,
        expected_message_id: Option<&str>,
        message: String,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.resume_message_for_decision_inner(assignment_id, expected_message_id, message, None)
            .await
    }

    async fn resume_message_for_decision_inner(
        &self,
        assignment_id: &str,
        expected_message_id: Option<&str>,
        message: String,
        steer_message_id: Option<&str>,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        let Some(expected_message_id) = expected_message_id.filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
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
            ) || job.unsafe_replay
                || !waiting_checkpoint_matches_message(&job.checkpoint, expected_message_id)
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
            if let Some(message_id) = steer_message_id {
                if !self
                    .enqueue_steer_for_assignment(assignment_id, message_id, &message)
                    .await
                {
                    return Ok(None);
                }
            }
            let resume_text = if steer_message_id.is_some() {
                String::new()
            } else {
                message
            };
            let outcome = self
                .configured_engine_for(&request)
                .await?
                .continue_message(request, resume_text)
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
                    self.configured_engine_for(&request)
                        .await?
                        .continue_question(request, answer)
                        .await?
                }
                WaitingContinuation::Takeover(note) => {
                    let mut request = request;
                    if let Some(note) = note {
                        request.instruction = note;
                    }
                    self.configured_engine_for(&request)
                        .await?
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
        self.configure_browser_for_bot(&request.bot_id).await
    }

    async fn configure_browser_for_bot(&self, bot_id: &str) -> Result<(), String> {
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        let bot_exists = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .is_some_and(|bots| bots.contains_key(bot_id));
        if !bot_exists {
            return Err(format!("browser Bot {bot_id} not found"));
        }
        let settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| json!({}));
        ProductionBrowserBridge::new(self.gateway_state.clone())
            .configure_bot_from_snapshots(bot_id, &snapshot, &settings)
            .await
    }

    /// Apply the same live Bot browser configuration used by execution before
    /// a screen connection creates or restores its session. This is needed
    /// after a restart when no new run has invoked the execution path yet.
    pub async fn configure_browser_for_screen(&self, bot_id: &str) -> Result<(), RuntimeError> {
        self.configure_browser_for_bot(bot_id)
            .await
            .map_err(RuntimeError::Provider)
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

    // Every execution entry, including approval/question/message restoration,
    // needs the persisted browser configuration before any tool can create a
    // session. Otherwise a continuation after restart uses the default config
    // without its assignment-to-tab state path.
    async fn configured_engine_for(
        &self,
        request: &ExecutionRequest,
    ) -> Result<ExecutionEngine, RuntimeError> {
        self.configure_browser_for_request(request)
            .await
            .map_err(RuntimeError::Provider)?;
        self.engine_for(request)
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
        let rpc = Arc::new(
            ProductionCoordinationRpc::new(self.backend.clone(), self.gateway_state.clone())
                .with_runtime(self.clone(), request.assignment_id.clone()),
        );
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

fn is_run_status_boundary(event: &ExecutionEvent) -> bool {
    event.event == "trace.item"
        && matches!(
            event.data.pointer("/item/type").and_then(Value::as_str),
            Some("run.start" | "run.resume" | "llm.request" | "run.wait" | "run.end")
        )
}

async fn publish_live_status(inner: Arc<ProductionBackend>, state: &GatewayState) {
    // `live_status` walks every durable job and deserializes its checkpoint
    // to keep Workbench in sync.  A model context can make one checkpoint
    // megabytes large; doing that synchronous work on a Tokio worker stalls
    // the execution task that is trying to publish its next trace item.  Keep
    // the durable read off the async scheduler and only await the cheap live
    // fan-out here.
    let snapshot = match tokio::task::spawn_blocking(move || inner.live_status()).await {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(error)) => {
            tracing::warn!(%error, "failed to read live status after execution boundary");
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "live status worker failed");
            return;
        }
    };
    let host = snapshot.get("host").cloned().unwrap_or_else(|| {
        json!({
            "running": snapshot.get("running").cloned().unwrap_or(Value::Null),
            "queued": snapshot.get("queued").cloned().unwrap_or(Value::Null),
            "global_limit": snapshot.get("global_limit").cloned().unwrap_or(Value::Null),
            "subagents_running": snapshot
                .get("subagents_running")
                .cloned()
                .unwrap_or(Value::Null),
        })
    });
    state.publish_temporary("host.status", host).await;
    if let Some(bots) = snapshot.get("bots").and_then(Value::as_array) {
        for bot in bots {
            let Some(bot_id) = bot
                .get("bot_id")
                .or_else(|| bot.get("id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(status) = bot.get("status") else {
                continue;
            };
            state
                .publish_temporary("bot.status", json!({"bot_id":bot_id,"status":status}))
                .await;
        }
    }
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
        let assignment_id = message.get("assignment_id").cloned().unwrap_or(Value::Null);
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
        let source_assignment_id = message.get("assignment_id").cloned().unwrap_or(Value::Null);
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
                    "assignment_id": source_assignment_id,
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "run_id": message_id.strip_prefix("msg_takeover_").unwrap_or(message_id),
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
        let Some(request) = result.get("takeover_request") else {
            return;
        };
        let Some(dm_chat_id) = request.get("chat_id").and_then(Value::as_str) else {
            return;
        };
        let effective_assignment_id = request.get("assignment_id").cloned().unwrap_or(Value::Null);
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
            // `assignment_id` above is the effective scope used when the
            // source card has no assignment (private model messages use the
            // synthetic dm_<chat> scope). Keep the generated DM card in that
            // same scope so lifecycle projection cannot silently skip it.
            "assignment_id": effective_assignment_id,
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

pub(crate) fn transition_takeover_message(
    message: &mut Value,
    request: &Value,
    chat_id: &str,
    message_id: &str,
) -> bool {
    let Some(request_state) = request.get("state").and_then(Value::as_str) else {
        return false;
    };
    let Some(bot_id) = request.get("bot_id").and_then(Value::as_str) else {
        return false;
    };
    if !takeover_message_scope_matches(message, request, chat_id, message_id) {
        return false;
    }
    let Some(block) = message
        .get_mut("blocks")
        .and_then(Value::as_array_mut)
        .and_then(|blocks| {
            blocks.iter_mut().find(|block| {
                block.get("type").and_then(Value::as_str) == Some("takeover_request")
                    && block.get("bot_id").and_then(Value::as_str) == Some(bot_id)
            })
        })
    else {
        return false;
    };
    if block.get("state").and_then(Value::as_str) == Some(request_state) {
        return false;
    }
    block["state"] = json!(request_state);
    true
}

pub(crate) fn takeover_message_scope_matches(
    message: &Value,
    request: &Value,
    chat_id: &str,
    message_id: &str,
) -> bool {
    let (Some(assignment_id), Some(bot_id), Some(run_id), Some(original_message_id)) = (
        request.get("assignment_id").and_then(Value::as_str),
        request.get("bot_id").and_then(Value::as_str),
        request.get("run_id").and_then(Value::as_str),
        request.get("message_id").and_then(Value::as_str),
    ) else {
        return false;
    };
    let original_message_matches = original_message_id == message_id
        && safe_component(run_id) == run_id
        && original_message_id == format!("msg_takeover_{}", run_id)
        && safe_component(original_message_id) == original_message_id;
    let dm_message_matches = message_id
        == format!(
            "msg_takeover_question_{}",
            safe_component(original_message_id)
        );
    let assignment_matches = message.get("assignment_id").and_then(Value::as_str)
        == Some(assignment_id)
        || (original_message_matches
            && message.get("assignment_id").is_some_and(Value::is_null)
            && assignment_id == format!("dm_{}", safe_component(chat_id)));
    (original_message_matches || dm_message_matches)
        && message.get("chat_id").and_then(Value::as_str) == Some(chat_id)
        && assignment_matches
        && message.pointer("/sender/bot_id").and_then(Value::as_str) == Some(bot_id)
}

#[async_trait]
impl ExecutionSink for OrchestratorSink {
    async fn emit(&self, event: ExecutionEvent) {
        let refresh_status = is_run_status_boundary(&event);
        if event.event == "message.updated" {
            if let Some(message) = event.data.get("message") {
                if let (Some(id), Some(deliveries)) = (
                    message.get("id").and_then(Value::as_str),
                    message.get("delivery").and_then(Value::as_array),
                ) {
                    if !deliveries.is_empty() {
                        for delivery in deliveries {
                            if let Err(error) = self
                                .backend
                                .execution_update_steer_delivery(&self.state, id, delivery)
                                .await
                            {
                                tracing::warn!(%error, message_id = id, "failed to persist steer delivery");
                            }
                        }
                        return;
                    }
                }
            }
        }
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
        if refresh_status {
            publish_live_status(self.backend.clone(), &self.state).await;
        }
    }

    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        self.inner.send_group_message(message).await
    }

    async fn validate_send_msg_target(&self, message: &Value) -> Result<(), String> {
        self.inner.validate_send_msg_target(message).await
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
        if let Err(error) = self
            .backend
            .persist_orchestrator(json!({
                "method": "execution.approval.request",
                "params": approval,
                "result": {"approval": created},
                "status": "done",
                "at": crate::now()
            }))
            .await
        {
            tracing::error!(%error, "failed to persist execution approval operation");
            return;
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
        match self
            .backend
            .update_assignment_usage(assignment_id, &usage)
            .await
        {
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
    async fn validate_send_msg_target(&self, message: &Value) -> Result<(), String> {
        self.backend
            .execution_validate_send_msg_target(message)
            .map_err(|error| error.to_string())
    }

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
    #[test]
    fn takeover_message_projection_requires_exact_run_scope_and_updates_lifecycle() {
        let mut message = serde_json::json!({
            "id":"msg_takeover_run-1",
            "chat_id":"group-1",
            "sender":{"kind":"bot","bot_id":"bot-1"},
            "assignment_id":"assignment-1",
            "blocks":[{"type":"takeover_request","bot_id":"bot-1","reason":"登录","state":"pending"}]
        });
        let mut request = serde_json::json!({
            "message_id":"msg_takeover_run-1",
            "run_id":"run-1",
            "group_chat_id":"group-1",
            "bot_id":"bot-1",
            "assignment_id":"assignment-1",
            "state":"active"
        });

        assert!(super::transition_takeover_message(
            &mut message,
            &request,
            "group-1",
            "msg_takeover_run-1"
        ));
        assert_eq!(message["blocks"][0]["state"], "active");
        assert!(!super::transition_takeover_message(
            &mut message,
            &request,
            "group-1",
            "msg_takeover_run-1"
        ));

        request["state"] = serde_json::json!("done");
        assert!(super::transition_takeover_message(
            &mut message,
            &request,
            "group-1",
            "msg_takeover_run-1"
        ));
        assert_eq!(message["blocks"][0]["state"], "done");

        let mut unrelated = message.clone();
        unrelated["assignment_id"] = serde_json::json!("other-assignment");
        assert!(!super::transition_takeover_message(
            &mut unrelated,
            &request,
            "group-1",
            "msg_takeover_run-1"
        ));
        assert_eq!(unrelated["blocks"][0]["state"], "done");

        let mut wrong_run = message.clone();
        let mut wrong_request = request.clone();
        wrong_request["run_id"] = serde_json::json!("run-2");
        assert!(!super::transition_takeover_message(
            &mut wrong_run,
            &wrong_request,
            "group-1",
            "msg_takeover_run-1"
        ));
    }

    #[test]
    fn takeover_message_projection_updates_private_card_with_same_scope() {
        let mut message = serde_json::json!({
            "id":"msg_takeover_question_msg_takeover_run-1",
            "chat_id":"dm-bot-1",
            "sender":{"kind":"bot","bot_id":"bot-1"},
            "assignment_id":"assignment-1",
            "blocks":[
                {"type":"question","question_id":"question-1"},
                {"type":"takeover_request","bot_id":"bot-1","reason":"登录","state":"pending"}
            ]
        });
        let request = serde_json::json!({
            "message_id":"msg_takeover_run-1",
            "run_id":"run-1",
            "group_chat_id":"group-1",
            "chat_id":"dm-bot-1",
            "bot_id":"bot-1",
            "assignment_id":"assignment-1",
            "state":"done"
        });

        assert!(super::transition_takeover_message(
            &mut message,
            &request,
            "dm-bot-1",
            "msg_takeover_question_msg_takeover_run-1"
        ));
        assert_eq!(message["blocks"][1]["state"], "done");

        let mut wrong_dm = message.clone();
        assert!(!super::transition_takeover_message(
            &mut wrong_dm,
            &request,
            "dm-bot-1",
            "msg_takeover_question_msg_takeover_run-2"
        ));
    }

    #[test]
    fn group_history_keeps_other_bots_out_of_own_assistant_turns() {
        let message = serde_json::json!({
            "id":"message-1", "sender":{"kind":"bot","bot_id":"main"},
            "fallback_text":"Tester will run the browser checks.",
            "role":"assistant", "tool_calls":[{"id":"unrelated-call"}]
        });
        let incoming =
            super::ComposedBackend::chat_message_to_model(message.clone(), "tester").unwrap();
        assert_eq!(incoming["role"], "user");
        let content = incoming["content"].as_str().unwrap();
        assert!(content.contains("bot_id=main"));
        assert!(content.contains("message_id=message-1"));
        assert!(content.ends_with("Tester will run the browser checks."));
        assert!(incoming.get("tool_calls").is_none());
        let own = super::ComposedBackend::chat_message_to_model(message, "main").unwrap();
        assert_eq!(own["role"], "assistant");
        assert_eq!(own["content"], "Tester will run the browser checks.");
        assert!(own.get("tool_calls").is_some());
    }

    #[test]
    fn group_history_distinguishes_user_and_system_notifications() {
        let user = super::ComposedBackend::chat_message_to_model(
            serde_json::json!({
                "id":"user-1", "sender":{"kind":"user"}, "text":"Run the actual test."
            }),
            "tester",
        )
        .unwrap();
        assert_eq!(user["role"], "user");
        assert_eq!(user["content"], "Run the actual test.");
        let notification = super::ComposedBackend::chat_message_to_model(
            serde_json::json!({
                "id":"notice-1", "sender":{"kind":"system"}, "text":"Project is waiting."
            }),
            "tester",
        )
        .unwrap();
        assert_eq!(notification["role"], "user");
        assert!(notification["content"]
            .as_str()
            .unwrap()
            .contains("sender=system"));
    }

    #[test]
    fn main_can_report_from_a_dm_but_never_acquires_worker_tools() {
        let bot = serde_json::json!({"tools":{
            "files":true,"bash":true,"browser":true,"subagent":true
        }});
        let names = super::bot_tool_allowlist(Some(&bot), true, true, false).unwrap();
        assert!(names.iter().any(|name| name == "send_msg"));
        assert!(names.iter().any(|name| name == "create_project"));
        for name in ["read", "write", "edit", "bash", "browser_open", "subagent"] {
            assert!(!names.iter().any(|allowed| allowed == name), "{name}");
        }
        assert!(!names.iter().any(|name| name == "skill_draft"));
        let worker_names = super::bot_tool_allowlist(Some(&bot), false, true, false).unwrap();
        assert!(worker_names.iter().any(|name| name == "skill_draft"));
    }
    use super::{
        answered_decision_answer, answered_decision_checkpoint_is_safe,
        answered_decision_request_matches, assignment_result_text,
        decision_checkpoint_matches_question, is_run_status_boundary,
        missing_model_bot_from_snapshot, pending_decision_question_id, project_assignment_targets,
        project_summary, resolve_model, routable_missing_model_chat_from_snapshot,
        waiting_assignment_for_message, waiting_checkpoint_matches_message,
        waiting_message_id_for_assignment, waiting_parent_for_child, ComposedBackend,
        ExecutionEvent, ExecutionRequest, ModelRole,
    };
    use macbot_store::Event;
    use serde_json::json;

    #[test]
    fn answered_decision_recovery_uses_text_or_option_without_guessing() {
        assert_eq!(
            answered_decision_answer(&json!({
                "state":"answered",
                "options":["继续","停止"],
                "answer":{"option_index":1}
            }))
            .as_deref(),
            Some("停止")
        );
        assert_eq!(
            answered_decision_answer(&json!({
                "state":"answered",
                "options":["继续","停止"],
                "answer":{"option_index":0,"text":"用户自定义"}
            }))
            .as_deref(),
            Some("用户自定义")
        );
        assert_eq!(
            answered_decision_answer(&json!({
                "state":"answered",
                "options":[],
                "answer":{"option_index":0}
            })),
            None
        );
        assert_eq!(
            answered_decision_answer(&json!({
                "state":"pending",
                "options":["继续"],
                "answer":{"option_index":0}
            })),
            None
        );
    }

    #[test]
    fn answered_decision_recovery_rejects_unsafe_or_incomplete_jobs() {
        let safe = json!({
            "waiting_reason":"decision",
            "waiting_message_id":"message",
            "pending_tool":null,
            "pending_tools":[]
        });
        assert!(answered_decision_checkpoint_is_safe(
            &safe, false, "waiting", "message"
        ));
        assert!(!answered_decision_checkpoint_is_safe(
            &safe, true, "waiting", "message"
        ));
        assert!(!answered_decision_checkpoint_is_safe(
            &safe, false, "running", "message"
        ));
        assert!(!answered_decision_checkpoint_is_safe(
            &json!({
                "waiting_reason":"decision",
                "waiting_message_id":"message",
                "pending_tool":{"call_id":"write"},
                "pending_tools":[]
            }),
            false,
            "waiting",
            "message"
        ));
        assert!(!answered_decision_checkpoint_is_safe(
            &json!({
                "waiting_reason":"decision",
                "waiting_message_id":"message",
                "pending_tool":null,
                "pending_tools":[{"name":"write"}]
            }),
            false,
            "waiting",
            "message"
        ));
    }

    #[test]
    fn answered_decision_recovery_requires_exact_assignment_or_private_scope() {
        let request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":"run-1",
            "assignment_id":"assignment-1",
            "chat_id":"chat-1",
            "bot_id":"bot-1",
            "model":"mock/model",
            "instruction":"resume",
            "private":false
        }))
        .unwrap();
        let snapshot = json!({"assignments":{"assignment-1":{
            "id":"assignment-1",
            "bot_id":"bot-1",
            "origin_chat_id":"chat-1",
            "status":"working",
            "wait":{"reason":"decision","message_id":"message-1"}
        }}});
        let question = json!({
            "assignment_id":"assignment-1",
            "chat_id":"chat-1",
            "bot_id":"bot-1"
        });
        let message = json!({"id":"message-1","chat_id":"chat-1","sender":"bot-1","assignment_id":"assignment-1"});
        assert!(answered_decision_request_matches(
            &snapshot, &question, &message, &request
        ));
        let mut delivered_state = snapshot.clone();
        delivered_state["assignments"]["assignment-1"]["wait"] = json!(null);
        assert!(answered_decision_request_matches(
            &delivered_state,
            &question,
            &message,
            &request
        ));
        delivered_state["assignments"]["assignment-1"]["status"] = json!("waiting_bot");
        assert!(!answered_decision_request_matches(
            &delivered_state,
            &question,
            &message,
            &request
        ));
        let mut conflicting_assignment_chat = snapshot.clone();
        conflicting_assignment_chat["assignments"]["assignment-1"]["origin_chat_id"] =
            json!("other-chat");
        assert!(!answered_decision_request_matches(
            &conflicting_assignment_chat,
            &question,
            &message,
            &request
        ));
        let mut wrong_chat = request.clone();
        wrong_chat.chat_id = "other-chat".into();
        assert!(!answered_decision_request_matches(
            &snapshot,
            &question,
            &message,
            &wrong_chat
        ));
        let mut queued = snapshot.clone();
        queued["assignments"]["assignment-1"]["status"] = json!("queued");
        assert!(!answered_decision_request_matches(
            &queued, &question, &message, &request
        ));
        let mut wrong_wait = snapshot.clone();
        wrong_wait["assignments"]["assignment-1"]["wait"]["message_id"] = json!("old-message");
        assert!(!answered_decision_request_matches(
            &wrong_wait,
            &question,
            &message,
            &request
        ));
        let mut wrong_assignment_message = message.clone();
        wrong_assignment_message["assignment_id"] = json!("other-assignment");
        assert!(!answered_decision_request_matches(
            &snapshot,
            &question,
            &wrong_assignment_message,
            &request
        ));

        let private_request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":"run-private",
            "assignment_id":null,
            "chat_id":"dm-bot-1",
            "bot_id":"bot-1",
            "model":"mock/model",
            "instruction":"resume",
            "private":true
        }))
        .unwrap();
        let private_question = json!({
            "assignment_id":"dm_dm-bot-1",
            "chat_id":"dm-bot-1",
            "bot_id":"bot-1"
        });
        let private_message = json!({"id":"message-2","chat_id":"dm-bot-1","sender":"bot-1"});
        let private_snapshot = json!({
            "assignments":{},
            "bots":{"bot-1":{"id":"bot-1","dm_chat_id":"dm-bot-1"}}
        });
        assert!(answered_decision_request_matches(
            &private_snapshot,
            &private_question,
            &private_message,
            &private_request
        ));
        let mut non_private = private_request.clone();
        non_private.private = false;
        assert!(!answered_decision_request_matches(
            &private_snapshot,
            &private_question,
            &private_message,
            &non_private
        ));
        let mut wrong_scope = private_question.clone();
        wrong_scope["assignment_id"] = json!("dm_other-chat");
        assert!(!answered_decision_request_matches(
            &private_snapshot,
            &wrong_scope,
            &private_message,
            &private_request
        ));
    }

    #[test]
    fn waiting_message_reply_matches_decision_wait_id() {
        let snapshot = json!({
            "assignments": {
                "assignment": {
                    "id":"assignment",
                    "status":"waiting_bot",
                    "result_message_id":null,
                    "wait":{"reason":"decision","message_id":"decision-message"}
                }
            }
        });
        assert_eq!(
            waiting_assignment_for_message(&snapshot, "decision-message").as_deref(),
            Some("assignment")
        );
    }

    #[test]
    fn decision_resume_rejects_old_checkpoint_for_same_assignment() {
        let snapshot = json!({
            "assignments": {
                "assignment": {
                    "status":"waiting_bot",
                    "wait":{"reason":"decision","message_id":"current-message"}
                }
            }
        });
        assert_eq!(
            waiting_message_id_for_assignment(&snapshot, "assignment").as_deref(),
            Some("current-message")
        );
        assert!(waiting_checkpoint_matches_message(
            &json!({
                "waiting_reason":"decision",
                "waiting_message_id":"current-message",
                "pending_tool":null
            }),
            "current-message"
        ));
        assert!(!waiting_checkpoint_matches_message(
            &json!({
                "waiting_reason":"decision",
                "waiting_message_id":"old-message",
                "pending_tool":null
            }),
            "current-message"
        ));
        assert!(waiting_checkpoint_matches_message(
            &json!({
                "waiting_reason":"blocked",
                "waiting_message_id":"current-message"
            }),
            "current-message"
        ));
    }

    #[test]
    fn child_completion_targets_only_a_waiting_decision_parent() {
        let snapshot = json!({
            "assignments": {
                "parent": {
                    "id":"parent",
                    "status":"waiting_bot",
                    "wait":{"reason":"decision","message_id":"decision-message"}
                },
                "child": {
                    "id":"child",
                    "status":"done",
                    "parent_assignment_id":"parent",
                    "trigger_message_id":"decision-message",
                    "result_message_id":"child-result"
                },
                "other": {
                    "id":"other",
                    "status":"working"
                },
                "question-parent": {
                    "id":"question-parent",
                    "status":"waiting_user",
                    "wait":{"reason":"decision","message_id":"question-message"}
                },
                "question-child": {
                    "id":"question-child",
                    "status":"done",
                    "parent_assignment_id":"question-parent",
                    "trigger_message_id":"question-message",
                    "result_message_id":"question-result"
                }
            },
            "messages": {
                "child-result": {"fallback_text":"child completed"},
                "question-message": {"question_id":"question-1"},
                "question-result": {"fallback_text":"question child completed"}
            },
            "questions": {
                "question-1": {
                    "id":"question-1",
                    "assignment_id":"question-parent",
                    "state":"pending"
                }
                }
        });
        assert_eq!(
            waiting_parent_for_child(&snapshot, "child").as_deref(),
            Some("parent")
        );
        assert_eq!(waiting_parent_for_child(&snapshot, "other"), None);
        assert_eq!(
            assignment_result_text(&snapshot, "child").as_deref(),
            Some("child completed")
        );
        assert_eq!(
            waiting_parent_for_child(&snapshot, "question-child").as_deref(),
            Some("question-parent")
        );
        assert_eq!(
            pending_decision_question_id(&snapshot, "question-parent", "question-message")
                .as_deref(),
            Some("question-1")
        );
        assert!(decision_checkpoint_matches_question(
            &json!({"waiting_reason":"decision","waiting_message_id":"question-message"}),
            "question-message"
        ));
        assert!(!decision_checkpoint_matches_question(
            &json!({"waiting_reason":"decision","waiting_message_id":"old-message"}),
            "question-message"
        ));
        assert!(!decision_checkpoint_matches_question(
            &json!({"waiting_reason":"decision","waiting_message_id":"question-message","pending_tools":[{"name":"write"}]}),
            "question-message"
        ));
        assert!(!waiting_checkpoint_matches_message(
            &json!({"waiting_reason":"decision","waiting_message_id":"question-message","pending_tools":[{"name":"write"}]}),
            "question-message"
        ));
        let mut stale = snapshot.clone();
        stale["assignments"]["question-child"]["trigger_message_id"] =
            json!("old-decision-message");
        assert_eq!(waiting_parent_for_child(&stale, "question-child"), None);
    }

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

    #[test]
    fn live_status_refreshes_only_at_run_boundaries() {
        for kind in [
            "run.start",
            "run.resume",
            "llm.request",
            "run.wait",
            "run.end",
        ] {
            assert!(is_run_status_boundary(&ExecutionEvent {
                event: "trace.item".into(),
                data: json!({"item":{"type":kind}}),
                persistent: true,
            }));
        }
        for event in [
            ExecutionEvent {
                event: "trace.item".into(),
                data: json!({"item":{"type":"llm.response"}}),
                persistent: true,
            },
            ExecutionEvent {
                event: "trace.delta".into(),
                data: json!({"type":"run.end"}),
                persistent: false,
            },
        ] {
            assert!(!is_run_status_boundary(&event));
        }
    }

    #[test]
    fn queued_promotion_emits_one_working_assignment_update() {
        let assignment = json!({"id":"assignment-2","status":"working"});
        let queued = Event {
            seq: 1,
            event: "assignment.created".into(),
            data: json!({"assignment":{"id":"assignment-2","status":"queued"}}),
        };
        let working = Event {
            seq: 2,
            event: "assignment.updated".into(),
            data: json!({"assignment":{"id":"assignment-2","status":"working"}}),
        };
        assert!(ComposedBackend::assignment_status_event_needed(
            std::slice::from_ref(&queued),
            &assignment
        ));
        assert!(!ComposedBackend::assignment_status_event_needed(
            &[queued, working],
            &assignment
        ));
    }

    #[test]
    fn project_summary_contains_goal_artifacts_and_highlights() {
        let summary = project_summary(
            &json!({"name":"登录项目","goal":"只做邮箱登录"}),
            &json!({
                "artifacts":[{"title":"PRD","path_or_url":"docs/prd.md"}],
                "highlights":[{"text":"测试 20/20 通过"}]
            }),
        );
        assert!(summary.contains("只做邮箱登录"));
        assert!(summary.contains("PRD (docs/prd.md)"));
        assert!(summary.contains("测试 20/20 通过"));
    }

    #[test]
    fn project_summary_artifacts_are_order_independent_and_deduplicated() {
        let project = json!({"name":"登录项目","goal":"只做邮箱登录"});
        let first = project_summary(
            &project,
            &json!({
                "artifacts":[
                    {"title":"代码","path_or_url":"src/login.rs"},
                    {"title":"PRD","path_or_url":"docs/prd.md"},
                    {"title":"PRD","path_or_url":"docs/prd.md"}
                ]
            }),
        );
        let reversed = project_summary(
            &project,
            &json!({
                "artifacts":[
                    {"title":"PRD","path_or_url":"docs/prd.md"},
                    {"title":"代码","path_or_url":"src/login.rs"}
                ]
            }),
        );
        assert_eq!(first, reversed);
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::{ComposedBackend, ExecutionRequest, ProductionBackend, RuntimeExecution};
    use crate::features::FeatureService;
    use crate::{Gateway, GatewayConfig, RpcBackend};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn seeded_backend() -> (Arc<ProductionBackend>, crate::GatewayState, Vec<String>) {
        let home = tempdir().expect("temporary home");
        // The caller owns the temporary directory through the leaked path. It
        // is reclaimed by the test process after the backend is dropped.
        let path = home.keep();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).expect("open backend"));
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"persist-worker"}),
                &gateway.state,
            )
            .await
            .expect("create bot");
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"persist-project",
                    "goal":"concurrent persistence",
                    "member_bot_ids":[bot["bot"]["id"]]
                }),
                &gateway.state,
            )
            .await
            .expect("create project");
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let mut assignments = Vec::new();
        for index in 0..8 {
            let value = backend
                .call(
                    "assignment.create",
                    json!({
                        "project_id":project_id,
                        "origin_chat_id":chat_id,
                        "bot_id":bot["bot"]["id"],
                        "title":format!("persist-{index}"),
                        "instruction":"persist",
                        "from":"main"
                    }),
                    &gateway.state,
                )
                .await
                .expect("create assignment");
            assignments.push(value["id"].as_str().unwrap().to_owned());
        }
        (backend, gateway.state, assignments)
    }

    fn unwrap_arc<T>(value: Arc<T>) -> T {
        match Arc::try_unwrap(value) {
            Ok(value) => value,
            Err(_) => panic!("all persistence test tasks must be joined before reopen"),
        }
    }

    #[tokio::test]
    async fn trace_history_keeps_waiting_and_parent_runs_live_across_paging() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let composed = ComposedBackend::open(backend.clone(), gateway.state, path).unwrap();
        for key in ["chat_id", "assignment_id"] {
            let scope = format!("live-{key}");
            let trace_path = format!("data/traces/{scope}.jsonl");
            assert_eq!(
                composed.trace_history(&json!({key:&scope})).unwrap()["live"],
                false
            );
            for (index, (run_id, kind)) in [
                ("parent", "run.start"),
                ("parent", "run.wait"),
                ("parent", "run.resume"),
                ("child", "run.start"),
                ("child", "run.end"),
            ]
            .into_iter()
            .enumerate()
            {
                backend
                    .store
                    .append_jsonl(
                        &trace_path,
                        &json!({
                            "aseq":index+1,"run_id":run_id,"type":kind,"data":{}
                        }),
                    )
                    .unwrap();
                let result = composed
                    .trace_history(&json!({key:&scope,"tail":true,"limit":1}))
                    .unwrap();
                assert_eq!(result["live"], true, "{kind}");
                assert_eq!(result["items"].as_array().unwrap().len(), 1);
            }
            backend
                .store
                .append_jsonl(
                    &trace_path,
                    &json!({
                        "aseq":6,"run_id":"parent","type":"run.end","data":{"status":"done"}
                    }),
                )
                .unwrap();
            // The requested page excludes the end, but this is still replay.
            let result = composed
                .trace_history(&json!({key:&scope,"before_aseq":3}))
                .unwrap();
            assert_eq!(result["live"], false);
            assert_eq!(result["items"].as_array().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn concurrent_finish_usage_and_rpc_rebuild_latest_fresh_snapshots() {
        let (backend, state, assignments) = seeded_backend().await;
        let mut jobs = Vec::new();

        // Runtime usage callbacks and ordinary RPC mutations deliberately run
        // together. Each callback must persist a fresh snapshot while holding
        // the adapter's persistence lock.
        for (index, assignment_id) in assignments.iter().enumerate() {
            let backend = backend.clone();
            let assignment_id = assignment_id.clone();
            jobs.push(tokio::spawn(async move {
                backend
                    .update_assignment_usage(
                        &assignment_id,
                        &json!({
                            "input_tokens":index as u64 + 1,
                            "output_tokens":2,
                            "cache_read_tokens":0,
                            "cache_write_tokens":0,
                            "cost":null
                        }),
                    )
                    .await
                    .expect("persist usage");
            }));
        }
        for seq in 1..=8_u64 {
            let backend = backend.clone();
            let state = state.clone();
            jobs.push(tokio::spawn(async move {
                backend
                    .call(
                        "chat.mark_read",
                        json!({"chat_id":"chat_main","seq":seq}),
                        &state,
                    )
                    .await
                    .expect("persist RPC");
            }));
        }

        // A runtime completion is another writer and races with the usage
        // callbacks above. Its operation intentionally carries no snapshot;
        // persist_orchestrator must supply the current one under its lock.
        let finish_backend = backend.clone();
        let finish_id = assignments[0].clone();
        jobs.push(tokio::spawn(async move {
            let assignment = finish_backend
                .orchestrator
                .finish_assignment(&finish_id, "done")
                .expect("finish assignment");
            finish_backend
                .persist_orchestrator(json!({
                    "method":"execution.finish",
                    "result":{"assignment":assignment},
                    "status":"done"
                }))
                .await
                .expect("persist finish");
        }));
        for job in jobs {
            job.await.expect("writer task");
        }

        let expected = backend.orchestrator.snapshot().expect("memory snapshot");
        let operations = backend
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .expect("operation WAL");
        assert!(!operations.is_empty());
        assert!(operations
            .iter()
            .filter(|operation| { operation.get("status").and_then(Value::as_str) == Some("done") })
            .all(|operation| operation.get("snapshot").is_some()));
        let disk = backend
            .store
            .read_snapshot::<Value>("data/orchestrator/state.json")
            .expect("state snapshot")
            .expect("state snapshot exists");
        assert_eq!(disk, expected, "disk must contain the final fresh snapshot");

        let backend = unwrap_arc(backend);
        let root = backend.store.root().to_path_buf();
        drop(backend);
        drop(state);
        let reopened = ProductionBackend::open(root).expect("reopen backend");
        assert_eq!(
            reopened.orchestrator.snapshot().expect("replayed snapshot"),
            expected,
            "WAL replay must preserve concurrent RPC/runtime mutations"
        );
    }

    #[tokio::test]
    async fn stale_caller_snapshot_is_replaced_before_wal_append() {
        let (backend, state, assignments) = seeded_backend().await;
        let stale = backend.orchestrator.snapshot().expect("stale snapshot");
        let assignment = backend
            .orchestrator
            .finish_assignment(&assignments[0], "done")
            .expect("finish assignment");
        backend
            .persist_orchestrator(json!({
                "method":"execution.finish",
                "result":{"assignment":assignment},
                "snapshot":stale,
                "status":"done"
            }))
            .await
            .expect("persist fresh snapshot");

        let operations = backend
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .expect("operation WAL");
        let last = operations.last().expect("finish operation");
        assert_eq!(
            last["snapshot"]["assignments"][&assignments[0]]["status"], "done",
            "caller-provided stale snapshot must not win"
        );
        let backend = unwrap_arc(backend);
        let root = backend.store.root().to_path_buf();
        drop(backend);
        drop(state);
        let reopened = ProductionBackend::open(root).expect("reopen backend");
        assert_eq!(
            reopened.orchestrator.snapshot().expect("replayed snapshot")["assignments"]
                [&assignments[0]]["status"],
            "done"
        );
    }

    #[tokio::test]
    async fn screen_browser_configuration_restores_persisted_bot_without_run() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"screen-restore-worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();

        // No execution request is created. A fresh screen connection must
        // still receive the same per-Bot state path used by execution.
        let composed = ComposedBackend::open(backend, gateway.state.clone(), path.clone()).unwrap();
        RpcBackend::configure_browser_for_screen(&composed, &bot_id)
            .await
            .unwrap();

        let config = gateway.state.browser.lock().await.bot_config(&bot_id);
        assert_eq!(
            config.state_path,
            Some(path.join("browser/sessions").join(format!("{bot_id}.json")))
        );
    }

    #[tokio::test]
    async fn continuation_engine_prepares_browser_without_scheduler_or_screen() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"continuation-restore"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap();
        let composed = ComposedBackend::open(backend, gateway.state.clone(), path.clone()).unwrap();
        let request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":"restored-run", "assignment_id":"restored-assignment",
            "chat_id":"restored-chat", "bot_id":bot_id, "model":"mock", "instruction":"continue"
        }))
        .unwrap();
        assert!(gateway
            .state
            .browser
            .lock()
            .await
            .bot_config(bot_id)
            .state_path
            .is_none());
        composed
            .runtime
            .configured_engine_for(&request)
            .await
            .unwrap();
        let config = gateway.state.browser.lock().await.bot_config(bot_id);
        assert_eq!(
            config.state_path,
            Some(path.join("browser/sessions").join(format!("{bot_id}.json")))
        );
        assert_eq!(config.mode, macbot_browser::BrowserMode::Headless);
    }

    #[tokio::test]
    async fn model_finish_project_finalizes_summary_and_current_assignment() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let worker = backend
            .call(
                "bot.create",
                json!({"name":"summary-worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let worker_id = worker["bot"]["id"].as_str().unwrap().to_owned();
        let created = backend
            .call(
                "project.create",
                json!({"name":"model finish","goal":"验收真实产物","member_bot_ids":[worker_id]}),
                &gateway.state,
            )
            .await
            .unwrap();
        let project_id = created["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = created["chat"]["id"].as_str().unwrap().to_owned();
        let working_worker = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":worker_id,
                    "title":"正在执行",
                    "instruction":"等待取消",
                    "from":"user"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let working_worker_id = working_worker["id"].as_str().unwrap().to_owned();
        let queued_worker = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":worker_id,
                    "title":"排队中",
                    "instruction":"等待取消",
                    "from":"user"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let queued_worker_id = queued_worker["id"].as_str().unwrap().to_owned();
        assert_eq!(working_worker["status"], "working");
        assert_eq!(queued_worker["status"], "queued");
        backend
            .orchestrator
            .mark_project_review(&project_id)
            .unwrap();
        let assignment = backend
            .orchestrator
            .rpc(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":"main",
                    "title":"模型确认",
                    "instruction":"确认项目",
                    "from":"main"
                }),
            )
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap().to_owned();
        let feature_service = Arc::new(
            FeatureService::with_store(
                backend.store.clone(),
                path.clone(),
                Vec::<std::path::PathBuf>::new(),
            )
            .unwrap(),
        );
        let runtime = RuntimeExecution::open(
            path,
            backend.clone(),
            backend.usage.clone(),
            backend.providers.clone(),
            gateway.state.clone(),
            feature_service.clone(),
        )
        .unwrap();
        let worker_run_id = format!("run_{working_worker_id}");
        let worker_request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":worker_run_id,
            "assignment_id":working_worker_id,
            "chat_id":chat_id,
            "bot_id":worker_id,
            "model":"mock/worker",
            "instruction":"等待取消"
        }))
        .unwrap();
        runtime.persist_request(&worker_request).unwrap();
        runtime
            .state
            .durable
            .lock()
            .await
            .create_job("worker", "dm", json!({"run_id":worker_request.run_id}))
            .unwrap();
        runtime
            .confirm_project_for_coordination(
                &json!({
                    "project_id":project_id,
                    "summary":"模型确认摘要",
                    "client_request_id":"model-finish-test"
                }),
                Some(&assignment_id),
            )
            .await
            .unwrap();
        let snapshot = backend.orchestrator.snapshot().unwrap();
        assert_eq!(snapshot["projects"][&project_id]["status"], "done");
        assert_eq!(snapshot["assignments"][&assignment_id]["status"], "done");
        assert_eq!(
            snapshot["assignments"][&working_worker_id]["status"],
            "cancelled"
        );
        assert_eq!(
            snapshot["assignments"][&queued_worker_id]["status"],
            "cancelled"
        );
        let entries = feature_service.shared_memory.entries().unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.id == format!("project-summary:{project_id}")));
        assert!(!entries
            .iter()
            .any(|entry| { entry.id == format!("project-summary-worklog:{project_id}:main") }));
    }
}
