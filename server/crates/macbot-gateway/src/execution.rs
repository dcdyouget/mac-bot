//! Durable model/tool execution used by the real gateway backend.
//!
//! The HTTP layer owns authentication and RPC framing. This module owns one
//! run: durable checkpointing, provider streaming, tool rounds, trace entries,
//! and publication through a small gateway-owned sink. It intentionally does
//! not know Axum or orchestrator internals, which keeps its side effects
//! reviewable and makes the mock provider usable in tests.

use crate::rate_limit::ModelRateLimiter;
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use futures_util::future::join_all;
use macbot_durable::{DurableError, DurableRuntime, InboxItem, Job, JobStatus};
use macbot_providers::{Completion, ModelEvent, ModelProvider, ModelRequest, TokenUsage, ToolCall};
use macbot_store::{Store, StoreError};
use macbot_tools::{Part, Risk, Tool, ToolCancellation, ToolContext, ToolOutputChunk, ToolResult};
use macbot_usage::{Totals, UsageLedger, UsageRecord};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("durable runtime: {0}")]
    Durable(#[from] DurableError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("provider: {0}")]
    Provider(#[from] macbot_providers::Error),
    #[error("usage ledger: {0}")]
    Usage(#[from] macbot_usage::Error),
    #[error("execution sink: {0}")]
    Sink(String),
    #[error("provider resolver: {0}")]
    Resolver(String),
    #[error("run exceeded tool/model turn limit")]
    TurnLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub run_id: String,
    pub assignment_id: Option<String>,
    pub chat_id: String,
    pub bot_id: String,
    pub model: String,
    #[serde(default = "default_provider_id")]
    pub provider_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub instruction: String,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub allow_unsafe: bool,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub routine: bool,
    #[serde(default)]
    pub price: Option<macbot_usage::Price>,
    #[serde(default)]
    pub resume_approved: bool,
    /// Child assignments never receive tool schemas and tool calls are
    /// rejected defensively even if a provider emits one anyway.
    #[serde(default)]
    pub subagent: bool,
    /// Per-run tool permission list. `None` keeps the bot's normal tool set;
    /// subagents default to the read-only allowlist below.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Internal runtime phase; omitted callers are classified from the run
    /// semantics (subagent/chat/main coordination/work).
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub subagent_task: Option<String>,
    /// Internal runtime setting. It is never sent to providers; when enabled
    /// the sanitized model request is persisted under the run directory.
    #[serde(default)]
    pub save_full_requests: bool,
    /// User/group response that resumes a send_msg decision/blocked wait.
    #[serde(default)]
    pub resume_message: Option<String>,
}

fn default_max_turns() -> usize {
    16
}
fn default_provider_id() -> String {
    "mock".into()
}

fn usage_model_id(provider_id: &str, model: &str) -> String {
    model
        .strip_prefix(&format!("{provider_id}/"))
        .unwrap_or(model)
        .to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    pub run_id: String,
    pub job_id: String,
    pub status: String,
    pub text: String,
    pub usage: TokenUsage,
    pub turns: usize,
}

struct ApprovedFollowupContext<'a> {
    request: &'a ExecutionRequest,
    job: &'a Job,
    turn: usize,
    messages: &'a mut Vec<Value>,
    group_progress_count: &'a mut usize,
    group_done: &'a mut bool,
    usage: &'a TokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub event: String,
    pub data: Value,
    pub persistent: bool,
}

#[derive(Debug, Clone, Default)]
struct UsageTickState {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    requests: u64,
    cost: Option<f64>,
    unknown_cost: bool,
    last_emitted_ms: u64,
}

impl UsageTickState {
    fn add(&mut self, usage: &TokenUsage, cost: Option<f64>) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.cache_read_tokens += usage.cache_read_tokens;
        self.cache_write_tokens += usage.cache_write_tokens;
        self.requests += 1;
        match cost {
            Some(cost) if !self.unknown_cost => {
                self.cost = Some(self.cost.unwrap_or_default() + cost);
            }
            Some(_) => {}
            None => {
                self.unknown_cost = true;
                self.cost = None;
            }
        }
    }

    fn wire(&self) -> Value {
        json!({
            "input_tokens":self.input_tokens,
            "output_tokens":self.output_tokens,
            "cache_read_tokens":self.cache_read_tokens,
            "cache_write_tokens":self.cache_write_tokens,
            "requests":self.requests,
            "cost":if self.unknown_cost { Value::Null } else { self.cost.map_or(Value::Null, |cost| json!(cost)) }
        })
    }
}

/// Gateway integration point. Implementations may forward events to a
/// WebSocket subscription and route completed group messages through the
/// orchestrator. The executor never calls the model or tools through this
/// trait, so the permission boundary stays in the gateway.
#[async_trait]
pub trait ExecutionSink: Send + Sync {
    /// Persistent events must be written before the implementation fans them
    /// out. Temporary deltas are subscriber-only and never enter the WAL.
    async fn emit(&self, event: ExecutionEvent);
    /// Admit a group message and return the canonical persisted Message.
    /// The request payload contains the durable receipt, whose logical ID is
    /// intentionally separate from the canonical message ID returned here.
    async fn send_group_message(&self, message: Value) -> Result<Value, String>;
    async fn approval_required(&self, data: Value);
    /// Publish the protocol's temporary workbench usage tick. Implementations
    /// may also update their durable assignment snapshot in this hook.
    async fn usage_tick(&self, assignment_id: &str, usage: Value) {
        self.emit(ExecutionEvent {
            event: "usage.tick".into(),
            data: json!({"assignment_id":assignment_id,"usage":usage}),
            persistent: false,
        })
        .await;
    }
    async fn assignment_usage(&self, _assignment_id: &str, _usage: Value) {}
    async fn memory_context(&self, _request: &ExecutionRequest) -> Option<Value> {
        None
    }
    /// Prepare the context for a fresh model turn. Implementations that need
    /// to compact memory can override this and return an error before the
    /// model is called; the legacy hook remains the default for simple sinks.
    async fn prepare_model_context(
        &self,
        request: &ExecutionRequest,
    ) -> Result<Option<Value>, String> {
        Ok(self.memory_context(request).await)
    }
    async fn commit_succeeded(&self, _request: &ExecutionRequest, _data: Value) {}
}

/// Resolves a provider for each model call. The gateway can back this with its
/// live provider registry so model/provider changes apply to newly scheduled
/// turns without rebuilding the execution engine.
#[async_trait]
pub trait ProviderResolver: Send + Sync {
    async fn resolve(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Result<Arc<dyn ModelProvider>, String>;
}

/// Adapter supplied by the gateway/orchestrator boundary for the only group
/// side effect an executor may perform: admitting a `send_msg` tool result.
#[async_trait]
pub trait GroupMessageBridge: Send + Sync {
    /// Return the canonical persisted Message created (or found on replay).
    async fn send_msg(&self, message: Value) -> Result<Value, String>;
}

/// Production sink for the real GatewayState. Persistent events are written
/// before the live fan-out; temporary deltas bypass the durable event log.
pub struct GatewayStateSink {
    state: crate::GatewayState,
    store: Store,
    group: Arc<dyn GroupMessageBridge>,
}

impl GatewayStateSink {
    pub fn new(
        state: crate::GatewayState,
        store: Store,
        group: Arc<dyn GroupMessageBridge>,
    ) -> Self {
        Self {
            state,
            store,
            group,
        }
    }
}

#[async_trait]
impl ExecutionSink for GatewayStateSink {
    async fn emit(&self, event: ExecutionEvent) {
        if event.persistent {
            match self
                .store
                .append_event(event.event.clone(), event.data.clone())
            {
                Ok(stored) => {
                    self.state
                        .publish_event(stored.seq, &stored.event, stored.data)
                        .await;
                }
                Err(error) => {
                    tracing::error!(%error, event = %event.event, "failed to persist execution event")
                }
            }
        } else {
            self.state.publish_temporary(&event.event, event.data).await;
        }
    }

    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        self.group.send_msg(message).await
    }

    async fn approval_required(&self, data: Value) {
        self.emit(ExecutionEvent {
            event: "approval.requested".into(),
            data,
            persistent: true,
        })
        .await;
    }
}

#[derive(Default)]
pub struct NullExecutionSink;

#[async_trait]
impl ExecutionSink for NullExecutionSink {
    async fn emit(&self, _: ExecutionEvent) {}
    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        Ok(message.get("message").cloned().unwrap_or(message))
    }
    async fn approval_required(&self, _: Value) {}
}

/// Process-wide mutable execution state. Engines are intentionally cheap
/// per-run views over this state so concurrent runs share durable jobs and
/// sequence cursors instead of loading stale snapshots.
pub struct ExecutionState {
    pub durable: Arc<Mutex<DurableRuntime>>,
    pub aseq: Arc<Mutex<HashMap<String, u64>>>,
    pub cancelled: Arc<Mutex<HashSet<String>>>,
    pub cancellations: Arc<Mutex<HashMap<String, ToolCancellation>>>,
    usage_ticks: Arc<Mutex<HashMap<String, UsageTickState>>>,
    recovered: Mutex<bool>,
    model_rate_limiter: ModelRateLimiter,
}

impl ExecutionState {
    pub fn from_store(store: Store) -> Result<Arc<Self>, ExecutionError> {
        Ok(Arc::new(Self {
            durable: Arc::new(Mutex::new(DurableRuntime::from_store(store.clone())?)),
            model_rate_limiter: ModelRateLimiter::new(store),
            aseq: Arc::new(Mutex::new(HashMap::new())),
            cancelled: Arc::new(Mutex::new(HashSet::new())),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            usage_ticks: Arc::new(Mutex::new(HashMap::new())),
            recovered: Mutex::new(false),
        }))
    }

    async fn cancellation_for(&self, run_id: &str) -> ToolCancellation {
        let mut cancellations = self.cancellations.lock().await;
        let cancellation = cancellations.entry(run_id.to_owned()).or_default().clone();
        drop(cancellations);
        if self.cancelled.lock().await.contains(run_id) {
            cancellation.cancel();
        }
        cancellation
    }

    async fn cancel_tools(&self, run_id: &str) {
        if let Some(cancellation) = self.cancellations.lock().await.get(run_id).cloned() {
            cancellation.cancel();
        }
    }

    async fn forget_cancellation(&self, run_id: &str) {
        self.cancellations.lock().await.remove(run_id);
    }
}

pub struct ExecutionEngine {
    store: Store,
    state: Arc<ExecutionState>,
    provider: Arc<dyn ModelProvider>,
    provider_resolver: Option<Arc<dyn ProviderResolver>>,
    tools: HashMap<String, Arc<dyn Tool>>,
    sink: Arc<dyn ExecutionSink>,
    home: PathBuf,
    usage: Arc<Mutex<UsageLedger>>,
}

impl ExecutionEngine {
    pub fn new(
        store: Store,
        provider: Arc<dyn ModelProvider>,
        tools: impl IntoIterator<Item = Arc<dyn Tool>>,
        sink: Arc<dyn ExecutionSink>,
        home: impl Into<PathBuf>,
    ) -> Result<Self, ExecutionError> {
        let usage = UsageLedger::from_store(store.clone())?;
        Self::new_with_usage(
            store,
            provider,
            tools,
            sink,
            home,
            Arc::new(Mutex::new(usage)),
        )
    }

    /// Construct an engine against the gateway's process-wide usage ledger.
    /// Every execution engine sharing a gateway must use this constructor so
    /// request-id deduplication and hourly aggregation are not split by clone.
    pub fn new_with_usage(
        store: Store,
        provider: Arc<dyn ModelProvider>,
        tools: impl IntoIterator<Item = Arc<dyn Tool>>,
        sink: Arc<dyn ExecutionSink>,
        home: impl Into<PathBuf>,
        usage: Arc<Mutex<UsageLedger>>,
    ) -> Result<Self, ExecutionError> {
        let state = ExecutionState::from_store(store.clone())?;
        Self::new_with_usage_and_state(store, provider, tools, sink, home, usage, state)
    }

    pub fn new_with_usage_and_state(
        store: Store,
        provider: Arc<dyn ModelProvider>,
        tools: impl IntoIterator<Item = Arc<dyn Tool>>,
        sink: Arc<dyn ExecutionSink>,
        home: impl Into<PathBuf>,
        usage: Arc<Mutex<UsageLedger>>,
        state: Arc<ExecutionState>,
    ) -> Result<Self, ExecutionError> {
        Ok(Self {
            store,
            state,
            provider,
            provider_resolver: None,
            tools: tools
                .into_iter()
                .map(|tool| (tool.name().to_string(), tool))
                .collect(),
            sink,
            home: home.into(),
            usage,
        })
    }

    pub fn state(&self) -> Arc<ExecutionState> {
        self.state.clone()
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn with_provider_resolver(mut self, resolver: Arc<dyn ProviderResolver>) -> Self {
        self.provider_resolver = Some(resolver);
        self
    }

    async fn next_aseq(&self, request: &ExecutionRequest) -> Result<u64, ExecutionError> {
        let scope = trace_scope(request);
        let mut state = self.state.aseq.lock().await;
        let entry = state.entry(scope.clone()).or_insert_with(|| {
            self.store
                .read_jsonl::<Value>(format!("data/traces/{scope}.jsonl"))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|item| item.get("aseq").and_then(Value::as_u64))
                .max()
                .unwrap_or(0)
        });
        *entry += 1;
        Ok(*entry)
    }

    async fn cleanup_run_tools(&self, request: &ExecutionRequest) {
        let cancellation = self.state.cancellation_for(&request.run_id).await;
        let context = ToolContext::new(
            request.cwd.clone().unwrap_or_else(|| self.home.clone()),
            request.run_id.clone(),
            self.home.join("runs"),
        )
        .with_cancellation(cancellation);
        for tool in self.tools.values() {
            tool.cleanup(&context).await;
        }
        self.state.forget_cancellation(&request.run_id).await;
    }

    async fn fail_run(
        &self,
        request: &ExecutionRequest,
        job: &Job,
        error: &ExecutionError,
        streaming_message_id: Option<&str>,
    ) -> Result<(), ExecutionError> {
        let mut checkpoint = job.checkpoint.clone();
        checkpoint["run_id"] = json!(request.run_id);
        checkpoint["error"] = json!(error.to_string());
        if self
            .commit_checkpoint(request, &job.id, JobStatus::Failed, checkpoint, false)
            .await?
        {
            self.publish_answer(
                request,
                0,
                &format!("运行失败：{error}"),
                streaming_message_id,
            )
            .await?;
            self.trace(
                request,
                "run.end",
                json!({"status":"failed","error":error.to_string()}),
            )
            .await?;
        }
        self.cleanup_run_tools(request).await;
        Ok(())
    }

    async fn is_cancelled(&self, run_id: &str) -> bool {
        self.state.cancelled.lock().await.contains(run_id)
    }

