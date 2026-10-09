//! Durable model/tool execution used by the real gateway backend.
//!
//! The HTTP layer owns authentication and RPC framing. This module owns one
//! run: durable checkpointing, provider streaming, tool rounds, trace entries,
//! and publication through a small gateway-owned sink. It intentionally does
//! not know Axum or orchestrator internals, which keeps its side effects
//! reviewable and makes the mock provider usable in tests.

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use macbot_durable::{DurableError, DurableRuntime, InboxItem, Job, JobStatus};
use macbot_providers::{Completion, ModelEvent, ModelProvider, ModelRequest, TokenUsage, ToolCall};
use macbot_store::{Store, StoreError};
use macbot_tools::{Part, Risk, Tool, ToolContext, ToolResult};
use macbot_usage::{Totals, UsageLedger, UsageRecord};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
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
}

fn default_max_turns() -> usize {
    16
}
fn default_provider_id() -> String {
    "mock".into()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub event: String,
    pub data: Value,
    pub persistent: bool,
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
    async fn send_group_message(&self, message: Value) -> Result<(), String>;
    async fn approval_required(&self, data: Value);
    async fn memory_context(&self, _request: &ExecutionRequest) -> Option<Value> {
        None
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
    async fn send_msg(&self, message: Value) -> Result<(), String>;
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
            let _ = self.state.events.send(json!({
                "v": 1,
                "kind": "evt",
                "event": event.event,
                "data": event.data
            }));
        }
    }

    async fn send_group_message(&self, message: Value) -> Result<(), String> {
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
    async fn send_group_message(&self, _: Value) -> Result<(), String> {
        Ok(())
    }
    async fn approval_required(&self, _: Value) {}
}

pub struct ExecutionEngine {
    store: Store,
    durable: Mutex<DurableRuntime>,
    provider: Arc<dyn ModelProvider>,
    provider_resolver: Option<Arc<dyn ProviderResolver>>,
    tools: HashMap<String, Arc<dyn Tool>>,
    sink: Arc<dyn ExecutionSink>,
    home: PathBuf,
    usage: Arc<Mutex<UsageLedger>>,
    aseq: Mutex<HashMap<String, u64>>,
    message_seq: Mutex<HashMap<String, u64>>,
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
        let durable = DurableRuntime::from_store(store.clone())?;
        Ok(Self {
            store,
            durable: Mutex::new(durable),
            provider,
            provider_resolver: None,
            tools: tools
                .into_iter()
                .map(|tool| (tool.name().to_string(), tool))
                .collect(),
            sink,
            home: home.into(),
            usage,
            aseq: Mutex::new(HashMap::new()),
            message_seq: Mutex::new(HashMap::new()),
        })
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
        let mut state = self.aseq.lock().await;
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