    /// Commit a checkpoint unless cancellation already made the job terminal.
    /// DurableRuntime also rejects a late transition after the cancellation
    /// commit, closing the race between the check and the write.
    async fn commit_checkpoint(
        &self,
        request: &ExecutionRequest,
        job_id: &str,
        status: JobStatus,
        checkpoint: Value,
        unsafe_replay: bool,
    ) -> Result<bool, ExecutionError> {
        if self.is_cancelled(&request.run_id).await {
            return Ok(false);
        }
        let result = {
            let mut durable = self.state.durable.lock().await;
            durable.commit(job_id, status, checkpoint, unsafe_replay)
        };
        match result {
            Ok(_) => Ok(true),
            Err(DurableError::TerminalJob(_)) if self.is_cancelled(&request.run_id).await => {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn save_full_requests_enabled(&self, request: &ExecutionRequest) -> bool {
        request.save_full_requests
            || self
                .store
                .read_snapshot::<Value>("data/settings.json")
                .ok()
                .flatten()
                .and_then(|settings| settings.pointer("/trace/save_full_requests").cloned())
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
    }

    /// Approval policy is deliberately read for every risky call.  Settings
    /// updates are live and must not require rebuilding the execution engine.
    /// `allow_unsafe` is the one-shot decision carried by an approval
    /// continuation; otherwise an explicit global mode or matching
    /// `auto_allow` rule can skip the approval card. A matching `ask_first`
    /// rule always wins over an `auto_allow` rule (and over the global mode).
    fn risky_call_allowed(
        &self,
        request: &ExecutionRequest,
        tool_name: &str,
        args: &Value,
    ) -> bool {
        let settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .unwrap_or_default();
        risky_call_allowed_from_settings(&settings, request.allow_unsafe, tool_name, args)
    }

    fn coordination_tool_requires_approval(&self, tool_name: &str, args: &Value) -> bool {
        let settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .unwrap_or_default();
        coordination_tool_requires_approval_from_settings(&settings, tool_name, args)
    }

    fn persist_model_request(
        &self,
        request: &ExecutionRequest,
        request_id: &str,
        model_request: &ModelRequest,
    ) -> Result<Option<String>, ExecutionError> {
        if !self.save_full_requests_enabled(request) {
            return Ok(None);
        }
        let path = format!(
            "data/runs/{}/requests/{}.json",
            safe_id(&request.run_id),
            safe_id(request_id)
        );
        self.store.write_snapshot(&path, model_request)?;
        Ok(Some(path))
    }

    async fn record_usage_tick(
        &self,
        request: &ExecutionRequest,
        usage: &TokenUsage,
        cost: Option<f64>,
    ) {
        let Some(assignment_id) = request.assignment_id.as_deref() else {
            return;
        };
        let (cumulative, tick) = {
            let mut ticks = self.state.usage_ticks.lock().await;
            let state = ticks.entry(assignment_id.to_owned()).or_default();
            state.add(usage, cost);
            let cumulative = state.wire();
            let now = now_ms();
            let tick = (state.last_emitted_ms == 0
                || now.saturating_sub(state.last_emitted_ms) >= 10_000)
                .then(|| {
                    state.last_emitted_ms = now;
                    cumulative.clone()
                });
            (cumulative, tick)
        };
        self.sink.assignment_usage(assignment_id, cumulative).await;
        if let Some(usage) = tick {
            self.sink.usage_tick(assignment_id, usage).await;
        }
    }

    /// Resume working jobs. Unsafe checkpoints are converted to `Suspended` by
    /// `DurableRuntime::resume_plan`; the gateway can turn these into approval
    /// cards without replaying a write or command.
    pub async fn recover(&self) -> Result<Vec<Job>, ExecutionError> {
        let mut recovered = self.state.recovered.lock().await;
        if *recovered {
            return Ok(Vec::new());
        }
        *recovered = true;
        Ok(self.state.durable.lock().await.resume_plan()?)
    }

    /// Resume a queued or running job from its latest durable checkpoint.
    /// Keeping this as a named gateway API prevents callers from accidentally
    /// creating a second job for a retry.
    pub async fn resume(
        &self,
        request: ExecutionRequest,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        self.run(request).await
    }

    /// Continue an approval decision. The pending unsafe call is executed once
    /// from the checkpoint; the model is never asked to replay that call.
    pub async fn continue_approved(
        &self,
        mut request: ExecutionRequest,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        request.resume_approved = true;
        self.run(request).await
    }

    /// Resume a run after the user has released browser takeover. The
    /// checkpoint is consumed without invoking `request_takeover` again;
    /// that tool is a request for user control, never a model-side grant.
    pub async fn continue_takeover(
        &self,
        request: ExecutionRequest,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        self.continue_approved(request).await
    }

    /// Resume a model question after the orchestrator has recorded the
    /// user's answer. The answer is carried only by this internal request
    /// and is inserted as a synthetic tool result.
    pub async fn continue_question(
        &self,
        mut request: ExecutionRequest,
        answer: impl Into<String>,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        request.instruction = answer.into();
        self.continue_approved(request).await
    }

    /// Resume a group run waiting on a `send_msg` decision or blocked report.
    /// The original send call is already admitted by its durable receipt; only
    /// the user's reply is injected. Any other calls from that same model turn
    /// remain in the checkpoint for audit and receive a synthetic deferred
    /// result so the provider never sees an incomplete tool-call turn.
    pub async fn continue_waiting_message(
        &self,
        request: ExecutionRequest,
        reply: impl Into<String>,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        self.continue_message(request, reply).await
    }

    pub async fn continue_message(
        &self,
        mut request: ExecutionRequest,
        message: impl Into<String>,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        request.resume_approved = true;
        request.resume_message = Some(message.into());
        self.run(request).await
    }

    /// Cancel an existing run durably. A cancelled job is terminal and cannot
    /// be resumed by a later duplicate request.
    pub async fn cancel(
        &self,
        request: &ExecutionRequest,
    ) -> Result<Option<ExecutionOutcome>, ExecutionError> {
        let outcome = {
            let mut durable = self.state.durable.lock().await;
            let Some(job) = durable
                .jobs()
                .find(|job| job.checkpoint["run_id"].as_str() == Some(request.run_id.as_str()))
                .cloned()
            else {
                return Ok(None);
            };
            if matches!(
                &job.status,
                JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
            ) {
                return Ok(Some(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id,
                    status: match &job.status {
                        JobStatus::Done => "done",
                        JobStatus::Failed => "failed",
                        _ => "cancelled",
                    }
                    .into(),
                    text: job.checkpoint["text"].as_str().unwrap_or_default().into(),
                    usage: serde_json::from_value(job.checkpoint["usage"].clone())
                        .unwrap_or_default(),
                    turns: job.checkpoint["turns"].as_u64().unwrap_or(0) as usize,
                }));
            } else {
                self.state
                    .cancelled
                    .lock()
                    .await
                    .insert(request.run_id.clone());
                let mut checkpoint = job.checkpoint.clone();
                checkpoint["cancelled_at"] = json!(now_rfc3339());
                let cancelled = durable.commit(&job.id, JobStatus::Cancelled, checkpoint, false)?;
                ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: cancelled.id,
                    status: "cancelled".into(),
                    text: String::new(),
                    usage: TokenUsage::default(),
                    turns: cancelled.checkpoint["round"].as_u64().unwrap_or(0) as usize,
                }
            }
        };
        if outcome.status == "cancelled" {
            self.state.cancel_tools(&request.run_id).await;
            self.cleanup_run_tools(request).await;
            self.publish_answer(request, 0, "已停止", None).await?;
            self.trace(
                request,
                "run.end",
                json!({"status":"cancelled","error":null}),
            )
            .await?;
        }
        Ok(Some(outcome))
    }

    pub async fn run(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, ExecutionError> {
        let _cancellation = self.state.cancellation_for(&request.run_id).await;
        let max_turns = request.max_turns.max(1);
        let initial_messages = if request.messages.is_empty() {
            vec![json!({"role":"user","content":request.instruction})]
        } else {
            request.messages.clone()
        };
        let (job, mut messages, start_turn, resumed, approved_call, approved_followups) = {
            let mut durable = self.state.durable.lock().await;
            let existing = durable
                .jobs()
                .find(|job| job.checkpoint["run_id"].as_str() == Some(request.run_id.as_str()))
                .cloned();
            if let Some(job) = existing {
                if job.status == JobStatus::Done {
                    let text = job.checkpoint["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let usage =
                        serde_json::from_value(job.checkpoint["usage"].clone()).unwrap_or_default();
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id,
                        job_id: job.id,
                        status: "done".into(),
                        text,
                        usage,
                        turns: job.checkpoint["turns"].as_u64().unwrap_or(0) as usize,
                    });
                }
                if matches!(job.status, JobStatus::Cancelled | JobStatus::Failed) {
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id,
                        job_id: job.id,
                        status: match job.status {
                            JobStatus::Cancelled => "cancelled",
                            _ => "failed",
                        }
                        .into(),
                        text: job.checkpoint["text"].as_str().unwrap_or_default().into(),
                        usage: serde_json::from_value(job.checkpoint["usage"].clone())
                            .unwrap_or_default(),
                        turns: job.checkpoint["turns"].as_u64().unwrap_or(0) as usize,
                    });
                }
                if matches!(job.status, JobStatus::Waiting | JobStatus::Suspended)
                    && !(request.resume_approved
                        && matches!(job.status, JobStatus::Waiting | JobStatus::Suspended))
                {
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id,
                        job_id: job.id,
                        status: "suspended".into(),
                        text: String::new(),
                        usage: TokenUsage::default(),
                        turns: job.checkpoint["round"].as_u64().unwrap_or(0) as usize,
                    });
                }
                let messages = job.checkpoint["messages"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| initial_messages.clone());
                let round = job.checkpoint["round"].as_u64().unwrap_or(0) as usize;
                if request.resume_message.is_some()
                    && job
                        .checkpoint
                        .get("waiting_reason")
                        .and_then(Value::as_str)
                        .is_some()
                    && matches!(job.status, JobStatus::Waiting | JobStatus::Suspended)
                {
                    let resumed_job = durable.commit(
                        &job.id,
                        JobStatus::Running,
                        json!({"run_id":request.run_id,"round":round,"messages":messages}),
                        false,
                    )?;
                    (resumed_job, messages, round, true, None, Vec::new())
                } else if request.resume_approved
                    && matches!(job.status, JobStatus::Waiting | JobStatus::Suspended)
                {
                    let mut pending_calls = job.checkpoint["pending_tools"]
                        .as_array()
                        .map(|calls| {
                            calls
                                .iter()
                                .cloned()
                                .map(serde_json::from_value::<ToolCall>)
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .transpose()
                        .map_err(|error| {
                            ExecutionError::Durable(DurableError::Invalid(error.to_string()))
                        })?
                        .unwrap_or_default();
                    if pending_calls.is_empty() {
                        pending_calls.push(
                            serde_json::from_value::<ToolCall>(
                                job.checkpoint["pending_tool"].clone(),
                            )
                            .map_err(|error| {
                                ExecutionError::Durable(DurableError::Invalid(error.to_string()))
                            })?,
                        );
                    }
                    let call = pending_calls.remove(0);
                    let mut write_ahead_calls = Vec::with_capacity(pending_calls.len() + 1);
                    write_ahead_calls.push(call.clone());
                    write_ahead_calls.extend(pending_calls.iter().cloned());
                    let resumed_job = durable.commit(
                        &job.id,
                        JobStatus::Running,
                        json!({
                            "run_id":request.run_id,
                            "round":round,
                            "messages":messages,
                            "pending_tool":call.clone(),
                            "pending_tools":write_ahead_calls
                        }),
                        true,
                    )?;
                    (
                        resumed_job,
                        messages,
                        round,
                        true,
                        Some(call),
                        pending_calls,
                    )
                } else {
                    (job, messages, round, true, None, Vec::new())
                }
            } else {
                let job = durable.create_job(
                    &request.bot_id,
                    "model_run",
                    json!({"run_id":request.run_id,"round":0,"messages":initial_messages}),
                )?;
                (job, initial_messages, 0, false, None, Vec::new())
            }
        };
        let streaming_message_id = if request.private {
            Some(self.create_streaming_message(&request).await?)
        } else {
            None
        };
        let mut usage = TokenUsage::default();
        let mut final_text = String::new();
        let mut turns_used = 0;
        let mut completed = false;
        let mut group_progress_count = 0usize;
        let mut group_done = false;
        let mut group_no_report_rounds = 0usize;
        if resumed {
            self.trace(&request, "run.resume", json!({"by_message_id":null}))
                .await?;
        } else {
            self.trace(
                &request,
                "run.start",
                json!({"phase":trace_phase(&request),"model":request.model,"parent_run_id":request.parent_run_id,"subagent_task":request.subagent_task}),
            )
            .await?;
        }
        if !resumed {
            match self.sink.prepare_model_context(&request).await {
                Ok(Some(context)) => {
                    messages.insert(0, json!({"role":"system","content":context}));
                }
                Ok(None) => {}
                Err(message) => {
                    let error = ExecutionError::Sink(message);
                    self.fail_run(&request, &job, &error, streaming_message_id.as_deref())
                        .await?;
                    return Err(error);
                }
            }
        }
        if let Some(message) = request.resume_message.as_deref() {
            if let Some(calls) = job.checkpoint["pending_tools"].as_array() {
                for call in calls {
                    let call: ToolCall = serde_json::from_value(call.clone()).map_err(|error| {
                        ExecutionError::Durable(DurableError::Invalid(error.to_string()))
                    })?;
                    messages.push(json!({
                        "role":"tool",
                        "tool_call_id":call.call_id,
                        "content":"deferred until the waiting reply",
                        "is_error":true
                    }));
                }
            }
            messages.push(json!({"role":"user","content":message}));
            self.state.durable.lock().await.commit(
                &job.id,
                JobStatus::Running,
                json!({"run_id":request.run_id,"round":start_turn,"messages":messages}),
                false,
            )?;
        }
        let model_start_turn = if let Some(call) = approved_call {
            if matches!(call.name.as_str(), "ask_user" | "question") {
                self.trace(
                    &request,
                    "tool.start",
                    json!({"call_id":call.call_id,"name":call.name,"args":call.args}),
                )
                .await?;
                let result = ToolResult::text(request.instruction.clone());
                self.trace(
                    &request,
                    "tool.end",
                    json!({"call_id":call.call_id,"is_error":false,"preview":request.instruction,"details":{},"truncated":false,"full_output":null,"duration_ms":0}),
                )
                .await?;
                messages.push(tool_message(&call.call_id, &result));
                self.state.durable.lock().await.commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":request.run_id,"round":start_turn + 1,"messages":messages}),
                    false,
                )?;
            } else if call.name == "request_takeover" {
                self.trace(
                    &request,
                    "tool.start",
                    json!({"call_id":call.call_id,"name":call.name,"args":call.args}),
                )
                .await?;
                let result = ToolResult::text("用户已完成接管");
                self.trace(
                    &request,
                    "tool.end",
                    json!({"call_id":call.call_id,"is_error":false,"preview":"用户已完成接管","details":{},"truncated":false,"full_output":null,"duration_ms":0}),
                )
                .await?;
                messages.push(tool_message(&call.call_id, &result));
                self.state.durable.lock().await.commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":request.run_id,"round":start_turn + 1,"messages":messages}),
                    false,
                )?;
            } else {
                self.execute_approved_tool(&request, &job, start_turn, call, &mut messages)
                    .await?;
            }
            if let Some(outcome) = self
                .process_approved_followups(
                    approved_followups,
                    ApprovedFollowupContext {
                        request: &request,
                        job: &job,
                        turn: start_turn,
                        messages: &mut messages,
                        group_progress_count: &mut group_progress_count,
                        group_done: &mut group_done,
                        usage: &usage,
                    },
                )
                .await?
            {
                return Ok(outcome);
            }
            start_turn + 1
        } else {
            start_turn
        };

        for turn in model_start_turn..max_turns {
            if self.state.cancelled.lock().await.contains(&request.run_id) {
                self.cleanup_run_tools(&request).await;
                return Ok(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "cancelled".into(),
                    text: final_text,
                    usage,
                    turns: turns_used,
                });
            }
            turns_used = turn + 1;
            if let Some(steer) = {
                let mut durable = self.state.durable.lock().await;
                durable.deliver_next(&job.id)?
            } {
                messages.push(json!({"role":"user","content":steer.text.clone()}));
                self.trace(
                    &request,
                    "steer",
                    json!({"message_id":steer.message_id,"text":steer.text,"from":{"kind":"user"}}),
                )
                .await?;
                self.sink
                    .emit(ExecutionEvent {
                        event: "message.updated".into(),
                        data: json!({"message":steer_message(&request, &steer, "delivered")}),
                        persistent: true,
                    })
                    .await;
                {
                    let mut durable = self.state.durable.lock().await;
                    durable.mark_read(&steer.id)?;
                }
                self.sink
                    .emit(ExecutionEvent {
                        event: "message.updated".into(),
                        data: json!({"message":steer_message(&request, &steer, "read")}),
                        persistent: true,
                    })
                    .await;
            }
            if !self
                .commit_checkpoint(
                    &request,
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":request.run_id,"round":turn,"messages":messages}),
                    false,
                )
                .await?
            {
                self.cleanup_run_tools(&request).await;
                return Ok(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "cancelled".into(),
                    text: final_text,
                    usage,
                    turns: turns_used,
                });
            }
            if let Some(bucket) = model_rate_bucket(&request) {
                let cancellation = self.state.cancellation_for(&request.run_id).await;
                if !self
                    .state
                    .model_rate_limiter
                    .acquire(&bucket, &cancellation)
                    .await?
                {
                    self.cleanup_run_tools(&request).await;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id.clone(),
                        job_id: job.id.clone(),
                        status: "cancelled".into(),
                        text: final_text,
                        usage,
                        turns: turns_used,
                    });
                }
            }
            // A run/turn is one billable model request. Keeping this ID stable
            // makes a durable retry deduplicate its usage record.
            let request_id = format!("{}:llm:{}", request.run_id, turn);
            let model_request = ModelRequest {
                model: request.model.clone(),
                messages: messages.clone(),
                tools: self.tool_schemas(&request, !subagent_requested(&request)),
                max_output: 8192,
                session_id: Some(request.run_id.clone()),
            };
            let prompt_ref = self.persist_model_request(&request, &request_id, &model_request)?;
            let tools = model_request
                .tools
                .iter()
                .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
                .collect::<Vec<_>>();
            self.trace(&request, "llm.request", json!({"request_id":request_id,"model":request.model,"context":estimate_context(&messages),"tools":tools,"prompt_ref":prompt_ref})).await?;
            let provider = if let Some(resolver) = &self.provider_resolver {
                match resolver.resolve(&request.provider_id, &request.model).await {
                    Ok(provider) => provider,
                    Err(message) => {
                        let error = ExecutionError::Resolver(message);
                        self.fail_run(&request, &job, &error, streaming_message_id.as_deref())
                            .await?;
                        return Err(error);
                    }
                }
            } else {
                self.provider.clone()
            };
            let completion = self
                .stream_completion(
                    &request,
                    provider,
                    model_request,
                    &request_id,
                    streaming_message_id.as_deref(),
                )
                .await;
            let (completion, latency_ms, ttft_ms) = match completion {
                Ok(completion) => completion,
                Err(_) if self.is_cancelled(&request.run_id).await => {
                    self.cleanup_run_tools(&request).await;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id.clone(),
                        job_id: job.id.clone(),
                        status: "cancelled".into(),
                        text: final_text,
                        usage,
                        turns: turns_used,
                    });
                }
                Err(error) => {
                    self.fail_run(&request, &job, &error, streaming_message_id.as_deref())
                        .await?;
                    return Err(error);
                }
            };
            if self.state.cancelled.lock().await.contains(&request.run_id) {
                self.cleanup_run_tools(&request).await;
                return Ok(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "cancelled".into(),
                    text: final_text,
                    usage,
                    turns: turns_used,
                });
            }
            usage.input_tokens += completion.usage.input_tokens;
            usage.output_tokens += completion.usage.output_tokens;
            usage.cache_read_tokens += completion.usage.cache_read_tokens;
            usage.cache_write_tokens += completion.usage.cache_write_tokens;
            let request_cost = request.price.as_ref().map(|price| {
                price.cost(&Totals {
                    input_tokens: completion.usage.input_tokens,
                    output_tokens: completion.usage.output_tokens,
                    cache_read_tokens: completion.usage.cache_read_tokens,
                    cache_write_tokens: completion.usage.cache_write_tokens,
                    requests: 1,
                    cost: None,
                })
            });
            self.usage.lock().await.record(UsageRecord {
                request_id: request_id.clone(),
                ts: Utc::now(),
                bot_id: request.bot_id.clone(),
                project_id: request.project_id.clone(),
                chat_id: request.chat_id.clone(),
                assignment_id: request.assignment_id.clone(),
                run_id: request.run_id.clone(),
                phase: execution_phase(&request).into(),
                provider_id: request.provider_id.clone(),
                model_id: usage_model_id(&request.provider_id, &request.model),
                routine: request.routine,
                usage: Totals {
                    input_tokens: completion.usage.input_tokens,
                    output_tokens: completion.usage.output_tokens,
                    cache_read_tokens: completion.usage.cache_read_tokens,
                    cache_write_tokens: completion.usage.cache_write_tokens,
                    requests: 1,
                    cost: request_cost,
                },
                // A model request is only task-complete when the run has
                // reached its terminal text response. Chat turns without an
                // assignment, subagents, and routines never count as tasks.
                task_done: !subagent_requested(&request)
                    && !request.routine
                    && request.assignment_id.is_some()
                    && completion.tool_calls.is_empty()
                    && (request.private || group_done),
            })?;
            self.record_usage_tick(&request, &completion.usage, request_cost)
                .await;
            self.trace(
                &request,
                    "llm.response",
                json!({"request_id":request_id,"text":completion.text,"thinking":if completion.thinking.is_empty(){Value::Null}else{json!(completion.thinking)},"tool_calls":completion.tool_calls,"stop_reason":completion.stop_reason,"usage":{"input_tokens":completion.usage.input_tokens,"output_tokens":completion.usage.output_tokens,"cache_read_tokens":completion.usage.cache_read_tokens,"cache_write_tokens":completion.usage.cache_write_tokens,"requests":1,"cost":null},"latency_ms":latency_ms,"ttft_ms":ttft_ms}),
            )
            .await?;

            if completion.tool_calls.is_empty() {
                if !request.private
                    && !group_done
                    && group_no_report_rounds < 2
                    && turn + 1 < max_turns
                {
                    group_no_report_rounds += 1;
                    messages.push(assistant_message(&completion));
                    messages.push(json!({"role":"user","content":"请通过 send_msg 工具发送本轮进展或最终结果。"}));
                    continue;
                }
                final_text = completion.text.clone();
                completed = true;
                messages.push(assistant_message(&completion));
                self.publish_answer(
                    &request,
                    turn,
                    &completion.text,
                    streaming_message_id.as_deref(),
                )
                .await?;
                break;
            }

            messages.push(assistant_message(&completion));

            // Independent safe calls can overlap in any chat. A subagent is
            // safe here because its runtime receives its own read-only tool
            // allowlist; writes and browser/external calls stay ordered.
            let tool_calls = completion.tool_calls.clone();
            if tool_calls.len() > 1
                && tool_calls
                    .iter()
                    .all(|call| can_parallelize_tool_call(&request, call, &self.tools))
            {
                for call in &completion.tool_calls {
                    self.trace(
                        &request,
                        "tool.start",
                        json!({"call_id":call.call_id,"name":call.name,"args":call.args}),
                    )
                    .await?;
                }
                let results = join_all(
                    tool_calls
                        .iter()
                        .cloned()
                        .map(|call| self.execute_read_tool(&request, call)),
                )
                .await;
                for (call, result, duration_ms) in results {
                    self.trace(
                        &request,
                        "tool.end",
                        tool_end_data(&request, &call, &result, duration_ms),
                    )
                    .await?;
                    messages.push(tool_message(&call.call_id, &result));
                }
                continue;
            }
            for (call_index, call) in tool_calls.iter().cloned().enumerate() {
                if self.is_cancelled(&request.run_id).await {
                    self.cleanup_run_tools(&request).await;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id.clone(),
                        job_id: job.id.clone(),
                        status: "cancelled".into(),
                        text: final_text,
                        usage,
                        turns: turns_used,
                    });
                }
                self.trace(
                    &request,
                    "tool.start",
                    json!({"call_id":call.call_id,"name":call.name,"args":call.args}),
                )
                .await?;
                if !tool_allowed(&request, &call.name) {
                    let result = ToolResult::error("tool is not permitted for this run");
                    messages.push(tool_message(&call.call_id, &result));
                    self.trace(&request, "tool.end", json!({"call_id":call.call_id,"is_error":true,"preview":"tool is not permitted for this run","details":{},"truncated":false,"full_output":null,"duration_ms":0})).await?;
                    continue;
                }
                if matches!(call.name.as_str(), "ask_user" | "question") {
                    let question = call.args["question"]
                        .as_str()
                        .or_else(|| call.args["prompt"].as_str())
                        .unwrap_or("请回答这个问题")
                        .to_owned();
                    let checkpoint = json!({
                        "run_id":request.run_id,
                        "round":turn,
                        "messages":messages,
                        "pending_tool":call,
                        "pending_tools":tool_calls[call_index..]
                    });
                    self.state.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Waiting,
                        checkpoint,
                        true,
                    )?;
                    self.trace(
                        &request,
                        "run.wait",
                        json!({"reason":"decision","message_id":Value::Null}),
                    )
                    .await?;
                    self.publish_question_request(&request, &call.call_id, &question)
                        .await?;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id,
                        job_id: job.id,
                        status: "suspended".into(),
                        text: final_text,
                        usage,
                        turns: turn + 1,
                    });
                }
                if call.name == "request_takeover" {
                    let reason = call.args["reason"]
                        .as_str()
                        .unwrap_or("用户需要接管浏览器")
                        .to_owned();
                    let checkpoint = json!({
                        "run_id":request.run_id,
                        "round":turn,
                        "messages":messages,
                        "pending_tool":call,
                        "pending_tools":tool_calls[call_index..]
                    });
                    self.state.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Waiting,
                        checkpoint,
                        true,
                    )?;
                    self.trace(
                        &request,
                        "run.wait",
                        json!({"reason":"takeover","message_id":Value::Null}),
                    )
                    .await?;
                    self.publish_takeover_request(&request, &reason).await?;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id,
                        job_id: job.id,
                        status: "suspended".into(),
                        text: final_text,
                        usage,
                        turns: turn + 1,
                    });
                }
                let Some(tool) = self.tools.get(&call.name).cloned() else {
                    if call.name == "send_msg" {
                        let intent = call.args["intent"].as_str().unwrap_or("progress");
                        if intent == "progress" && group_progress_count >= 3 {
                            let result = ToolResult::error(
                                "progress messages are limited to three per task",
                            );
                            messages.push(tool_message(&call.call_id, &result));
                            continue;
                        }
                        let chat_id = call.args["chat_id"]
                            .as_str()
                            .unwrap_or(&request.chat_id)
                            .to_owned();
                        let message_id = call.args["message_id"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("msg_{}", safe_id(&call.call_id)));
                        let mut payload = call.args.clone();
                        if let Some(object) = payload.as_object_mut() {
                            object.insert("message_id".into(), json!(message_id.clone()));
                            object.insert("chat_id".into(), json!(chat_id.clone()));
                            object.insert("bot_id".into(), json!(request.bot_id.clone()));
                            object.insert(
                                "assignment_id".into(),
                                json!(request.assignment_id.clone()),
                            );
                            object.insert("intent".into(), json!(intent));
                        }
                        let (receipt, _created) = self.state.durable.lock().await.send_msg_once(
                            &request.run_id,
                            &call.call_id,
                            intent,
                            &message_id,
                            payload.clone(),
                        )?;
                        // Re-submit an existing receipt too: a crash can
                        // happen after durable admission and before the
                        // bridge call. The bridge is idempotent by run/call.
                        let persisted_payload = receipt.payload.clone();
                        let canonical = match self
                            .sink
                            .send_group_message(
                                json!({"receipt":receipt,"message":persisted_payload}),
                            )
                            .await
                        {
                            Ok(message) => message,
                            Err(error) => {
                                let result = ToolResult::error(error);
                                messages.push(tool_message(&call.call_id, &result));
                                self.trace(&request, "tool.end", json!({"call_id":call.call_id,"is_error":true,"preview":"send_msg bridge failed","details":{},"truncated":false,"full_output":null,"duration_ms":0})).await?;
                                continue;
                            }
                        };
                        let Some(canonical_id) = canonical_message_id(&canonical) else {
                            let result = ToolResult::error(
                                "send_msg bridge returned no canonical message id",
                            );
                            messages.push(tool_message(&call.call_id, &result));
                            self.trace(&request, "tool.end", json!({"call_id":call.call_id,"is_error":true,"preview":"send_msg bridge returned malformed message","details":{},"truncated":false,"full_output":null,"duration_ms":0})).await?;
                            continue;
                        };
                        if intent == "progress" {
                            group_progress_count += 1;
                        }
                        if intent == "done" {
                            group_done = true;
                        }
                        self.trace(&request, "send_msg", json!({"call_id":call.call_id,"intent":intent,"message_id":canonical_id,"chat_id":chat_id})).await?;
                        let result = ToolResult::text("message admitted");
                        messages.push(tool_message(&call.call_id, &result));
                        if intent == "done" {
                            final_text = call.args["text"]
                                .as_str()
                                .unwrap_or("任务已完成")
                                .to_owned();
                            self.state.durable.lock().await.commit(
                                &job.id,
                                JobStatus::Done,
                                json!({"run_id":request.run_id,"text":final_text,"usage":usage,"turns":turns_used,"messages":messages}),
                                false,
                            )?;
                            self.sink
                                .commit_succeeded(
                                    &request,
                                    json!({"run_id":request.run_id,"text":final_text}),
                                )
                                .await;
                            self.trace(
                                &request,
                                "run.end",
                                json!({"status":"done","error":Value::Null}),
                            )
                            .await?;
                            self.cleanup_run_tools(&request).await;
                            return Ok(ExecutionOutcome {
                                run_id: request.run_id,
                                job_id: job.id,
                                status: "done".into(),
                                text: final_text,
                                usage,
                                turns: turns_used,
                            });
                        }
                        if matches!(intent, "decision" | "blocked") {
                            let remaining = tool_calls[(call_index + 1)..].to_vec();
                            self.state.durable.lock().await.commit(
                                &job.id,
                                JobStatus::Waiting,
                                json!({
                                    "run_id":request.run_id,
                                    "round":turn,
                                    "messages":messages,
                                    "pending_tools":remaining,
                                    "waiting_reason":intent,
                                    "waiting_message_id":canonical_id,
                                    "waiting_message":true,
                                    "wait_intent":intent
                                }),
                                false,
                            )?;
                            self.trace(
                                &request,
                                "run.wait",
                                json!({"reason":if intent == "decision" {"decision"} else {"blocked"},"message_id":canonical_id}),
                            )
                            .await?;
                            return Ok(ExecutionOutcome {
                                run_id: request.run_id,
                                job_id: job.id,
                                status: "suspended".into(),
                                text: final_text,
                                usage,
                                turns: turn + 1,
                            });
                        }
                        continue;
                    }
                    let result = ToolResult::error(format!("unknown tool: {}", call.name));
                    messages.push(tool_message(&call.call_id, &result));
                    self.trace(&request, "tool.end", json!({"call_id":call.call_id,"is_error":true,"preview":"unknown tool","details":{},"truncated":false,"full_output":null,"duration_ms":0})).await?;
                    continue;
                };
                let risk = tool.risk(&call.args);
                let cwd = request.cwd.as_deref().unwrap_or(self.home.as_path());
                let builtin_risky =
                    builtin_requires_approval(&call.name, &call.args, cwd, &self.home);
                let coordination_confirmation =
                    self.coordination_tool_requires_approval(&call.name, &call.args);
                let risky = coordination_confirmation
                    || (!coordination_tool_uses_internal_policy(&call.name)
                        && (matches!(&risk, Risk::Write | Risk::Exec | Risk::External)
                            || builtin_risky));
                if risky && !self.risky_call_allowed(&request, &call.name, &call.args) {
                    let checkpoint = json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call,"pending_tools":tool_calls[call_index..]});
                    self.state.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Waiting,
                        checkpoint,
                        true,
                    )?;
                    self.trace(
                        &request,
                        "run.wait",
                        json!({"reason":"approval","message_id":Value::Null}),
                    )
                    .await?;
                    let risk = match risk {
                        Risk::Write => "write",
                        Risk::Exec => "exec",
                        Risk::External => "external",
                        Risk::Read => "write",
                    };
                    self.sink
                        .approval_required(json!({"approval":{
                            "id":format!("apr_{}",safe_id(&call.call_id)),
                            "bot_id":request.bot_id,
                            "assignment_id":request.assignment_id,
                            "chat_id":request.chat_id,
                            "tool":call.name,
                            "risk":risk,
                            "summary":format!("Approval required for {}",call.name),
                            "detail":call.args.to_string(),
                            "state":"pending",
                            "created_at":now_rfc3339(),
                            "decided_at":null
                        }}))
                        .await;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id,
                        job_id: job.id,
                        status: "suspended".into(),
                        text: final_text,
                        usage,
                        turns: turn + 1,
                    });
                }
                if risky {
                    let checkpoint = json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call,"pending_tools":tool_calls[call_index..]});
                    self.state.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Running,
                        checkpoint,
                        true,
                    )?;
                }
                let cwd = request.cwd.clone().unwrap_or_else(|| self.home.clone());
                let (output_tx, output_rx) = mpsc::channel(64);
                let output_forwarder = self.forward_tool_output(&request, output_rx);
                let context = ToolContext::new(cwd, request.run_id.clone(), self.home.join("runs"))
                    .with_cancellation(self.state.cancellation_for(&request.run_id).await)
                    .with_output_channel(call.call_id.clone(), output_tx);
                let started = now_ms();
                let result = tool.call(&context, call.args.clone()).await;
                drop(context);
                let _ = output_forwarder.await;
                self.trace(
                    &request,
                    "tool.end",
                    tool_end_data(&request, &call, &result, now_ms().saturating_sub(started)),
                )
                .await?;
                if self.is_cancelled(&request.run_id).await {
                    self.cleanup_run_tools(&request).await;
                    return Ok(ExecutionOutcome {
                        run_id: request.run_id.clone(),
                        job_id: job.id.clone(),
                        status: "cancelled".into(),
                        text: final_text,
                        usage,
                        turns: turns_used,
                    });
                }
                messages.push(tool_message(&call.call_id, &result));
                if risky {
                    let remaining = &tool_calls[(call_index + 1)..];
                    let mut checkpoint = json!({
                        "run_id":request.run_id,
                        "round":turn,
                        "messages":messages
                    });
                    if let Some(object) = checkpoint.as_object_mut() {
                        if let Some(next) = remaining.first() {
                            object.insert("pending_tool".into(), json!(next));
                            object.insert("pending_tools".into(), json!(remaining));
                        }
                    }
                    if !self
                        .commit_checkpoint(
                            &request,
                            &job.id,
                            JobStatus::Running,
                            checkpoint,
                            !remaining.is_empty(),
                        )
                        .await?
                    {
                        self.cleanup_run_tools(&request).await;
                        return Ok(ExecutionOutcome {
                            run_id: request.run_id.clone(),
                            job_id: job.id.clone(),
                            status: "cancelled".into(),
                            text: final_text,
                            usage,
                            turns: turns_used,
                        });
                    }
                }
            }
        }
        if !completed {
            let error = ExecutionError::TurnLimit;
            self.fail_run(&request, &job, &error, streaming_message_id.as_deref())
                .await?;
            return Err(error);
        }
        {
            let mut durable = self.state.durable.lock().await;
            let cancelled = self.state.cancelled.lock().await.contains(&request.run_id);
            if cancelled {
                drop(durable);
                self.cleanup_run_tools(&request).await;
                return Ok(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "cancelled".into(),
                    text: final_text,
                    usage,
                    turns: turns_used,
                });
            }
            durable.commit(
                &job.id,
                JobStatus::Done,
                json!({"run_id":request.run_id,"text":final_text,"usage":usage,"turns":turns_used,"messages":messages}),
                false,
            )?;
        }
        self.sink
            .commit_succeeded(&request, json!({"run_id":request.run_id,"text":final_text}))
            .await;
        self.trace(
            &request,
            "run.end",
            json!({"status":"done","error":Value::Null}),
        )
        .await?;
        self.cleanup_run_tools(&request).await;
        Ok(ExecutionOutcome {
            run_id: request.run_id,
            job_id: job.id,
            status: "done".into(),
            text: final_text,
            usage,
            turns: turns_used,
        })
    }

    async fn execute_approved_tool(
        &self,
        request: &ExecutionRequest,
        job: &Job,
        turn: usize,
        call: ToolCall,
        messages: &mut Vec<Value>,
    ) -> Result<(), ExecutionError> {
        if !tool_allowed(request, &call.name) {
            return Err(ExecutionError::Sink(format!(
                "approved tool is not permitted: {}",
                call.name
            )));
        }
        let Some(tool) = self.tools.get(&call.name).cloned() else {
            return Err(ExecutionError::Sink(format!(
                "approved tool disappeared: {}",
                call.name
            )));
        };
        self.trace(
            request,
            "tool.start",
            json!({"call_id":call.call_id,"name":call.name,"args":call.args.clone()}),
        )
        .await?;
        let (output_tx, output_rx) = mpsc::channel(64);
        let output_forwarder = self.forward_tool_output(request, output_rx);
        let context = ToolContext::new(
            request.cwd.clone().unwrap_or_else(|| self.home.clone()),
            request.run_id.clone(),
            self.home.join("runs"),
        )
        .with_cancellation(self.state.cancellation_for(&request.run_id).await)
        .with_output_channel(call.call_id.clone(), output_tx);
        let started = now_ms();
        let result = tool.call(&context, call.args.clone()).await;
        drop(context);
        let _ = output_forwarder.await;
        self.trace(
            request,
            "tool.end",
            tool_end_data(request, &call, &result, now_ms().saturating_sub(started)),
        )
        .await?;
        messages.push(tool_message(&call.call_id, &result));
        self.state.durable.lock().await.commit(
            &job.id,
            JobStatus::Running,
            json!({"run_id":request.run_id,"round":turn + 1,"messages":messages}),
            false,
        )?;
        Ok(())
    }

    /// Continue the remaining tool calls from an approval checkpoint one by
    /// one.  A model turn can mix a write with reads and another write; the
    /// latter must not inherit the first call's hard-coded risk or approval.
    async fn process_approved_followups(
        &self,
        followups: Vec<ToolCall>,
        context: ApprovedFollowupContext<'_>,
    ) -> Result<Option<ExecutionOutcome>, ExecutionError> {
        let ApprovedFollowupContext {
            request,
            job,
            turn,
            messages,
            group_progress_count,
            group_done,
            usage,
        } = context;
        let mut pending = followups;
        while !pending.is_empty() {
            let call = pending.remove(0);
            if !tool_allowed(request, &call.name) {
                let result = ToolResult::error("tool is not permitted for this run");
                self.trace(
                    request,
                    "tool.start",
                    json!({"call_id":call.call_id,"name":call.name,"args":call.args}),
                )
                .await?;
                self.trace(
                    request,
                    "tool.end",
                    json!({"call_id":call.call_id,"is_error":true,"preview":"tool is not permitted for this run","details":{},"truncated":false,"full_output":null,"duration_ms":0}),
                )
                .await?;
                messages.push(tool_message(&call.call_id, &result));
                continue;
            }

            if matches!(call.name.as_str(), "ask_user" | "question") {
                let question = call.args["question"]
                    .as_str()
                    .or_else(|| call.args["prompt"].as_str())
                    .unwrap_or("请回答这个问题")
                    .to_owned();
                let mut all_calls = Vec::with_capacity(pending.len() + 1);
                all_calls.push(call.clone());
                all_calls.extend(pending.iter().cloned());
                self.state.durable.lock().await.commit(
                    &job.id,
                    JobStatus::Waiting,
                    json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call,"pending_tools":all_calls}),
                    true,
                )?;
                self.trace(
                    request,
                    "run.wait",
                    json!({"reason":"decision","message_id":Value::Null}),
                )
                .await?;
                self.publish_question_request(request, &call.call_id, &question)
                    .await?;
                return Ok(Some(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "suspended".into(),
                    text: String::new(),
                    usage: usage.clone(),
                    turns: turn + 1,
                }));
            }

            if call.name == "request_takeover" {
                let reason = call.args["reason"]
                    .as_str()
                    .unwrap_or("用户需要接管浏览器")
                    .to_owned();
                let mut all_calls = Vec::with_capacity(pending.len() + 1);
                all_calls.push(call.clone());
                all_calls.extend(pending.iter().cloned());
                self.state.durable.lock().await.commit(
                    &job.id,
                    JobStatus::Waiting,
                    json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call,"pending_tools":all_calls}),
                    true,
                )?;
                self.trace(
                    request,
                    "run.wait",
                    json!({"reason":"takeover","message_id":Value::Null}),
                )
                .await?;
                self.publish_takeover_request(request, &reason).await?;
                return Ok(Some(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "suspended".into(),
                    text: String::new(),
                    usage: usage.clone(),
                    turns: turn + 1,
                }));
            }

            // send_msg is a gateway bridge rather than a Tool implementation.
            // It is already idempotent by run/call and must be admitted once,
            // instead of being mistaken for a generic write follow-up.
            if call.name == "send_msg" {
                let intent = call.args["intent"].as_str().unwrap_or("progress");
                if intent == "progress" && *group_progress_count >= 3 {
                    messages.push(tool_message(
                        &call.call_id,
                        &ToolResult::error("progress messages are limited to three per task"),
                    ));
                    continue;
                }
                let chat_id = call.args["chat_id"]
                    .as_str()
                    .unwrap_or(&request.chat_id)
                    .to_owned();
                let message_id = call.args["message_id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("msg_{}", safe_id(&call.call_id)));
                let mut payload = call.args.clone();
                if let Some(object) = payload.as_object_mut() {
                    object.insert("message_id".into(), json!(message_id.clone()));
                    object.insert("chat_id".into(), json!(chat_id.clone()));
                    object.insert("bot_id".into(), json!(request.bot_id.clone()));
                    object.insert("assignment_id".into(), json!(request.assignment_id.clone()));
                    object.insert("intent".into(), json!(intent));
                }
                let (receipt, _) = self.state.durable.lock().await.send_msg_once(
                    &request.run_id,
                    &call.call_id,
                    intent,
                    &message_id,
                    payload.clone(),
                )?;
                let canonical = match self
                    .sink
                    .send_group_message(json!({"receipt":receipt,"message":payload}))
                    .await
                {
                    Ok(message) => message,
                    Err(error) => {
                        let result = ToolResult::error(error);
                        messages.push(tool_message(&call.call_id, &result));
                        self.trace(
                            request,
                            "tool.end",
                            json!({"call_id":call.call_id,"is_error":true,"preview":"send_msg bridge failed","details":{},"truncated":false,"full_output":null,"duration_ms":0}),
                        )
                        .await?;
                        continue;
                    }
                };
                let Some(canonical_id) = canonical_message_id(&canonical) else {
                    let result =
                        ToolResult::error("send_msg bridge returned no canonical message id");
                    messages.push(tool_message(&call.call_id, &result));
                    self.trace(
                        request,
                        "tool.end",
                        json!({"call_id":call.call_id,"is_error":true,"preview":"send_msg bridge returned malformed message","details":{},"truncated":false,"full_output":null,"duration_ms":0}),
                    )
                    .await?;
                    continue;
                };
                if intent == "progress" {
                    *group_progress_count += 1;
                }
                if intent == "done" {
                    *group_done = true;
                }
                self.trace(
                    request,
                    "send_msg",
                    json!({"call_id":call.call_id,"intent":intent,"message_id":canonical_id,"chat_id":chat_id}),
                )
                .await?;
                let result = ToolResult::text("message admitted");
                messages.push(tool_message(&call.call_id, &result));
                if intent == "done" {
                    let final_text = call.args["text"]
                        .as_str()
                        .unwrap_or("任务已完成")
                        .to_owned();
                    self.state.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Done,
                        json!({"run_id":request.run_id,"text":final_text,"usage":usage,"turns":turn + 1,"messages":messages}),
                        false,
                    )?;
                    self.sink
                        .commit_succeeded(
                            request,
                            json!({"run_id":request.run_id,"text":final_text}),
                        )
                        .await;
                    self.trace(
                        request,
                        "run.end",
                        json!({"status":"done","error":Value::Null}),
                    )
                    .await?;
                    self.cleanup_run_tools(request).await;
                    return Ok(Some(ExecutionOutcome {
                        run_id: request.run_id.clone(),
                        job_id: job.id.clone(),
                        status: "done".into(),
                        text: final_text,
                        usage: usage.clone(),
                        turns: turn + 1,
                    }));
                }
                if matches!(intent, "decision" | "blocked") {
                    self.state.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Waiting,
                        json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tools":pending,"waiting_reason":intent,"waiting_message_id":canonical_id,"waiting_message":true,"wait_intent":intent}),
                        false,
                    )?;
                    self.trace(
                        request,
                        "run.wait",
                        json!({"reason":if intent == "decision" {"decision"} else {"blocked"},"message_id":canonical_id}),
                    )
                    .await?;
                    return Ok(Some(ExecutionOutcome {
                        run_id: request.run_id.clone(),
                        job_id: job.id.clone(),
                        status: "suspended".into(),
                        text: String::new(),
                        usage: usage.clone(),
                        turns: turn + 1,
                    }));
                }
                continue;
            }

            let Some(tool) = self.tools.get(&call.name).cloned() else {
                messages.push(tool_message(
                    &call.call_id,
                    &ToolResult::error(format!("unknown tool: {}", call.name)),
                ));
                continue;
            };
            let risk = tool.risk(&call.args);
            let cwd = request.cwd.as_deref().unwrap_or(self.home.as_path());
            let builtin_risky = builtin_requires_approval(&call.name, &call.args, cwd, &self.home);
            let coordination_confirmation =
                self.coordination_tool_requires_approval(&call.name, &call.args);
            let risky = coordination_confirmation
                || (!coordination_tool_uses_internal_policy(&call.name)
                    && (matches!(&risk, Risk::Write | Risk::Exec | Risk::External)
                        || builtin_risky));
            if risky && !self.risky_call_allowed(request, &call.name, &call.args) {
                let mut all_calls = Vec::with_capacity(pending.len() + 1);
                all_calls.push(call.clone());
                all_calls.extend(pending.iter().cloned());
                self.state.durable.lock().await.commit(
                    &job.id,
                    JobStatus::Waiting,
                    json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call,"pending_tools":all_calls}),
                    true,
                )?;
                self.trace(
                    request,
                    "run.wait",
                    json!({"reason":"approval","message_id":Value::Null}),
                )
                .await?;
                let risk_name = match risk {
                    Risk::Write => "write",
                    Risk::Exec => "exec",
                    Risk::External => "external",
                    Risk::Read => "write",
                };
                self.sink
                    .approval_required(json!({"approval":{
                        "id":format!("apr_{}",safe_id(&call.call_id)),
                        "bot_id":request.bot_id,
                        "assignment_id":request.assignment_id,
                        "chat_id":request.chat_id,
                        "tool":call.name,
                        "risk":risk_name,
                        "summary":format!("Approval required for {}",call.name),
                        "detail":call.args.to_string(),
                        "state":"pending",
                        "created_at":now_rfc3339(),
                        "decided_at":null
                    }}))
                    .await;
                return Ok(Some(ExecutionOutcome {
                    run_id: request.run_id.clone(),
                    job_id: job.id.clone(),
                    status: "suspended".into(),
                    text: String::new(),
                    usage: usage.clone(),
                    turns: turn + 1,
                }));
            }
            if risky {
                let mut all_calls = Vec::with_capacity(pending.len() + 1);
                all_calls.push(call.clone());
                all_calls.extend(pending.iter().cloned());
                self.state.durable.lock().await.commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call,"pending_tools":all_calls}),
                    true,
                )?;
            }
            self.execute_approved_tool(request, job, turn, call, messages)
                .await?;
        }
        Ok(None)
    }

    async fn execute_read_tool(
        &self,
        request: &ExecutionRequest,
        call: ToolCall,
    ) -> (ToolCall, ToolResult, u64) {
        let started = now_ms();
        let result = if let Some(tool) = self.tools.get(&call.name).cloned() {
            let (output_tx, output_rx) = mpsc::channel(64);
            let output_forwarder = self.forward_tool_output(request, output_rx);
            let context = ToolContext::new(
                request.cwd.clone().unwrap_or_else(|| self.home.clone()),
                request.run_id.clone(),
                self.home.join("runs"),
            )
            .with_cancellation(self.state.cancellation_for(&request.run_id).await)
            .with_output_channel(call.call_id.clone(), output_tx);
            let result = tool.call(&context, call.args.clone()).await;
            drop(context);
            let _ = output_forwarder.await;
            result
        } else {
            ToolResult::error(format!("unknown tool: {}", call.name))
        };
        (call, result, now_ms().saturating_sub(started))
    }

    fn forward_tool_output(
        &self,
        request: &ExecutionRequest,
        mut output: mpsc::Receiver<ToolOutputChunk>,
    ) -> tokio::task::JoinHandle<()> {
        let sink = self.sink.clone();
        let stream = trace_scope(request);
        tokio::spawn(async move {
            while let Some(chunk) = output.recv().await {
                sink.emit(ExecutionEvent {
                    event: "trace.tool_output".into(),
                    data: json!({
                        "stream":stream,
                        "call_id":chunk.call_id,
                        "chunk":chunk.chunk
                    }),
                    persistent: false,
                })
                .await;
            }
        })
    }

    fn tool_schemas(&self, request: &ExecutionRequest, include_group_send: bool) -> Vec<Value> {
        let mut schemas = self
            .tools
            .values()
            .filter(|tool| tool_allowed(request, tool.name()))
            .map(|tool| {
                json!({"type":"function","function":{"name":tool.name(),"description":tool.description(),"parameters":tool.schema()}})
            })
            .collect::<Vec<_>>();
        if include_group_send
            && tool_allowed(request, "send_msg")
            && !self.tools.contains_key("send_msg")
        {
            schemas.push(json!({
                "type":"function",
                "function":{
                    "name":"send_msg",
                    "description":"Send a complete message to a group chat.",
                    "parameters":{
                        "type":"object",
                        "required":["intent","text"],
                        "properties":{
                            "intent":{"type":"string","enum":["ack","progress","decision","done","blocked"]},
                            "text":{"type":"string"},
                                "to":{"type":["string","object"]},
                                "mentions":{"type":"array","items":{}},
                                "artifacts":{"type":"array","items":{"type":"object"}},
                                "options":{"type":"array","items":{"type":"string"}},
                            "chat_id":{"type":"string"},
                            "message_id":{"type":"string"}
                        }
                    }
                }
            }));
        }
        schemas
    }

    async fn stream_completion(
        &self,
        request: &ExecutionRequest,
        provider: Arc<dyn ModelProvider>,
        model: ModelRequest,
        request_id: &str,
        message_id: Option<&str>,
    ) -> Result<(Completion, u64, u64), ExecutionError> {
        let (tx, mut rx) = mpsc::channel(256);
        let started = Instant::now();
        let first_delta = Arc::new(Mutex::new(None::<Instant>));
        let first_delta_for_notify = first_delta.clone();
        let sink = self.sink.clone();
        let chat_id = request.chat_id.clone();
        let private = request.private;
        let request_id = request_id.to_owned();
        let message_id = message_id.map(str::to_owned);
        let notify = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ModelEvent::TextDelta { text } => {
                        let mut first = first_delta_for_notify.lock().await;
                        if first.is_none() {
                            *first = Some(Instant::now());
                        }
                        drop(first);
                        if private {
                            if let Some(message_id) = message_id.as_deref() {
                                sink.emit(ExecutionEvent {
                                    event: "message.delta".into(),
                                    data: json!({"chat_id":chat_id,"message_id":message_id,"text":text}),
                                    persistent: false,
                                }).await;
                            }
                        }
                        sink.emit(ExecutionEvent {
                            event: "trace.delta".into(),
                            data: json!({"stream":format!("chat_{}",safe_id(&chat_id)),"request_id":request_id,"channel":"text","call_id":null,"text":text}),
                            persistent: false,
                        }).await;
                    }
                    ModelEvent::ThinkingDelta { text } => {
                        let mut first = first_delta_for_notify.lock().await;
                        if first.is_none() {
                            *first = Some(Instant::now());
                        }
                        drop(first);
                        sink.emit(ExecutionEvent {
                            event: "trace.delta".into(),
                            data: json!({"stream":format!("chat_{}",safe_id(&chat_id)),"request_id":request_id,"channel":"thinking","call_id":null,"text":text}),
                            persistent: false,
                        }).await
                    }
                    ModelEvent::ToolCallDelta { call_id, args, .. } => {
                        let mut first = first_delta_for_notify.lock().await;
                        if first.is_none() {
                            *first = Some(Instant::now());
                        }
                        drop(first);
                        sink.emit(ExecutionEvent {
                            event: "trace.delta".into(),
                            data: json!({"stream":format!("chat_{}",safe_id(&chat_id)),"request_id":request_id,"channel":"tool_args","call_id":call_id,"text":args}),
                            persistent: false,
                        }).await
                    }
                    ModelEvent::Usage { .. } | ModelEvent::Stop { .. } => {}
                }
            }
        });
        let cancellation = self.state.cancellation_for(&request.run_id).await;
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                notify.abort();
                let _ = notify.await;
                return Err(ExecutionError::Sink("run cancelled".into()));
            }
            result = provider.stream(model, tx) => result,
        };
        let _ = notify.await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let ttft_ms = first_delta
            .lock()
            .await
            .as_ref()
            .map_or(latency_ms, |first| {
                first.duration_since(started).as_millis() as u64
            });
        Ok((result?, latency_ms, ttft_ms))
    }

    async fn publish_answer(
        &self,
        request: &ExecutionRequest,
        _turn: usize,
        text: &str,
        streaming_message_id: Option<&str>,
    ) -> Result<(), ExecutionError> {
        if !request.private {
            // Group messages are emitted only by the send_msg tool bridge.
            return Ok(());
        }
        let message_id = streaming_message_id
            .map(str::to_owned)
            .unwrap_or_else(|| format!("msg_{}", safe_id(&request.run_id)));
        let path = format!("data/chats/{}/messages.jsonl", safe_id(&request.chat_id));
        let Some(mut message) = self
            .store
            .read_jsonl::<Value>(&path)?
            .into_iter()
            .rev()
            .find(|message| message["id"] == message_id)
        else {
            return Ok(());
        };
        message["blocks"] = json!([{"type":"text","markdown":text}]);
        message["fallback_text"] = json!(text);
        message["streaming"] = json!(false);
        message["edited_at"] = json!(now_rfc3339());
        let message = self.canonical_chat_message(&request.chat_id, message)?;
        self.replace_message(&path, &message)?;
        self.sink
            .emit(ExecutionEvent {
                event: "message.updated".into(),
                data: json!({"message":message}),
                persistent: true,
            })
            .await;
        Ok(())
    }

    fn canonical_chat_message(
        &self,
        chat_id: &str,
        message: Value,
    ) -> Result<Value, ExecutionError> {
        self.store
            .sequence_chat_messages(chat_id, &[message])?
            .into_iter()
            .next()
            .ok_or_else(|| {
                ExecutionError::Sink("message sequence allocator returned no row".into())
            })
    }

    async fn publish_takeover_request(
        &self,
        request: &ExecutionRequest,
        reason: &str,
    ) -> Result<(), ExecutionError> {
        let message_id = format!("msg_takeover_{}", safe_id(&request.run_id));
        let path = format!("data/chats/{}/messages.jsonl", safe_id(&request.chat_id));
        let message = self.canonical_chat_message(&request.chat_id, json!({
            "id": message_id,
            "chat_id": request.chat_id,
            "sender": {"kind":"bot","bot_id":request.bot_id},
            "created_at": now_rfc3339(),
            "edited_at": null,
            "deleted": false,
            "reply_to": null,
            "thread_count": 0,
            "mentions": [],
            "blocks": [{"type":"takeover_request","bot_id":request.bot_id,"reason":reason,"state":"pending"}],
            "fallback_text": reason,
            "intent": null,
            "assignment_id": request.assignment_id,
            "streaming": false,
            "delivery": [],
            "reactions": []
        }))?;
        self.store.append_jsonl(&path, &message)?;
        // Keep a restart-safe marker for private runs, whose orchestrator
        // assignment is synthesized by the takeover RPC and is therefore not
        // present on ExecutionRequest.
        self.store.write_snapshot(
            format!("data/waiting/{}.json", safe_id(&message_id)),
            &json!({"kind":"takeover","run_id":request.run_id,"assignment_id":request.assignment_id,"chat_id":request.chat_id}),
        )?;
        self.sink
            .emit(ExecutionEvent {
                event: "message.created".into(),
                data: json!({"message":message}),
                persistent: true,
            })
            .await;
        Ok(())
    }

    async fn publish_question_request(
        &self,
        request: &ExecutionRequest,
        question_id: &str,
        question: &str,
    ) -> Result<(), ExecutionError> {
        let message_id = format!("msg_question_{}", safe_id(&request.run_id));
        let path = format!("data/chats/{}/messages.jsonl", safe_id(&request.chat_id));
        let message = self.canonical_chat_message(
            &request.chat_id,
            json!({
                "id": message_id,
                "chat_id": request.chat_id,
                "sender": {"kind":"bot","bot_id":request.bot_id},
                "created_at": now_rfc3339(),
                "edited_at": null,
                "deleted": false,
                "reply_to": null,
                "thread_count": 0,
                "mentions": [],
                "blocks": [{"type":"question","question_id":question_id}],
                "fallback_text": question,
                "intent": null,
                "assignment_id": request.assignment_id,
                "streaming": false,
                "delivery": [],
                "reactions": []
            }),
        )?;
        self.store.append_jsonl(&path, &message)?;
        // `ask_user` is an executor-level question, not an orchestrator
        // question. Persist the call id so question.answer can resume it even
        // when assignment_id is absent (private chat) or after a restart.
        self.store.write_snapshot(
            format!("data/waiting/{}.json", safe_id(question_id)),
            &json!({"kind":"question","run_id":request.run_id,"assignment_id":request.assignment_id,"chat_id":request.chat_id,"question_id":question_id,"text":question}),
        )?;
        self.sink
            .emit(ExecutionEvent {
                event: "message.created".into(),
                data: json!({"message":message}),
                persistent: true,
            })
            .await;
        Ok(())
    }

    async fn create_streaming_message(
        &self,
        request: &ExecutionRequest,
    ) -> Result<String, ExecutionError> {
        let message_id = format!("msg_{}", safe_id(&request.run_id));
        let path = format!("data/chats/{}/messages.jsonl", safe_id(&request.chat_id));
        if self
            .store
            .read_jsonl::<Value>(&path)?
            .iter()
            .any(|message| message["id"] == message_id)
        {
            // Ensure legacy placeholders also reserve a canonical position;
            // existing IDs never consume a second sequence number.
            if let Some(existing) = self
                .store
                .read_jsonl::<Value>(&path)?
                .into_iter()
                .find(|message| message["id"] == message_id)
            {
                let _ = self.canonical_chat_message(&request.chat_id, existing)?;
            }
            return Ok(message_id);
        }
        let message = self.canonical_chat_message(
            &request.chat_id,
            json!({
                "id": message_id,
                "chat_id": request.chat_id,
                "sender": {"kind":"bot","bot_id":request.bot_id},
                "created_at": now_rfc3339(),
                "edited_at": null,
                "deleted": false,
                "reply_to": null,
                "thread_count": 0,
                "mentions": [],
                "blocks": [],
                "fallback_text": "",
                "intent": null,
                "assignment_id": request.assignment_id,
                "streaming": true,
                "delivery": [],
                "reactions": []
            }),
        )?;
        self.store.append_jsonl(&path, &message)?;
        self.sink
            .emit(ExecutionEvent {
                event: "message.created".into(),
                data: json!({"message":message}),
                persistent: true,
            })
            .await;
        Ok(message_id)
    }

    fn replace_message(&self, path: &str, message: &Value) -> Result<(), ExecutionError> {
        // Message history is append-only; consumers fold later updates by id.
        self.store.append_jsonl(path, message)?;
        Ok(())
    }

    async fn trace(
        &self,
        request: &ExecutionRequest,
        kind: &str,
        data: Value,
    ) -> Result<(), ExecutionError> {
        let aseq = self.next_aseq(request).await?;
        let item = json!({"assignment_id":request.assignment_id,"chat_id":request.chat_id,"run_id":request.run_id,"aseq":aseq,"at":now_rfc3339(),"type":kind,"data":data});
        self.store.append_jsonl(
            format!(
                "data/traces/{}.jsonl",
                safe_id(request.assignment_id.as_deref().unwrap_or(&request.chat_id))
            ),
            &item,
        )?;
        // The run thread and its durable checkpoint share an append-only entry
        // stream, so recovery can reconstruct the exact visible conversation.
        self.store.append_jsonl(
            format!("data/runs/{}/entries.jsonl", safe_id(&request.run_id)),
            &item,
        )?;
        self.sink
            .emit(ExecutionEvent {
                event: "trace.item".into(),
                data: json!({"stream":trace_scope(request),"item":item}),
                persistent: true,
            })
            .await;
        Ok(())
    }
}

fn tool_message(call_id: &str, result: &ToolResult) -> Value {
    let text = result
        .content
        .iter()
        .filter_map(|part| match part {
            Part::Text { text } => Some(text.as_str()),
            Part::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    json!({"role":"tool","tool_call_id":call_id,"content":text,"is_error":result.is_error})
}

/// Keep provider-native content (for example signed thinking/tool blocks)
/// alongside the normalized text. Serialization makes this forward compatible
/// with provider crate additions while preserving omission when older providers
/// do not expose the field.
fn assistant_message(completion: &Completion) -> Value {
    let mut message = json!({"role":"assistant","content":completion.text});
    if !completion.tool_calls.is_empty() {
        message["tool_calls"] = json!(completion
            .tool_calls
            .iter()
            .map(|call| json!({"id":call.call_id,"type":"function","function":{"name":call.name,"arguments":call.args.to_string()}}))
            .collect::<Vec<_>>());
    }
    if let Some(native) = completion
        .assistant_content
        .as_ref()
        .filter(|value| !value.is_null())
    {
        message["assistant_content"] = native.clone();
    }
    message
}

/// The gateway currently receives subagent metadata in the execution message
/// envelope. Accept both the compact and nested forms so old callers remain
/// source compatible while subagents can never execute a tool.
fn subagent_requested(request: &ExecutionRequest) -> bool {
    request.subagent
        || request.messages.iter().any(|message| {
            message.get("subagent").and_then(Value::as_bool) == Some(true)
                || message
                    .pointer("/metadata/subagent")
                    .and_then(Value::as_bool)
                    == Some(true)
        })
}

fn execution_phase(request: &ExecutionRequest) -> &str {
    request.phase.as_deref().unwrap_or_else(|| {
        if subagent_requested(request) {
            "subagent"
        } else if request.private {
            "chat"
        } else if request.bot_id == "main" {
            "coordinate"
        } else {
            "work"
        }
    })
}

fn trace_phase(request: &ExecutionRequest) -> &str {
    match execution_phase(request) {
        "chat" => "chat",
        "subagent" => "subagent",
        "memory" => "memory",
        "compact" => "compact",
        _ => "work",
    }
}

fn estimate_context(messages: &[Value]) -> Value {
    let total = messages
        .iter()
        .filter_map(|message| serde_json::to_string(message).ok())
        .map(|message| message.len().div_ceil(4) as u64)
        .sum::<u64>();
    json!({"l0":total,"l1":0,"l2":0,"l3":0,"l4":0,"total":total})
}

fn tool_allowed(request: &ExecutionRequest, name: &str) -> bool {
    if let Some(allowlist) = &request.tools {
        if !allowlist.iter().any(|tool| tool == name) {
            return false;
        }
    }
    if !subagent_requested(request) {
        return true;
    }
    if matches!(
        name,
        "send_msg" | "memory" | "memory_search" | "subagent" | "request_takeover" | "takeover"
    ) {
        return false;
    }
    request
        .tools
        .as_ref()
        .is_some_and(|allowlist| allowlist.iter().any(|tool| tool == name))
        || matches!(
            name,
            "read" | "grep" | "find" | "ls" | "web_fetch" | "skill"
        )
}

fn can_parallelize_tool_call(
    request: &ExecutionRequest,
    call: &ToolCall,
    tools: &HashMap<String, Arc<dyn Tool>>,
) -> bool {
    if !tool_allowed(request, &call.name) {
        return false;
    }
    let Some(tool) = tools.get(&call.name) else {
        return false;
    };
    matches!(tool.risk(&call.args), Risk::Read) || call.name == "subagent"
}

fn preview(result: &ToolResult) -> String {
    tool_message("preview", result)["content"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(8_000)
        .collect()
}

fn tool_end_data(
    request: &ExecutionRequest,
    call: &ToolCall,
    result: &ToolResult,
    duration_ms: u64,
) -> Value {
    let details = result.details.as_object().cloned().unwrap_or_default();
    let truncated = details
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let full_output = details
        .get("full_output_path")
        .and_then(Value::as_str)
        .and_then(|path| {
            canonical_full_output_ref(request, &call.call_id, PathBuf::from(path).as_path())
        });
    json!({
        "call_id":call.call_id,
        "is_error":result.is_error,
        "preview":preview(result),
        "details":details,
        "truncated":truncated,
        "full_output":full_output,
        "duration_ms":duration_ms
    })
}

fn canonical_full_output_ref(
    request: &ExecutionRequest,
    call_id: &str,
    source: &std::path::Path,
) -> Option<Value> {
    let name = source.file_name()?.to_string_lossy().into_owned();
    let size = std::fs::metadata(source).ok()?.len();
    let run_dir = source.parent()?;
    let runs_dir = run_dir.parent()?;
    if runs_dir.file_name()?.to_string_lossy() != "runs" {
        return None;
    }
    let home = runs_dir.parent()?;
    let run_id = safe_id(&request.run_id);
    let call_id = safe_id(call_id);
    if run_id.is_empty() || call_id.is_empty() || !is_safe_component(&run_id) {
        return None;
    }
    let canonical_run = runs_dir.join(&run_id).join(format!("{call_id}.txt"));
    if !materialize_output_link(source, &canonical_run) {
        return None;
    }
    let bot_root = home.join("bots").join(safe_id(&request.bot_id));
    let bot_file = bot_root
        .join("runs")
        .join(&run_id)
        .join(format!("{call_id}.txt"));
    if !materialize_output_link(&canonical_run, &bot_file) {
        return None;
    }
    Some(json!({
        "root":"bot",
        "root_id":request.bot_id,
        "path":format!("runs/{run_id}/{call_id}.txt"),
        "name":name,
        "size":size,
        "mime":"text/plain"
    }))
}

fn materialize_output_link(source: &std::path::Path, destination: &std::path::Path) -> bool {
    if source == destination {
        return source.is_file();
    }
    if std::fs::create_dir_all(destination.parent().unwrap_or(destination)).is_err() {
        return false;
    }
    let _ = std::fs::remove_file(destination);
    std::fs::hard_link(source, destination)
        .or_else(|_| std::fs::copy(source, destination).map(|_| ()))
        .is_ok()
}

fn is_safe_component(value: &str) -> bool {
    let mut components = std::path::Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

fn safe_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
fn trace_scope(request: &ExecutionRequest) -> String {
    safe_id(request.assignment_id.as_deref().unwrap_or(&request.chat_id))
}
fn steer_message(request: &ExecutionRequest, steer: &InboxItem, state: &str) -> Value {
    json!({
        "id": steer.message_id,
        "chat_id": request.chat_id,
        "seq": 0,
        "sender": {"kind":"user"},
        "created_at": now_rfc3339(),
        "edited_at": null,
        "deleted": false,
        "reply_to": null,
        "thread_count": 0,
        "mentions": [],
        "blocks": [{"type":"text","markdown":steer.text}],
        "fallback_text": steer.text,
        "intent": null,
        "assignment_id": request.assignment_id,
        "streaming": false,
        "delivery": [{"bot_id":request.bot_id,"assignment_id":request.assignment_id,"state":state,"at":now_rfc3339()}],
        "reactions": []
    })
}

fn approval_settings_allow(settings: &Value, tool_name: &str, args: &Value) -> bool {
    let Some(approvals) = settings.get("approvals") else {
        return false;
    };
    let Some(rules) = approvals.get("rules").and_then(Value::as_array) else {
        return approvals.get("mode").and_then(Value::as_str) == Some("always_allow");
    };
    let generated_summary = format!("Approval required for {tool_name}");
    let matches_rule = |rule: &Value| {
        if rule.get("enabled").and_then(Value::as_bool) == Some(false) {
            return false;
        }
        if rule.get("tool").and_then(Value::as_str) == Some(tool_name) {
            return true;
        }
        let Some(text) = rule.get("text").and_then(Value::as_str) else {
            return false;
        };
        text == tool_name || text == generated_summary || contains_argument_literal(args, text)
    };
    if rules.iter().any(|rule| {
        rule.get("kind").and_then(Value::as_str) == Some("ask_first") && matches_rule(rule)
    }) {
        return false;
    }
    if approvals.get("mode").and_then(Value::as_str) == Some("always_allow") {
        return true;
    }
    rules.iter().any(|rule| {
        rule.get("kind").and_then(Value::as_str) == Some("auto_allow") && matches_rule(rule)
    })
}

fn builtin_requires_approval(
    tool_name: &str,
    args: &Value,
    cwd: &std::path::Path,
    home: &std::path::Path,
) -> bool {
    if tool_name == "browser_eval"
        || tool_name == "pay"
        || tool_name == "payment"
        || tool_name.contains("payment")
    {
        return true;
    }
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return false;
    };
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    if tokens
        .iter()
        .any(|token| *token == "sudo" || token.ends_with("/sudo"))
        || tokens.windows(2).any(|pair| pair == ["git", "push"])
    {
        return true;
    }
    let Some(rm_index) = tokens
        .iter()
        .position(|token| *token == "rm" || token.ends_with("/rm"))
    else {
        return false;
    };
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
            let target = std::path::Path::new(token);
            let resolved = if target.is_absolute() {
                target.to_path_buf()
            } else {
                cwd.join(target)
            };
            if !lexically_normalize(&resolved).starts_with(lexically_normalize(home)) {
                return true;
            }
        }
    }
    false
}

fn lexically_normalize(path: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn contains_argument_literal(value: &Value, literal: &str) -> bool {
    match value {
        Value::String(value) => value == literal,
        Value::Array(values) => values
            .iter()
            .any(|value| contains_argument_literal(value, literal)),
        Value::Object(values) => values
            .values()
            .any(|value| contains_argument_literal(value, literal)),
        _ => false,
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn canonical_message_id(message: &Value) -> Option<String> {
    [
        message.pointer("/id"),
        message.pointer("/message_id"),
        message.pointer("/message/id"),
        message.pointer("/message/message_id"),
    ]
    .into_iter()
    .filter_map(|value| value.and_then(Value::as_str))
    .find(|id| !id.is_empty())
    .map(str::to_owned)
}

/// Private and coordinator calls use independent persisted rate buckets.
fn model_rate_bucket(request: &ExecutionRequest) -> Option<String> {
    if request.parent_run_id.is_some() || request.phase.as_deref() == Some("subagent") {
        None
    } else if request.phase.as_deref() == Some("coordinate") {
        Some(format!("{}:coordinate", request.bot_id))
    } else if request.private {
        Some(format!("{}:private", request.bot_id))
    } else {
        None
    }
}

/// Main-Bot coordination is governed by the RPC's own policy gates.  Routing
/// these calls through the generic unsafe-tool approval would make ordinary
/// project creation and assignment impossible under the default `require`
/// mode.  File, shell, browser, routine and subagent tools remain subject to
/// the normal approval path.
fn coordination_tool_uses_internal_policy(name: &str) -> bool {
    matches!(
        name,
        "list_bots"
            | "create_project"
            | "project_create"
            | "assign"
            | "delegate"
            | "project_status"
            | "get_status"
            | "request_review"
            | "propose_bot"
            | "notify_user"
            | "remind"
            | "send_msg"
    )
}

fn coordination_tool_requires_approval_from_settings(
    settings: &Value,
    tool_name: &str,
    args: &Value,
) -> bool {
    if auto_create_project_requires_confirmation(settings, tool_name) {
        return true;
    }
    let Some(rules) = settings
        .get("approvals")
        .and_then(|approvals| approvals.get("rules"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let summary = format!("Approval required for {tool_name}");
    rules.iter().any(|rule| {
        rule.get("enabled").and_then(Value::as_bool) != Some(false)
            && rule.get("kind").and_then(Value::as_str) == Some("ask_first")
            && (rule.get("tool").and_then(Value::as_str) == Some(tool_name)
                || rule.get("text").and_then(Value::as_str) == Some(tool_name)
                || rule.get("text").and_then(Value::as_str) == Some(summary.as_str())
                || rule
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| contains_argument_literal(args, text)))
    })
}

fn auto_create_project_requires_confirmation(settings: &Value, tool_name: &str) -> bool {
    matches!(tool_name, "create_project" | "project_create")
        && settings
            .pointer("/main_bot/auto_create_project")
            .and_then(Value::as_bool)
            == Some(false)
}

fn risky_call_allowed_from_settings(
    settings: &Value,
    allow_unsafe: bool,
    tool_name: &str,
    args: &Value,
) -> bool {
    if auto_create_project_requires_confirmation(settings, tool_name) {
        return allow_unsafe;
    }
    allow_unsafe || approval_settings_allow(settings, tool_name, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use macbot_providers::{MockProvider, ToolCall};
    use macbot_tools::{BashTool, ReadTool, WriteTool};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    #[derive(Default)]
    struct RecordingSink {
        events: StdMutex<Vec<ExecutionEvent>>,
        groups: StdMutex<Vec<Value>>,
        approvals: StdMutex<Vec<Value>>,
        return_canonical_id: bool,
        context_error: Option<String>,
    }

    impl RecordingSink {
        fn canonical_ids() -> Self {
            Self {
                return_canonical_id: true,
                ..Default::default()
            }
        }
    }
    #[async_trait]
    impl ExecutionSink for RecordingSink {
        async fn emit(&self, event: ExecutionEvent) {
            self.events.lock().unwrap().push(event);
        }
        async fn send_group_message(&self, message: Value) -> Result<Value, String> {
            self.groups.lock().unwrap().push(message.clone());
            let mut canonical = message.get("message").cloned().unwrap_or_else(|| json!({}));
            let return_canonical_id = self.return_canonical_id;
            if return_canonical_id {
                if let Some(message_id) = canonical_message_id(&canonical) {
                    canonical["id"] = json!(format!("canonical_{message_id}"));
                }
            }
            if canonical.get("id").and_then(Value::as_str).is_none()
                && canonical
                    .get("message_id")
                    .and_then(Value::as_str)
                    .is_none()
            {
                if let Some(message_id) = canonical.get("message_id").cloned() {
                    canonical["id"] = message_id;
                }
            }
            Ok(canonical)
        }
        async fn approval_required(&self, data: Value) {
            self.approvals.lock().unwrap().push(data);
        }
        async fn prepare_model_context(
            &self,
            _request: &ExecutionRequest,
        ) -> Result<Option<Value>, String> {
            match &self.context_error {
                Some(error) => Err(error.clone()),
                None => Ok(None),
            }
        }
    }

    struct FixedResolver(Arc<dyn ModelProvider>);

    struct FailingProvider;
    #[async_trait]
    impl ModelProvider for FailingProvider {
        async fn stream(
            &self,
            _: ModelRequest,
            _: mpsc::Sender<ModelEvent>,
        ) -> macbot_providers::Result<Completion> {
            Err(macbot_providers::Error::Response("fake failure".into()))
        }
    }

    struct FailingResolver;
    #[async_trait]
    impl ProviderResolver for FailingResolver {
        async fn resolve(&self, _: &str, _: &str) -> Result<Arc<dyn ModelProvider>, String> {
            Err("fake missing model".into())
        }
    }

    struct PendingProvider {
        started: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
    }
    struct ProviderDropSignal(Arc<AtomicBool>);
    impl Drop for ProviderDropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl ModelProvider for PendingProvider {
        async fn stream(
            &self,
            _: ModelRequest,
            _: mpsc::Sender<ModelEvent>,
        ) -> macbot_providers::Result<Completion> {
            let _signal = ProviderDropSignal(self.dropped.clone());
            self.started.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn provider_and_resolver_failures_close_private_streams_and_keep_job_identity() {
        for resolver_failure in [false, true] {
            let dir = tempdir().unwrap();
            let sink = Arc::new(RecordingSink::default());
            let mut engine = ExecutionEngine::new(
                Store::open(dir.path()).unwrap(),
                Arc::new(FailingProvider),
                Vec::<Arc<dyn Tool>>::new(),
                sink.clone(),
                dir.path(),
            )
            .unwrap();
            if resolver_failure {
                engine = engine.with_provider_resolver(Arc::new(FailingResolver));
            }
            let req = request(true);
            assert!(engine.run(req.clone()).await.is_err());
            let durable = engine.state.durable.lock().await;
            let job = durable.jobs().next().unwrap();
            assert_eq!(job.status, JobStatus::Failed);
            assert_eq!(job.checkpoint["run_id"], req.run_id);
            drop(durable);
            let messages = engine
                .store
                .read_jsonl::<Value>("data/chats/chat_mock/messages.jsonl")
                .unwrap();
            assert_eq!(messages.last().unwrap()["streaming"], false);
            let _: macbot_protocol::Message =
                serde_json::from_value(messages.last().unwrap().clone()).unwrap();
            assert!(messages.last().unwrap()["fallback_text"]
                .as_str()
                .unwrap()
                .contains("fake"));
            assert_eq!(
                sink.events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(
                        |event| event.data.pointer("/item/type").and_then(Value::as_str)
                            == Some("run.end")
                    )
                    .count(),
                1
            );
            assert_eq!(engine.run(req).await.unwrap().status, "failed");
        }
    }

    #[tokio::test]
    async fn context_compaction_failure_fails_run_before_model_and_next_run_can_continue() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink {
            context_error: Some("context compaction failed: fake provider 500".into()),
            ..Default::default()
        });
        let failed_request = request(true);
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            Arc::new(MockProvider::new(Vec::new())),
            Vec::<Arc<dyn Tool>>::new(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();

        let error = engine.run(failed_request.clone()).await.unwrap_err();
        assert!(error.to_string().contains("context compaction failed"));
        let durable = engine.state.durable.lock().await;
        let job = durable
            .jobs()
            .find(|job| job.checkpoint["run_id"] == failed_request.run_id)
            .unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        drop(durable);
        {
            let events = sink.events.lock().unwrap();
            assert!(events.iter().any(|event| {
                event.data.pointer("/item/type").and_then(Value::as_str) == Some("run.start")
            }));
            assert!(events.iter().any(|event| {
                event.data.pointer("/item/type").and_then(Value::as_str) == Some("run.end")
                    && event
                        .data
                        .pointer("/item/data/status")
                        .and_then(Value::as_str)
                        == Some("failed")
            }));
            assert!(!events.iter().any(|event| {
                event.data.pointer("/item/type").and_then(Value::as_str) == Some("llm.request")
            }));
        }
        let messages = engine
            .store
            .read_jsonl::<Value>("data/chats/chat_mock/messages.jsonl")
            .unwrap();
        assert!(messages
            .last()
            .and_then(|message| message["fallback_text"].as_str())
            .is_some_and(|text| text.contains("context compaction failed")));
        drop(engine);

        let mut recovered_request = request(true);
        recovered_request.run_id = "run_recovered".into();
        let recovered_sink = Arc::new(RecordingSink::default());
        let recovered_engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            Arc::new(MockProvider::new(vec![Completion {
                text: "recovered".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            }])),
            Vec::<Arc<dyn Tool>>::new(),
            recovered_sink,
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            recovered_engine
                .run(recovered_request)
                .await
                .unwrap()
                .status,
            "done"
        );
    }

    #[tokio::test]
    async fn cancellation_drops_pending_model_request_and_closes_private_stream_once() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(
            ExecutionEngine::new(
                Store::open(dir.path()).unwrap(),
                Arc::new(PendingProvider {
                    started: started.clone(),
                    dropped: dropped.clone(),
                }),
                Vec::<Arc<dyn Tool>>::new(),
                sink.clone(),
                dir.path(),
            )
            .unwrap(),
        );
        let req = request(true);
        let worker = {
            let engine = engine.clone();
            let req = req.clone();
            tokio::spawn(async move { engine.run(req).await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        assert_eq!(
            engine.cancel(&req).await.unwrap().unwrap().status,
            "cancelled"
        );
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, "cancelled");
        assert!(dropped.load(Ordering::SeqCst));
        engine.cancel(&req).await.unwrap();
        let messages = engine
            .store
            .read_jsonl::<Value>("data/chats/chat_mock/messages.jsonl")
            .unwrap();
        assert_eq!(messages.last().unwrap()["streaming"], false);
        assert_eq!(
            sink.events
                .lock()
                .unwrap()
                .iter()
                .filter(
                    |event| event.data.pointer("/item/type").and_then(Value::as_str)
                        == Some("run.end")
                )
                .count(),
            1
        );
    }
    #[async_trait]
    impl ProviderResolver for FixedResolver {
        async fn resolve(
            &self,
            _provider_id: &str,
            _model: &str,
        ) -> Result<Arc<dyn ModelProvider>, String> {
            Ok(self.0.clone())
        }
    }

    struct NeverCalledTool {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for NeverCalledTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "Request user browser takeover."
        }
        fn schema(&self) -> Value {
            json!({"type":"object","required":["reason"],"properties":{"reason":{"type":"string"}}})
        }
        fn risk(&self, _: &Value) -> Risk {
            Risk::External
        }
        async fn call(&self, _: &ToolContext, _: Value) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::error("request_takeover must wait for the user")
        }
    }

    struct SlowReadTool {
        name: &'static str,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for SlowReadTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "Concurrent read test tool."
        }
        fn schema(&self) -> Value {
            json!({"type":"object","properties":{}})
        }
        fn risk(&self, _: &Value) -> Risk {
            Risk::Read
        }
        async fn call(&self, _: &ToolContext, _: Value) -> ToolResult {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            ToolResult::text(self.name)
        }
    }

    struct BlockingWriteTool {
        calls: Arc<AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Tool for BlockingWriteTool {
        fn name(&self) -> &str {
            "blocking_write"
        }
        fn description(&self) -> &str {
            "Write test tool that waits for cancellation."
        }
        fn schema(&self) -> Value {
            json!({"type":"object","properties":{}})
        }
        fn risk(&self, _: &Value) -> Risk {
            Risk::Write
        }
        async fn call(&self, context: &ToolContext, _: Value) -> ToolResult {
            let call_number = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call_number == 1 {
                self.started.notify_one();
                while !context
                    .cancellation()
                    .is_some_and(|cancellation| cancellation.is_cancelled())
                {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                ToolResult::error("cancelled")
            } else {
                ToolResult::text("second call must not run")
            }
        }
    }

    struct FullOutputTool {
        path: PathBuf,
    }

    #[async_trait]
    impl Tool for FullOutputTool {
        fn name(&self) -> &str {
            "full_output"
        }
        fn description(&self) -> &str {
            "Returns a truncated result with an archived output file."
        }
        fn schema(&self) -> Value {
            json!({"type":"object","properties":{}})
        }
        fn risk(&self, _: &Value) -> Risk {
            Risk::Read
        }
        async fn call(&self, _: &ToolContext, _: Value) -> ToolResult {
            ToolResult {
                content: vec![Part::Text {
                    text: "visible preview".into(),
                }],
                details: json!({
                    "truncated":true,
                    "full_output_path":self.path.display().to_string()
                }),
                is_error: false,
            }
        }
    }

    fn request(private: bool) -> ExecutionRequest {
        ExecutionRequest {
            run_id: "run_mock".into(),
            assignment_id: None,
            chat_id: "chat_mock".into(),
            bot_id: "bot_mock".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "完成登录功能".into(),
            messages: Vec::new(),
            max_turns: 4,
            private,
            allow_unsafe: true,
            cwd: None,
            routine: false,
            price: None,
            resume_approved: false,
            subagent: false,
            tools: None,
            phase: None,
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
        }
    }

    #[test]
    fn execution_phase_follows_runtime_semantics() {
        let mut chat = request(true);
        assert_eq!(execution_phase(&chat), "chat");
        chat.private = false;
        chat.bot_id = "main".into();
        assert_eq!(execution_phase(&chat), "coordinate");
        chat.bot_id = "worker".into();
        assert_eq!(execution_phase(&chat), "work");
        chat.subagent = true;
        assert_eq!(execution_phase(&chat), "subagent");
        chat.phase = Some("custom".into());
        assert_eq!(execution_phase(&chat), "custom");
    }

    #[test]
    fn approval_settings_match_global_mode_tools_and_literal_commands() {
        let always = json!({"approvals":{"mode":"always_allow","rules":[]}});
        assert!(approval_settings_allow(
            &always,
            "bash",
            &json!({"command":"sudo id"})
        ));

        let require = json!({"approvals":{"mode":"require","rules":[
            {"kind":"auto_allow","text":"Approval required for write"},
            {"kind":"auto_allow","text":"git push origin main"}
        ]}});
        assert!(approval_settings_allow(
            &require,
            "write",
            &json!({"path":"x"})
        ));
        assert!(approval_settings_allow(
            &require,
            "bash",
            &json!({"command":"git push origin main"})
        ));
        assert!(!approval_settings_allow(
            &require,
            "bash",
            &json!({"command":"git status"})
        ));
        let conflict = json!({"approvals":{"mode":"always_allow","rules":[
            {"kind":"auto_allow","text":"git push origin main"},
            {"kind":"ask_first","text":"git push origin main"}
        ]}});
        assert!(!approval_settings_allow(
            &conflict,
            "bash",
            &json!({"command":"git push origin main"})
        ));
        let home = std::path::Path::new("/Users/test/MacBot");
        let cwd = home.join("runs");
        assert!(builtin_requires_approval(
            "browser_eval",
            &json!({}),
            &cwd,
            home
        ));
        assert!(builtin_requires_approval(
            "bash",
            &json!({"command":"sudo id"}),
            &cwd,
            home
        ));
        assert!(builtin_requires_approval(
            "bash",
            &json!({"command":"git push origin main"}),
            &cwd,
            home
        ));
        assert!(builtin_requires_approval(
            "bash",
            &json!({"command":"rm -rf /tmp/build"}),
            &cwd,
            home
        ));
        assert!(!builtin_requires_approval(
            "bash",
            &json!({"command":"rm -rf runs/cache"}),
            &cwd,
            home
        ));
        assert!(builtin_requires_approval(
            "bash",
            &json!({"command":"rm -rf /Users/test/MacBot/../outside"}),
            &cwd,
            home
        ));
    }

    #[test]
    fn model_call_limits_only_private_and_coordinator_buckets() {
        let mut work = request(false);
        assert_eq!(model_rate_bucket(&work), None);
        work.phase = Some("coordinate".into());
        assert_eq!(
            model_rate_bucket(&work),
            Some(format!("{}:coordinate", work.bot_id))
        );
        let mut private = request(true);
        assert_eq!(
            model_rate_bucket(&private),
            Some(format!("{}:private", private.bot_id))
        );
        private.parent_run_id = Some("parent".into());
        assert_eq!(model_rate_bucket(&private), None);
    }

    #[test]
    fn coordination_tools_use_internal_policy_under_require_mode() {
        for name in [
            "list_bots",
            "create_project",
            "project_create",
            "assign",
            "delegate",
            "project_status",
            "get_status",
            "request_review",
            "propose_bot",
            "notify_user",
            "remind",
            "send_msg",
        ] {
            assert!(coordination_tool_uses_internal_policy(name), "{name}");
        }
        for name in [
            "write",
            "bash",
            "browser_open",
            "routine",
            "subagent",
            "finish_project",
        ] {
            assert!(!coordination_tool_uses_internal_policy(name), "{name}");
        }
    }

    #[test]
    fn project_creation_and_explicit_ask_first_are_internal_gates() {
        let args = json!({"name":"new project"});
        assert!(!coordination_tool_requires_approval_from_settings(
            &json!({"main_bot":{"auto_create_project":true},"approvals":{"mode":"require","rules":[]}}),
            "create_project",
            &args,
        ));
        assert!(coordination_tool_requires_approval_from_settings(
            &json!({"main_bot":{"auto_create_project":false},"approvals":{"mode":"require","rules":[]}}),
            "create_project",
            &args,
        ));
        assert!(coordination_tool_requires_approval_from_settings(
            &json!({"main_bot":{"auto_create_project":true},"approvals":{"mode":"require","rules":[{"kind":"ask_first","tool":"assign"}]}}),
            "assign",
            &json!({"bot_id":"worker"}),
        ));
        assert!(!coordination_tool_requires_approval_from_settings(
            &json!({"main_bot":{"auto_create_project":true},"approvals":{"mode":"require","rules":[]}}),
            "assign",
            &json!({"bot_id":"worker"}),
        ));
        assert!(!coordination_tool_requires_approval_from_settings(
            &json!({"approvals":{"mode":"require","rules":[{"kind":"ask_first","tool":"assign","enabled":false}]}}),
            "assign",
            &json!({"bot_id":"worker"}),
        ));
        let always_allow = json!({
            "main_bot":{"auto_create_project":false},
            "approvals":{"mode":"always_allow","rules":[]}
        });
        assert!(!risky_call_allowed_from_settings(
            &always_allow,
            false,
            "create_project",
            &args,
        ));
        assert!(risky_call_allowed_from_settings(
            &always_allow,
            true,
            "create_project",
            &args,
        ));
    }

    #[tokio::test]
    async fn safe_tools_run_concurrently_in_group_chats() {
        let dir = tempdir().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![
                    ToolCall {
                        call_id: "read_a".into(),
                        name: "read_a".into(),
                        args: json!({}),
                    },
                    ToolCall {
                        call_id: "read_b".into(),
                        name: "read_b".into(),
                        args: json!({}),
                    },
                ],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![
                Arc::new(SlowReadTool {
                    name: "read_a",
                    active: active.clone(),
                    max_active: max_active.clone(),
                }) as Arc<dyn Tool>,
                Arc::new(SlowReadTool {
                    name: "read_b",
                    active,
                    max_active: max_active.clone(),
                }) as Arc<dyn Tool>,
            ],
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        let mut req = request(false);
        req.assignment_id = Some("assignment_parallel".into());
        assert_eq!(engine.run(req).await.unwrap().status, "done");
        assert_eq!(max_active.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn error_tool_traces_are_typed() {
        let dir = tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "unknown_1".into(),
                    name: "missing".into(),
                    args: json!({}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        engine.run(request(true)).await.unwrap();
        let trace =
            std::fs::read_to_string(dir.path().join("data/traces/chat_mock.jsonl")).unwrap();
        let items = trace
            .lines()
            .map(serde_json::from_str::<macbot_protocol::TraceItem>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(items.iter().any(|item| {
            matches!(
                item.data,
                macbot_protocol::TraceData::ToolEnd { is_error: true, .. }
            )
        }));
    }

    #[tokio::test]
    async fn truncated_tool_trace_contains_typed_full_output_ref() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("runs/run_mock/output.log");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, "complete output").unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "full_output_1".into(),
                    name: "full_output".into(),
                    args: json!({}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(FullOutputTool {
                path: output.clone(),
            }) as Arc<dyn Tool>],
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        engine.run(request(true)).await.unwrap();
        let trace =
            std::fs::read_to_string(dir.path().join("data/traces/chat_mock.jsonl")).unwrap();
        let item = trace
            .lines()
            .map(serde_json::from_str::<macbot_protocol::TraceItem>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .find(|item| matches!(item.data, macbot_protocol::TraceData::ToolEnd { .. }))
            .unwrap();
        let macbot_protocol::TraceData::ToolEnd {
            truncated,
            full_output,
            ..
        } = item.data
        else {
            panic!("expected tool.end trace");
        };
        assert!(truncated);
        let full_output = full_output.expect("truncated output must have a file reference");
        assert_eq!(full_output.name, "output.log");
        assert_eq!(full_output.mime, "text/plain");
        assert_eq!(full_output.size, "complete output".len() as u64);
        assert!(matches!(full_output.root, macbot_protocol::FileRoot::Bot));
        assert_eq!(full_output.root_id, "bot_mock");
        let canonical_path = dir.path().join("runs/run_mock/full_output_1.txt");
        let bot_path = dir
            .path()
            .join("bots/bot_mock/runs/run_mock/full_output_1.txt");
        assert_eq!(full_output.path, "runs/run_mock/full_output_1.txt");
        assert_eq!(std::fs::read(canonical_path).unwrap(), b"complete output");
        assert_eq!(std::fs::read(bot_path).unwrap(), b"complete output");
    }

    #[tokio::test]
    async fn tool_output_is_forwarded_as_temporary_trace_event() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "bash_stream".into(),
                    name: "bash".into(),
                    args: json!({"command":"printf realtime"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(BashTool::default()) as Arc<dyn Tool>],
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        engine.run(request(true)).await.unwrap();
        let events = sink.events.lock().unwrap();
        assert!(events.iter().any(|event| {
            event.event == "trace.tool_output"
                && event.data["call_id"] == "bash_stream"
                && event.data["chunk"]
                    .as_str()
                    .is_some_and(|chunk| chunk.contains("realtime"))
                && !event.persistent
        }));
    }

    #[tokio::test]
    async fn cancellation_during_serial_tool_calls_stays_terminal_and_stops_queue() {
        let dir = tempdir().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(MockProvider::new(vec![Completion {
            tool_calls: vec![
                ToolCall {
                    call_id: "blocking_1".into(),
                    name: "blocking_write".into(),
                    args: json!({}),
                },
                ToolCall {
                    call_id: "blocking_2".into(),
                    name: "blocking_write".into(),
                    args: json!({}),
                },
            ],
            stop_reason: "tool_calls".into(),
            ..Default::default()
        }]));
        let engine = Arc::new(
            ExecutionEngine::new(
                Store::open(dir.path()).unwrap(),
                provider,
                vec![Arc::new(BlockingWriteTool {
                    calls: calls.clone(),
                    started: started.clone(),
                }) as Arc<dyn Tool>],
                Arc::new(RecordingSink::default()),
                dir.path(),
            )
            .unwrap(),
        );
        let request = request(true);
        let running = {
            let engine = engine.clone();
            let request = request.clone();
            tokio::spawn(async move { engine.run(request).await.unwrap() })
        };
        started.notified().await;
        assert_eq!(
            engine.cancel(&request).await.unwrap().unwrap().status,
            "cancelled"
        );
        assert_eq!(running.await.unwrap().status, "cancelled");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let durable = engine.state.durable.lock().await;
        let job = durable
            .jobs()
            .find(|job| job.checkpoint["run_id"] == request.run_id)
            .unwrap();
        assert_eq!(job.status, JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn send_msg_done_ends_run_before_later_model_tools() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::canonical_ids());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            tool_calls: vec![
                ToolCall {
                    call_id: "done_1".into(),
                    name: "send_msg".into(),
                    args: json!({"intent":"done","text":"完成"}),
                },
                ToolCall {
                    call_id: "must_not_run".into(),
                    name: "missing".into(),
                    args: json!({}),
                },
            ],
            stop_reason: "tool_calls".into(),
            ..Default::default()
        }]));
        let mut req = request(false);
        req.assignment_id = Some("assignment_done".into());
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let outcome = engine.run(req).await.unwrap();
        assert_eq!(outcome.status, "done");
        assert_eq!(outcome.text, "完成");
        assert_eq!(sink.groups.lock().unwrap().len(), 1);
        assert_eq!(sink.groups.lock().unwrap()[0]["message"]["intent"], "done");
        let trace =
            std::fs::read_to_string(dir.path().join("data/traces/assignment_done.jsonl")).unwrap();
        assert!(trace.lines().any(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["type"] == "send_msg" && value["data"]["message_id"] == "canonical_msg_done_1"
        }));
        assert!(!trace.lines().any(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["type"] == "tool.start" && value["data"]["call_id"] == "must_not_run"
        }));
    }

    #[tokio::test]
    async fn send_msg_decision_waits_and_resumes_without_replaying_receipt() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::canonical_ids());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "decision_1".into(),
                    name: "send_msg".into(),
                    args: json!({"intent":"decision","text":"请选择","options":["A","B"]}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "done_2".into(),
                    name: "send_msg".into(),
                    args: json!({"intent":"done","text":"已完成"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
        ]));
        let mut req = request(false);
        req.assignment_id = Some("assignment_decision".into());
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        assert_eq!(engine.run(req.clone()).await.unwrap().status, "suspended");
        let jobs = std::fs::read_dir(dir.path().join("data/jobs"))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
            .collect::<Vec<_>>();
        assert!(jobs.iter().any(|job| {
            job["checkpoint"]["waiting_reason"] == "decision"
                && job["checkpoint"]["waiting_message"] == true
                && job["checkpoint"]["waiting_message_id"] == "canonical_msg_decision_1"
        }));
        assert_eq!(
            engine
                .continue_waiting_message(req, "选择 A")
                .await
                .unwrap()
                .status,
            "done"
        );
        let groups = sink.groups.lock().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups
                .iter()
                .filter(|message| message["receipt"]["call_id"] == "decision_1")
                .count(),
            1
        );
        let trace =
            std::fs::read_to_string(dir.path().join("data/traces/assignment_decision.jsonl"))
                .unwrap();
        assert_eq!(trace.matches("\"type\":\"send_msg\"").count(), 2);
        assert!(trace.lines().any(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["type"] == "send_msg" && value["data"]["message_id"] == "canonical_msg_decision_1"
        }));
        assert!(trace.lines().any(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["type"] == "send_msg" && value["data"]["message_id"] == "canonical_msg_done_2"
        }));
    }

    #[tokio::test]
    async fn request_takeover_waits_for_user_and_never_grants_itself() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "takeover_1".into(),
                    name: "request_takeover".into(),
                    args: json!({"reason":"需要完成两步验证"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "已继续".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(NeverCalledTool {
                name: "request_takeover",
                calls: calls.clone(),
            }) as Arc<dyn Tool>],
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let req = request(true);
        assert_eq!(engine.run(req.clone()).await.unwrap().status, "suspended");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(sink.events.lock().unwrap().iter().any(|event| {
            event.event == "message.created"
                && event.data["message"]["blocks"][0]["type"] == "takeover_request"
        }));
        assert_eq!(engine.continue_takeover(req).await.unwrap().text, "已继续");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn ask_user_waits_and_resumes_with_answer() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "question_1".into(),
                    name: "ask_user".into(),
                    args: json!({"question":"选择哪个环境？"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "已选择生产环境".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(NeverCalledTool {
                name: "ask_user",
                calls: calls.clone(),
            }) as Arc<dyn Tool>],
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let req = request(true);
        assert_eq!(engine.run(req.clone()).await.unwrap().status, "suspended");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(sink.events.lock().unwrap().iter().any(|event| {
            event.event == "message.created"
                && event.data["message"]["blocks"][0]["type"] == "question"
        }));
        assert_eq!(
            engine
                .continue_question(req, "生产环境")
                .await
                .unwrap()
                .text,
            "已选择生产环境"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn usage_marks_only_terminal_assignment_request_as_done() {
        let dir = tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "missing_tool".into(),
                    name: "missing".into(),
                    args: json!({}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "finished".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        let mut req = request(true);
        req.assignment_id = Some("assignment_usage".into());
        engine.run(req).await.unwrap();

        let mut records = Vec::new();
        for entry in std::fs::read_dir(dir.path().join("data/usage/raw")).unwrap() {
            let contents = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            records.extend(
                contents
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap()),
            );
        }
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["task_done"], false);
        assert_eq!(records[1]["task_done"], true);
    }

    #[tokio::test]
    async fn mock_provider_private_run_streams_and_persists_trace_and_usage() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "已完成".into(),
            assistant_content: Some(json!({
                "api_kind":"anthropic-messages",
                "content":[{"type":"thinking","thinking":"signed"}]
            })),
            stop_reason: "stop".into(),
            usage: TokenUsage {
                input_tokens: 3,
                output_tokens: 2,
                ..Default::default()
            },
            ..Default::default()
        }]));
        let store = Store::open(dir.path()).unwrap();
        store
            .write_snapshot(
                "data/settings.json",
                &json!({"trace":{"save_full_requests":true}}),
            )
            .unwrap();
        let engine = ExecutionEngine::new(
            store,
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let mut execution_request = request(true);
        execution_request.assignment_id = Some("assignment_usage_tick".into());
        let outcome = engine.run(execution_request).await.unwrap();
        assert_eq!(outcome.status, "done");
        assert_eq!(outcome.text, "已完成");
        assert_eq!(outcome.usage.output_tokens, 2);
        let events = sink.events.lock().unwrap();
        assert!(events.iter().any(|event| event.event == "message.delta"));
        assert!(events.iter().any(|event| event.event == "message.updated"));
        assert!(!events.iter().any(|event| event.event == "usage.updated"));
        assert!(events.iter().any(|event| event.event == "message.created"));
        assert!(events.iter().any(|event| event.event == "usage.tick"));
        let updated = events
            .iter()
            .find(|event| event.event == "message.updated")
            .unwrap();
        assert_eq!(updated.data["message"]["streaming"], false);
        assert!(updated.data["message"].get("assistant_content").is_none());
        let checkpoints = std::fs::read_dir(dir.path().join("data/jobs"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
            .collect::<Vec<_>>();
        assert!(checkpoints.iter().any(|job| job["checkpoint"]["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| {
                message["assistant_content"]["api_kind"] == "anthropic-messages"
            }))));
        let usage_records = std::fs::read_dir(dir.path().join("data/usage/raw"))
            .unwrap()
            .flat_map(|entry| {
                std::fs::read_to_string(entry.unwrap().path())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(usage_records.len(), 1);
        assert_eq!(usage_records[0]["model_id"], "model");
        let request_path = dir
            .path()
            .join("data/runs/run_mock/requests/run_mock_llm_0.json");
        assert!(request_path.exists());
        for (seq, event) in events.iter().enumerate() {
            let frame = json!({"v":1,"kind":"evt","seq":seq as u64 + 1,"event":event.event,"data":event.data});
            let _: macbot_protocol::EventFrame = serde_json::from_value(frame).unwrap();
        }
        assert!(
            !std::fs::read_to_string(dir.path().join("data/runs/run_mock/entries.jsonl"))
                .unwrap()
                .is_empty()
        );
        let trace_path = dir.path().join("data/traces/assignment_usage_tick.jsonl");
        let trace_lines = std::fs::read_to_string(trace_path).unwrap();
        let run_start = trace_lines
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|item| item["type"] == "run.start")
            .unwrap();
        assert_eq!(run_start["data"]["model"], "mock/model");
        let llm_request = trace_lines
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|item| item["type"] == "llm.request")
            .unwrap();
        assert_eq!(
            llm_request["data"]["prompt_ref"],
            "data/runs/run_mock/requests/run_mock_llm_0.json"
        );
        for line in trace_lines.lines() {
            let _: macbot_protocol::TraceItem = serde_json::from_str(line).unwrap();
        }
    }

    #[tokio::test]
    async fn private_stream_messages_use_shared_durable_chat_sequences() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let user_one = store
            .sequence_chat_messages("chat_sequence", &[json!({"id":"user_one"})])
            .unwrap();
        assert_eq!(user_one[0]["seq"], 1);
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                text: "first".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
            Completion {
                text: "second".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            store.clone(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        let mut first = request(true);
        first.chat_id = "chat_sequence".into();
        first.run_id = "sequence_first".into();
        assert_eq!(engine.run(first).await.unwrap().status, "done");
        assert_eq!(store.last_chat_sequence("chat_sequence").unwrap(), 2);

        let user_two = store
            .sequence_chat_messages("chat_sequence", &[json!({"id":"user_two"})])
            .unwrap();
        assert_eq!(user_two[0]["seq"], 3);
        let mut second = request(true);
        second.chat_id = "chat_sequence".into();
        second.run_id = "sequence_second".into();
        assert_eq!(engine.run(second).await.unwrap().status, "done");
        assert_eq!(store.last_chat_sequence("chat_sequence").unwrap(), 4);

        let rows = store
            .read_jsonl::<Value>("data/chats/chat_sequence/messages.jsonl")
            .unwrap();
        let first_rows = rows
            .iter()
            .filter(|message| message["id"] == "msg_sequence_first")
            .map(|message| message["seq"].as_u64().unwrap())
            .collect::<Vec<_>>();
        let second_rows = rows
            .iter()
            .filter(|message| message["id"] == "msg_sequence_second")
            .map(|message| message["seq"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(first_rows, vec![2, 2]);
        assert_eq!(second_rows, vec![4, 4]);
    }

    #[tokio::test]
    async fn group_send_is_once_even_when_run_replayed() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::canonical_ids());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "send_1".into(),
                    name: "send_msg".into(),
                    args: json!({"intent":"done","text":"群里汇报","chat_id":"chat_mock","message_id":"msg_seed","to":{"bot":"main"},"mentions":["main"],"artifacts":[{"title":"report","path_or_url":"out/report.md"}],"options":["继续"]}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "群里汇报".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        {
            let store = Store::open(dir.path()).unwrap();
            let mut durable = macbot_durable::DurableRuntime::from_store(store).unwrap();
            durable
                .send_msg_once(
                    "run_mock",
                    "send_1",
                    "done",
                    "msg_seed",
                    json!({"intent":"done","text":"群里汇报","chat_id":"chat_mock","message_id":"msg_seed","to":{"bot":"main"},"mentions":["main"],"artifacts":[{"title":"report","path_or_url":"out/report.md"}],"options":["继续"],"bot_id":"bot_mock","assignment_id":null}),
                )
                .unwrap();
        }
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let mut req = request(false);
        engine.run(req.clone()).await.unwrap();
        req.run_id = "run_mock".into();
        engine.run(req).await.unwrap();
        assert_eq!(sink.groups.lock().unwrap().len(), 1);
        let admitted = sink.groups.lock().unwrap()[0].clone();
        assert_eq!(admitted["message"]["mentions"][0], "main");
        assert_eq!(admitted["message"]["artifacts"][0]["title"], "report");
        let trace =
            std::fs::read_to_string(dir.path().join("data/traces/chat_mock.jsonl")).unwrap();
        assert!(trace.lines().any(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["type"] == "send_msg" && value["data"]["message_id"] == "canonical_msg_seed"
        }));
        let submissions = engine
            .store
            .read_jsonl::<Value>("data/submissions.jsonl")
            .unwrap();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0]["receipt"]["message_id"], "msg_seed");
        let usage_records = std::fs::read_dir(dir.path().join("data/usage/raw"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .map(|contents| contents.lines().count())
            .sum::<usize>();
        assert_eq!(
            usage_records, 1,
            "done send_msg ends the run before another model turn"
        );
    }

    #[tokio::test]
    async fn trace_cursor_is_monotonic_across_restart() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "first".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        engine.run(request(true)).await.unwrap();
        drop(engine);

        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "second".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }]));
        let mut second = request(true);
        second.run_id = "run_second".into();
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink,
            dir.path(),
        )
        .unwrap();
        engine.run(second).await.unwrap();
        let aseqs = std::fs::read_to_string(dir.path().join("data/traces/chat_mock.jsonl"))
            .unwrap()
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["aseq"]
                    .as_u64()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(aseqs.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[tokio::test]
    async fn shared_execution_state_serializes_sequences_across_engines() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let state = ExecutionState::from_store(store.clone()).unwrap();
        let usage = Arc::new(Mutex::new(
            macbot_usage::UsageLedger::from_store(store.clone()).unwrap(),
        ));
        let first = ExecutionEngine::new_with_usage_and_state(
            store.clone(),
            Arc::new(MockProvider::new(vec![Completion {
                text: "one".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            }])),
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
            usage.clone(),
            state.clone(),
        )
        .unwrap();
        let second = ExecutionEngine::new_with_usage_and_state(
            store.clone(),
            Arc::new(MockProvider::new(vec![Completion {
                text: "two".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            }])),
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
            usage,
            state,
        )
        .unwrap();
        let mut first_request = request(true);
        first_request.run_id = "run_one".into();
        first_request.chat_id = "shared_chat".into();
        let mut second_request = request(true);
        second_request.run_id = "run_two".into();
        second_request.chat_id = "shared_chat".into();
        let (first_result, second_result) =
            tokio::join!(first.run(first_request), second.run(second_request));
        assert_eq!(first_result.unwrap().status, "done");
        assert_eq!(second_result.unwrap().status, "done");
        let messages = store
            .read_jsonl::<Value>("data/chats/shared_chat/messages.jsonl")
            .unwrap();
        let seqs = messages
            .iter()
            .filter(|message| message["streaming"] == true)
            .filter_map(|message| message["seq"].as_u64())
            .collect::<Vec<_>>();
        assert_eq!(seqs.len(), 2);
        let mut unique_seqs = seqs.clone();
        unique_seqs.sort_unstable();
        unique_seqs.dedup();
        assert_eq!(unique_seqs.len(), seqs.len());
        let aseqs = store
            .read_jsonl::<Value>("data/traces/shared_chat.jsonl")
            .unwrap()
            .iter()
            .filter_map(|item| item["aseq"].as_u64())
            .collect::<Vec<_>>();
        assert!(!aseqs.is_empty());
        let mut unique_aseqs = aseqs.clone();
        unique_aseqs.sort_unstable();
        unique_aseqs.dedup();
        assert_eq!(unique_aseqs.len(), aseqs.len());
    }

    #[tokio::test]
    async fn shared_execution_state_recovers_unsafe_jobs_once() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let state = ExecutionState::from_store(store.clone()).unwrap();
        {
            let mut durable = state.durable.lock().await;
            let job = durable
                .create_job(
                    "bot_mock",
                    "model_run",
                    json!({"run_id":"recover_once","round":0,"messages":[]}),
                )
                .unwrap();
            durable
                .commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":"recover_once","round":0,"messages":[]}),
                    true,
                )
                .unwrap();
        }
        let usage = Arc::new(Mutex::new(
            macbot_usage::UsageLedger::from_store(store.clone()).unwrap(),
        ));
        let first = ExecutionEngine::new_with_usage_and_state(
            store.clone(),
            Arc::new(MockProvider::new(Vec::new())),
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
            usage.clone(),
            state.clone(),
        )
        .unwrap();
        let second = ExecutionEngine::new_with_usage_and_state(
            store,
            Arc::new(MockProvider::new(Vec::new())),
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
            usage,
            state,
        )
        .unwrap();
        assert_eq!(first.recover().await.unwrap().len(), 1);
        assert!(second.recover().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn execution_recover_returns_safe_jobs_and_suspends_unsafe_jobs() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let state = ExecutionState::from_store(store.clone()).unwrap();
        let (safe_queued, safe_running, unsafe_running) = {
            let mut durable = state.durable.lock().await;
            let safe_queued = durable
                .create_job("bot_mock", "dm", json!({"run_id": "safe_queued"}))
                .unwrap();
            let safe_running = durable
                .create_job("bot_mock", "dm", json!({"run_id": "safe_running"}))
                .unwrap();
            durable
                .commit(
                    &safe_running.id,
                    JobStatus::Running,
                    safe_running.checkpoint.clone(),
                    false,
                )
                .unwrap();
            let unsafe_running = durable
                .create_job("bot_mock", "bash", json!({"run_id": "unsafe_running"}))
                .unwrap();
            durable
                .commit(
                    &unsafe_running.id,
                    JobStatus::Running,
                    unsafe_running.checkpoint.clone(),
                    true,
                )
                .unwrap();
            (safe_queued, safe_running, unsafe_running)
        };
        let usage = Arc::new(Mutex::new(
            macbot_usage::UsageLedger::from_store(store.clone()).unwrap(),
        ));
        let engine = ExecutionEngine::new_with_usage_and_state(
            store,
            Arc::new(MockProvider::new(Vec::new())),
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
            usage,
            state,
        )
        .unwrap();

        let recovered = engine.recover().await.unwrap();
        assert_eq!(recovered.len(), 3);
        assert_eq!(
            recovered
                .iter()
                .find(|job| job.id == safe_queued.id)
                .unwrap()
                .status,
            JobStatus::Queued
        );
        assert_eq!(
            recovered
                .iter()
                .find(|job| job.id == safe_running.id)
                .unwrap()
                .status,
            JobStatus::Running
        );
        assert_eq!(
            recovered
                .iter()
                .find(|job| job.id == unsafe_running.id)
                .unwrap()
                .status,
            JobStatus::Suspended
        );
    }

    #[tokio::test]
    async fn provider_is_resolved_per_run_from_live_registry() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let base = Arc::new(MockProvider::new(vec![Completion {
            text: "base".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }])) as Arc<dyn ModelProvider>;
        let selected = Arc::new(MockProvider::new(vec![Completion {
            text: "selected".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }])) as Arc<dyn ModelProvider>;
        let resolver = Arc::new(FixedResolver(selected));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            base,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink,
            dir.path(),
        )
        .unwrap()
        .with_provider_resolver(resolver);
        let outcome = engine.run(request(true)).await.unwrap();
        assert_eq!(outcome.text, "selected");
    }

    #[tokio::test]
    async fn rebuilt_engine_continues_running_checkpoint_without_new_job() {
        let dir = tempdir().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let mut durable = macbot_durable::DurableRuntime::from_store(store).unwrap();
            let job = durable
                .create_job(
                    "bot_mock",
                    "model_run",
                    json!({"run_id":"run_mock","round":0,"messages":[{"role":"user","content":"continue"}]}),
                )
                .unwrap();
            durable
                .commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":"run_mock","round":0,"messages":[{"role":"user","content":"continue"}]}),
                    false,
                )
                .unwrap();
        }
        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "continued".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        let outcome = engine.run(request(true)).await.unwrap();
        assert_eq!(outcome.text, "continued");
        assert_eq!(
            std::fs::read_dir(dir.path().join("data/jobs"))
                .unwrap()
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn steer_is_delivered_and_read_at_model_boundary() {
        let dir = tempdir().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let mut durable = macbot_durable::DurableRuntime::from_store(store).unwrap();
            let job = durable
                .create_job(
                    "bot_mock",
                    "model_run",
                    json!({"run_id":"run_mock","round":0,"messages":[{"role":"user","content":"continue"}]}),
                )
                .unwrap();
            durable
                .commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":"run_mock","round":0,"messages":[{"role":"user","content":"continue"}]}),
                    false,
                )
                .unwrap();
            durable
                .enqueue_steer(&job.id, "msg_steer", "use the new plan")
                .unwrap();
        }
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "done".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            std::iter::empty::<Arc<dyn Tool>>(),
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        engine.run(request(true)).await.unwrap();
        let states = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.event == "message.updated")
            .filter_map(|event| event.data["message"]["delivery"][0]["state"].as_str())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(states.iter().any(|state| state == "delivered"));
        assert!(states.iter().any(|state| state == "read"));
    }

    #[tokio::test]
    async fn cancel_is_durable_and_terminal() {
        let dir = tempdir().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let mut durable = macbot_durable::DurableRuntime::from_store(store).unwrap();
            durable
                .create_job(
                    "bot_mock",
                    "model_run",
                    json!({"run_id":"run_mock","round":0,"messages":[]}),
                )
                .unwrap();
        }
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            Arc::new(MockProvider::new(Vec::new())),
            std::iter::empty::<Arc<dyn Tool>>(),
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        let req = request(true);
        let cancelled = engine.cancel(&req).await.unwrap().unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(engine.run(req).await.unwrap().status, "cancelled");
    }

    #[tokio::test]
    async fn subagent_cannot_execute_provider_tool_calls() {
        let dir = tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "subagent_write".into(),
                    name: "write".into(),
                    args: json!({"path":"subagent.txt","content":"must not write"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
            Arc::new(RecordingSink::default()),
            dir.path(),
        )
        .unwrap();
        let mut req = request(true);
        req.subagent = true;
        assert_eq!(engine.run(req).await.unwrap().status, "done");
        assert!(!dir.path().join("subagent.txt").exists());
    }

    #[tokio::test]
    async fn always_allow_mode_skips_second_approval_for_risky_tool() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .write_snapshot(
                "data/settings.json",
                &json!({"approvals":{"mode":"always_allow","rules":[]}}),
            )
            .unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "always_write".into(),
                    name: "write".into(),
                    args: json!({"path":"always.txt","content":"ok"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            store,
            provider,
            vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let mut req = request(false);
        req.allow_unsafe = false;
        assert_eq!(engine.run(req).await.unwrap().status, "done");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("always.txt")).unwrap(),
            "ok"
        );
        assert!(sink.approvals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn approval_rules_are_live_and_ask_first_prefers_auto_allow() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .write_snapshot(
                "data/settings.json",
                &json!({"approvals":{"mode":"require","rules":[]}}),
            )
            .unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "first_write".into(),
                    name: "write".into(),
                    args: json!({"path":"first.txt","content":"first"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "second_write".into(),
                    name: "write".into(),
                    args: json!({"path":"second.txt","content":"second"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            store.clone(),
            provider,
            vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
            sink.clone(),
            dir.path(),
        )
        .unwrap();

        let mut first = request(false);
        first.run_id = "approval_live_first".into();
        first.allow_unsafe = false;
        assert_eq!(engine.run(first).await.unwrap().status, "suspended");
        assert_eq!(sink.approvals.lock().unwrap().len(), 1);

        store
            .write_snapshot(
                "data/settings.json",
                &json!({"approvals":{"mode":"require","rules":[
                    {"kind":"auto_allow","text":"Approval required for write"}
                ]}}),
            )
            .unwrap();
        let mut second = request(false);
        second.run_id = "approval_live_second".into();
        second.allow_unsafe = false;
        assert_eq!(engine.run(second).await.unwrap().status, "done");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("second.txt")).unwrap(),
            "second"
        );
        assert_eq!(sink.approvals.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn approved_followups_recheck_each_risk_and_preserve_safe_calls() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .write_snapshot(
                "data/settings.json",
                &json!({"approvals":{"mode":"require","rules":[]}}),
            )
            .unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            tool_calls: vec![
                ToolCall {
                    call_id: "chain_write".into(),
                    name: "write".into(),
                    args: json!({"path":"chain.txt","content":"chain"}),
                },
                ToolCall {
                    call_id: "chain_read".into(),
                    name: "read".into(),
                    args: json!({"path":"chain.txt"}),
                },
                ToolCall {
                    call_id: "chain_bash".into(),
                    name: "bash".into(),
                    args: json!({"command":"printf done > bash-chain.txt"}),
                },
            ],
            stop_reason: "tool_calls".into(),
            ..Default::default()
        }]));
        let engine = ExecutionEngine::new(
            store,
            provider,
            vec![
                Arc::new(WriteTool::default()) as Arc<dyn Tool>,
                Arc::new(ReadTool) as Arc<dyn Tool>,
                Arc::new(BashTool::default()) as Arc<dyn Tool>,
            ],
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let mut request = request(false);
        request.run_id = "approval_chain".into();
        request.allow_unsafe = false;
        assert_eq!(
            engine.run(request.clone()).await.unwrap().status,
            "suspended"
        );
        assert_eq!(sink.approvals.lock().unwrap().len(), 1);
        assert!(!dir.path().join("chain.txt").exists());

        let first_continuation = engine.continue_approved(request.clone()).await.unwrap();
        assert_eq!(first_continuation.status, "suspended");
        assert_eq!(sink.approvals.lock().unwrap().len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("chain.txt")).unwrap(),
            "chain"
        );
        assert!(!dir.path().join("bash-chain.txt").exists());
        {
            let events = sink.events.lock().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| {
                        event.event == "trace.item"
                            && event.data["item"]["type"] == "tool.end"
                            && event.data["item"]["data"]["call_id"] == "chain_read"
                    })
                    .count(),
                1
            );
        }

        let final_continuation = engine.continue_approved(request).await.unwrap();
        assert_eq!(final_continuation.status, "done");
        assert_eq!(sink.approvals.lock().unwrap().len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("bash-chain.txt")).unwrap(),
            "done"
        );
    }

    #[tokio::test]
    async fn unsafe_tool_checkpoint_waits_for_approval_without_side_effect() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            tool_calls: vec![
                ToolCall {
                    call_id: "call_write".into(),
                    name: "write".into(),
                    args: json!({"path":"created.txt","content":"secret"}),
                },
                ToolCall {
                    call_id: "call_write_2".into(),
                    name: "write".into(),
                    args: json!({"path":"created-2.txt","content":"secret-2"}),
                },
            ],
            stop_reason: "tool_calls".into(),
            ..Default::default()
        }]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
            sink.clone(),
            dir.path(),
        )
        .unwrap();
        let mut req = request(false);
        req.allow_unsafe = false;
        let outcome = engine.run(req).await.unwrap();
        assert_eq!(outcome.status, "suspended");
        assert!(!dir.path().join("created.txt").exists());
        assert!(!dir.path().join("created-2.txt").exists());
        assert_eq!(sink.approvals.lock().unwrap().len(), 1);
        let jobs = std::fs::read_dir(dir.path().join("data/jobs"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
            .collect::<Vec<_>>();
        assert!(jobs.iter().any(|job| {
            job["checkpoint"]["pending_tools"]
                .as_array()
                .is_some_and(|calls| calls.len() == 2)
        }));
        assert!(jobs.iter().any(|job| {
            job["unsafe_replay"] == true && job["checkpoint"]["pending_tool"]["name"] == "write"
        }));
    }

    #[tokio::test]
    async fn allowed_unsafe_tool_still_has_a_write_ahead_checkpoint() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "call_write".into(),
                    name: "write".into(),
                    args: json!({"path":"created.txt","content":"ok"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            },
            Completion {
                text: "done".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            },
        ]));
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
            sink,
            dir.path(),
        )
        .unwrap();
        let mut req = request(false);
        req.allow_unsafe = true;
        engine.run(req).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("created.txt")).unwrap(),
            "ok"
        );
        let commits = std::fs::read_to_string(dir.path().join("data/jobs/commits.jsonl")).unwrap();
        assert!(commits
            .lines()
            .any(|line| line.contains("\"unsafe_replay\":true")));
    }

    #[tokio::test]
    async fn approval_continuation_executes_pending_tool_after_engine_restart() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            tool_calls: vec![ToolCall {
                call_id: "call_write".into(),
                name: "write".into(),
                args: json!({"path":"approved.txt","content":"approved"}),
            }],
            stop_reason: "tool_calls".into(),
            ..Default::default()
        }]));
        let mut pending = request(false);
        pending.allow_unsafe = false;
        {
            let engine = ExecutionEngine::new(
                Store::open(dir.path()).unwrap(),
                provider,
                vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
                sink.clone(),
                dir.path(),
            )
            .unwrap();
            assert_eq!(
                engine.run(pending.clone()).await.unwrap().status,
                "suspended"
            );
        }
        // A daemon restart can classify a write-ahead checkpoint as
        // Suspended. Approval must continue that checkpoint too, without
        // asking the model to emit the unsafe call again.
        {
            let mut durable =
                macbot_durable::DurableRuntime::from_store(Store::open(dir.path()).unwrap())
                    .unwrap();
            let job = durable
                .jobs()
                .find(|job| job.checkpoint["run_id"].as_str() == Some("run_mock"))
                .cloned()
                .unwrap();
            durable
                .commit(&job.id, JobStatus::Suspended, job.checkpoint, true)
                .unwrap();
        }
        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "continued".into(),
            stop_reason: "stop".into(),
            ..Default::default()
        }]));
        pending.resume_approved = true;
        let engine = ExecutionEngine::new(
            Store::open(dir.path()).unwrap(),
            provider,
            vec![Arc::new(WriteTool::default()) as Arc<dyn Tool>],
            sink,
            dir.path(),
        )
        .unwrap();
        let outcome = engine.run(pending).await.unwrap();
        assert_eq!(outcome.status, "done");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("approved.txt")).unwrap(),
            "approved"
        );
    }
}