    async fn next_message_seq(&self, chat_id: &str) -> Result<u64, ExecutionError> {
        let mut state = self.message_seq.lock().await;
        let entry = state.entry(chat_id.to_owned()).or_insert_with(|| {
            self.store
                .read_jsonl::<Value>(format!("data/chats/{}/messages.jsonl", safe_id(chat_id)))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|message| message.get("seq").and_then(Value::as_u64))
                .max()
                .unwrap_or(0)
        });
        *entry += 1;
        Ok(*entry)
    }

    /// Resume working jobs. Unsafe checkpoints are converted to `Suspended` by
    /// `DurableRuntime::resume_plan`; the gateway can turn these into approval
    /// cards without replaying a write or command.
    pub async fn recover(&self) -> Result<Vec<Job>, ExecutionError> {
        Ok(self.durable.lock().await.resume_plan()?)
    }

    pub async fn run(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, ExecutionError> {
        let max_turns = request.max_turns.max(1);
        let initial_messages = if request.messages.is_empty() {
            vec![json!({"role":"user","content":request.instruction})]
        } else {
            request.messages.clone()
        };
        let (job, mut messages, start_turn, resumed, approved_call) = {
            let mut durable = self.durable.lock().await;
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
                if matches!(job.status, JobStatus::Waiting | JobStatus::Suspended)
                    && !(job.status == JobStatus::Waiting && request.resume_approved)
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
                if request.resume_approved && job.status == JobStatus::Waiting {
                    let call =
                        serde_json::from_value::<ToolCall>(job.checkpoint["pending_tool"].clone())
                            .map_err(|error| {
                                ExecutionError::Durable(DurableError::Invalid(error.to_string()))
                            })?;
                    let resumed_job = durable.commit(
                        &job.id,
                        JobStatus::Running,
                        json!({"run_id":request.run_id,"round":round,"messages":messages}),
                        false,
                    )?;
                    (resumed_job, messages, round, true, Some(call))
                } else {
                    (job, messages, round, true, None)
                }
            } else {
                let job = durable.create_job(
                    &request.bot_id,
                    "model_run",
                    json!({"run_id":request.run_id,"round":0,"messages":initial_messages}),
                )?;
                (job, initial_messages, 0, false, None)
            }
        };
        if !resumed {
            if let Some(context) = self.sink.memory_context(&request).await {
                messages.insert(0, json!({"role":"system","content":context}));
            }
        }
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
                json!({"phase":if request.private {"chat"} else {"work"},"model":request.model,"parent_run_id":null,"subagent_task":null}),
            )
            .await?;
        }
        let streaming_message_id = if request.private {
            Some(self.create_streaming_message(&request).await?)
        } else {
            None
        };

        let model_start_turn = if let Some(call) = approved_call {
            self.execute_approved_tool(&request, &job, start_turn, call, &mut messages)
                .await?;
            start_turn + 1
        } else {
            start_turn
        };

        for turn in model_start_turn..max_turns {
            turns_used = turn + 1;
            if let Some(steer) = {
                let mut durable = self.durable.lock().await;
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
                    let mut durable = self.durable.lock().await;
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
            {
                let mut durable = self.durable.lock().await;
                durable.commit(
                    &job.id,
                    JobStatus::Running,
                    json!({"run_id":request.run_id,"round":turn,"messages":messages}),
                    false,
                )?;
            }
            // A run/turn is one billable model request. Keeping this ID stable
            // makes a durable retry deduplicate its usage record.
            let request_id = format!("{}:llm:{}", request.run_id, turn);
            self.trace(&request, "llm.request", json!({"request_id":request_id,"model":request.model,"context":{"l0":0,"l1":0,"l2":0,"l3":0,"l4":0,"total":0},"tools":self.tools.keys().collect::<Vec<_>>(),"prompt_ref":null})).await?;
            let model_request = ModelRequest {
                model: request.model.clone(),
                messages: messages.clone(),
                tools: self.tool_schemas(!request.private),
                max_output: 8192,
                session_id: Some(request.run_id.clone()),
            };
            let provider = if let Some(resolver) = &self.provider_resolver {
                resolver
                    .resolve(&request.provider_id, &request.model)
                    .await
                    .map_err(ExecutionError::Resolver)?
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
            let completion = match completion {
                Ok(completion) => completion,
                Err(error) => {
                    let _ = self.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Failed,
                        json!({"error":error.to_string()}),
                        false,
                    );
                    self.trace(
                        &request,
                        "run.end",
                        json!({"status":"failed","error":error.to_string()}),
                    )
                    .await?;
                    return Err(error);
                }
            };
            usage.input_tokens += completion.usage.input_tokens;
            usage.output_tokens += completion.usage.output_tokens;
            usage.cache_read_tokens += completion.usage.cache_read_tokens;
            usage.cache_write_tokens += completion.usage.cache_write_tokens;
            self.usage.lock().await.record(UsageRecord {
                request_id: request_id.clone(),
                ts: Utc::now(),
                bot_id: request.bot_id.clone(),
                project_id: request.project_id.clone(),
                chat_id: request.chat_id.clone(),
                assignment_id: request.assignment_id.clone(),
                run_id: request.run_id.clone(),
                phase: if request.private {
                    "chat".into()
                } else {
                    "work".into()
                },
                provider_id: request.provider_id.clone(),
                model_id: request.model.clone(),
                routine: request.routine,
                usage: Totals {
                    input_tokens: completion.usage.input_tokens,
                    output_tokens: completion.usage.output_tokens,
                    cache_read_tokens: completion.usage.cache_read_tokens,
                    cache_write_tokens: completion.usage.cache_write_tokens,
                    requests: 1,
                    cost: request.price.as_ref().map(|price| {
                        price.cost(&Totals {
                            input_tokens: completion.usage.input_tokens,
                            output_tokens: completion.usage.output_tokens,
                            cache_read_tokens: completion.usage.cache_read_tokens,
                            cache_write_tokens: completion.usage.cache_write_tokens,
                            requests: 1,
                            cost: None,
                        })
                    }),
                },
                task_done: request.private || group_done,
            })?;
            self.trace(
                &request,
                    "llm.response",
                json!({"request_id":request_id,"text":completion.text,"thinking":if completion.thinking.is_empty(){Value::Null}else{json!(completion.thinking)},"tool_calls":completion.tool_calls,"stop_reason":completion.stop_reason,"usage":{"input_tokens":completion.usage.input_tokens,"output_tokens":completion.usage.output_tokens,"cache_read_tokens":completion.usage.cache_read_tokens,"cache_write_tokens":completion.usage.cache_write_tokens,"requests":1,"cost":null},"latency_ms":0,"ttft_ms":0}),
            )
            .await?;

            if completion.tool_calls.is_empty() {
                if !request.private
                    && !group_done
                    && group_no_report_rounds < 2
                    && turn + 1 < max_turns
                {
                    group_no_report_rounds += 1;
                    messages.push(json!({"role":"assistant","content":completion.text}));
                    messages.push(json!({"role":"user","content":"请通过 send_msg 工具发送本轮进展或最终结果。"}));
                    continue;
                }
                final_text = completion.text.clone();
                completed = true;
                self.publish_answer(
                    &request,
                    turn,
                    &completion.text,
                    streaming_message_id.as_deref(),
                )
                .await?;
                break;
            }

            messages.push(json!({"role":"assistant","content":completion.text,"tool_calls":completion.tool_calls.iter().map(|call| json!({"id":call.call_id,"type":"function","function":{"name":call.name,"arguments":call.args.to_string()}})).collect::<Vec<_>>() }));
            for call in completion.tool_calls {
                self.trace(
                    &request,
                    "tool.start",
                    json!({"call_id":call.call_id,"name":call.name,"args":call.args}),
                )
                .await?;
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
                        if intent == "progress" {
                            group_progress_count += 1;
                        }
                        if intent == "done" {
                            group_done = true;
                        }
                        let chat_id = call.args["chat_id"]
                            .as_str()
                            .unwrap_or(&request.chat_id)
                            .to_owned();
                        let message_id = call.args["message_id"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("msg_{}", safe_id(&call.call_id)));
                        let payload = json!({
                            "message_id":message_id,
                            "chat_id":chat_id,
                            "bot_id":request.bot_id,
                            "assignment_id":request.assignment_id,
                            "text":call.args["text"].as_str().unwrap_or_default(),
                            "intent":intent
                        });
                        let (receipt, created) = self.durable.lock().await.send_msg_once(
                            &request.run_id,
                            &call.call_id,
                            intent,
                            &message_id,
                            payload.clone(),
                        )?;
                        if created {
                            self.sink
                                .send_group_message(json!({"receipt":receipt,"message":payload}))
                                .await
                                .map_err(ExecutionError::Sink)?;
                        }
                        self.trace(&request, "send_msg", json!({"call_id":call.call_id,"intent":intent,"message_id":message_id,"chat_id":chat_id})).await?;
                        let result = ToolResult::text("message admitted");
                        messages.push(tool_message(&call.call_id, &result));
                        continue;
                    }
                    let result = ToolResult::error(format!("unknown tool: {}", call.name));
                    messages.push(tool_message(&call.call_id, &result));
                    self.trace(&request, "tool.end", json!({"call_id":call.call_id,"is_error":true,"preview":"unknown tool","details":{}})).await?;
                    continue;
                };
                let risk = tool.risk(&call.args);
                let risky = matches!(&risk, Risk::Write | Risk::Exec | Risk::External);
                if risky && !request.allow_unsafe {
                    let checkpoint = json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call});
                    self.durable.lock().await.commit(
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
                    let checkpoint = json!({"run_id":request.run_id,"round":turn,"messages":messages,"pending_tool":call});
                    self.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Running,
                        checkpoint,
                        true,
                    )?;
                }
                let cwd = request.cwd.clone().unwrap_or_else(|| self.home.clone());
                let context = ToolContext::new(cwd, request.run_id.clone(), self.home.join("runs"));
                let started = now_ms();
                let result = tool.call(&context, call.args.clone()).await;
                self.trace(&request, "tool.end", json!({"call_id":call.call_id,"is_error":result.is_error,"preview":preview(&result),"details":result.details.as_object().cloned().unwrap_or_default(),"truncated":false,"full_output":null,"duration_ms":now_ms().saturating_sub(started)})).await?;
                messages.push(tool_message(&call.call_id, &result));
                if risky {
                    self.durable.lock().await.commit(
                        &job.id,
                        JobStatus::Running,
                        json!({"run_id":request.run_id,"round":turn,"messages":messages}),
                        false,
                    )?;
                }
            }
        }
        if !completed {
            let error = ExecutionError::TurnLimit;
            let _ = self.durable.lock().await.commit(
                &job.id,
                JobStatus::Failed,
                json!({"run_id":request.run_id,"error":error.to_string(),"turns":turns_used}),
                false,
            );
            self.trace(
                &request,
                "run.end",
                json!({"status":"failed","error":error.to_string()}),
            )
            .await?;
            return Err(error);
        }
        {
            let mut durable = self.durable.lock().await;
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
        let context = ToolContext::new(
            request.cwd.clone().unwrap_or_else(|| self.home.clone()),
            request.run_id.clone(),
            self.home.join("runs"),
        );
        let started = now_ms();
        let result = tool.call(&context, call.args).await;
        self.trace(
            request,
            "tool.end",
            json!({"call_id":call.call_id,"is_error":result.is_error,"preview":preview(&result),"details":result.details.as_object().cloned().unwrap_or_default(),"truncated":false,"full_output":null,"duration_ms":now_ms().saturating_sub(started)}),
        )
        .await?;
        messages.push(tool_message(&call.call_id, &result));
        self.durable.lock().await.commit(
            &job.id,
            JobStatus::Running,
            json!({"run_id":request.run_id,"round":turn + 1,"messages":messages}),
            false,
        )?;
        Ok(())
    }

    fn tool_schemas(&self, include_group_send: bool) -> Vec<Value> {
        let mut schemas = self
            .tools
            .values()
            .map(|tool| {
                json!({"type":"function","function":{"name":tool.name(),"description":tool.description(),"parameters":tool.schema()}})
            })
            .collect::<Vec<_>>();
        if include_group_send && !self.tools.contains_key("send_msg") {
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
    ) -> Result<Completion, ExecutionError> {
        let (tx, mut rx) = mpsc::channel(256);
        let sink = self.sink.clone();
        let chat_id = request.chat_id.clone();
        let private = request.private;
        let request_id = request_id.to_owned();
        let message_id = message_id.map(str::to_owned);
        let notify = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    ModelEvent::TextDelta { text } => {
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
                    ModelEvent::ThinkingDelta { text } => sink.emit(ExecutionEvent {
                        event: "trace.delta".into(),
                        data: json!({"stream":format!("chat_{}",safe_id(&chat_id)),"request_id":request_id,"channel":"thinking","call_id":null,"text":text}),
                        persistent: false,
                    }).await,
                    ModelEvent::ToolCallDelta { call_id, args, .. } => sink.emit(ExecutionEvent {
                        event: "trace.delta".into(),
                        data: json!({"stream":format!("chat_{}",safe_id(&chat_id)),"request_id":request_id,"channel":"tool_args","call_id":call_id,"text":args}),
                        persistent: false,
                    }).await,
                    ModelEvent::Usage { .. } | ModelEvent::Stop { .. } => {}
                }
            }
        });
        let result = provider.stream(model, tx).await;
        let _ = notify.await;
        Ok(result?)
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
        let mut message = self
            .store
            .read_jsonl::<Value>(&path)?
            .into_iter()
            .find(|message| message["id"] == message_id)
            .unwrap_or_else(|| json!({"id":message_id,"chat_id":request.chat_id,"seq":0}));
        message["blocks"] = json!([{"type":"text","markdown":text}]);
        message["fallback_text"] = json!(text);
        message["streaming"] = json!(false);
        message["edited_at"] = json!(now_rfc3339());
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
            return Ok(message_id);
        }
        let seq = self.next_message_seq(&request.chat_id).await?;
        let message = json!({
            "id": message_id,
            "chat_id": request.chat_id,
            "seq": seq,
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
        });
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
fn preview(result: &ToolResult) -> String {
    tool_message("preview", result)["content"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(8_000)
        .collect()
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
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[cfg(test)]
mod tests {
    use super::*;
    use macbot_providers::{MockProvider, ToolCall};
    use macbot_tools::WriteTool;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    #[derive(Default)]
    struct RecordingSink {
        events: StdMutex<Vec<ExecutionEvent>>,
        groups: StdMutex<Vec<Value>>,
        approvals: StdMutex<Vec<Value>>,
    }
    #[async_trait]
    impl ExecutionSink for RecordingSink {
        async fn emit(&self, event: ExecutionEvent) {
            self.events.lock().unwrap().push(event);
        }
        async fn send_group_message(&self, message: Value) -> Result<(), String> {
            self.groups.lock().unwrap().push(message);
            Ok(())
        }
        async fn approval_required(&self, data: Value) {
            self.approvals.lock().unwrap().push(data);
        }
    }

    struct FixedResolver(Arc<dyn ModelProvider>);
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
        }
    }

    #[tokio::test]
    async fn mock_provider_private_run_streams_and_persists_trace_and_usage() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            text: "已完成".into(),
            stop_reason: "stop".into(),
            usage: TokenUsage {
                input_tokens: 3,
                output_tokens: 2,
                ..Default::default()
            },
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
        let outcome = engine.run(request(true)).await.unwrap();
        assert_eq!(outcome.status, "done");
        assert_eq!(outcome.text, "已完成");
        assert_eq!(outcome.usage.output_tokens, 2);
        let events = sink.events.lock().unwrap();
        assert!(events.iter().any(|event| event.event == "message.delta"));
        assert!(events.iter().any(|event| event.event == "message.updated"));
        assert!(!events.iter().any(|event| event.event == "usage.updated"));
        assert!(events.iter().any(|event| event.event == "message.created"));
        let updated = events
            .iter()
            .find(|event| event.event == "message.updated")
            .unwrap();
        assert_eq!(updated.data["message"]["streaming"], false);
        for (seq, event) in events.iter().enumerate() {
            let frame = json!({"v":1,"kind":"evt","seq":seq as u64 + 1,"event":event.event,"data":event.data});
            let _: macbot_protocol::EventFrame = serde_json::from_value(frame).unwrap();
        }
        assert!(
            !std::fs::read_to_string(dir.path().join("data/runs/run_mock/entries.jsonl"))
                .unwrap()
                .is_empty()
        );
        let trace_path = dir.path().join("data/traces/chat_mock.jsonl");
        for line in std::fs::read_to_string(trace_path).unwrap().lines() {
            let _: macbot_protocol::TraceItem = serde_json::from_str(line).unwrap();
        }
    }

    #[tokio::test]
    async fn group_send_is_once_even_when_run_replayed() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![
            Completion {
                tool_calls: vec![ToolCall {
                    call_id: "send_1".into(),
                    name: "send_msg".into(),
                    args: json!({"intent":"done","text":"群里汇报","chat_id":"chat_mock"}),
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
        let usage_records = std::fs::read_dir(dir.path().join("data/usage/raw"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .map(|contents| contents.lines().count())
            .sum::<usize>();
        assert_eq!(usage_records, 2, "replaying a run must not charge twice");
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
    async fn unsafe_tool_checkpoint_waits_for_approval_without_side_effect() {
        let dir = tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let provider = Arc::new(MockProvider::new(vec![Completion {
            tool_calls: vec![ToolCall {
                call_id: "call_write".into(),
                name: "write".into(),
                args: json!({"path":"created.txt","content":"secret"}),
            }],
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
        assert_eq!(sink.approvals.lock().unwrap().len(), 1);
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
