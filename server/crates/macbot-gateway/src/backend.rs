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
        FeatureError, FeatureService, MaintenanceTickReport, MaintenanceUsageContext,
        MaintenanceUsageSink, ModelMaintenanceAdapter, ProfileOnboardingPrompt,
        SharedFeatureService, UserMemoryCuratorProvider,
    },
    memory_tools::{FeatureExecutionSink, FeatureRunContext, FeatureToolRuntime},
    GatewayState, RpcBackend,
};
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use macbot_memory::{MemoryAccess, MemoryTarget, ProjectRecord, SessionMessage};
use macbot_orchestrator::Orchestrator;
use macbot_providers::{
    registry::ProviderRegistry, Completion, ModelEvent, ModelProvider, ModelRequest, TokenUsage,
};
use macbot_skills::SkillDraftFrom;
use macbot_store::{Event, Store};
use macbot_tools::{
    BashJobManager, BashJobTool, BashTool, EditTool, FileMutationQueue, FindTool, GrepTool, LsTool,
    Part, ReadTool, Tool, ToolContext, ToolResult, WriteTool,
};
use macbot_usage::{Price, Totals, UsageLedger, UsageRecord};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use thiserror::Error;
#[cfg(test)]
use tokio::sync::Notify;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

fn feature_rpc_error(error: FeatureError) -> crate::RpcError {
    let details = match &error {
        FeatureError::UserMemoryConflict(current) | FeatureError::UserProfileConflict(current) => {
            Some(current.clone())
        }
        _ => None,
    };
    crate::RpcError {
        code: error.code().into(),
        message: error.to_string(),
        details,
    }
}

fn host_default_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find_map(|value| normalize_host_locale(&value))
        .unwrap_or_else(|| "en".into())
}

fn normalize_host_locale(value: &str) -> Option<String> {
    let base = value.split(['.', '@']).next()?.trim();
    if base.is_empty() || matches!(base, "C" | "POSIX") {
        return None;
    }
    let normalized = base.replace('_', "-");
    crate::validate_client_locale(&json!({"client_locale":normalized}))
        .ok()
        .map(|_| normalized)
}

fn reply_language(
    profile_language: Option<&str>,
    client_locale: Option<&str>,
    host: &str,
) -> String {
    profile_language
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .or_else(|| client_locale.filter(|locale| !locale.is_empty()))
        .unwrap_or(host)
        .to_owned()
}

fn reply_language_instruction(language: &str) -> String {
    format!("本轮回复请使用 {language}。用户档案中明确设置的语言优先于客户端系统语言。")
}

fn idle_bot_memory_targets(snapshot: &Value, active_bots: &HashSet<String>) -> Vec<MemoryTarget> {
    snapshot
        .get("bots")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|bots| bots.values())
        .filter(|bot| bot.get("is_main").and_then(Value::as_bool) != Some(true))
        .filter_map(|bot| bot.get("id").and_then(Value::as_str))
        // Older snapshots may not have the explicit is_main marker. The
        // runtime treats the canonical id as main in that case as well.
        .filter(|bot_id| *bot_id != "main" && !active_bots.contains(*bot_id))
        .map(MemoryTarget::bot)
        .collect()
}

#[cfg(test)]
type StartupDispatchGate = Arc<Notify>;
#[cfg(not(test))]
type StartupDispatchGate = ();

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
    cancelling_assignments: Arc<Mutex<HashSet<String>>>,
}

enum WaitingContinuation {
    Question(String),
    Takeover(Option<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ContinuationAction {
    Approval {
        approval_id: String,
    },
    Decision {
        question_id: String,
        answer: String,
    },
    ToolQuestion {
        marker_id: String,
        answer: String,
    },
    Takeover {
        assignment_id: String,
        note: Option<String>,
    },
    Message {
        assignment_id: String,
        expected_message_id: String,
        text: String,
        steer_message_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DurableContinuation {
    id: String,
    assignment_id: String,
    run_id: String,
    action: ContinuationAction,
    state: String,
    created_at: String,
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
    continuation_lock: Arc<std::sync::Mutex<()>>,
    approved_recovery_claims: Arc<std::sync::Mutex<HashSet<String>>>,
    denied_cancellation_claims: Arc<std::sync::Mutex<HashSet<String>>>,
    #[cfg(test)]
    recovery_done: Arc<AtomicBool>,
    #[cfg(test)]
    recovery_notify: Arc<Notify>,
}

impl ComposedBackend {
    pub fn open(
        inner: Arc<ProductionBackend>,
        state: GatewayState,
        home: impl Into<std::path::PathBuf>,
    ) -> Result<Self, RuntimeError> {
        Self::open_inner(inner, state, home, None)
    }

    #[cfg(test)]
    fn open_with_dispatch_gate(
        inner: Arc<ProductionBackend>,
        state: GatewayState,
        home: impl Into<std::path::PathBuf>,
        gate: Arc<Notify>,
    ) -> Result<Self, RuntimeError> {
        Self::open_inner(inner, state, home, Some(gate))
    }

    fn open_inner(
        inner: Arc<ProductionBackend>,
        state: GatewayState,
        home: impl Into<std::path::PathBuf>,
        dispatch_gate: Option<StartupDispatchGate>,
    ) -> Result<Self, RuntimeError> {
        let home = home.into();
        #[cfg(not(test))]
        let _ = dispatch_gate;
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
            continuation_lock: Arc::new(std::sync::Mutex::new(())),
            approved_recovery_claims: Arc::new(std::sync::Mutex::new(HashSet::new())),
            denied_cancellation_claims: Arc::new(std::sync::Mutex::new(HashSet::new())),
            #[cfg(test)]
            recovery_done: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            recovery_notify: Arc::new(Notify::new()),
        };
        // Recovery must happen before the first client request.  A suspended
        // unsafe job is left for approval; ordinary working jobs resume from
        // their durable checkpoint with the original run id.
        let recovery = backend.runtime.clone();
        let recovery_backend = backend.clone();
        #[cfg(test)]
        let dispatch_gate = dispatch_gate.clone();
        #[cfg(test)]
        let recovery_done = backend.recovery_done.clone();
        #[cfg(test)]
        let recovery_notify = backend.recovery_notify.clone();
        tokio::spawn(async move {
            if let Err(error) = recovery.configure_feature_runtime().await {
                tracing::warn!(%error, "feature runtime configuration unavailable during recovery");
            }
            let continuation_runs = match recovery_backend.read_continuations() {
                Ok(records) => Some(
                    records
                        .into_iter()
                        .filter(|record| record.state != "done")
                        .map(|record| record.run_id)
                        .collect::<HashSet<_>>(),
                ),
                Err(error) => {
                    tracing::error!(%error, "cannot read continuation ownership before recovery; ordinary resume deferred");
                    None
                }
            };
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
                            tracing::error!(job_id=%job.id, "recoverable job has no run id; ordinary resume deferred");
                            continue;
                        };
                        if continuation_runs
                            .as_ref()
                            .is_none_or(|runs| runs.contains(run_id))
                        {
                            continue;
                        }
                        let request = recovery.store.read_snapshot::<ExecutionRequest>(format!(
                            "data/run_requests/{run_id}.json"
                        ));
                        match request {
                            Ok(Some(request)) => {
                                if let Some(assignment_id) = request.assignment_id.as_deref() {
                                    if let Err(error) = recovery_backend
                                        .inner
                                        .record_assignment_recovered(
                                            &recovery_backend.state,
                                            assignment_id,
                                            crate::now(),
                                        )
                                        .await
                                    {
                                        tracing::error!(
                                            %error,
                                            %assignment_id,
                                            "cannot persist recovered assignment marker"
                                        );
                                        continue;
                                    }
                                }
                                if request.private {
                                    recovery_backend
                                        .scheduled
                                        .lock()
                                        .await
                                        .insert(format!("orphan-private-chat:{}", request.chat_id));
                                }
                                recovery_backend.spawn_request(request)
                            }
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
            // Recover an unread private turn immediately after durable jobs
            // have been claimed.  The remaining startup reconciliation paths
            // may scan large approval/job sets or dispatch many assignments;
            // none of those should delay the one-message recovery boundary.
            recovery_backend.recover_orphan_private_messages().await;
            recovery_backend.recover_invalid_tool_approvals().await;
            recovery_backend.recover_denied_tool_approvals().await;
            recovery_backend.recover_allowed_tool_approvals().await;
            recovery_backend.recover_answered_decisions().await;
            recovery_backend.recover_continuations().await;
            #[cfg(test)]
            if let Some(gate) = dispatch_gate {
                gate.notified().await;
            }
            recovery_backend.dispatch_ready_assignments().await;
            #[cfg(test)]
            recovery_done.store(true, Ordering::Release);
            #[cfg(test)]
            recovery_notify.notify_waiters();
        });
        let curator_inner = Arc::downgrade(&backend.inner);
        let curator_features = Arc::downgrade(&backend.feature_service);
        let curator_state = backend.state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.tick().await;
            loop {
                interval.tick().await;
                let (Some(inner), Some(features)) =
                    (curator_inner.upgrade(), curator_features.upgrade())
                else {
                    break;
                };
                run_user_curator_tick(inner, features, &curator_state).await;
            }
        });
        Ok(backend)
    }

    /// Recover only user messages which are provably an unstarted private Bot
    /// turn.  `chat.send` persists the message before the composed scheduler
    /// creates the execution request, so a task cancellation can leave an
    /// unread message without a durable job or request.  Recovery deliberately
    /// does not replay every unread message: it requires a Bot-owned DM,
    /// excludes messages already represented by an assignment, request, job,
    /// Bot reply, or pending approval, and uses the message id as the stable
    /// run id.  This keeps restart recovery idempotent and out of the approval
    /// replay paths.
    async fn recover_orphan_private_messages(&self) {
        let Ok(snapshot) = self.inner.orchestrator.snapshot() else {
            return;
        };
        let message_locales = self
            .inner
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .unwrap_or_default()
            .into_iter()
            .filter(|operation| {
                operation.get("method").and_then(Value::as_str) == Some("chat.send")
            })
            .filter_map(|operation| {
                let message_id = operation
                    .pointer("/result/message/id")
                    .and_then(Value::as_str)?;
                let locale = operation
                    .pointer("/params/client_locale")
                    .and_then(Value::as_str)?;
                Some((message_id.to_owned(), locale.to_owned()))
            })
            .collect::<HashMap<_, _>>();
        let dm_chats = snapshot
            .get("bots")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|bots| bots.values())
            .filter_map(|bot| {
                Some((
                    bot.get("id")?.as_str()?.to_owned(),
                    bot.get("dm_chat_id")?.as_str()?.to_owned(),
                ))
            })
            .collect::<Vec<_>>();
        let assigned_message_ids = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|assignments| assignments.values())
            .filter_map(|assignment| {
                assignment
                    .get("trigger_message_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<HashSet<_>>();
        let pending_approval_chats = snapshot
            .get("approvals")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|approvals| approvals.values())
            .filter(|approval| approval.get("state").and_then(Value::as_str) == Some("pending"))
            .filter_map(|approval| {
                approval
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<HashSet<_>>();
        let pending_question_chats = snapshot
            .get("questions")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|questions| questions.values())
            .filter(|question| question.get("state").and_then(Value::as_str) == Some("pending"))
            .filter_map(|question| {
                question
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<HashSet<_>>();

        for (bot_id, chat_id) in dm_chats {
            if pending_approval_chats.contains(&chat_id)
                || pending_question_chats.contains(&chat_id)
                || self
                    .private_chat_has_nonterminal_execution(&bot_id, &chat_id)
                    .await
            {
                tracing::info!(%chat_id, "private message recovery skipped while chat has pending or active work");
                continue;
            }
            let last_read_seq = self
                .inner
                .read_chat_overlay(&chat_id)
                .ok()
                .and_then(|overlay| overlay.get("last_read_seq").and_then(Value::as_u64));
            let Some(last_read_seq) = last_read_seq else {
                // A missing cursor is legacy/unknown state.  Do not infer
                // that all messages in such a chat need execution.
                continue;
            };
            let Ok(messages) = self.inner.load_chat_messages(&chat_id) else {
                continue;
            };
            let chat_claim_key = format!("orphan-private-chat:{chat_id}");
            if self.scheduled.lock().await.contains(&chat_claim_key) {
                tracing::info!(%chat_id, "private message recovery skipped while chat recovery claim is active");
                continue;
            }
            for message in messages.iter().filter(|message| {
                message
                    .get("seq")
                    .and_then(Value::as_u64)
                    .is_some_and(|seq| seq > last_read_seq)
                    && message
                        .get("sender")
                        .and_then(|sender| sender.get("kind"))
                        .and_then(Value::as_str)
                        == Some("user")
                    && message.get("deleted").and_then(Value::as_bool) != Some(true)
                    && message.get("assignment_id").is_none_or(Value::is_null)
            }) {
                let Some(message_id) = message.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if assigned_message_ids.contains(message_id) {
                    continue;
                }
                let run_id = format!(
                    "run_chat_{}",
                    message_id
                        .chars()
                        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                        .collect::<String>()
                );
                if self.private_execution_exists(&run_id).await {
                    continue;
                }
                let has_matching_bot_reply = messages.iter().any(|reply| {
                    reply
                        .get("sender")
                        .and_then(|sender| sender.get("kind"))
                        .and_then(Value::as_str)
                        == Some("bot")
                        && reply
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| id.contains(&run_id))
                });
                if has_matching_bot_reply {
                    continue;
                }
                let text = message
                    .get("fallback_text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned);
                let Some(text) = text else {
                    continue;
                };
                let params = json!({
                    "chat_id": chat_id,
                    "text": text,
                    "mentions": message.get("mentions").cloned().unwrap_or_else(|| json!([])),
                    "reply_to": message.get("reply_to").cloned().unwrap_or(Value::Null)
                });
                let mut params = params;
                if let Some(locale) = message_locales.get(message_id) {
                    params["client_locale"] = json!(locale);
                }
                let result = json!({"message": message});
                let Some(request) = self.request_for_chat(&params, &result).await else {
                    tracing::warn!(%chat_id, %message_id, "orphan private message has no configured model");
                    continue;
                };
                // Claim one message per chat for this recovery pass.  The
                // remaining unread messages stay durable and can be reviewed
                // or recovered after the resulting run reaches a terminal
                // state; they must not become concurrent fresh runs here.
                if !self.scheduled.lock().await.insert(chat_claim_key.clone()) {
                    break;
                }
                tracing::warn!(
                    %chat_id,
                    %message_id,
                    run_id = %request.run_id,
                    "recovering orphan private message"
                );
                self.spawn_request(request);
            }
        }
    }

    async fn private_chat_has_nonterminal_execution(&self, bot_id: &str, chat_id: &str) -> bool {
        let runs = {
            let durable = self.runtime.state.durable.lock().await;
            durable
                .jobs()
                .filter(|job| {
                    matches!(
                        job.status,
                        macbot_durable::JobStatus::Queued
                            | macbot_durable::JobStatus::Running
                            | macbot_durable::JobStatus::Waiting
                            | macbot_durable::JobStatus::Suspended
                    )
                })
                .map(|job| {
                    (
                        job.checkpoint
                            .get("run_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        job.owner.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        runs.into_iter().any(|(run_id, owner)| {
            let Some(run_id) = run_id else {
                return owner == bot_id;
            };
            self.inner
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .ok()
                .flatten()
                .map_or(owner == bot_id, |request| {
                    request.private && request.chat_id == chat_id && request.bot_id == bot_id
                })
        })
    }

    async fn private_execution_exists(&self, run_id: &str) -> bool {
        if self
            .inner
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
            .ok()
            .flatten()
            .is_some()
            || self
                .runtime
                .find_request(|request| request.run_id == run_id)
                .is_some()
        {
            return true;
        }
        self.runtime
            .state
            .durable
            .lock()
            .await
            .jobs()
            .any(|job| job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id))
    }

    async fn run_user_curator_for_chat(&self, chat_id: &str, daily_fallback: bool) {
        run_curator_for_chat(
            &self.inner,
            &self.feature_service,
            &self.state,
            chat_id,
            daily_fallback,
        )
        .await;
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
        let jobs = self.durable_jobs_snapshot().await;
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
                let Ok(Some(mut request)) = self
                    .inner
                    .store
                    .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                else {
                    continue;
                };
                if self.runtime.tighten_request_tools(&mut request).is_err() {
                    continue;
                }
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

    /// A denied private approval must terminate its waiting durable run. This
    /// is a separate recovery path from approved continuations: it never
    /// enters the model again and only cancels a uniquely matched checkpoint.
    async fn recover_denied_tool_approvals(&self) {
        let Ok(snapshot) = self.inner.orchestrator.snapshot() else {
            return;
        };
        let Some(approvals) = snapshot.get("approvals").and_then(Value::as_object) else {
            return;
        };
        for (approval_id, approval) in approvals {
            if approval.get("state").and_then(Value::as_str) != Some("denied")
                || approval
                    .get("assignment_id")
                    .is_some_and(|value| !value.is_null())
            {
                continue;
            }
            if !self.claim_denied_cancellation(approval_id) {
                continue;
            }
            match self.runtime.cancel_denied_approval(approval_id).await {
                Ok(Some(outcome)) => {
                    tracing::info!(
                        %approval_id,
                        run_id = %outcome.run_id,
                        status = %outcome.status,
                        "reconciled denied private approval"
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%approval_id, %error, "denied private approval reconciliation failed")
                }
            }
            self.release_denied_cancellation(approval_id);
        }
    }

    /// Resume an unsafe tool only when approval and the durable waiting
    /// checkpoint still identify the same call.  Approval decisions can be
    /// persisted just before the continuation fails (for example on ENOSPC),
    /// so this pass is intentionally retryable after restart and maintenance.
    async fn recover_allowed_tool_approvals(&self) {
        let Ok(snapshot) = self.inner.orchestrator.snapshot() else {
            return;
        };
        let Some(approvals) = snapshot.get("approvals").and_then(Value::as_object) else {
            return;
        };
        let jobs = self.durable_jobs_snapshot().await;
        let mut waiting_by_call: HashMap<String, Vec<macbot_durable::Job>> = HashMap::new();
        for job in jobs {
            if job.status != macbot_durable::JobStatus::Waiting || !job.unsafe_replay {
                continue;
            }
            let Some(call_id) = job
                .checkpoint
                .pointer("/pending_tool/call_id")
                .and_then(Value::as_str)
            else {
                continue;
            };
            waiting_by_call
                .entry(normalize_call_id(call_id).to_owned())
                .or_default()
                .push(job);
        }
        for (approval_id, approval) in approvals {
            if !matches!(
                approval.get("state").and_then(Value::as_str),
                Some("allowed_once" | "always_allowed")
            ) {
                continue;
            }
            let mapped_call_id = self
                .inner
                .store
                .read_snapshot::<Value>(format!(
                    "data/approval-map/{}.json",
                    safe_component(approval_id)
                ))
                .ok()
                .flatten()
                .and_then(|value| {
                    value
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let Some(mapped_call_id) = mapped_call_id else {
                continue;
            };
            let mut candidates = Vec::new();
            let Some(jobs) = waiting_by_call.get(normalize_call_id(&mapped_call_id)) else {
                continue;
            };
            for job in jobs {
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
                if allowed_tool_approval_matches(
                    &snapshot,
                    approval,
                    Some(mapped_call_id.as_str()),
                    job,
                    &request,
                    false,
                ) {
                    candidates.push(run_id.to_owned());
                }
            }
            if candidates.len() != 1 {
                continue;
            }
            let run_id = candidates.pop().expect("candidate length checked");
            if let Some(assignment_id) = approval.get("assignment_id").and_then(Value::as_str) {
                if let Err(error) = self
                    .enqueue_continuation(
                        format!("approval-{approval_id}"),
                        assignment_id.to_owned(),
                        run_id,
                        ContinuationAction::Approval {
                            approval_id: approval_id.clone(),
                        },
                    )
                    .await
                {
                    tracing::warn!(%approval_id, %error, "approved recovery could not be queued");
                }
                continue;
            }
            if !self.claim_approved_continuation(approval_id) {
                continue;
            }
            let approval_assignment = approval.get("assignment_id").cloned();
            let runtime = self.runtime.clone();
            let scheduler = self.clone();
            let approval_id = approval_id.to_owned();
            tokio::spawn(async move {
                let result = runtime.resume_approved_waiting(&approval_id, &run_id).await;
                scheduler.release_approved_continuation(&approval_id);
                scheduler
                    .finish_approved_continuation(approval_assignment, result, &run_id)
                    .await;
            });
        }
    }

    async fn finish_approved_continuation(
        &self,
        approval_assignment: Option<Value>,
        result: Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError>,
        expected_run_id: &str,
    ) {
        match result {
            Ok(Some((assignment_id, outcome))) => {
                if outcome.run_id != expected_run_id {
                    tracing::warn!(expected_run_id, actual_run_id = %outcome.run_id, "approved continuation returned a different run");
                    return;
                }
                let assignment_id = assignment_id.or_else(|| {
                    approval_assignment
                        .as_ref()
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
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
                tracing::info!(
                    run_id = expected_run_id,
                    "approved recovery found no waiting continuation"
                )
            }
            Err(error) => {
                tracing::warn!(run_id = expected_run_id, %error, "approved recovery continuation failed")
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
        let jobs = self.durable_jobs_snapshot().await;
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
            for job in &jobs {
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
            if let Some(assignment_id) = request.assignment_id.as_deref() {
                if let Err(error) = self
                    .enqueue_continuation(
                        format!("question-{question_id}"),
                        assignment_id.to_owned(),
                        request.run_id.clone(),
                        ContinuationAction::Decision {
                            question_id: question_id.clone(),
                            answer,
                        },
                    )
                    .await
                {
                    tracing::warn!(%question_id, %error, "answered decision recovery could not be queued");
                }
                continue;
            }
            let Some(active_key) = self.reserve_run(&request) else {
                continue;
            };
            let assignment_id = request.assignment_id.clone();
            self.mark_waiting(assignment_id.as_deref(), &request.run_id, true);
            let continuation = self
                .runtime
                .resume_question_for_run(question_id, answer, Some(&request.run_id))
                .await;
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

    async fn durable_jobs_snapshot(&self) -> Vec<macbot_durable::Job> {
        self.runtime
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect()
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
                source_message_id: None,
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
                continuation_id: None,
                continuation_payload: None,
                subagent: false,
                tools: bot_tool_allowlist(bot, is_main, false, true, self.web_search_configured()),
                phase: Some(if is_main { "coordinate" } else { "work" }.into()),
                parent_run_id: None,
                subagent_task: None,
                save_full_requests: false,
                resume_message: None,
                client_locale: None,
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
        // An approval may have reached durable state immediately before a
        // previous continuation hit a transient I/O failure.  Re-run the
        // exact checkpoint recovery from the maintenance path as well as at
        // startup; the in-process claim and durable job state make retries
        // idempotent.
        self.recover_allowed_tool_approvals().await;
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
        let targets = idle_bot_memory_targets(&snapshot, &active_bots);
        let (committed, maintenance_error) = if let Some(error) = configure_error {
            (0, Some(error))
        } else {
            self.publish_maintenance_report(
                self.feature_service
                    .maintenance_tick_with_events(&targets)
                    .await,
            )
            .await
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

    async fn publish_maintenance_report(
        &self,
        report: MaintenanceTickReport,
    ) -> (usize, Option<String>) {
        // Maintenance commits its memory records before returning them.
        // Publish every exact durable event even when a later target in the
        // same pass fails, so a partial pass cannot create a cursor hole.
        for event in report.events {
            self.state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        (
            report.committed.len(),
            report.error.map(|error| error.to_string()),
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
        // A DM is an incarnation of exactly one Bot. Mentions can enqueue
        // work for another Bot, but must not replace the Bot that answers in
        // this conversation; otherwise its streaming reply is persisted in
        // the wrong private chat. Project chats have no `chat_bot` and still
        // use the explicitly mentioned Bot (or Main as the default).
        let bot_id = if chat_id == "chat_main" {
            snapshot
                .get("bots")
                .and_then(Value::as_object)
                .and_then(|bots| {
                    bots.values()
                        .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                })
                .and_then(|bot| bot.get("id").and_then(Value::as_str))
                .unwrap_or("main")
        } else if let Some(chat_bot) = chat_bot {
            chat_bot
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
        let mut tools = bot_tool_allowlist(
            bot,
            bot_is_main,
            private,
            assignment_id.is_some(),
            self.web_search_configured(),
        );
        let host_timezone = self
            .inner
            .orchestrator
            .rpc("settings.get", json!({}))
            .await
            .ok()
            .and_then(|result| result.get("settings").cloned())
            .and_then(|settings| {
                settings
                    .get("timezone")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "UTC".to_owned());
        let profile_language = self
            .feature_service
            .user_profile_language(&host_timezone)
            .ok()
            .flatten();
        let client_locale = params
            .get("client_locale")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let response_language = reply_language(
            profile_language.as_deref(),
            client_locale.as_deref(),
            &host_default_locale(),
        );
        let onboarding =
            if bot_is_main && private && assignment_id.is_none() && project_id.is_none() {
                self.feature_service
                    .profile_onboarding_status(&host_timezone)
                    .ok()
            } else {
                None
            };
        if onboarding.is_some_and(|status| status.available) {
            tools
                .get_or_insert_with(Vec::new)
                .push("set_user_profile".into());
        }
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
        messages.insert(
            0,
            json!({"role":"system","content":reply_language_instruction(&response_language)}),
        );
        if let Some(prompt) = onboarding.and_then(|status| status.prompt) {
            let content = match prompt {
                ProfileOnboardingPrompt::Initialize => {
                    "首次私聊初始化：自然地询问用户希望使用的称呼、语言和身份；用户可以随时跳过。只在用户回答或明确跳过后调用 set_user_profile，并传入档案当前 expected_version。language 必须传 BCP 47 标签，例如简体中文用 zh-CN、繁体中文用 zh-TW、英语用 en；跟随系统语言用空字符串。Host 时区只读，不询问也不修改。完成后简短确认。"
                }
                ProfileOnboardingPrompt::Reminder => {
                    "用户档案仍未完成。这是唯一一次主动提醒：自然询问用户是否愿意补充称呼、语言和身份，也可以跳过；Host 时区只读。用户若明确补充，可用 set_user_profile 完成初始化。"
                }
            };
            messages.insert(0, json!({"role":"system","content":content}));
        }
        messages.push(json!({"role":"user","content":instruction}));
        let price = self.runtime.price_for_model(&model);
        let cwd = self.runtime.cwd_for(project_id.as_deref(), &bot_id);
        Some(ExecutionRequest {
            run_id,
            assignment_id,
            source_message_id: message_id.map(str::to_owned),
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
            continuation_id: None,
            continuation_payload: None,
            subagent: false,
            tools,
            phase: None,
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
            client_locale,
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
        // Queued routine assignments must keep the same identity and source
        // as immediately dispatched routines, including after a restart.
        let routine_run_id = snapshot
            .get("routine_runs")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|routines| routines.values())
            .filter_map(Value::as_array)
            .flatten()
            .find(|run| run.get("assignment_id").and_then(Value::as_str) == Some(&assignment_id))
            .and_then(|run| run.get("id"))
            .and_then(Value::as_str);
        let routine = routine_run_id.is_some();
        let private = !routine && (chat_id == "chat_main" || chat_id.starts_with("dm_"));
        let mut messages = if routine {
            Vec::new()
        } else {
            self.chat_model_messages(&snapshot, &chat_id, &bot_id, None, None)
        };
        messages.push(json!({"role":"user","content":instruction}));
        let run_id = routine_run_id
            .map(str::to_owned)
            .unwrap_or_else(|| format!("run_{assignment_id}"));
        let price = self.runtime.price_for_model(&model);
        let cwd = self.runtime.cwd_for(project_id.as_deref(), &bot_id);
        Some(ExecutionRequest {
            run_id,
            assignment_id: Some(assignment_id),
            source_message_id: assignment
                .get("trigger_message_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
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
            routine,
            price,
            resume_approved: false,
            continuation_id: None,
            continuation_payload: None,
            subagent: false,
            tools: bot_tool_allowlist(bot, is_main, private, true, self.web_search_configured()),
            phase: Some(if is_main { "coordinate" } else { "work" }.into()),
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
            client_locale: None,
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
        let Ok(event) =
            crate::adapter::append_assignment_updated_event(&inner.store, assignment.clone())
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
        let status_events = match self.inner.store.events_since(0) {
            Ok(events) => events,
            Err(error) => {
                tracing::error!(%error, "cannot inspect assignment event tail; dispatch deferred");
                return;
            }
        };
        let continuations = match self.read_continuations() {
            Ok(records) => records,
            Err(error) => {
                tracing::error!(%error, "cannot inspect durable continuations; dispatch deferred");
                return;
            }
        };
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
            let assignment_id = assignment.get("id").and_then(Value::as_str);
            let continuation = assignment_id.and_then(|id| {
                continuations
                    .iter()
                    .filter(|record| {
                        record.assignment_id == id
                            && !matches!(record.state.as_str(), "done" | "uncertain")
                    })
                    .min_by(|left, right| {
                        left.created_at
                            .cmp(&right.created_at)
                            .then_with(|| left.id.cmp(&right.id))
                    })
                    .cloned()
            });
            if assignment.get("status").and_then(Value::as_str) == Some("working") {
                if let Some(continuation) = continuation {
                    let request = match self.request_for_run(&continuation.run_id) {
                        Ok(Some(request)) => request,
                        Ok(None) => {
                            tracing::error!(continuation_id=%continuation.id, "continuation run request is missing");
                            continue;
                        }
                        Err(error) => {
                            tracing::error!(continuation_id=%continuation.id, %error, "continuation run request read failed");
                            continue;
                        }
                    };
                    if request.assignment_id.as_deref() == Some(continuation.assignment_id.as_str())
                    {
                        if Self::assignment_status_event_needed(&status_events, &assignment) {
                            Self::publish_assignment_status(&self.inner, &self.state, &assignment)
                                .await;
                        }
                        // The assignment is already admitted by the orchestrator's
                        // pump. `waiting_runs` records the old checkpoint state,
                        // so it must not suppress this exact continuation; the
                        // active-run reservation is the duplicate-spawn guard.
                        if let Some(active_key) = self.reserve_run(&request) {
                            self.spawn_reserved_continuation(continuation, request, active_key);
                        }
                    }
                    continue;
                }
            }
            // A question/approval/reply mutation can make an assignment
            // eligible before its continuation record is written. The old
            // run is still the owner of that admitted slot; never create a
            // replacement run in this persistence window.
            if assignment.get("status").and_then(Value::as_str) == Some("working") {
                let Some(assignment_id) = assignment_id else {
                    continue;
                };
                match self.durable_request_for_assignment(assignment_id).await {
                    Ok(Some((
                        macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended,
                        _,
                    ))) => {
                        continue;
                    }
                    Ok(Some((_, request))) => {
                        let Some(active_key) = self.reserve_run(&request) else {
                            continue;
                        };
                        if Self::assignment_status_event_needed(&status_events, &assignment) {
                            Self::publish_assignment_status(&self.inner, &self.state, &assignment)
                                .await;
                        }
                        self.spawn_reserved_request(request, active_key);
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::error!(%assignment_id, %error, "cannot establish whether a waiting run owns this assignment; dispatch deferred");
                        continue;
                    }
                }
            }
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
            if request.bot_id != "main"
                && assignment
                    .get("project_id")
                    .and_then(Value::as_str)
                    .is_some()
            {
                let Some(assignment_id) = request.assignment_id.as_deref() else {
                    continue;
                };
                let ack = self
                    .inner
                    .execution_send_msg(
                        &self.state,
                        json!({
                            "receipt":{"run_id":format!("assignment-ack-{assignment_id}"),"call_id":"received"},
                            "message":{
                                "message_id":format!("msg_assignment_ack_{assignment_id}"),
                                "chat_id":request.chat_id,
                                "bot_id":request.bot_id,
                                "assignment_id":assignment_id,
                                "intent":"ack",
                                "text":"收到，开始处理。",
                                "mentions":[],
                                "artifacts":[],
                                "options":[]
                            }
                        }),
                    )
                    .await;
                if let Err(error) = ack {
                    tracing::warn!(%assignment_id, %error, "worker assignment ack was not persisted; deferring execution");
                    continue;
                }
            }
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

    fn canonical_steer_text(result: &Value, params: &Value) -> Option<String> {
        params
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| result.pointer("/message/text").and_then(Value::as_str))
            .or_else(|| {
                result
                    .pointer("/message/fallback_text")
                    .and_then(Value::as_str)
            })
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    }

    async fn schedule_assignment_steer(&self, result: &Value, params: &Value) {
        let assignment_id = result
            .get("assignment")
            .and_then(|assignment| assignment.get("id"))
            .and_then(Value::as_str)
            .or_else(|| result.get("assignment_id").and_then(Value::as_str))
            .or_else(|| params.get("assignment_id").and_then(Value::as_str));
        let message_id = result
            .get("message")
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
            .or_else(|| params.get("message_id").and_then(Value::as_str));
        let Some((assignment_id, message_id)) = assignment_id.zip(message_id) else {
            tracing::warn!("steer result has no assignment or message id");
            return;
        };
        let Some(text) = Self::canonical_steer_text(result, params) else {
            tracing::error!(%assignment_id, %message_id, "steer result has no canonical message text");
            return;
        };

        // Persist the user-visible delivery before competing continuations
        // inspect the same waiting checkpoint. The checkpoint continuation
        // will enqueue this exact message id idempotently as a second guard.
        enqueue_steer_runtime(
            self.runtime.clone(),
            &json!({"steer":{"message_id":message_id,"assignment_id":assignment_id}}),
            &json!({"text":text}),
        )
        .await;

        // A queued steer may interrupt a safe decision wait.  Keep this on
        // the same continuation + admission path as a chat reply; pending
        // unsafe tool effects remain waiting for their approval.
        let expected_wait = self
            .inner
            .orchestrator
            .snapshot()
            .ok()
            .and_then(|snapshot| waiting_message_id_for_assignment(&snapshot, assignment_id));
        match self
            .waiting_run_for_assignment(assignment_id, expected_wait.as_deref(), &[], Some(false))
            .await
        {
            Ok(Some(run_id)) => {
                let job = self.durable_job_for_run(&run_id).await;
                let expected_message_id = job.as_ref().and_then(|job| {
                    (matches!(
                        job.checkpoint.get("waiting_reason").and_then(Value::as_str),
                        Some("decision" | "blocked")
                    ) && job
                        .checkpoint
                        .get("pending_tool")
                        .is_none_or(Value::is_null))
                    .then(|| {
                        job.checkpoint
                            .get("waiting_message_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten()
                });
                if let Some(expected_message_id) = expected_message_id {
                    if let Err(error) = self
                        .enqueue_continuation(
                            format!("steer-{message_id}"),
                            assignment_id.to_owned(),
                            run_id,
                            ContinuationAction::Message {
                                assignment_id: assignment_id.to_owned(),
                                expected_message_id,
                                text: text.clone(),
                                steer_message_id: Some(message_id.to_owned()),
                            },
                        )
                        .await
                    {
                        tracing::error!(%assignment_id, %error, "steer continuation could not be queued");
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%assignment_id, %error, "steer continuation lookup failed");
            }
        }
    }

    fn continuation_path(&self, id: &str) -> PathBuf {
        self.inner
            .store
            .root()
            .join("data/continuations")
            .join(format!("{}.json", safe_component(id)))
    }

    fn read_continuations(&self) -> Result<Vec<DurableContinuation>, String> {
        let directory = self.inner.store.root().join("data/continuations");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
            records.push(serde_json::from_slice(&bytes).map_err(|error| error.to_string())?);
        }
        records.sort_by(|left: &DurableContinuation, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    async fn enqueue_late_question_answer(
        &self,
        assignment_id: &str,
        question_id: &str,
        answer: &str,
    ) -> Result<bool, String> {
        let snapshot = self
            .inner
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        let question_message_id = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .and_then(|messages| {
                messages.values().find_map(|message| {
                    (message.get("question_id").and_then(Value::as_str) == Some(question_id))
                        .then(|| message.get("id").and_then(Value::as_str).map(str::to_owned))
                        .flatten()
                })
            });
        let Some(question_message_id) = question_message_id else {
            return Ok(false);
        };
        let candidate = {
            let _guard = self
                .continuation_lock
                .lock()
                .map_err(|_| "continuation state lock poisoned".to_owned())?;
            self.read_continuations()?.into_iter().find(|record| {
                record.assignment_id == assignment_id
                    && matches!(record.state.as_str(), "claimed" | "input_committed")
                    && matches!(
                        &record.action,
                        ContinuationAction::Message {
                            expected_message_id,
                            ..
                        } if expected_message_id == &question_message_id
                    )
            })
        };
        let Some(record) = candidate else {
            return Ok(false);
        };
        let Some(job) = self.durable_job_for_run(&record.run_id).await else {
            return Err(format!(
                "claimed continuation run {} has no durable job",
                record.run_id
            ));
        };
        if matches!(
            job.status,
            macbot_durable::JobStatus::Done
                | macbot_durable::JobStatus::Failed
                | macbot_durable::JobStatus::Cancelled
        ) {
            return Err("the assignment run has already finished".into());
        }
        let claimed_before_resume = record.state == "claimed"
            && matches!(
                job.status,
                macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
            )
            && job.checkpoint.get("run_id").and_then(Value::as_str) == Some(record.run_id.as_str())
            && matches!(
                &record.action,
                ContinuationAction::Message {
                    expected_message_id,
                    ..
                } if job.checkpoint.get("waiting_message_id").and_then(Value::as_str)
                    == Some(expected_message_id.as_str())
                    && job.checkpoint.get("waiting_message").and_then(Value::as_bool)
                        == Some(true)
            );
        if !continuation_checkpoint_matches(&job.checkpoint, &record) && !claimed_before_resume {
            return Err(format!(
                "claimed continuation {} is not attached to its durable run",
                record.id
            ));
        }
        let input_id = format!("question-answer:{question_id}");
        if !self
            .runtime
            .enqueue_input_for_run(&record.run_id, &input_id, answer)
            .await
        {
            return Err(format!(
                "cannot persist answer input for run {}",
                record.run_id
            ));
        }
        Ok(true)
    }

    async fn persist_continuation(&self, record: &DurableContinuation) -> Result<(), String> {
        let _guard = self
            .continuation_lock
            .lock()
            .map_err(|_| "continuation state lock poisoned".to_owned())?;
        self.inner
            .store
            .write_snapshot(
                format!("data/continuations/{}.json", safe_component(&record.id)),
                record,
            )
            .map_err(|error| error.to_string())
    }

    fn request_for_run(&self, run_id: &str) -> Result<Option<ExecutionRequest>, String> {
        self.inner
            .store
            .read_snapshot(format!("data/run_requests/{run_id}.json"))
            .map_err(|error| error.to_string())
    }

    async fn durable_job_for_run(&self, run_id: &str) -> Option<macbot_durable::Job> {
        self.runtime
            .state
            .durable
            .lock()
            .await
            .jobs()
            .find(|job| job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id))
            .cloned()
    }

    async fn durable_request_for_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<Option<(macbot_durable::JobStatus, ExecutionRequest)>, String> {
        let jobs = {
            let durable = self.runtime.state.durable.lock().await;
            durable
                .jobs()
                .filter(|job| {
                    job.kind == "model_run"
                        && !matches!(
                            job.status,
                            macbot_durable::JobStatus::Done
                                | macbot_durable::JobStatus::Failed
                                | macbot_durable::JobStatus::Cancelled
                        )
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut matches = Vec::new();
        for job in jobs {
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                return Err(format!("nonterminal durable job {} has no run id", job.id));
            };
            let request = self
                .request_for_run(run_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("durable run request {run_id} is missing"))?;
            if is_root_request_for_assignment(&request, assignment_id) {
                matches.push((job.status, request));
            }
        }
        if matches.len() > 1 {
            return Err(format!(
                "assignment {assignment_id} has multiple nonterminal root durable runs"
            ));
        }
        Ok(matches.pop())
    }

    async fn waiting_run_for_assignment(
        &self,
        assignment_id: &str,
        expected_message_id: Option<&str>,
        pending_tools: &[&str],
        unsafe_replay: Option<bool>,
    ) -> Result<Option<String>, String> {
        let jobs = self
            .runtime
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        let mut matches = Vec::new();
        for job in jobs {
            if !matches!(
                job.status,
                macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
            ) {
                continue;
            }
            if let Some(message_id) = expected_message_id {
                if !waiting_checkpoint_matches_message(&job.checkpoint, message_id) {
                    continue;
                }
            }
            if !pending_tools.is_empty() {
                let Some(name) = job
                    .checkpoint
                    .pointer("/pending_tool/name")
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if !pending_tools.contains(&name) {
                    continue;
                }
            }
            if !waiting_checkpoint_safety_matches(&job, unsafe_replay) {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(request) = self.request_for_run(run_id)? else {
                return Err(format!("durable job {run_id} has no run request"));
            };
            if request.assignment_id.as_deref() == Some(assignment_id) && request.run_id == run_id {
                matches.push(run_id.to_owned());
            }
        }
        Ok((matches.len() == 1).then(|| matches.remove(0)))
    }

    async fn approved_run_for_assignment(
        &self,
        approval_id: &str,
        assignment_id: &str,
    ) -> Result<Option<String>, String> {
        let snapshot = self
            .inner
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        let Some(approval) = snapshot
            .get("approvals")
            .and_then(|items| items.get(approval_id))
        else {
            return Ok(None);
        };
        if approval.get("assignment_id").and_then(Value::as_str) != Some(assignment_id) {
            return Ok(None);
        }
        let mapped_call = self
            .inner
            .store
            .read_snapshot::<Value>(format!(
                "data/approval-map/{}.json",
                safe_component(approval_id)
            ))
            .map_err(|error| error.to_string())?
            .and_then(|value| {
                value
                    .get("call_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let Some(mapped_call) = mapped_call else {
            return Ok(None);
        };
        let jobs = self
            .runtime
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        let mut matches = Vec::new();
        for job in jobs {
            if job.status != macbot_durable::JobStatus::Waiting || !job.unsafe_replay {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(request) = self.request_for_run(run_id)? else {
                return Err(format!("durable job {run_id} has no run request"));
            };
            if request.assignment_id.as_deref() == Some(assignment_id)
                && allowed_tool_approval_matches(
                    &snapshot,
                    approval,
                    Some(&mapped_call),
                    &job,
                    &request,
                    false,
                )
            {
                matches.push(run_id.to_owned());
            }
        }
        Ok((matches.len() == 1).then(|| matches.remove(0)))
    }

    async fn enqueue_continuation(
        &self,
        id: String,
        assignment_id: String,
        run_id: String,
        action: ContinuationAction,
    ) -> Result<(), String> {
        if self.durable_job_for_run(&run_id).await.is_none() {
            return Err(format!(
                "durable job for continuation run {run_id} not found"
            ));
        }
        let _continuation_guard = self
            .continuation_lock
            .lock()
            .map_err(|_| "continuation state lock poisoned".to_owned())?;
        for mut existing in self
            .read_continuations()
            .map_err(|error| error.to_owned())?
        {
            if existing.run_id != run_id || matches!(existing.state.as_str(), "done" | "uncertain")
            {
                continue;
            }
            if existing.id == id {
                if existing.assignment_id != assignment_id || existing.action != action {
                    return Err(format!(
                        "continuation id {} was reused with different data",
                        id
                    ));
                }
                if existing.state == "queued" {
                    drop(_continuation_guard);
                    self.queue_assignment_for_continuation(&assignment_id)
                        .await?;
                    self.dispatch_ready_assignments().await;
                    return Ok(());
                }
                if existing.state == "claimed" {
                    return Ok(());
                }
            }
            // A safe decision answer is the authoritative continuation input.
            // Keep any concurrent queued message steer in the durable inbox,
            // but let the answer continue the original checkpoint.  If the
            // steer won the race while still queued, switch that single slot
            // to the answer action so the run cannot be resumed twice.
            match (&existing.action, &action, existing.state.as_str()) {
                (ContinuationAction::Decision { .. }, ContinuationAction::Message { .. }, _) => {
                    return Ok(());
                }
                (
                    ContinuationAction::Message { .. },
                    ContinuationAction::Decision { .. },
                    "queued",
                ) => {
                    existing.action = action;
                    self.inner
                        .store
                        .write_snapshot(
                            format!("data/continuations/{}.json", safe_component(&existing.id)),
                            &existing,
                        )
                        .map_err(|error| error.to_string())?;
                    drop(_continuation_guard);
                    self.queue_assignment_for_continuation(&assignment_id)
                        .await?;
                    self.dispatch_ready_assignments().await;
                    return Ok(());
                }
                _ => {
                    return Err(format!(
                        "run {run_id} already has continuation {} in state {}",
                        existing.id, existing.state
                    ));
                }
            }
        }
        let path = self.continuation_path(&id);
        if let Some(existing) = self
            .inner
            .store
            .read_snapshot::<DurableContinuation>(
                path.strip_prefix(self.inner.store.root()).unwrap_or(&path),
            )
            .map_err(|error| error.to_string())?
        {
            if existing.assignment_id != assignment_id
                || existing.run_id != run_id
                || existing.action != action
            {
                return Err(format!(
                    "continuation id {} was reused with different data",
                    id
                ));
            }
            if matches!(existing.state.as_str(), "done" | "claimed" | "uncertain") {
                return Ok(());
            }
            drop(_continuation_guard);
            self.queue_assignment_for_continuation(&assignment_id)
                .await?;
            self.dispatch_ready_assignments().await;
            return Ok(());
        }
        let record = DurableContinuation {
            id: id.clone(),
            assignment_id: assignment_id.clone(),
            run_id,
            action,
            state: "queued".into(),
            created_at: crate::now(),
        };
        self.inner
            .store
            .write_snapshot(
                format!("data/continuations/{}.json", safe_component(&id)),
                &record,
            )
            .map_err(|error| error.to_string())?;
        drop(_continuation_guard);
        self.queue_assignment_for_continuation(&assignment_id)
            .await?;
        self.dispatch_ready_assignments().await;
        Ok(())
    }

    async fn queue_assignment_for_continuation(&self, assignment_id: &str) -> Result<(), String> {
        let assignment = self
            .inner
            .orchestrator
            .queue_assignment_resume(assignment_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("assignment {assignment_id} is no longer resumable"))?;
        self.inner
            .persist_orchestrator(json!({
                "method":"assignment.continuation_queued",
                "params":{"assignment_id":assignment_id},
                "result":{"assignment":assignment},
                "status":"done"
            }))
            .await
            .map_err(|error| error.message)?;
        let wire_assignment =
            serde_json::to_value(&assignment).map_err(|error| error.to_string())?;
        if let Ok(event) =
            crate::adapter::append_assignment_updated_event(&self.inner.store, wire_assignment)
        {
            self.state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
        Ok(())
    }

    async fn recover_continuations(&self) {
        let records = match self.read_continuations() {
            Ok(records) => records,
            Err(error) => {
                tracing::error!(%error, "cannot recover durable continuations");
                return;
            }
        };
        for mut record in records {
            if record.state == "done" {
                continue;
            }
            if record.state == "uncertain" {
                let reason = if matches!(record.action, ContinuationAction::Approval { .. }) {
                    "approval"
                } else {
                    "blocked"
                };
                self.suspend_uncertain_assignment(&record.assignment_id, reason)
                    .await;
                continue;
            }
            if record.state == "claimed" {
                if self
                    .active_runs
                    .lock()
                    .map(|runs| runs.contains(&record.assignment_id))
                    .unwrap_or(false)
                {
                    continue;
                }
                // Repair the WAL-answer / inbox-append crash window before
                // deciding whether this continuation was consumed. The
                // stable inbox id makes this safe alongside an RPC retry.
                if let ContinuationAction::Message {
                    expected_message_id,
                    ..
                } = &record.action
                {
                    let snapshot = match self.inner.orchestrator.snapshot() {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            tracing::error!(continuation_id=%record.id, %error, "cannot inspect question answer during continuation recovery");
                            continue;
                        }
                    };
                    let answered =
                        (|| -> Option<(String, String)> {
                            let message = snapshot.get("messages")?.as_object()?.values().find(
                                |message| {
                                    message.get("id").and_then(Value::as_str)
                                        == Some(expected_message_id.as_str())
                                },
                            )?;
                            let question_id = message.get("question_id")?.as_str()?;
                            let question = snapshot.get("questions")?.get(question_id)?;
                            if question.get("state").and_then(Value::as_str) != Some("answered") {
                                return None;
                            }
                            let answer = question.get("answer")?;
                            let text = answer
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .or_else(|| {
                                    answer.get("option_index").and_then(Value::as_u64).and_then(
                                        |index| {
                                            question
                                                .get("options")?
                                                .as_array()?
                                                .get(index as usize)?
                                                .as_str()
                                                .map(str::to_owned)
                                        },
                                    )
                                })?;
                            Some((question_id.to_owned(), text))
                        })();
                    if let Some((question_id, answer)) = answered {
                        let _input_gate = self
                            .runtime
                            .state
                            .assignment_input_gate(&record.assignment_id)
                            .await;
                        if let Err(error) = self
                            .enqueue_late_question_answer(
                                &record.assignment_id,
                                &question_id,
                                &answer,
                            )
                            .await
                        {
                            tracing::error!(continuation_id=%record.id, %error, "cannot repair answered question inbox during continuation recovery");
                            continue;
                        }
                    }
                }
                let mut suspension_reason = "blocked";
                match self.durable_job_for_run(&record.run_id).await {
                    Some(job) if continuation_checkpoint_matches(&job.checkpoint, &record) => {
                        let phase = job
                            .checkpoint
                            .pointer("/continuation/phase")
                            .and_then(Value::as_str);
                        let approval = matches!(record.action, ContinuationAction::Approval { .. });
                        if matches!(phase, Some("input_committed" | "effect_result_committed"))
                            || matches!(
                                job.status,
                                macbot_durable::JobStatus::Done
                                    | macbot_durable::JobStatus::Failed
                                    | macbot_durable::JobStatus::Cancelled
                            )
                        {
                            record.state = "done".into();
                        } else if approval
                            && (phase == Some("effect_unknown")
                                || job.status == macbot_durable::JobStatus::Suspended)
                        {
                            suspension_reason = "approval";
                            record.state = "uncertain".into();
                            tracing::error!(continuation_id=%record.id, run_id=%record.run_id, "approved side effect has no durable result; retaining suspended job without replay");
                        } else {
                            record.state = "uncertain".into();
                            tracing::error!(continuation_id=%record.id, run_id=%record.run_id, ?phase, "continuation checkpoint does not prove input consumption; retaining run without replay");
                        }
                        if let Err(error) = self.persist_continuation(&record).await {
                            tracing::error!(continuation_id=%record.id, %error, "cannot persist continuation consumption");
                        }
                        if record.state == "uncertain" {
                            self.suspend_uncertain_assignment(
                                &record.assignment_id,
                                suspension_reason,
                            )
                            .await;
                        }
                        continue;
                    }
                    None => {
                        record.state = "uncertain".into();
                        tracing::error!(continuation_id=%record.id, run_id=%record.run_id, "claimed continuation has no durable run; refusing replay");
                    }
                    _ => {
                        record.state = "uncertain".into();
                        tracing::error!(continuation_id=%record.id, run_id=%record.run_id, "continuation checkpoint provenance does not match the claimed payload; refusing replay");
                    }
                }
                if let Err(error) = self.persist_continuation(&record).await {
                    tracing::error!(continuation_id=%record.id, %error, "cannot restore continuation queue state");
                    continue;
                }
                if record.state == "uncertain" {
                    let reason = if matches!(record.action, ContinuationAction::Approval { .. }) {
                        "approval"
                    } else {
                        "blocked"
                    };
                    self.suspend_uncertain_assignment(&record.assignment_id, reason)
                        .await;
                    continue;
                }
            }
            if let Err(error) = self
                .queue_assignment_for_continuation(&record.assignment_id)
                .await
            {
                tracing::warn!(continuation_id=%record.id, %error, "cannot restore queued continuation");
            }
        }
    }

    async fn suspend_uncertain_assignment(&self, assignment_id: &str, reason: &str) {
        let assignment = match self
            .inner
            .orchestrator
            .suspend_assignment_for_recovery(assignment_id, reason)
        {
            Ok(Some(assignment)) => assignment,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%assignment_id, %error, "cannot suspend assignment with uncertain continuation");
                return;
            }
        };
        if let Err(error) = self
            .inner
            .persist_orchestrator(json!({
                "method":"assignment.recovery_suspended",
                "params":{"assignment_id":assignment_id,"reason":reason},
                "result":{"assignment":assignment},
                "status":"done"
            }))
            .await
        {
            tracing::error!(%assignment_id, %error, "cannot persist recovery suspension");
            return;
        }
        let wire_assignment = match serde_json::to_value(&assignment) {
            Ok(assignment) => assignment,
            Err(error) => {
                tracing::error!(%assignment_id, %error, "cannot serialize recovery suspension event");
                return;
            }
        };
        if let Ok(event) =
            crate::adapter::append_assignment_updated_event(&self.inner.store, wire_assignment)
        {
            self.state
                .publish_event(event.seq, &event.event, event.data)
                .await;
        }
    }

    fn spawn_reserved_continuation(
        &self,
        mut record: DurableContinuation,
        mut request: ExecutionRequest,
        active_key: String,
    ) {
        let _continuation_guard = match self.continuation_lock.lock() {
            Ok(guard) => guard,
            Err(_) => {
                if let Ok(mut active) = self.active_runs.lock() {
                    active.remove(&active_key);
                }
                tracing::error!(continuation_id=%record.id, "continuation state lock poisoned");
                return;
            }
        };
        let path = format!("data/continuations/{}.json", safe_component(&record.id));
        let persisted = match self.inner.store.read_snapshot::<DurableContinuation>(&path) {
            Ok(Some(persisted)) if persisted.state == "queued" => persisted,
            Ok(_) => {
                if let Ok(mut active) = self.active_runs.lock() {
                    active.remove(&active_key);
                }
                return;
            }
            Err(error) => {
                if let Ok(mut active) = self.active_runs.lock() {
                    active.remove(&active_key);
                }
                tracing::error!(continuation_id=%record.id, %error, "cannot inspect continuation before claim");
                return;
            }
        };
        if persisted.assignment_id != record.assignment_id
            || persisted.run_id != record.run_id
            || persisted.action != record.action
        {
            if let Ok(mut active) = self.active_runs.lock() {
                active.remove(&active_key);
            }
            tracing::error!(continuation_id=%record.id, "queued continuation changed before claim");
            return;
        }
        request.continuation_id = Some(record.id.clone());
        request.continuation_payload = serde_json::to_value(&record.action).ok();
        if request.continuation_payload.is_none() {
            tracing::error!(continuation_id=%record.id, "cannot serialize continuation payload");
            if let Ok(mut active) = self.active_runs.lock() {
                active.remove(&active_key);
            }
            return;
        }
        if let Err(error) = self.runtime.persist_request(&request) {
            if let Ok(mut active) = self.active_runs.lock() {
                active.remove(&active_key);
            }
            tracing::error!(continuation_id=%record.id, %error, "cannot persist continuation request marker");
            return;
        }
        record.state = "claimed".into();
        if let Err(error) = self.inner.store.write_snapshot(&path, &record) {
            if let Ok(mut active) = self.active_runs.lock() {
                active.remove(&active_key);
            }
            tracing::error!(continuation_id=%record.id, %error, "cannot claim durable continuation");
            return;
        }
        drop(_continuation_guard);
        let scheduler = self.clone();
        tokio::spawn(async move {
            // `question.answer` first commits the canonical answered state,
            // then records a stable inbox item. A crash between those writes
            // is repaired here before the original run resumes.
            if let ContinuationAction::Message {
                expected_message_id,
                ..
            } = &record.action
            {
                let snapshot = match scheduler.inner.orchestrator.snapshot() {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        tracing::error!(continuation_id=%record.id, %error, "cannot inspect answered question before continuation");
                        if let Ok(mut active) = scheduler.active_runs.lock() {
                            active.remove(&active_key);
                        }
                        return;
                    }
                };
                let answered = (|| -> Option<(String, String)> {
                    let message =
                        snapshot
                            .get("messages")?
                            .as_object()?
                            .values()
                            .find(|message| {
                                message.get("id").and_then(Value::as_str)
                                    == Some(expected_message_id.as_str())
                            })?;
                    let question_id = message.get("question_id")?.as_str()?;
                    let question = snapshot.get("questions")?.get(question_id)?;
                    if question.get("state").and_then(Value::as_str) != Some("answered") {
                        return None;
                    }
                    let text = answered_decision_answer(question)?;
                    Some((question_id.to_owned(), text))
                })();
                if let Some((question_id, answer)) = answered {
                    let _input_gate = scheduler
                        .runtime
                        .state
                        .assignment_input_gate(&record.assignment_id)
                        .await;
                    if let Err(error) = scheduler
                        .enqueue_late_question_answer(&record.assignment_id, &question_id, &answer)
                        .await
                    {
                        tracing::error!(continuation_id=%record.id, %error, "cannot recover answered question input before continuation");
                        if let Ok(mut active) = scheduler.active_runs.lock() {
                            active.remove(&active_key);
                        }
                        return;
                    }
                }
            }
            let result = match &record.action {
                ContinuationAction::Approval { approval_id } => {
                    scheduler
                        .runtime
                        .resume_approved_waiting(approval_id, &record.run_id)
                        .await
                }
                ContinuationAction::Decision {
                    question_id,
                    answer,
                } => {
                    scheduler
                        .runtime
                        .resume_question_for_run(question_id, answer.clone(), Some(&record.run_id))
                        .await
                }
                ContinuationAction::ToolQuestion { marker_id, answer } => {
                    scheduler
                        .runtime
                        .resume_waiting_assignment(
                            marker_id,
                            &["ask_user", "question"],
                            WaitingContinuation::Question(answer.clone()),
                            Some(&record.run_id),
                        )
                        .await
                }
                ContinuationAction::Takeover {
                    assignment_id,
                    note,
                } => {
                    scheduler
                        .runtime
                        .resume_takeover_for_run(assignment_id, note.clone(), Some(&record.run_id))
                        .await
                }
                ContinuationAction::Message {
                    assignment_id,
                    expected_message_id,
                    text,
                    steer_message_id,
                } => {
                    scheduler
                        .runtime
                        .resume_message_for_decision_inner(
                            assignment_id,
                            Some(expected_message_id),
                            text.clone(),
                            steer_message_id.as_deref(),
                            Some(&record.run_id),
                        )
                        .await
                }
            };
            if let Ok(mut active) = scheduler.active_runs.lock() {
                active.remove(&active_key);
            }
            match result {
                Ok(Some((assignment_id, outcome))) => {
                    scheduler.mark_waiting(
                        assignment_id
                            .as_deref()
                            .or(Some(record.assignment_id.as_str())),
                        &outcome.run_id,
                        matches!(outcome.status.as_str(), "waiting" | "blocked" | "suspended"),
                    );
                    if outcome.status == "done" {
                        scheduler
                            .finish_assignment_and_resume_parent(
                                record.assignment_id.clone(),
                                outcome.text,
                            )
                            .await;
                    } else if matches!(outcome.status.as_str(), "failed" | "cancelled") {
                        fail_assignment(
                            scheduler.inner.clone(),
                            scheduler.state.clone(),
                            record.assignment_id.clone(),
                            &outcome.status,
                        )
                        .await;
                    }
                }
                Err(error) => {
                    tracing::error!(continuation_id=%record.id, %error, "durable continuation failed")
                }
                Ok(None) => {
                    tracing::error!(continuation_id=%record.id, run_id=%record.run_id, "durable continuation found no matching run to resume")
                }
            }
            let job = scheduler.durable_job_for_run(&record.run_id).await;
            let phase = job.as_ref().and_then(|job| {
                continuation_checkpoint_matches(&job.checkpoint, &record)
                    .then(|| {
                        job.checkpoint
                            .pointer("/continuation/phase")
                            .and_then(Value::as_str)
                    })
                    .flatten()
            });
            let terminal_job = job.as_ref().is_some_and(|job| {
                matches!(
                    job.status,
                    macbot_durable::JobStatus::Done
                        | macbot_durable::JobStatus::Failed
                        | macbot_durable::JobStatus::Cancelled
                )
            });
            if terminal_job || matches!(phase, Some("input_committed" | "effect_result_committed"))
            {
                record.state = "done".into();
            } else {
                record.state = "uncertain".into();
                let reason = if matches!(record.action, ContinuationAction::Approval { .. }) {
                    "approval"
                } else {
                    "blocked"
                };
                tracing::error!(continuation_id=%record.id, run_id=%record.run_id, ?phase, "continuation has no durable consumption/result proof; leaving it suspended instead of replaying");
                scheduler
                    .suspend_uncertain_assignment(&record.assignment_id, reason)
                    .await;
            }
            if let Err(error) = scheduler.persist_continuation(&record).await {
                tracing::error!(continuation_id=%record.id, %error, "cannot persist continuation completion state");
                return;
            }
            scheduler.dispatch_ready_assignments().await;
        });
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
            if let Some(wait_message_id) = parent_wait_message_id.as_deref() {
                if let Ok(Some(run_id)) = self
                    .waiting_run_for_assignment(&parent_id, Some(wait_message_id), &[], Some(false))
                    .await
                {
                    if let Err(error) = self
                        .enqueue_continuation(
                            format!("child-{}-{parent_id}", completed_id),
                            parent_id.clone(),
                            run_id,
                            ContinuationAction::Message {
                                assignment_id: parent_id.clone(),
                                expected_message_id: wait_message_id.to_owned(),
                                text: completed_text,
                                steer_message_id: None,
                            },
                        )
                        .await
                    {
                        tracing::warn!(%parent_id, %error, "parent continuation could not be queued");
                    }
                    return;
                }
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

    fn claim_approved_continuation(&self, approval_id: &str) -> bool {
        self.approved_recovery_claims
            .lock()
            .map(|mut claims| claims.insert(approval_id.to_owned()))
            .unwrap_or(false)
    }

    fn release_approved_continuation(&self, approval_id: &str) {
        if let Ok(mut claims) = self.approved_recovery_claims.lock() {
            claims.remove(approval_id);
        }
    }

    fn claim_denied_cancellation(&self, approval_id: &str) -> bool {
        self.denied_cancellation_claims
            .lock()
            .map(|mut claims| claims.insert(approval_id.to_owned()))
            .unwrap_or(false)
    }

    fn release_denied_cancellation(&self, approval_id: &str) {
        if let Ok(mut claims) = self.denied_cancellation_claims.lock() {
            claims.remove(approval_id);
        }
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

fn is_root_request_for_assignment(request: &ExecutionRequest, assignment_id: &str) -> bool {
    request.assignment_id.as_deref() == Some(assignment_id)
        && !request.subagent
        && request.parent_run_id.is_none()
}

fn waiting_checkpoint_safety_matches(
    job: &macbot_durable::Job,
    expected_unsafe_replay: Option<bool>,
) -> bool {
    let pending_name = job
        .checkpoint
        .pointer("/pending_tool/name")
        .and_then(Value::as_str);
    let legacy_safe_wait = matches!(
        pending_name,
        Some("ask_user" | "question" | "request_takeover")
    ) || (pending_name.is_none()
        && job.checkpoint.get("waiting_reason").and_then(Value::as_str) == Some("decision")
        && job
            .checkpoint
            .get("pending_tool")
            .is_none_or(Value::is_null)
        && job
            .checkpoint
            .get("pending_tools")
            .is_none_or(|tools| tools.is_null() || tools.as_array().is_some_and(Vec::is_empty)));
    expected_unsafe_replay.is_none_or(|expected| job.unsafe_replay == expected || legacy_safe_wait)
}

fn continuation_checkpoint_matches(checkpoint: &Value, record: &DurableContinuation) -> bool {
    let expected_payload = serde_json::to_value(&record.action).ok();
    checkpoint
        .pointer("/continuation/id")
        .and_then(Value::as_str)
        == Some(record.id.as_str())
        && expected_payload.as_ref() == checkpoint.pointer("/continuation/payload")
}

async fn run_user_curator_tick(
    inner: Arc<ProductionBackend>,
    features: SharedFeatureService,
    state: &GatewayState,
) {
    let chats = inner
        .call("chat.list", json!({"limit":100}), state)
        .await
        .ok()
        .and_then(|result| result.get("chats").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    for chat in chats {
        if let Some(chat_id) = chat.get("id").and_then(Value::as_str) {
            run_curator_for_chat(&inner, &features, state, chat_id, true).await;
        }
    }
}

async fn run_curator_for_chat(
    inner: &ProductionBackend,
    features: &FeatureService,
    state: &GatewayState,
    chat_id: &str,
    daily_fallback: bool,
) {
    let timezone = inner
        .orchestrator
        .rpc("settings.get", json!({}))
        .await
        .ok()
        .and_then(|result| result.get("settings").cloned())
        .and_then(|settings| {
            settings
                .get("timezone")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "UTC".into());
    match features.recover_user_curator_pending(&timezone).await {
        Ok(events) => {
            for event in events {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
        }
        Err(error) => {
            tracing::warn!(%error, "user memory curator recovery failed");
            return;
        }
    }
    let messages = match inner.load_chat_messages(chat_id) {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!(%chat_id, %error, "user memory curator could not load chat history");
            return;
        }
    };
    let now = Utc::now();
    let batch = match features
        .prepare_user_curator_batch(chat_id, &messages, now, daily_fallback)
        .await
    {
        Ok(Some(batch)) => batch,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%chat_id, %error, "user memory curator batch preparation failed");
            return;
        }
    };
    match features.run_user_curator_batch(batch, &timezone).await {
        Ok(events) => {
            for event in events {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
        }
        Err(error) => tracing::warn!(%chat_id, %error, "user memory curator batch failed"),
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
        .or_else(|| result.pointer("/message/text").and_then(Value::as_str))
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
        if let Ok(assignment) = serde_json::to_value(&assignment) {
            if let Ok(event) =
                crate::adapter::append_assignment_updated_event(&inner.store, assignment)
            {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
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
            if let Ok(event) =
                crate::adapter::append_assignment_updated_event(&inner.store, assignment.clone())
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
        let stop_gate = backend
            .runtime
            .state
            .assignment_input_gate(&assignment_id)
            .await;
        backend
            .inner
            .call("assignment.stop", stop_params, &backend.state)
            .await?;
        drop(stop_gate);
        backend
            .runtime
            .clear_assignment_cancellation(&assignment_id)
            .await;
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
    if outcome.is_none()
        && status != "blocked"
        && !runtime
            .assignment_cancellation_in_progress(assignment_id)
            .await
    {
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

fn active_assignment_for_reply(snapshot: &Value, message_id: &str) -> Option<String> {
    waiting_assignment_for_message(snapshot, message_id).or_else(|| {
        let assignment_id = snapshot
            .get("messages")
            .and_then(Value::as_object)
            .and_then(|messages| messages.get(message_id))
            .and_then(|message| message.get("assignment_id"))
            .and_then(Value::as_str)?;
        let status = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .and_then(|assignments| assignments.get(assignment_id))
            .and_then(|assignment| assignment.get("status"))
            .and_then(Value::as_str)?;
        matches!(
            status,
            "working" | "queued" | "waiting_user" | "waiting_bot" | "blocked"
        )
        .then(|| assignment_id.to_owned())
    })
}

fn directed_chat_delivery_assignments(
    message: &Value,
    deliveries: &[Value],
    reply_to: Option<&str>,
    snapshot: &Value,
) -> std::collections::HashSet<String> {
    let current_message_id = message.get("id").and_then(Value::as_str);
    let mentioned_bots = message
        .get("mentions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mention| {
            mention.as_str().or_else(|| {
                let kind = mention.get("kind").and_then(Value::as_str)?;
                match kind {
                    "main" => Some("main"),
                    "bot" => mention.get("bot_id").and_then(Value::as_str),
                    _ => mention.get("bot_id").and_then(Value::as_str),
                }
            })
        })
        .collect::<std::collections::HashSet<_>>();
    let mut targets = deliveries
        .iter()
        .filter_map(|delivery| {
            if delivery.get("state").and_then(Value::as_str) != Some("queued") {
                return None;
            }
            let bot_id = delivery.get("bot_id").and_then(Value::as_str)?;
            let assignment_id = delivery.get("assignment_id").and_then(Value::as_str)?;
            let assignment = snapshot
                .get("assignments")
                .and_then(Value::as_object)
                .and_then(|assignments| assignments.get(assignment_id))?;
            let status = assignment.get("status").and_then(Value::as_str)?;
            if !matches!(
                status,
                "working" | "queued" | "waiting_user" | "waiting_bot" | "blocked"
            ) || assignment.get("trigger_message_id").and_then(Value::as_str)
                == current_message_id
            {
                return None;
            }
            mentioned_bots
                .contains(bot_id)
                .then(|| assignment_id.to_owned())
        })
        .collect::<std::collections::HashSet<_>>();
    if let Some(reply_to) = reply_to {
        if let Some(assignment_id) = active_assignment_for_reply(snapshot, reply_to) {
            if deliveries.iter().any(|delivery| {
                delivery.get("assignment_id").and_then(Value::as_str)
                    == Some(assignment_id.as_str())
            }) {
                targets.insert(assignment_id);
            }
        }
    }
    targets
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

pub(crate) fn answered_decision_answer(question: &Value) -> Option<String> {
    if question.get("state").and_then(Value::as_str) != Some("answered") {
        return None;
    }
    let answer = question.get("answer")?.as_object()?;
    let text = answer
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned);
    let indexes = answer
        .get("option_indexes")
        .and_then(Value::as_array)
        .filter(|indexes| !indexes.is_empty())
        .cloned()
        .unwrap_or_else(|| {
            answer
                .get("option_index")
                .and_then(Value::as_u64)
                .map(|index| vec![json!(index)])
                .unwrap_or_default()
        });
    let options = question.get("options").and_then(Value::as_array);
    let selected = indexes
        .iter()
        .filter_map(Value::as_u64)
        .filter_map(|index| options.and_then(|options| options.get(index as usize)))
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    match (selected.is_empty(), text) {
        (true, None) => None,
        (true, Some(text)) => Some(text),
        (false, None) => Some(selected.join("、")),
        (false, Some(text)) => Some(format!("{}（补充：{}）", selected.join("、"), text)),
    }
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
    _unsafe_replay: bool,
    status: &str,
    message_id: &str,
) -> bool {
    matches!(status, "waiting" | "suspended")
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
        let _answer_gate = if method == "question.answer" {
            let question_id = params
                .get("question_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    crate::rpc_error("invalid_params", "question_id is required", None)
                })?;
            let snapshot = self
                .inner
                .orchestrator
                .snapshot()
                .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
            let assignment_id = snapshot
                .get("questions")
                .and_then(|items| items.get(question_id))
                .and_then(|question| question.get("assignment_id"))
                .and_then(Value::as_str)
                .filter(|id| {
                    snapshot
                        .get("assignments")
                        .and_then(|items| items.get(*id))
                        .is_some()
                });
            if let Some(assignment_id) = assignment_id {
                let gate = self
                    .runtime
                    .state
                    .assignment_input_gate(assignment_id)
                    .await;
                if self
                    .runtime
                    .assignment_cancellation_in_progress(assignment_id)
                    .await
                {
                    return Err(crate::rpc_error(
                        "conflict",
                        "assignment is being stopped",
                        Some(json!({"assignment_id": assignment_id})),
                    ));
                }
                let snapshot = self
                    .inner
                    .orchestrator
                    .snapshot()
                    .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
                let current_question = snapshot
                    .get("questions")
                    .and_then(|items| items.get(question_id));
                if current_question
                    .and_then(|question| question.get("assignment_id"))
                    .and_then(Value::as_str)
                    != Some(assignment_id)
                {
                    return Err(crate::rpc_error(
                        "conflict",
                        "question assignment changed while acquiring its input gate",
                        Some(json!({"question_id": question_id})),
                    ));
                }
                let question_state = snapshot
                    .get("questions")
                    .and_then(|items| items.get(question_id))
                    .and_then(|question| question.get("state"))
                    .and_then(Value::as_str);
                if question_state == Some("pending") {
                    let marker = self
                        .inner
                        .store
                        .read_snapshot::<Value>(format!(
                            "data/waiting/{}.json",
                            safe_component(question_id)
                        ))
                        .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
                    if let Some(run_id) = marker
                        .as_ref()
                        .and_then(|marker| marker.get("run_id"))
                        .and_then(Value::as_str)
                    {
                        let terminal = self
                            .runtime
                            .state
                            .durable
                            .lock()
                            .await
                            .jobs()
                            .find(|job| {
                                job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id)
                            })
                            .is_some_and(|job| {
                                matches!(
                                    job.status,
                                    macbot_durable::JobStatus::Done
                                        | macbot_durable::JobStatus::Failed
                                        | macbot_durable::JobStatus::Cancelled
                                )
                            });
                        if terminal {
                            return Err(crate::rpc_error(
                                "conflict",
                                "question belongs to a terminal execution",
                                Some(json!({"question_id": question_id, "run_id": run_id})),
                            ));
                        }
                    }
                }
                Some(gate)
            } else {
                None
            }
        } else {
            None
        };
        if method == "bootstrap" {
            let snapshot = self
                .inner
                .orchestrator
                .snapshot()
                .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?;
            let main_bot = snapshot
                .get("bots")
                .and_then(Value::as_object)
                .and_then(|bots| {
                    bots.values()
                        .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                });
            if let Some(bot) = main_bot {
                let bot_id = bot.get("id").and_then(Value::as_str).unwrap_or("main");
                let chat_id = bot
                    .get("dm_chat_id")
                    .and_then(Value::as_str)
                    .unwrap_or("chat_main");
                let timezone = self
                    .inner
                    .orchestrator
                    .rpc("settings.get", json!({}))
                    .await
                    .ok()
                    .and_then(|result| result.get("settings").cloned())
                    .and_then(|settings| {
                        settings
                            .get("timezone")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "UTC".to_owned());
                let needs_greeting = self
                    .feature_service
                    .profile_needs_first_connection_greeting(&timezone)
                    .map_err(feature_rpc_error)?;
                if needs_greeting {
                    self.inner
                        .call(
                            "send_msg",
                            json!({
                                "bot_id":bot_id,
                                "chat_id":chat_id,
                                "client_request_id":"profile-onboarding:first-connection-greeting:v1",
                                "text":"你好，我是秘书长。为了之后更好地配合你，想先了解你希望我怎么称呼你、使用什么语言，以及你的身份或角色；你也可以跳过。",
                                "intent":"ack",
                                "mentions":[]
                            }),
                            state,
                        )
                        .await?;
                }
            }
        }
        if method == "trace.history" {
            return self.trace_history(&params);
        }
        if matches!(
            method,
            "memory.user.list"
                | "memory.user.create"
                | "memory.user.update"
                | "memory.user.delete"
                | "memory.profile.get"
                | "memory.profile.update"
        ) {
            let timezone = self
                .inner
                .orchestrator
                .rpc("settings.get", json!({}))
                .await
                .ok()
                .and_then(|result| result.get("settings").cloned())
                .and_then(|settings| {
                    settings
                        .get("timezone")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "UTC".to_owned());
            let response = self
                .feature_service
                .user_memory_rpc(method, params, &timezone)
                .map_err(feature_rpc_error)?;
            for event in response.events {
                state
                    .publish_event(event.seq, &event.event, event.data)
                    .await;
            }
            return Ok(response.result);
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
                self.runtime
                    .clear_assignment_cancellation(&assignment_id)
                    .await;
                // A repeated stop must still reconcile persisted approval
                // events, but never cancel or restart the engine a second time.
                if status == "cancelled" {
                    let result = self.inner.call(method, params, state).await;
                    return result;
                }
                return Ok(json!({"assignment": assignment}));
            }
            cancel_assignment_engine(self, &assignment_id, &status).await?;
        }
        if matches!(method, "assignment.create" | "assign" | "delegate") {
            let bot_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
            let from_main = method == "delegate"
                || params.get("from").and_then(Value::as_str).unwrap_or("main") == "main";
            if from_main && bot_id != "main" {
                if params
                    .get("project_id")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    return Err(crate::rpc_error(
                        "forbidden",
                        "main Bot work assignments require a project group",
                        None,
                    ));
                }
                self.runtime
                    .preflight_bot_model(bot_id)
                    .await
                    .map_err(|error| crate::rpc_error("invalid_params", &error, None))?;
            }
        }
        if method == "project.remove_member" {
            let (project_id, bot_id) = validate_project_lifecycle(self, method, &params)?;
            cancel_project_assignments(self, &project_id, bot_id.as_deref()).await?;
        } else if method == "project.confirm_done" {
            let (project_id, _) = validate_project_lifecycle(self, method, &params)?;
            cancel_project_assignments(self, &project_id, None).await?;
        }
        let mut assignment_cancellation_to_clear: Option<String> = None;
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
                        assignment_cancellation_to_clear = Some(assignment_id);
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
        let mut resume_answered_question = if method == "question.answer" {
            let question_id = params.get("question_id").and_then(Value::as_str);
            question_id.is_some_and(|question_id| {
                self.inner
                    .orchestrator
                    .snapshot()
                    .ok()
                    .and_then(|snapshot| {
                        snapshot
                            .get("questions")
                            .and_then(|questions| questions.get(question_id))
                            .and_then(|question| question.get("state"))
                            .and_then(Value::as_str)
                            .map(|status| matches!(status, "pending" | "answered"))
                    })
                    .unwrap_or(false)
            })
        } else {
            false
        };
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
        let stop_gate = if method == "assignment.stop" {
            let assignment_id = required_nonempty_param(&params, "assignment_id")?;
            Some(
                self.runtime
                    .state
                    .assignment_input_gate(&assignment_id)
                    .await,
            )
        } else {
            None
        };
        let inner_result = if method == "takeover.start" {
            self.inner.execution_takeover_start(state, &params).await
        } else if method == "takeover.release" {
            self.inner.execution_takeover_release(state, &params).await
        } else {
            self.inner.call(method, params.clone(), state).await
        };
        drop(stop_gate);
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
                resume_answered_question = true;
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
        if method == "assignment.stop" {
            if let Some(assignment_id) = params.get("assignment_id").and_then(Value::as_str) {
                self.runtime
                    .clear_assignment_cancellation(assignment_id)
                    .await;
            }
        }
        if let Some(assignment_id) = assignment_cancellation_to_clear {
            self.runtime
                .clear_assignment_cancellation(&assignment_id)
                .await;
        }
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
        if method == "chat.send" {
            if let Some(chat_id) = result.pointer("/message/chat_id").and_then(Value::as_str) {
                let scheduler = self.clone();
                let chat_id = chat_id.to_owned();
                tokio::spawn(async move {
                    scheduler.run_user_curator_for_chat(&chat_id, false).await;
                });
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
            // A null-assignment delivery is a per-Bot consumption marker for
            // direct messages and unassigned group messages. It must not make
            // a reply look like an already-routed assignment delivery here.
            let has_assignment_deliveries = deliveries.iter().any(|delivery| {
                delivery
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .is_some()
            });
            let has_queued_deliveries = deliveries.iter().any(|delivery| {
                delivery
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .is_some()
                    && delivery.get("state").and_then(Value::as_str) == Some("queued")
            });
            // Unmentioned queued chat delivery is only a reviewable user
            // message. An explicit @Bot mention, reply, or message.steer can
            // promote a targeted delivery into the same safe continuation path.
            let quote_mode = params
                .get("quote_mode")
                .and_then(Value::as_str)
                .unwrap_or("reply");
            let reply_to = if quote_mode == "reference" {
                None
            } else {
                params
                    .get("quote")
                    .or_else(|| params.get("reply_to"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        result
                            .get("message")
                            .and_then(|message| message.get("reply_to"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .or_else(|| {
                        result
                            .pointer("/message/quote/message_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
            };
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
                                waiting_assignment_for_message(&snapshot, reply_to).or_else(|| {
                                    let assignment_id = snapshot
                                        .get("messages")
                                        .and_then(Value::as_object)
                                        .and_then(|messages| messages.get(reply_to))
                                        .and_then(|message| message.get("assignment_id"))
                                        .and_then(Value::as_str)?;
                                    let status = snapshot
                                        .get("assignments")
                                        .and_then(Value::as_object)
                                        .and_then(|assignments| assignments.get(assignment_id))
                                        .and_then(|assignment| assignment.get("status"))
                                        .and_then(Value::as_str)?;
                                    matches!(
                                        status,
                                        "working"
                                            | "queued"
                                            | "waiting_user"
                                            | "waiting_bot"
                                            | "blocked"
                                    )
                                    .then(|| assignment_id.to_owned())
                                })
                            })
                    })
                });

            // A queued delivery stays reviewable by default, but an explicit
            // @Bot or reply to this assignment's waiting message is an
            // intentional runtime input. Route only those canonical message
            // deliveries into the execution inbox; ordinary unmentioned
            // messages remain queued for review.
            let snapshot = self.inner.orchestrator.snapshot().unwrap_or(Value::Null);
            let targeted_assignments = directed_chat_delivery_assignments(
                result.get("message").unwrap_or(&Value::Null),
                &deliveries,
                reply_to.as_deref(),
                &snapshot,
            );
            if !targeted_assignments.is_empty() {
                let message = result.get("message").cloned().unwrap_or(Value::Null);
                let message_text = message
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| message.get("fallback_text").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned();
                for target_assignment in targeted_assignments {
                    let scheduler = self.clone();
                    let steer_result = json!({
                        "assignment_id": target_assignment,
                        "message": message
                    });
                    let steer_params = json!({
                        "text": message_text
                    });
                    tokio::spawn(async move {
                        scheduler
                            .schedule_assignment_steer(&steer_result, &steer_params)
                            .await;
                    });
                }
            }
            if let (false, Some(assignment_id), Some(text)) = (
                has_assignment_deliveries,
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
                let wait_id = expected_wait_message_id.as_deref();
                if let Some(run_id) = self
                    .waiting_run_for_assignment(&assignment_id, wait_id, &[], Some(false))
                    .await
                    .map_err(|error| crate::rpc_error("internal", &error, None))?
                {
                    let message_id = result
                        .pointer("/message/id")
                        .and_then(Value::as_str)
                        .unwrap_or("chat-reply");
                    self.enqueue_continuation(
                        format!("message-{message_id}"),
                        assignment_id.clone(),
                        run_id,
                        ContinuationAction::Message {
                            assignment_id: assignment_id.clone(),
                            expected_message_id: wait_id.unwrap_or_default().to_owned(),
                            text,
                            steer_message_id: None,
                        },
                    )
                    .await
                    .map_err(|error| crate::rpc_error("internal", &error, None))?;
                    resumed_waiting_message = true;
                } else {
                    let runtime = self.runtime.clone();
                    let scheduler = self.clone();
                    self.mark_waiting(Some(&assignment_id), "", false);
                    resumed_waiting_message = runtime
                        .resume_message_for_decision(&assignment_id, wait_id, text)
                        .await
                        .map_err(|error| crate::rpc_error("internal", &error.to_string(), None))?
                        .map(|(assignment, outcome)| {
                            scheduler.mark_waiting(
                                assignment.as_deref(),
                                &outcome.run_id,
                                matches!(
                                    outcome.status.as_str(),
                                    "waiting" | "blocked" | "suspended"
                                ),
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
        if matches!(method, "assignment.steer" | "steer" | "message.steer") {
            if Self::canonical_steer_text(&result, &params).is_none() {
                return Err(crate::rpc_error(
                    "internal",
                    "steer result is missing canonical message text",
                    None,
                ));
            }
            let scheduler = self.clone();
            let params_for_task = params.clone();
            let result_for_task = result.clone();
            tokio::spawn(async move {
                scheduler
                    .schedule_assignment_steer(&result_for_task, &params_for_task)
                    .await;
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
                    if let Some(assignment_id) = approval_assignment.as_deref() {
                        if let Some(run_id) = scheduler.approved_run_for_assignment(&approval_id, assignment_id).await
                            .map_err(|error| tracing::error!(%approval_id, %error, "approval continuation lookup failed"))
                            .ok().flatten()
                        {
                            if let Err(error) = scheduler
                                .enqueue_continuation(
                                    format!("approval-{approval_id}"),
                                    assignment_id.to_owned(),
                                    run_id,
                                    ContinuationAction::Approval {
                                        approval_id: approval_id.clone(),
                                    },
                                )
                                .await
                            {
                                tracing::error!(%approval_id, %error, "approval continuation could not be queued");
                            }
                        } else {
                            tracing::warn!(%approval_id, "approved receipt has no exact waiting unsafe checkpoint");
                        }
                        return;
                    }
                    if !scheduler.claim_approved_continuation(&approval_id) {
                        tracing::info!(%approval_id, "approval continuation already claimed");
                        return;
                    }
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
                    scheduler.release_approved_continuation(&approval_id);
                    scheduler.dispatch_ready_assignments().await;
                });
            }
        }
        if method == "approval.decide"
            && matches!(
                params.get("decision").and_then(Value::as_str),
                Some("deny" | "reject" | "decline")
            )
        {
            if let Some(approval_id) = params
                .get("approval_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let approval_assignment = approval_assignment_id(self, &approval_id).ok().flatten();
                if approval_assignment.is_none() && self.claim_denied_cancellation(&approval_id) {
                    let runtime = self.runtime.clone();
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        match runtime.cancel_denied_approval(&approval_id).await {
                            Ok(Some(outcome)) => {
                                tracing::info!(
                                    %approval_id,
                                    run_id = %outcome.run_id,
                                    status = %outcome.status,
                                    "cancelled denied private approval"
                                );
                            }
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(%approval_id, %error, "denied private approval cancellation failed")
                            }
                        }
                        scheduler.release_denied_cancellation(&approval_id);
                    });
                }
            }
        }
        let late_answer_queued = if method == "question.answer" && resume_answered_question {
            let question = result.get("question");
            let question_id = question
                .and_then(|question| question.get("id"))
                .and_then(Value::as_str);
            let assignment_id = question
                .and_then(|question| question.get("assignment_id"))
                .and_then(Value::as_str);
            let answer = question.and_then(answered_decision_answer);
            if let (Some(question_id), Some(assignment_id), Some(answer)) =
                (question_id, assignment_id, answer)
            {
                self.enqueue_late_question_answer(assignment_id, question_id, &answer)
                    .await
                    .map_err(|error| crate::rpc_error("internal", &error, None))?
            } else {
                false
            }
        } else {
            false
        };
        if method == "question.answer" && resume_answered_question {
            if let Some(question) = result.get("question") {
                let question_id = question
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let answer = answered_decision_answer(question);
                if let (Some(question_id), Some(answer)) = (question_id, answer) {
                    let question_assignment = question
                        .get("assignment_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let question_payload = question.to_owned();
                    let runtime = self.runtime.clone();
                    let inner = self.inner.clone();
                    let state = self.state.clone();
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        let is_group_assignment =
                            question_assignment.as_deref().is_some_and(|id| {
                                inner
                                    .orchestrator
                                    .snapshot()
                                    .ok()
                                    .and_then(|snapshot| snapshot.get("assignments").cloned())
                                    .and_then(|items| items.get(id).cloned())
                                    .and_then(|assignment| assignment.get("project_id").cloned())
                                    .is_some_and(|project_id| project_id.is_string())
                            });
                        let real_assignment = question_assignment.as_deref().filter(|id| {
                            inner
                                .orchestrator
                                .snapshot()
                                .ok()
                                .and_then(|snapshot| snapshot.get("assignments").cloned())
                                .and_then(|items| items.get(*id).cloned())
                                .is_some()
                        });
                        if is_group_assignment {
                            let question = &question_payload;
                            let Some(assignment_id) =
                                question.get("assignment_id").and_then(Value::as_str)
                            else {
                                return;
                            };
                            let Some(bot_id) = question.get("bot_id").and_then(Value::as_str)
                            else {
                                return;
                            };
                            let Some(chat_id) = question.get("chat_id").and_then(Value::as_str)
                            else {
                                return;
                            };
                            let ack_text = "收到，继续处理。";
                            if let Err(error) = inner
                                .execution_send_msg(
                                    &state,
                                    json!({
                                        "receipt":{"run_id":format!("question-answer-ack-{question_id}"),"call_id":"ack"},
                                        "message":{
                                            "message_id":format!("msg_question_answer_ack_{question_id}"),
                                            "chat_id":chat_id,
                                            "bot_id":bot_id,
                                            "assignment_id":assignment_id,
                                            "intent":"ack",
                                            "text":ack_text,
                                            "mentions":[],
                                            "artifacts":[],
                                            "options":[]
                                        }
                                    }),
                                )
                                .await
                            {
                                tracing::error!(%question_id, %error, "cannot persist group question answer ack; continuation deferred");
                                return;
                            }
                        }
                        if late_answer_queued {
                            return;
                        }
                        if let Some(assignment_id) = real_assignment {
                            let snapshot = inner.orchestrator.snapshot().ok();
                            let message_id = snapshot.as_ref().and_then(|snapshot| {
                                snapshot
                                    .get("messages")
                                    .and_then(Value::as_object)
                                    .and_then(|messages| {
                                        messages
                                            .values()
                                            .find(|message| {
                                                message.get("question_id").and_then(Value::as_str)
                                                    == Some(question_id.as_str())
                                            })
                                            .and_then(|message| message.get("id"))
                                            .and_then(Value::as_str)
                                            .map(str::to_owned)
                                    })
                            });
                            let waiting = if let Some(message_id) = message_id.as_deref() {
                                scheduler
                                    .waiting_run_for_assignment(
                                        assignment_id,
                                        Some(message_id),
                                        &[],
                                        Some(false),
                                    )
                                    .await
                            } else {
                                Ok(None)
                            };
                            let waiting = match waiting {
                                Ok(Some(run_id)) => Some(run_id),
                                Ok(None) => match scheduler
                                    .waiting_run_for_assignment(
                                        assignment_id,
                                        None,
                                        &["ask_user", "question"],
                                        Some(false),
                                    )
                                    .await
                                {
                                    Ok(run_id) => run_id,
                                    Err(error) => {
                                        tracing::error!(%question_id, %error, "question continuation lookup failed");
                                        None
                                    }
                                },
                                Err(error) => {
                                    tracing::error!(%question_id, %error, "question continuation lookup failed");
                                    None
                                }
                            };
                            if let Some(run_id) = waiting {
                                let queued = scheduler
                                    .enqueue_continuation(
                                        format!("question-{question_id}"),
                                        assignment_id.to_owned(),
                                        run_id,
                                        ContinuationAction::Decision {
                                            question_id: question_id.clone(),
                                            answer: answer.clone(),
                                        },
                                    )
                                    .await;
                                if let Err(error) = queued {
                                    tracing::error!(%question_id, %error, "question continuation could not be queued");
                                }
                                return;
                            }
                            if real_assignment.is_some() {
                                tracing::error!(%question_id, "assignment question has no exact safe waiting run; keeping the answered assignment from bypassing scheduler admission");
                                return;
                            }
                        }
                        let expected_run_id = match inner.store.read_snapshot::<Value>(format!(
                            "data/waiting/{}.json",
                            safe_component(&question_id)
                        )) {
                            Ok(Some(marker)) => marker
                                .get("run_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            Ok(None) => None,
                            Err(error) => {
                                tracing::error!(%question_id, %error, "cannot read private question run marker");
                                return;
                            }
                        };
                        match runtime
                            .resume_question_for_run(
                                &question_id,
                                answer,
                                expected_run_id.as_deref(),
                            )
                            .await
                        {
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
                            Ok(None) => {}
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
                    let scheduler = self.clone();
                    tokio::spawn(async move {
                        let waiting = scheduler
                            .waiting_run_for_assignment(
                                &assignment_id,
                                None,
                                &["request_takeover"],
                                Some(true),
                            )
                            .await;
                        match waiting {
                            Ok(Some(run_id)) => {
                                if let Err(error) = scheduler
                                    .enqueue_continuation(
                                        format!("takeover-{assignment_id}"),
                                        assignment_id.clone(),
                                        run_id,
                                        ContinuationAction::Takeover {
                                            assignment_id: assignment_id.clone(),
                                            note: note.clone(),
                                        },
                                    )
                                    .await
                                {
                                    tracing::error!(%assignment_id, %error, "takeover continuation could not be queued");
                                }
                            }
                            Ok(None) => {
                                tracing::error!(%assignment_id, "takeover release has no exact unsafe waiting run; bypass is forbidden");
                            }
                            Err(error) => {
                                tracing::error!(%assignment_id, %error, "takeover continuation lookup failed");
                            }
                        }
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

fn allowed_tool_approval_matches(
    snapshot: &Value,
    approval: &Value,
    mapped_call_id: Option<&str>,
    job: &macbot_durable::Job,
    request: &ExecutionRequest,
    allow_suspended: bool,
) -> bool {
    tool_approval_matches(
        snapshot,
        approval,
        mapped_call_id,
        job,
        request,
        &["allowed_once", "always_allowed"],
        allow_suspended,
    )
}

fn denied_tool_approval_matches(
    snapshot: &Value,
    approval: &Value,
    mapped_call_id: Option<&str>,
    job: &macbot_durable::Job,
    request: &ExecutionRequest,
) -> bool {
    tool_approval_matches(
        snapshot,
        approval,
        mapped_call_id,
        job,
        request,
        &["denied"],
        false,
    )
}

fn tool_approval_matches(
    snapshot: &Value,
    approval: &Value,
    mapped_call_id: Option<&str>,
    job: &macbot_durable::Job,
    request: &ExecutionRequest,
    approval_states: &[&str],
    _allow_suspended: bool,
) -> bool {
    if !approval_states.contains(
        &approval
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ) || job.status != macbot_durable::JobStatus::Waiting
        || !job.unsafe_replay
    {
        return false;
    }
    let Some(call) = job.checkpoint.get("pending_tool") else {
        return false;
    };
    let Some(call_id) = call.get("call_id").and_then(Value::as_str) else {
        return false;
    };
    if mapped_call_id.map(normalize_call_id) != Some(normalize_call_id(call_id)) {
        return false;
    }
    if call.get("name") != approval.get("tool") {
        return false;
    }
    let Some(detail) = approval.get("detail") else {
        return false;
    };
    let detail = detail
        .as_str()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .unwrap_or_else(|| detail.clone());
    let Some(checkpoint_detail) = job.checkpoint.get("approval_detail") else {
        return false;
    };
    if checkpoint_detail != &detail
        || !approval_detail_raw_args_match(
            call.get("args"),
            &detail,
            approval
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        || job
            .checkpoint
            .get("pending_tools")
            .and_then(Value::as_array)
            .is_some_and(|pending| !pending.is_empty() && pending.first() != Some(call))
    {
        return false;
    }
    let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
        return false;
    };
    if job.owner != request.bot_id
        || request.run_id != run_id
        || approval.get("bot_id").and_then(Value::as_str) != Some(request.bot_id.as_str())
        || approval.get("chat_id").and_then(Value::as_str) != Some(request.chat_id.as_str())
        || approval.get("assignment_id").and_then(Value::as_str) != request.assignment_id.as_deref()
    {
        return false;
    }
    if let Some(assignment_id) = request.assignment_id.as_deref() {
        let Some(assignment) = snapshot
            .get("assignments")
            .and_then(Value::as_object)
            .and_then(|assignments| assignments.get(assignment_id))
        else {
            return false;
        };
        let assignment_project = assignment.get("project_id").and_then(Value::as_str);
        if assignment_project != request.project_id.as_deref() {
            return false;
        }
    } else if request.project_id.is_some() {
        return false;
    }
    invalid_tool_approval_request_matches(snapshot, approval, request)
}

fn approval_detail_raw_args_match(call_args: Option<&Value>, detail: &Value, tool: &str) -> bool {
    let Some(call_args) = call_args else {
        return false;
    };
    let mut raw = detail.clone();
    if let Some(object) = raw.as_object_mut() {
        if matches!(tool, "read" | "write" | "edit") {
            object.remove("resolved_path");
            object.remove("path_resolution");
        }
        if tool == "skill_draft" {
            object.remove("effective_target");
        }
    }
    &raw == call_args
}

fn bot_tool_allowlist(
    bot: Option<&Value>,
    is_main: bool,
    private: bool,
    has_assignment: bool,
    web_search_configured: bool,
) -> Option<Vec<String>> {
    let config = bot.and_then(|value| value.get("tools").and_then(Value::as_object));
    let mut names = if is_main {
        vec!["question"]
    } else {
        vec![
            "skill",
            "memory",
            "memory_search",
            "session_search",
            "project_find",
            "chat_history",
            "question",
        ]
    };
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
    if !is_main
        && web_search_configured
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
        ]);
        if private && !has_assignment {
            names.push("subagent");
        }
    } else if config
        .and_then(|value| value.get("subagent"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        names.push("subagent");
    }
    // Coordination may begin in the main DM and report into the newly
    // created project. Subagents use their own restricted tool list.
    names.push("send_msg");
    Some(names.into_iter().map(str::to_owned).collect())
}

const MAIN_ANALYSIS_SUBAGENT_TOOLS: &[&str] = &["read", "ls", "find", "grep"];

/// Recompute persisted main-Bot tool capabilities from current durable identity
/// and scope before resuming any old checkpoint. Historical `request.tools`
/// values are not an authority after the main-Bot capability policy changes.
fn tighten_request_tools_for_current_policy(
    snapshot: &Value,
    request: &mut ExecutionRequest,
    web_search_configured: bool,
) -> bool {
    let bots = snapshot.get("bots").and_then(Value::as_object);
    let bot = bots.and_then(|bots| bots.get(&request.bot_id));
    let Some(bot) = bot else {
        return false;
    };
    let is_main = bot
        .get("is_main")
        .and_then(Value::as_bool)
        .unwrap_or(request.bot_id == "main");
    if !is_main {
        return true;
    }
    if request.subagent {
        request.tools = Some(
            MAIN_ANALYSIS_SUBAGENT_TOOLS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        );
        return true;
    }

    let main_dm_chat = bot
        .get("dm_chat_id")
        .and_then(Value::as_str)
        .unwrap_or("chat_main");
    let is_project_chat = snapshot
        .get("projects")
        .and_then(Value::as_object)
        .is_some_and(|projects| {
            projects.values().any(|project| {
                project.get("chat_id").and_then(Value::as_str) == Some(request.chat_id.as_str())
            })
        });
    let private_dm = request.private && !is_project_chat && request.chat_id == main_dm_chat;
    let coordinator_phase = request
        .phase
        .as_deref()
        .is_none_or(|phase| phase == "coordinate");
    let can_use_private_subagent = private_dm
        && request.assignment_id.is_none()
        && request.project_id.is_none()
        && !request.routine
        && coordinator_phase;
    let had_profile_onboarding = request
        .tools
        .as_ref()
        .is_some_and(|tools| tools.iter().any(|tool| tool == "set_user_profile"));
    request.tools = bot_tool_allowlist(
        Some(bot),
        true,
        can_use_private_subagent,
        request.assignment_id.is_some(),
        web_search_configured,
    );
    if had_profile_onboarding && can_use_private_subagent {
        request
            .tools
            .get_or_insert_with(Vec::new)
            .push("set_user_profile".into());
    }
    request.tools.is_some()
}

/// Private Bots may search projects they belong to, while the generic feature
/// tool intentionally limits non-main runs to the current project.  This
/// runtime-owned bridge supplies the broader, still membership-filtered view
/// without weakening the feature service's write authorization.
struct ProjectFindToolBridge {
    runtime: FeatureToolRuntime,
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
            "properties":{
                "query":{"type":"string"},
                "project_id":{"type":"string","description":"Optional id from a returned project result to load its memory into the current private chat."}
            }
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
        let result = (|| -> Result<Value, FeatureError> {
            let bot_id = self.runtime.run.actor.bot_id().ok_or_else(|| {
                FeatureError::NotAllowed("project search requires a Bot actor".into())
            })?;
            let user_id = self
                .runtime
                .run
                .access
                .user_id
                .clone()
                .unwrap_or_else(|| "local-user".into());
            let mut projects: Vec<ProjectRecord> = Vec::new();
            for project_id in &self.visible_project_ids {
                let access = MemoryAccess::group_for_user(
                    user_id.clone(),
                    project_id.clone(),
                    vec![bot_id.to_owned()],
                );
                for project in self.runtime.service.project_find_for_access(
                    &self.runtime.run.actor,
                    &access,
                    query,
                )? {
                    if !projects.iter().any(|found| found.id == project.id) {
                        projects.push(project);
                    }
                }
            }
            let selected = args.get("project_id").and_then(Value::as_str);
            if let Some(project_id) = selected {
                if !projects.iter().any(|project| project.id == project_id) {
                    return Err(FeatureError::NotAllowed(
                        "project id must be selected from authorized project_find results".into(),
                    ));
                }
                if !self.runtime.run.private {
                    return Err(FeatureError::NotAllowed(
                        "project memory can only be loaded into a private chat".into(),
                    ));
                }
                self.runtime.select_project_id(project_id.to_owned())?;
            }
            Ok(json!({"projects":projects,"selected_project_id":selected}))
        })();
        match result {
            Ok(details) => ToolResult {
                content: vec![Part::Text {
                    text: details.to_string(),
                }],
                details,
                is_error: false,
            },
            Err(error) => ToolResult::error(error.to_string()),
        }
    }
}

impl RuntimeExecution {
    /// Validate a worker's effective model reference before a delegation
    /// creates a group or dispatch message. This checks catalog membership,
    /// enabled state, and provider configuration without making a paid call.
    pub async fn preflight_bot_model(&self, bot_id: &str) -> Result<String, String> {
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| error.to_string())?;
        let settings = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .map_err(|error| error.to_string())?
            .unwrap_or(Value::Null);
        let model = resolve_model(&snapshot, &settings, bot_id, ModelRole::Bot)
            .filter(|model| !model.trim().is_empty())
            .ok_or_else(|| format!("Bot {bot_id} has no configured model"))?;
        self.provider_resolver
            .providers
            .lock()
            .await
            .resolve_model(&model)
            .map_err(|error| format!("cannot dispatch to Bot {bot_id}: {error}"))?;
        Ok(model)
    }

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
            cancelling_assignments: Arc::new(Mutex::new(HashSet::new())),
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
        let mut curator_provider: Option<Arc<dyn UserMemoryCuratorProvider>> = None;
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
            let adapter = Arc::new(
                ModelMaintenanceAdapter::new(provider, catalog.model_id)
                    .with_usage_sink(Arc::new(RuntimeMaintenanceUsage {
                        usage: self.usage.clone(),
                    }))
                    .with_usage_context(context),
            );
            curator_provider = Some(adapter.clone() as Arc<dyn UserMemoryCuratorProvider>);
            Some(adapter as Arc<dyn macbot_memory::AsyncMaintenanceProvider>)
        } else {
            None
        };
        self.feature_service
            .configure_runtime(extra_dirs, provider)
            .map_err(|error| RuntimeError::Provider(error.to_string()))?;
        self.feature_service
            .set_user_memory_curator_provider(curator_provider)
            .map_err(|error| RuntimeError::Provider(error.to_string()))
    }

    pub async fn run(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, RuntimeError> {
        self.configure_feature_runtime().await?;
        let mut request = request;
        self.tighten_request_tools(&mut request)?;
        let _admission = if let Some(assignment_id) = request.assignment_id.as_deref() {
            let cancelling = self.cancelling_assignments.lock().await;
            let snapshot = self
                .backend
                .orchestrator
                .snapshot()
                .map_err(|error| RuntimeError::Orchestrator(error.to_string()))?;
            let status = snapshot
                .get("assignments")
                .and_then(Value::as_object)
                .and_then(|assignments| assignments.get(assignment_id))
                .and_then(|assignment| assignment.get("status"))
                .and_then(Value::as_str);
            if status.is_none() || matches!(status, Some("done" | "failed" | "cancelled")) {
                return Ok(ExecutionOutcome {
                    run_id: request.run_id,
                    job_id: String::new(),
                    status: status.unwrap_or("cancelled").to_owned(),
                    text: String::new(),
                    usage: TokenUsage::default(),
                    turns: 0,
                });
            }
            if cancelling.contains(assignment_id) {
                return Ok(ExecutionOutcome {
                    run_id: request.run_id,
                    job_id: String::new(),
                    status: "cancelled".into(),
                    text: String::new(),
                    usage: TokenUsage::default(),
                    turns: 0,
                });
            }
            self.persist_request(&request)?;
            Some(cancelling)
        } else {
            self.persist_request(&request)?;
            None
        };
        drop(_admission);
        Ok(self
            .configured_engine_for(&request)
            .await?
            .run(request)
            .await?)
    }

    fn tighten_request_tools(&self, request: &mut ExecutionRequest) -> Result<(), RuntimeError> {
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| RuntimeError::Orchestrator(error.to_string()))?;
        if !tighten_request_tools_for_current_policy(&snapshot, request, false) {
            return Err(RuntimeError::Orchestrator(format!(
                "cannot revalidate tool policy for Bot {}",
                request.bot_id
            )));
        }
        Ok(())
    }

    /// Cancel the durable execution owned by an assignment.  Assignment.stop
    /// is a public orchestrator mutation, but cancellation must also reach the
    /// engine so foreground tools receive the run-scoped cancellation signal
    /// and background process groups are cleaned up.
    pub async fn cancel_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<Option<ExecutionOutcome>, RuntimeError> {
        {
            let _input_gate = self.state.assignment_input_gate(assignment_id).await;
            self.cancelling_assignments
                .lock()
                .await
                .insert(assignment_id.to_owned());
        }
        let requests =
            self.find_requests(|request| request.assignment_id.as_deref() == Some(assignment_id))?;
        if requests.is_empty() {
            return Ok(None);
        }

        // Parent runs and their subagent runs intentionally share one
        // assignment_id. Cancel every persisted run so a filesystem iteration
        // order can never leave a provider call running after assignment.stop.
        // Try all runs even if one cancellation fails, then report the first
        // error to the caller.
        let mut outcome = None;
        let mut first_error = None;
        for request in requests {
            let result = match self.engine_for(&request, None) {
                Ok(engine) => engine.cancel(&request).await.map_err(RuntimeError::from),
                Err(error) => Err(error),
            };
            match result {
                Ok(Some(cancelled)) if outcome.is_none() => outcome = Some(cancelled),
                Ok(_) => {}
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(outcome)
    }

    async fn clear_assignment_cancellation(&self, assignment_id: &str) {
        self.cancelling_assignments
            .lock()
            .await
            .remove(assignment_id);
    }

    async fn assignment_cancellation_in_progress(&self, assignment_id: &str) -> bool {
        self.cancelling_assignments
            .lock()
            .await
            .contains(assignment_id)
    }

    /// Cancel a denied private approval only when its approval map, waiting
    /// checkpoint, and persisted request all identify the same run. This is
    /// intentionally separate from approved continuation and never executes
    /// the pending tool or asks the model for another turn.
    async fn cancel_denied_approval(
        &self,
        approval_id: &str,
    ) -> Result<Option<ExecutionOutcome>, RuntimeError> {
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| RuntimeError::Orchestrator(error.to_string()))?;
        let Some(approval) = snapshot
            .get("approvals")
            .and_then(Value::as_object)
            .and_then(|approvals| approvals.get(approval_id))
        else {
            return Ok(None);
        };
        if approval.get("state").and_then(Value::as_str) != Some("denied")
            || approval
                .get("assignment_id")
                .is_some_and(|value| !value.is_null())
        {
            return Ok(None);
        }
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
        let Some(mapped_call_id) = mapped_call_id else {
            return Ok(None);
        };
        let jobs = self
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        let mut candidate = None;
        for job in jobs {
            if !matches!(job.status, macbot_durable::JobStatus::Waiting) {
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
            if request.assignment_id.is_some()
                || !denied_tool_approval_matches(
                    &snapshot,
                    approval,
                    Some(mapped_call_id.as_str()),
                    &job,
                    &request,
                )
            {
                continue;
            }
            if candidate.is_some() {
                tracing::warn!(%approval_id, "denied approval matched multiple private waiting jobs");
                return Ok(None);
            }
            candidate = Some(request);
        }
        let Some(request) = candidate else {
            return Ok(None);
        };
        self.engine
            .cancel(&request)
            .await
            .map_err(RuntimeError::from)
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

    /// Add the coordinator's final summary after a user has already confirmed
    /// the project. This path never changes project status or cancels work.
    pub(crate) async fn finalize_confirmed_project_for_coordination(
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
        if project.get("status").and_then(Value::as_str) != Some("done") {
            return Err(crate::rpc_error(
                "conflict",
                "the user must confirm project completion before the main Bot summarizes it",
                Some(json!({
                    "project_id":project_id,
                    "status":project.get("status")
                })),
            ));
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
            .call(
                "project.get",
                json!({"project_id":project_id}),
                &self.gateway_state,
            )
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
        self.resume_approved_inner(approval_id, None, true).await
    }

    /// Recovery-only approval continuation.  The scan already proved an
    /// untouched Waiting checkpoint; re-check it immediately before entering
    /// the engine so a scan-to-run state change cannot turn recovery into a
    /// replay of a Suspended unsafe call.
    async fn resume_approved_waiting(
        &self,
        approval_id: &str,
        expected_run_id: &str,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.resume_approved_inner(approval_id, Some(expected_run_id), false)
            .await
    }

    async fn resume_approved_inner(
        &self,
        approval_id: &str,
        expected_run_id: Option<&str>,
        allow_suspended: bool,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.configure_feature_runtime().await?;
        let snapshot = self
            .backend
            .orchestrator
            .snapshot()
            .map_err(|error| RuntimeError::Orchestrator(error.to_string()))?;
        let Some(approval) = snapshot
            .get("approvals")
            .and_then(Value::as_object)
            .and_then(|approvals| approvals.get(approval_id))
        else {
            return Ok(None);
        };
        if !matches!(
            approval.get("state").and_then(Value::as_str),
            Some("allowed_once" | "always_allowed")
        ) {
            return Ok(None);
        }
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
        let jobs = self
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        for job in jobs {
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            if expected_run_id.is_some_and(|expected| expected != run_id) {
                continue;
            }
            let Some(mut request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                tracing::warn!(%run_id, "approval has no persisted execution request");
                return Ok(None);
            };
            self.tighten_request_tools(&mut request)?;
            if !allowed_tool_approval_matches(
                &snapshot,
                approval,
                mapped_call_id.as_deref(),
                &job,
                &request,
                allow_suspended,
            ) {
                continue;
            }
            {
                let durable = self.state.durable.lock().await;
                let still_current = durable.jobs().any(|current| {
                    current.id == job.id
                        && current.status == job.status
                        && current.unsafe_replay == job.unsafe_replay
                        && current.checkpoint == job.checkpoint
                        && expected_run_id.is_none_or(|expected| {
                            current.checkpoint.get("run_id").and_then(Value::as_str)
                                == Some(expected)
                        })
                        && allowed_tool_approval_matches(
                            &snapshot,
                            approval,
                            mapped_call_id.as_deref(),
                            current,
                            &request,
                            allow_suspended,
                        )
                });
                if !still_current {
                    return Ok(None);
                }
            }
            let assignment_id = request.assignment_id.clone();
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
        self.resume_question_for_run(question_id, answer, None)
            .await
    }

    async fn resume_question_for_run(
        &self,
        question_id: &str,
        answer: String,
        expected_run_id: Option<&str>,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        if let Some(result) = self
            .resume_decision_question(question_id, &answer, expected_run_id)
            .await?
        {
            return Ok(Some(result));
        }
        self.resume_waiting_assignment(
            question_id,
            &["ask_user", "question"],
            WaitingContinuation::Question(answer),
            expected_run_id,
        )
        .await
    }

    async fn resume_decision_question(
        &self,
        question_id: &str,
        answer: &str,
        expected_run_id: Option<&str>,
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
        let jobs = self
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        for job in jobs {
            if !matches!(
                job.status,
                macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
            ) || !waiting_checkpoint_safety_matches(&job, Some(false))
                || !decision_checkpoint_matches_question(&job.checkpoint, &message_id)
            {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            if expected_run_id.is_some_and(|expected| expected != run_id) {
                continue;
            }
            let Some(mut request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                continue;
            };
            self.tighten_request_tools(&mut request)?;
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
            if !self.state.durable.lock().await.jobs().any(|current| {
                current.id == job.id
                    && current.status == job.status
                    && current.unsafe_replay == job.unsafe_replay
                    && current.checkpoint == job.checkpoint
                    && waiting_checkpoint_safety_matches(current, Some(false))
                    && decision_checkpoint_matches_question(&current.checkpoint, &message_id)
            }) {
                continue;
            }
            let assignment = request.assignment_id.clone();
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
        self.resume_takeover_for_run(assignment_id, note, None)
            .await
    }

    async fn resume_takeover_for_run(
        &self,
        assignment_id: &str,
        note: Option<String>,
        expected_run_id: Option<&str>,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.resume_waiting_assignment(
            assignment_id,
            &["request_takeover"],
            WaitingContinuation::Takeover(note),
            expected_run_id,
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
        self.resume_message_for_decision_inner(
            assignment_id,
            expected_message_id,
            message,
            None,
            None,
        )
        .await
    }

    async fn resume_message_for_decision_inner(
        &self,
        assignment_id: &str,
        expected_message_id: Option<&str>,
        message: String,
        steer_message_id: Option<&str>,
        expected_run_id: Option<&str>,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        let Some(expected_message_id) = expected_message_id.filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
        self.configure_feature_runtime().await?;
        let jobs = self
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        for job in jobs {
            if !matches!(
                job.status,
                macbot_durable::JobStatus::Waiting | macbot_durable::JobStatus::Suspended
            ) || !waiting_checkpoint_safety_matches(&job, Some(false))
                || !waiting_checkpoint_matches_message(&job.checkpoint, expected_message_id)
            {
                continue;
            }
            let Some(run_id) = job.checkpoint.get("run_id").and_then(Value::as_str) else {
                continue;
            };
            if expected_run_id.is_some_and(|expected| expected != run_id) {
                continue;
            }
            let Some(mut request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                continue;
            };
            self.tighten_request_tools(&mut request)?;
            if request.assignment_id.as_deref() != Some(assignment_id) {
                continue;
            }
            if !self.state.durable.lock().await.jobs().any(|current| {
                current.id == job.id
                    && current.status == job.status
                    && current.unsafe_replay == job.unsafe_replay
                    && current.checkpoint == job.checkpoint
                    && waiting_checkpoint_safety_matches(current, Some(false))
                    && waiting_checkpoint_matches_message(&current.checkpoint, expected_message_id)
            }) {
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
        expected_run_id: Option<&str>,
    ) -> Result<Option<(Option<String>, ExecutionOutcome)>, RuntimeError> {
        self.configure_feature_runtime().await?;
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
        let jobs = self
            .state
            .durable
            .lock()
            .await
            .jobs()
            .cloned()
            .collect::<Vec<_>>();
        for job in jobs {
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
            if expected_run_id.is_some_and(|expected| expected != run_id) {
                continue;
            }
            let Some(mut request) = self
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .map_err(|error| RuntimeError::Execution(error.into()))?
            else {
                continue;
            };
            self.tighten_request_tools(&mut request)?;
            let assignment_matches = request.assignment_id.as_deref() == Some(marker_or_assignment)
                || question_assignment.as_deref() == request.assignment_id.as_deref()
                || (request.assignment_id.is_none()
                    && format!("dm_{}", safe_component(&request.chat_id)) == marker_or_assignment);
            let marker = self
                .store
                .read_snapshot::<Value>(format!(
                    "data/waiting/{}.json",
                    safe_component(marker_or_assignment)
                ))
                .map_err(|error| RuntimeError::Execution(error.into()))?;
            let marker_matches = marker.is_some_and(|marker| {
                marker.get("run_id").and_then(Value::as_str) == Some(run_id)
                    || marker.get("assignment_id").and_then(Value::as_str)
                        == request.assignment_id.as_deref()
            });
            if !assignment_matches && !marker_matches {
                continue;
            }
            if !self.state.durable.lock().await.jobs().any(|current| {
                current.id == job.id
                    && current.status == job.status
                    && current.unsafe_replay == job.unsafe_replay
                    && current.checkpoint == job.checkpoint
                    && current
                        .checkpoint
                        .get("pending_tool")
                        .and_then(|tool| tool.get("name"))
                        .and_then(Value::as_str)
                        .is_some_and(|name| pending_tools.contains(&name))
            }) {
                continue;
            }
            let assignment = request.assignment_id.clone();
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
            .await?;
        self.backend
            .reconcile_browser_takeover(&self.gateway_state, bot_id)
            .await
            .map_err(|error| error.to_string())
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

    fn context_metadata(
        &self,
        request: &ExecutionRequest,
    ) -> (usize, String, String, Vec<String>, Option<SkillDraftFrom>) {
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
        let draft_from = Some(SkillDraftFrom {
            bot_id: request.bot_id.clone(),
            bot_name: bot
                .and_then(|bot| bot.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(&request.bot_id)
                .to_owned(),
            project_id: request.project_id.clone(),
            project_name: project
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned),
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
            draft_from,
        )
    }

    fn feature_access(&self, request: &ExecutionRequest) -> MemoryAccess {
        let Some(project_id) = request.project_id.as_deref() else {
            return MemoryAccess::user("local-user");
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
        MemoryAccess::group_for_user("local-user", project_id.to_owned(), members)
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
        let mut messages = Vec::new();
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
            messages.push(SessionMessage {
                id,
                session_id: request.chat_id.clone(),
                chat_id: request.chat_id.clone(),
                project_id: request.project_id.clone(),
                role: role.to_owned(),
                content: content.to_owned(),
                at: Utc::now(),
            });
        }
        if let Err(error) = self.feature_service.add_session_messages(messages) {
            tracing::warn!(chat_id = %request.chat_id, %error, "failed to sync recent feature messages");
        }
    }

    fn explicit_skill_source_message(
        &self,
        request: &ExecutionRequest,
        skill_name: &str,
    ) -> Option<String> {
        let messages = self.backend.load_chat_messages(&request.chat_id).ok()?;
        explicit_skill_source_message_from(
            &messages,
            request.source_message_id.as_deref(),
            skill_name,
        )
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
        let mut loaded_skill = None;
        let explicit_skill = match self
            .feature_service
            .load_skill_for_text(&request.instruction, Some(&request.bot_id))
        {
            Ok(Some((detail, instruction))) => {
                let loaded_at = detail
                    .skill
                    .last_invoked_at
                    .unwrap_or_else(Utc::now)
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
                loaded_skill = Some((detail.skill.name, loaded_at));
                Some((detail.content, instruction))
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(bot_id = %request.bot_id, %error, "explicit skill invocation could not be loaded");
                None
            }
        };
        let engine = self.engine_for(request, explicit_skill)?;
        if let Some((skill_name, loaded_at)) = loaded_skill {
            if let Some(message_id) = self.explicit_skill_source_message(request, &skill_name) {
                let record = json!({
                    "name":skill_name,
                    "bot_id":request.bot_id,
                    "assignment_id":request.assignment_id,
                    "loaded_at":loaded_at
                });
                if let Err(error) = self
                    .backend
                    .record_loaded_skill(&self.gateway_state, &request.chat_id, &message_id, record)
                    .await
                {
                    tracing::warn!(%error, %message_id, "could not persist explicit skill load on source message");
                }
            } else {
                tracing::warn!(chat_id = %request.chat_id, bot_id = %request.bot_id, "explicit skill loaded without a source user message");
            }
        }
        Ok(engine)
    }

    fn engine_for(
        &self,
        request: &ExecutionRequest,
        explicit_skill: Option<(String, String)>,
    ) -> Result<ExecutionEngine, RuntimeError> {
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
        let draft_from = context_metadata.4.clone();
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
        context.draft_from = draft_from;
        context.run_id = Some(request.run_id.clone());
        context.private = request.private;
        context.project_id = request.project_id.clone();
        context.session_id = Some(request.chat_id.clone());
        context.chat_id = Some(request.chat_id.clone());
        context.profile_onboarding = is_main
            && request.private
            && request.assignment_id.is_none()
            && request.project_id.is_none()
            && !request.routine
            && request
                .tools
                .as_ref()
                .is_some_and(|tools| tools.iter().any(|tool| tool == "set_user_profile"));
        context.host_timezone = self
            .store
            .read_snapshot::<Value>("data/settings.json")
            .ok()
            .flatten()
            .and_then(|settings| {
                settings
                    .get("timezone")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        // Main Bot model runs do not receive memory/skill tools. Profile
        // onboarding remains a separately authorized capability.
        context.allow_memory = !is_main && !request.subagent;
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
        if !is_main {
            if let Ok(skills) = self.feature_service.skill_registry.read() {
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
                    .map(|skill| {
                        format!("/{0}: {1} ({2})", skill.name, skill.description, skill.path)
                    })
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
        }
        if let Some((content, instruction)) = explicit_skill {
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
                runtime: feature_runtime.clone(),
                visible_project_ids: self.visible_project_ids(request, is_main),
            }));
        }
        let identity = CollaborationIdentity {
            bot_id: request.bot_id.clone(),
            chat_id: request.chat_id.clone(),
            assignment_id: request.assignment_id.clone(),
            source_message_id: request.source_message_id.clone(),
            project_id: request.project_id.clone(),
            is_main,
            private: request.private,
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
            // the injected encrypted file secret backend at request time.
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

    async fn enqueue_input_for_run(&self, run_id: &str, message_id: &str, text: &str) -> bool {
        let execution_state = self.engine.state();
        let mut durable = execution_state.durable.lock().await;
        let Some(job) = durable
            .jobs()
            .find(|job| {
                job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id)
                    && !matches!(
                        job.status,
                        macbot_durable::JobStatus::Done
                            | macbot_durable::JobStatus::Failed
                            | macbot_durable::JobStatus::Cancelled
                    )
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
            let bytes = fs::read(entry.path()).ok()?;
            let request = serde_json::from_slice::<ExecutionRequest>(&bytes).ok()?;
            predicate(&request).then_some(request)
        })
    }

    fn find_requests<F>(&self, predicate: F) -> Result<Vec<ExecutionRequest>, RuntimeError>
    where
        F: Fn(&ExecutionRequest) -> bool,
    {
        read_matching_requests(&self.home, predicate)
    }
}

fn read_matching_requests<F>(
    home: &std::path::Path,
    predicate: F,
) -> Result<Vec<ExecutionRequest>, RuntimeError>
where
    F: Fn(&ExecutionRequest) -> bool,
{
    let dir = home.join("data/run_requests");
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(RuntimeError::Execution(ExecutionError::Durable(
                error.into(),
            )))
        }
    };
    let mut requests = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("json")
        {
            continue;
        }
        let bytes = fs::read(entry.path())
            .map_err(|error| RuntimeError::Execution(ExecutionError::Durable(error.into())))?;
        let request = serde_json::from_slice::<ExecutionRequest>(&bytes).map_err(|error| {
            RuntimeError::Execution(ExecutionError::Durable(
                macbot_durable::DurableError::Invalid(error.to_string()),
            ))
        })?;
        if predicate(&request) {
            requests.push(request);
        }
    }
    // Stop parent runs first to prevent them from dispatching another child;
    // the assignment admission barrier blocks new family runs while this
    // persisted family snapshot is cancelled.
    requests.sort_by(|left, right| {
        left.subagent
            .cmp(&right.subagent)
            .then_with(|| left.run_id.cmp(&right.run_id))
    });
    Ok(requests)
}

#[cfg(test)]
mod assignment_run_request_tests {
    use super::{read_matching_requests, ComposedBackend, ProductionBackend};
    use crate::execution::ExecutionRequest;
    use crate::{Gateway, GatewayConfig, RpcBackend};
    use axum::routing::post;
    use axum::Json;
    use serde_json::{json, Value};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::{Notify, Semaphore};

    fn request(run_id: &str, assignment_id: Option<&str>, subagent: bool) -> ExecutionRequest {
        ExecutionRequest {
            run_id: run_id.into(),
            assignment_id: assignment_id.map(str::to_owned),
            source_message_id: None,
            chat_id: "chat-test".into(),
            bot_id: "bot-test".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "test".into(),
            messages: Vec::new(),
            max_turns: 1,
            private: false,
            allow_unsafe: false,
            cwd: None,
            routine: false,
            price: None,
            resume_approved: false,
            continuation_id: None,
            continuation_payload: None,
            subagent,
            tools: None,
            phase: None,
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
            client_locale: None,
        }
    }

    #[test]
    fn assignment_run_lookup_returns_parent_and_children_only_for_that_assignment() {
        let home = tempdir().unwrap();
        let dir = home.path().join("data/run_requests");
        fs::create_dir_all(&dir).unwrap();
        for request in [
            request("parent", Some("asg-target"), false),
            request("child-b", Some("asg-target"), true),
            request("child-a", Some("asg-target"), true),
            request("other-assignment", Some("asg-other"), true),
        ] {
            fs::write(
                dir.join(format!("{}.json", request.run_id)),
                serde_json::to_vec(&request).unwrap(),
            )
            .unwrap();
        }

        let matches = read_matching_requests(home.path(), |request| {
            request.assignment_id.as_deref() == Some("asg-target")
        })
        .unwrap();
        assert_eq!(
            matches
                .iter()
                .map(|request| request.run_id.as_str())
                .collect::<Vec<_>>(),
            ["parent", "child-a", "child-b"]
        );
    }

    #[test]
    fn assignment_run_lookup_propagates_corrupt_request_files() {
        let home = tempdir().unwrap();
        let dir = home.path().join("data/run_requests");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("broken.json"), b"not-json").unwrap();

        assert!(read_matching_requests(home.path(), |_| true).is_err());
    }

    #[tokio::test]
    async fn assignment_cancel_stops_parent_and_subagent_provider_runs_before_late_results() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(home.path()).unwrap());
        let bot = backend
            .call(
                "bot.create",
                json!({"name":"cancel family worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let calls_for_provider = provider_calls.clone();
        let all_provider_calls_started = Arc::new(Notify::new());
        let notify_provider_calls = all_provider_calls_started.clone();
        let release_provider = Arc::new(Semaphore::new(0));
        let release_for_provider = release_provider.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let calls = calls_for_provider.clone();
                let notify = notify_provider_calls.clone();
                let release = release_for_provider.clone();
                async move {
                    let response = if body.to_string().contains("unrelated assignment run") {
                        "unrelated provider result"
                    } else {
                        "late provider result"
                    };
                    if calls.fetch_add(1, Ordering::SeqCst) + 1 >= 2 {
                        notify.notify_one();
                    }
                    release.acquire().await.unwrap().forget();
                    (
                        [("content-type", "text/event-stream")],
                        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{response}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n\ndata: [DONE]\n\n"),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider_server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = backend
            .call(
                "provider.create",
                json!({
                    "name":"assignment cancellation provider",
                    "api_kind":"openai-completions",
                    "base_url":format!("http://{address}/v1"),
                    "client_request_id":"assignment-cancel-provider"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let provider_id = provider["provider"]["id"].as_str().unwrap().to_owned();
        let model = backend
            .call(
                "model.upsert",
                json!({
                    "provider_id":provider_id,
                    "model_id":"assignment-cancel-model",
                    "client_request_id":"assignment-cancel-model"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let model_ref = model["model"]["ref"].as_str().unwrap().to_owned();
        let project = backend
            .call(
                "project.create",
                json!({
                    "name":"cancel family project",
                    "goal":"exercise assignment family cancellation",
                    "member_bot_ids":[bot_id]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["chat"]["id"],
                    "bot_id":bot_id,
                    "title":"cancel parent and child",
                    "instruction":"wait for cancellation",
                    "from":"user"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap().to_owned();
        let other_assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["chat"]["id"],
                    "bot_id":bot_id,
                    "title":"unrelated assignment",
                    "instruction":"unrelated",
                    "from":"user"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let other_assignment_id = other_assignment["id"].as_str().unwrap().to_owned();
        let not_started_assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project["project"]["id"],
                    "origin_chat_id":project["chat"]["id"],
                    "bot_id":bot_id,
                    "title":"not started assignment",
                    "instruction":"cancel before run request",
                    "from":"user"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let not_started_assignment_id = not_started_assignment["id"].as_str().unwrap().to_owned();
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["assignments"][&assignment_id]["status"] = json!("working");
        snapshot["assignments"][&other_assignment_id]["status"] = json!("working");
        snapshot["assignments"][&not_started_assignment_id]["status"] = json!("working");
        backend.orchestrator.restore(snapshot).unwrap();
        let dispatch_gate = Arc::new(Notify::new());
        let composed = ComposedBackend::open_with_dispatch_gate(
            backend.clone(),
            gateway.state.clone(),
            home.path(),
            dispatch_gate,
        )
        .unwrap();
        let runtime = Arc::new(composed.runtime.clone());
        let make_request = |run_id: &str, subagent: bool| ExecutionRequest {
            run_id: run_id.into(),
            assignment_id: Some(assignment_id.clone()),
            source_message_id: None,
            chat_id: chat_id.clone(),
            bot_id: bot_id.clone(),
            model: model_ref.clone(),
            provider_id: provider_id.clone(),
            project_id: None,
            instruction: "wait for cancellation".into(),
            messages: Vec::new(),
            max_turns: 1,
            private: false,
            allow_unsafe: false,
            cwd: None,
            routine: false,
            price: None,
            resume_approved: false,
            continuation_id: None,
            continuation_payload: None,
            subagent,
            tools: None,
            phase: None,
            parent_run_id: subagent.then(|| "run-parent".into()),
            subagent_task: subagent.then(|| "child task".into()),
            save_full_requests: false,
            resume_message: None,
            client_locale: None,
        };
        let not_started_stop = composed
            .call(
                "assignment.stop",
                json!({
                    "assignment_id":not_started_assignment_id,
                    "client_request_id":"cancel-before-run-request"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(not_started_stop["assignment"]["status"], "cancelled");
        let mut racing_request = make_request("run-cancelled-before-admission", false);
        racing_request.assignment_id = Some(not_started_assignment_id.clone());
        assert_eq!(
            runtime.run(racing_request).await.unwrap().status,
            "cancelled"
        );
        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert!(!home
            .path()
            .join("data/run_requests/run-cancelled-before-admission.json")
            .exists());
        let parent_request = make_request("run-parent", false);
        let child_request = make_request("run-child", true);
        let parent_runtime = runtime.clone();
        let mut parent = tokio::spawn(async move { parent_runtime.run(parent_request).await });
        let child_runtime = runtime.clone();
        let mut child = tokio::spawn(async move { child_runtime.run(child_request).await });

        let provider_started = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while provider_calls.load(Ordering::SeqCst) < 2 {
                all_provider_calls_started.notified().await;
            }
        })
        .await;
        if provider_started.is_err() {
            let parent_error = tokio::time::timeout(std::time::Duration::from_secs(1), &mut parent)
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(Result::err);
            let child_error = tokio::time::timeout(std::time::Duration::from_secs(1), &mut child)
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(Result::err);
            panic!("provider calls did not start; parent={parent_error:?}, child={child_error:?}");
        }
        let parent_job_id = runtime
            .state
            .durable
            .lock()
            .await
            .jobs()
            .find(|job| job.checkpoint.get("run_id").and_then(Value::as_str) == Some("run-parent"))
            .expect("parent durable job")
            .id
            .clone();
        let parent_snapshot = home.path().join(format!("data/jobs/{parent_job_id}.json"));
        fs::remove_file(&parent_snapshot).unwrap();
        fs::create_dir(&parent_snapshot).unwrap();
        let stop_params = json!({
            "assignment_id":assignment_id,
            "client_request_id":"cancel-family-stop"
        });
        let first_stop_error = composed
            .call("assignment.stop", stop_params.clone(), &gateway.state)
            .await
            .unwrap_err();
        assert!(
            first_stop_error.message.contains("snapshot"),
            "{first_stop_error}"
        );
        assert!(
            runtime
                .assignment_cancellation_in_progress(&assignment_id)
                .await
        );
        assert_ne!(
            backend.orchestrator.snapshot().unwrap()["assignments"][&assignment_id]["status"],
            "cancelled",
            "a durable snapshot error must leave assignment.stop retryable"
        );
        assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
        let blocked_late_child = make_request("run-failed-stop-late-child", true);
        assert_eq!(
            runtime.run(blocked_late_child).await.unwrap().status,
            "cancelled",
            "the retained barrier must fail closed until stop retry succeeds"
        );
        assert!(!home
            .path()
            .join("data/run_requests/run-failed-stop-late-child.json")
            .exists());
        assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
        let parent_outcome = tokio::time::timeout(std::time::Duration::from_secs(5), parent)
            .await
            .expect("parent must settle after the failed stop attempt")
            .unwrap()
            .unwrap();
        let child_outcome = tokio::time::timeout(std::time::Duration::from_secs(5), child)
            .await
            .expect("child must settle after the failed stop attempt")
            .unwrap()
            .unwrap();
        assert_eq!(parent_outcome.status, "cancelled");
        assert_eq!(child_outcome.status, "cancelled");
        fs::remove_dir(&parent_snapshot).unwrap();
        let stopped = composed
            .call("assignment.stop", stop_params, &gateway.state)
            .await
            .unwrap();
        assert_eq!(stopped["assignment"]["status"], "cancelled");
        assert!(
            !runtime
                .assignment_cancellation_in_progress(&assignment_id)
                .await
        );
        let mut other_request = make_request("run-other-assignment", false);
        other_request.assignment_id = Some(other_assignment_id);
        other_request.instruction = "unrelated assignment run".into();
        let other_runtime = runtime.clone();
        let mut other = tokio::spawn(async move { other_runtime.run(other_request).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while provider_calls.load(Ordering::SeqCst) < 3 {
                all_provider_calls_started.notified().await;
            }
        })
        .await
        .expect("unrelated assignment must still reach the provider");
        assert_eq!(provider_calls.load(Ordering::SeqCst), 3);
        release_provider.add_permits(3);

        let other_outcome = tokio::time::timeout(std::time::Duration::from_secs(5), &mut other)
            .await
            .expect("unrelated assignment should settle")
            .unwrap()
            .unwrap();
        assert_eq!(parent_outcome.status, "cancelled");
        assert_eq!(child_outcome.status, "cancelled");
        assert_ne!(other_outcome.status, "cancelled");
        assert_eq!(provider_calls.load(Ordering::SeqCst), 3);
        let late_child = runtime.run(make_request("run-late-child", true)).await;
        assert_eq!(late_child.unwrap().status, "cancelled");
        assert_eq!(provider_calls.load(Ordering::SeqCst), 3);
        let request_dir = home.path().join("data/run_requests");
        assert!(!request_dir.join("run-late-child.json").exists());
        let request_count = fs::read_dir(request_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .count();
        assert_eq!(request_count, 3, "stop must not admit a late child run");
        let durable = runtime.state.durable.lock().await;
        for run_id in ["run-parent", "run-child"] {
            let job = durable
                .jobs()
                .find(|job| job.checkpoint.get("run_id").and_then(Value::as_str) == Some(run_id))
                .expect("durable run job");
            assert_eq!(job.status, macbot_durable::JobStatus::Cancelled, "{run_id}");
            assert!(!job.checkpoint.to_string().contains("late provider result"));
        }
        drop(durable);
        let late_provider_events = backend
            .store
            .events_since(0)
            .unwrap()
            .into_iter()
            // Message events carry their payload under `/message` and do not
            // include a run_id, so inspect the full event envelope for this
            // unique provider response across trace and chat projections.
            .filter(|event| event.data.to_string().contains("late provider result"))
            .collect::<Vec<_>>();
        assert!(
            late_provider_events.is_empty(),
            "cancelled provider text must not be published into the run trace"
        );
        provider_server.abort();
    }
}

fn explicit_skill_source_message_from(
    messages: &[Value],
    source_id: Option<&str>,
    skill_name: &str,
) -> Option<String> {
    let source_id = source_id?;
    messages.iter().find_map(|message| {
        (message.get("id").and_then(Value::as_str) == Some(source_id)
            && message.pointer("/sender/kind").and_then(Value::as_str) == Some("user")
            && message
                .get("fallback_text")
                .and_then(Value::as_str)
                .and_then(macbot_skills::forced_invocation)
                .is_some_and(|(name, _)| name == skill_name))
        .then(|| source_id.to_owned())
    })
}

#[cfg(test)]
mod explicit_skill_source_tests {
    use super::explicit_skill_source_message_from;
    use serde_json::{json, Value};

    #[test]
    fn source_annotation_requires_the_exact_message_association() {
        let messages: Vec<Value> = vec![
            json!({"id":"old", "sender":{"kind":"user"}, "fallback_text":"/deploy old request"}),
            json!({"id":"current", "sender":{"kind":"user"}, "fallback_text":"/deploy current request"}),
        ];

        assert_eq!(
            explicit_skill_source_message_from(&messages, Some("current"), "deploy"),
            Some("current".into())
        );
        assert_eq!(
            explicit_skill_source_message_from(&messages, None, "deploy"),
            None,
            "unassociated execution must not annotate a same-named historical invocation"
        );
        assert_eq!(
            explicit_skill_source_message_from(&messages, Some("missing"), "deploy"),
            None
        );
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
            assignment_id: request.assignment_id.clone(),
            source_message_id: None,
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
            continuation_id: None,
            continuation_payload: None,
            subagent: true,
            tools: Some(if request.assignment_id.is_none() {
                MAIN_ANALYSIS_SUBAGENT_TOOLS
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect()
            } else {
                vec![
                    "read".into(),
                    "ls".into(),
                    "find".into(),
                    "grep".into(),
                    "skill".into(),
                ]
            }),
            phase: Some("subagent".into()),
            parent_run_id: Some(request.parent_run_id.clone()),
            subagent_task: Some(request.task.clone()),
            save_full_requests: false,
            resume_message: None,
            client_locale: None,
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

    async fn persist_execution_question(&self, message: &Value, block: &Value) -> Option<Value> {
        let Some(call_id) = block.get("question_id").and_then(Value::as_str) else {
            tracing::error!("execution emitted question block without question_id");
            return None;
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
        let run_id = message
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| id.strip_prefix("msg_question_"))
            .map(str::to_owned)
            .unwrap_or_else(|| call_id.to_owned());
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
                    "client_request_id": format!("execution:question:{run_id}:{call_id}")
                }),
            )
            .await;
        let Ok(result) = result else {
            tracing::error!(call_id, "failed to persist execution question");
            return None;
        };
        let Some(question) = result.get("question") else {
            tracing::error!(call_id, "execution question RPC returned no question");
            return None;
        };
        let Some(question_id) = question.get("id").and_then(Value::as_str) else {
            tracing::error!(call_id, "execution question has no id");
            return None;
        };
        if let Err(error) = self.store.write_snapshot(
            format!("data/waiting/{}.json", safe_component(question_id)),
            &json!({
                "kind": "question",
                "run_id": run_id,
                "assignment_id": assignment_id,
                "chat_id": chat_id,
                "question_id": question_id,
                "call_id": call_id,
                "text": message.get("fallback_text").cloned().unwrap_or(Value::Null)
            }),
        ) {
            tracing::error!(%error, call_id, question_id, "failed to persist canonical execution question marker");
            return None;
        }
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
                return None;
            }
        };
        if let Err(error) = self.store.append_jsonl(
            format!("data/chats/{}/messages.jsonl", safe_component(chat_id)),
            &updated_message,
        ) {
            tracing::error!(%error, chat_id, "failed to persist question card update");
            return None;
        }
        Some(updated_message)
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
    let mut changed = false;
    let question_id = request.get("question_id").and_then(Value::as_str);
    if let Some(question_id) = question_id {
        if let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) {
            let before = blocks.len();
            blocks.retain(|block| {
                !(block.get("type").and_then(Value::as_str) == Some("approval_ref")
                    && block.get("approval_id").and_then(Value::as_str) == Some(question_id))
            });
            changed |= blocks.len() != before;
        }
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
        return changed;
    };
    if block.get("state").and_then(Value::as_str) != Some(request_state) {
        block["state"] = json!(request_state);
        changed = true;
    }
    changed
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
    async fn emit(&self, mut event: ExecutionEvent) {
        crate::adapter::normalize_event_data(&event.event, &mut event.data);
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
                    let Some(message) = self.persist_execution_question(message, block).await
                    else {
                        // Never expose the executor call id as a public
                        // question_id. If persistence stops after the durable
                        // waiting marker is written, startup repair can
                        // reconstruct the card from its exact run/job mapping.
                        return;
                    };
                    event.data = json!({"message":message});
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

    async fn publish_persisted(&self, event: Event) {
        self.inner.publish_persisted(event).await;
    }

    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        self.inner.send_group_message(message).await
    }

    async fn validate_send_msg_target(&self, message: &Value) -> Result<(), String> {
        self.inner.validate_send_msg_target(message).await
    }

    async fn update_message_delivery(
        &self,
        message_id: &str,
        bot_id: &str,
        assignment_id: Option<&str>,
        state: &str,
    ) -> Result<(), String> {
        self.inner
            .update_message_delivery(message_id, bot_id, assignment_id, state)
            .await
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

    async fn update_message_delivery(
        &self,
        message_id: &str,
        bot_id: &str,
        assignment_id: Option<&str>,
        state: &str,
    ) -> Result<(), String> {
        self.backend
            .execution_update_message_delivery(
                &self.state,
                message_id,
                &json!({
                    "bot_id":bot_id,
                    "assignment_id":assignment_id,
                    "state":state
                }),
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
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
    fn legacy_safe_question_marker_overrides_old_unsafe_flag_only_for_known_wait_tools() {
        let job = |name: &str, unsafe_replay| macbot_durable::Job {
            id: format!("job-{name}"),
            owner: "bot-1".into(),
            kind: "model_run".into(),
            status: macbot_durable::JobStatus::Waiting,
            checkpoint: serde_json::json!({
                "run_id":"run-1",
                "pending_tool":{"name":name,"call_id":"call-1"},
                "pending_tools":[{"name":name,"call_id":"call-1"}]
            }),
            unsafe_replay,
            updated_at: 1,
            commit_seq: 1,
        };

        assert!(super::waiting_checkpoint_safety_matches(
            &job("ask_user", true),
            Some(false)
        ));
        assert!(super::waiting_checkpoint_safety_matches(
            &job("request_takeover", false),
            Some(true)
        ));
        assert!(!super::waiting_checkpoint_safety_matches(
            &job("bash", false),
            Some(true)
        ));
    }

    #[test]
    fn assignment_recovery_selects_root_when_child_shares_assignment_id() {
        let root: super::ExecutionRequest = serde_json::from_value(serde_json::json!({
            "run_id":"root-run", "assignment_id":"assignment-1", "chat_id":"chat-1",
            "bot_id":"bot-1", "model":"mock/model", "instruction":"root"
        }))
        .unwrap();
        let child: super::ExecutionRequest = serde_json::from_value(serde_json::json!({
            "run_id":"child-run", "assignment_id":"assignment-1", "chat_id":"chat-1",
            "bot_id":"bot-1", "model":"mock/model", "instruction":"child",
            "subagent":true, "parent_run_id":"root-run"
        }))
        .unwrap();

        assert!(super::is_root_request_for_assignment(&root, "assignment-1"));
        assert!(!super::is_root_request_for_assignment(
            &child,
            "assignment-1"
        ));
    }

    #[test]
    fn allowed_approval_recovery_requires_exact_waiting_checkpoint() {
        let snapshot = serde_json::json!({
            "bots": {"bot-1": {"dm_chat_id": "dm-bot-1"}},
            "assignments": {"assignment-1": {
                "id":"assignment-1", "bot_id":"bot-1", "origin_chat_id":"chat-1",
                "status":"working", "wait":null
            }}
        });
        let approval = serde_json::json!({
            "state":"allowed_once", "bot_id":"bot-1", "chat_id":"chat-1",
            "assignment_id":"assignment-1", "tool":"browser_nav",
            "detail":"{\"action\":\"reload\"}"
        });
        let request: super::ExecutionRequest = serde_json::from_value(serde_json::json!({
            "run_id":"run-1", "assignment_id":"assignment-1", "chat_id":"chat-1",
            "bot_id":"bot-1", "model":"mock/model", "instruction":"resume"
        }))
        .unwrap();
        let job = |status| macbot_durable::Job {
            id: "job-1".into(),
            owner: "bot-1".into(),
            kind: "model_run".into(),
            status,
            checkpoint: serde_json::json!({
                "run_id":"run-1",
                "pending_tool":{"call_id":"call-1","name":"browser_nav","args":{"action":"reload"}},
                "pending_tools":[{"call_id":"call-1","name":"browser_nav","args":{"action":"reload"}}],
                "approval_detail":{"action":"reload"}
            }),
            unsafe_replay: true,
            updated_at: 1,
            commit_seq: 1,
        };
        assert!(super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Waiting),
            &request,
            false
        ));

        let mut denied = approval.clone();
        denied["state"] = serde_json::json!("denied");
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &denied,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Waiting),
            &request,
            false
        ));
        assert!(super::denied_tool_approval_matches(
            &snapshot,
            &denied,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Waiting),
            &request
        ));
        // A stale approval-map receipt must never cancel a different run.
        assert!(!super::denied_tool_approval_matches(
            &snapshot,
            &denied,
            Some("other-call"),
            &job(macbot_durable::JobStatus::Waiting),
            &request
        ));
        // Matching the call id alone is insufficient when the checkpoint args
        // no longer equal the approved args.
        let mut wrong_args_job = job(macbot_durable::JobStatus::Waiting);
        wrong_args_job.checkpoint["pending_tool"]["args"] = json!({"action":"other"});
        wrong_args_job.checkpoint["pending_tools"][0]["args"] = json!({"action":"other"});
        assert!(!super::denied_tool_approval_matches(
            &snapshot,
            &denied,
            Some("call-1"),
            &wrong_args_job,
            &request
        ));
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Running),
            &request,
            false
        ));
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Suspended),
            &request,
            false
        ));
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Suspended),
            &request,
            true
        ));
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("other-call"),
            &job(macbot_durable::JobStatus::Waiting),
            &request,
            false
        ));
        let mut wrong_scope = request.clone();
        wrong_scope.chat_id = "other-chat".into();
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("call-1"),
            &job(macbot_durable::JobStatus::Waiting),
            &wrong_scope,
            false
        ));
    }

    #[test]
    fn allowed_write_approval_matches_checkpoint_detail_and_raw_args() {
        let snapshot = serde_json::json!({
            "assignments": {"assignment-1": {
                "id":"assignment-1", "bot_id":"bot-1", "origin_chat_id":"chat-1",
                "status":"working", "project_id":"project-1", "wait":null
            }}
        });
        let detail = serde_json::json!({
            "path":"~/notes.txt", "content":"x",
            "resolved_path":"/Users/test/MacBot/notes.txt", "path_resolution":"home-v1"
        });
        let approval = serde_json::json!({
            "state":"allowed_once", "bot_id":"bot-1", "chat_id":"chat-1",
            "assignment_id":"assignment-1", "tool":"write", "detail":detail.to_string()
        });
        let request: super::ExecutionRequest = serde_json::from_value(serde_json::json!({
            "run_id":"run-write", "assignment_id":"assignment-1", "chat_id":"chat-1",
            "bot_id":"bot-1", "project_id":"project-1", "model":"mock/model",
            "instruction":"resume"
        }))
        .unwrap();
        let job = macbot_durable::Job {
            id: "job-write".into(),
            owner: "bot-1".into(),
            kind: "model_run".into(),
            status: macbot_durable::JobStatus::Waiting,
            checkpoint: serde_json::json!({
                "run_id":"run-write",
                "pending_tool":{"call_id":"call-write","name":"write","args":{"path":"~/notes.txt","content":"x"}},
                "pending_tools":[{"call_id":"call-write","name":"write","args":{"path":"~/notes.txt","content":"x"}}],
                "approval_detail":detail
            }),
            unsafe_replay: true,
            updated_at: 1,
            commit_seq: 1,
        };
        assert!(super::allowed_tool_approval_matches(
            &snapshot,
            &approval,
            Some("call-write"),
            &job,
            &request,
            false
        ));
        let mut changed = approval.clone();
        changed["detail"] = serde_json::json!({
            "path":"~/notes.txt", "content":"different",
            "resolved_path":"/Users/test/MacBot/notes.txt", "path_resolution":"home-v1"
        })
        .to_string()
        .into();
        assert!(!super::allowed_tool_approval_matches(
            &snapshot,
            &changed,
            Some("call-write"),
            &job,
            &request,
            false
        ));
    }

    #[test]
    fn takeover_message_projection_requires_exact_run_scope_and_updates_lifecycle() {
        let mut message = serde_json::json!({
            "id":"msg_takeover_run-1",
            "chat_id":"group-1",
            "sender":{"kind":"bot","bot_id":"bot-1"},
            "assignment_id":"assignment-1",
            "blocks":[
                {"type":"takeover_request","bot_id":"bot-1","reason":"登录","state":"pending"},
                {"type":"approval_ref","approval_id":"question-1","chat_id":"dm-bot-1"},
                {"type":"approval_ref","approval_id":"real-approval","chat_id":"group-1"}
            ]
        });
        let mut request = serde_json::json!({
            "message_id":"msg_takeover_run-1",
            "run_id":"run-1",
            "group_chat_id":"group-1",
            "bot_id":"bot-1",
            "assignment_id":"assignment-1",
            "question_id":"question-1",
            "state":"active"
        });

        assert!(super::transition_takeover_message(
            &mut message,
            &request,
            "group-1",
            "msg_takeover_run-1"
        ));
        assert_eq!(message["blocks"][0]["state"], "active");
        assert!(message["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| block["approval_id"] != "question-1"));
        assert!(message["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| block["approval_id"] == "real-approval"));
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
            "files":true,"bash":true,"browser":true,"web":true,"subagent":true
        }});
        let names = super::bot_tool_allowlist(Some(&bot), true, true, false, true).unwrap();
        assert_eq!(
            names,
            [
                "question",
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
                "subagent",
                "send_msg",
            ]
        );
        for forbidden in [
            "read",
            "write",
            "edit",
            "bash",
            "bash_job",
            "browser_open",
            "web_fetch",
            "web_search",
            "skill",
            "skill_draft",
            "memory",
            "memory_search",
            "session_search",
            "project_find",
            "chat_history",
            "routine",
            "update_announcement",
            "notify_user",
            "remind",
        ] {
            assert!(
                !names.iter().any(|allowed| allowed == forbidden),
                "{forbidden}"
            );
        }
        let group_main_names =
            super::bot_tool_allowlist(Some(&bot), true, false, false, true).unwrap();
        assert!(!group_main_names.iter().any(|name| name == "subagent"));
        let assigned_private_main_names =
            super::bot_tool_allowlist(Some(&bot), true, true, true, true).unwrap();
        assert!(!assigned_private_main_names
            .iter()
            .any(|name| name == "subagent"));
        let worker_names = super::bot_tool_allowlist(Some(&bot), false, true, false, true).unwrap();
        for expected in [
            "skill",
            "memory",
            "memory_search",
            "session_search",
            "project_find",
            "chat_history",
            "read",
            "write",
            "bash",
            "bash_job",
            "browser_open",
            "web_fetch",
            "web_search",
            "skill_draft",
            "subagent",
            "send_msg",
        ] {
            assert!(
                worker_names.iter().any(|name| name == expected),
                "{expected}"
            );
        }
        for main_only in ["list_bots", "create_project", "assign", "delegate"] {
            assert!(
                !worker_names.iter().any(|name| name == main_only),
                "{main_only}"
            );
        }
    }

    #[test]
    fn legacy_main_waiting_request_recomputes_tools_before_continuation() {
        let snapshot = json!({
            "bots": {"main": {
                "id":"main", "is_main":true, "dm_chat_id":"chat_main",
                "tools":{"files":true,"bash":true,"browser":true,"web":true,"subagent":true}
            }},
            "projects": {},
            "assignments": {}
        });
        let mut request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":"legacy-main-run", "chat_id":"chat_main", "bot_id":"main",
            "model":"mock/model", "instruction":"continue",
            "private":true, "phase":"coordinate", "routine":false,
            "tools":["read","write","bash","browser_open","web_fetch","memory",
                     "memory_search","project_find","chat_history","subagent","send_msg",
                     "set_user_profile"]
        }))
        .unwrap();
        assert!(super::tighten_request_tools_for_current_policy(
            &snapshot,
            &mut request,
            true
        ));
        let names = request.tools.unwrap();
        assert!(names.iter().any(|name| name == "question"));
        assert!(names.iter().any(|name| name == "delegate"));
        assert!(names.iter().any(|name| name == "subagent"));
        assert!(names.iter().any(|name| name == "send_msg"));
        assert!(names.iter().any(|name| name == "set_user_profile"));
        for forbidden in [
            "read",
            "write",
            "bash",
            "browser_open",
            "web_fetch",
            "memory",
            "memory_search",
            "project_find",
            "chat_history",
        ] {
            assert!(!names.iter().any(|name| name == forbidden), "{forbidden}");
        }

        let mut assigned = serde_json::from_value::<ExecutionRequest>(json!({
            "run_id":"legacy-assigned-main-run", "chat_id":"chat_main", "bot_id":"main",
            "model":"mock/model", "instruction":"continue",
            "private":true, "assignment_id":"assignment-1", "phase":"coordinate",
            "tools":["bash","subagent","set_user_profile"]
        }))
        .unwrap();
        assert!(super::tighten_request_tools_for_current_policy(
            &snapshot,
            &mut assigned,
            true
        ));
        let assigned_names = assigned.tools.unwrap();
        assert!(!assigned_names.iter().any(|name| name == "subagent"));
        assert!(!assigned_names.iter().any(|name| name == "set_user_profile"));

        let mut child = serde_json::from_value::<ExecutionRequest>(json!({
            "run_id":"legacy-main-child", "chat_id":"chat_main", "bot_id":"main",
            "model":"mock/model", "instruction":"analyze",
            "private":false, "subagent":true,
            "tools":["read","ls","find","grep","skill","web_fetch","send_msg"]
        }))
        .unwrap();
        assert!(super::tighten_request_tools_for_current_policy(
            &snapshot, &mut child, true
        ));
        assert_eq!(child.tools.unwrap(), ["read", "ls", "find", "grep"]);
    }

    use super::{
        answered_decision_answer, answered_decision_checkpoint_is_safe,
        answered_decision_request_matches, assignment_result_text,
        decision_checkpoint_matches_question, directed_chat_delivery_assignments,
        is_run_status_boundary, missing_model_bot_from_snapshot, pending_decision_question_id,
        project_assignment_targets, project_summary, resolve_model,
        routable_missing_model_chat_from_snapshot, waiting_assignment_for_message,
        waiting_checkpoint_matches_message, waiting_message_id_for_assignment,
        waiting_parent_for_child, ComposedBackend, ExecutionEvent, ExecutionRequest, ModelRole,
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
            Some("继续（补充：用户自定义）")
        );
        assert_eq!(
            answered_decision_answer(&json!({
                "state":"answered",
                "options":["红","蓝","绿"],
                "answer":{"option_index":0,"option_indexes":[0,2],"text":"低饱和"}
            }))
            .as_deref(),
            Some("红、绿（补充：低饱和）")
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
        assert!(answered_decision_checkpoint_is_safe(
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
    fn chat_delivery_promotion_requires_explicit_mention_or_reply_target() {
        let deliveries = vec![
            json!({"assignment_id":"assignment-worker","bot_id":"worker","state":"queued"}),
            json!({"assignment_id":"assignment-main","bot_id":"main","state":"queued"}),
        ];
        let snapshot = json!({
            "messages":{
                "active-bot-message":{"id":"active-bot-message","assignment_id":"assignment-worker"}
            },
            "assignments":{
                "assignment-worker":{"id":"assignment-worker","status":"working",
                    "trigger_message_id":"initial-worker-task"},
                "assignment-main":{"id":"assignment-main","status":"waiting_user",
                    "trigger_message_id":"initial-main-task",
                    "wait":{"reason":"decision","message_id":"decision-main"}}
            }
        });
        let empty = json!({"mentions":[]});
        assert!(
            directed_chat_delivery_assignments(&empty, &deliveries, None, &snapshot).is_empty(),
            "ordinary queued messages remain reviewable"
        );

        let explicit_bot = json!({"mentions":[{"kind":"bot","bot_id":"worker"}]});
        assert_eq!(
            directed_chat_delivery_assignments(&explicit_bot, &deliveries, None, &snapshot),
            ["assignment-worker".to_owned()].into_iter().collect()
        );
        let explicit_main = json!({"mentions":[{"kind":"main"}]});
        assert_eq!(
            directed_chat_delivery_assignments(&explicit_main, &deliveries, None, &snapshot),
            ["assignment-main".to_owned()].into_iter().collect()
        );
        let everyone_expanded = json!({
            "mentions":[{"kind":"bot","bot_id":"worker"},{"kind":"main"}]
        });
        assert_eq!(
            directed_chat_delivery_assignments(&everyone_expanded, &deliveries, None, &snapshot),
            ["assignment-worker".to_owned(), "assignment-main".to_owned()]
                .into_iter()
                .collect()
        );
        assert_eq!(
            directed_chat_delivery_assignments(
                &empty,
                &deliveries,
                Some("active-bot-message"),
                &snapshot
            ),
            ["assignment-worker".to_owned()].into_iter().collect(),
            "reply to an active Bot message targets that assignment"
        );
        assert!(
            directed_chat_delivery_assignments(&empty, &deliveries, None, &snapshot).is_empty(),
            "a reference quote has no reply target and must not promote delivery"
        );
        let mixed = vec![
            json!({"assignment_id":"assignment-worker","bot_id":"worker","state":"queued"}),
            json!({"assignment_id":"assignment-idle","bot_id":"idle","state":"delivered"}),
        ];
        let mixed_snapshot = json!({
            "assignments":{
                "assignment-worker":{"status":"working","trigger_message_id":"older"},
                "assignment-idle":{"status":"working","trigger_message_id":"current-message"}
            }
        });
        let everyone = json!({"id":"current-message","mentions":[
            {"kind":"bot","bot_id":"worker"},{"kind":"bot","bot_id":"idle"}
        ]});
        assert_eq!(
            directed_chat_delivery_assignments(&everyone, &mixed, None, &mixed_snapshot),
            ["assignment-worker".to_owned()].into_iter().collect(),
            "an @everyone batch must not re-inject the message that started an idle task"
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
    fn continuation_queue_and_promotion_events_decode_as_protocol_frames() {
        for status in ["queued", "working"] {
            let mut assignment = json!({
                "id":"assignment-2", "project_id":null, "origin_chat_id":"chat-1",
                "bot_id":"bot-1", "title":"work", "instruction":"do it",
                "from":"main", "trigger_message_id":null, "parent_assignment_id":null,
                "status":status, "queue_reason":null, "wait":null,
                "created_at":"2026-10-15T00:00:00Z", "started_at":null,
                "finished_at":null, "recovered_at":null,
                "usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,
                    "cache_write_tokens":0,"requests":0,"cost":null},
                "subagents_active":0, "pending_messages":0, "steers":[],
                "result_message_id":null, "model":""
            });
            crate::adapter::normalize_assignment(&mut assignment);
            let frame = json!({
                "v":1,"kind":"evt","seq":1,"event":"assignment.updated",
                "data":{"assignment":assignment}
            });
            let decoded: macbot_protocol::EventFrame = serde_json::from_value(frame)
                .expect("assignment.updated must remain protocol-decodable");
            assert_eq!(decoded.event, macbot_protocol::EventName::AssignmentUpdated);
        }
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
    use super::{
        host_default_locale, idle_bot_memory_targets, BashJobManager, ComposedBackend,
        ExecutionEvent, ExecutionRequest, ExecutionState, FeatureRunContext, FeatureToolRuntime,
        GatewayStateSink, OrchestratorGroupBridge, OrchestratorSink, ProductionBackend,
        ProjectFindToolBridge, RuntimeExecution,
    };
    use crate::execution::{ExecutionEngine, ExecutionSink};
    use crate::features::FeatureService;
    use crate::{Gateway, GatewayConfig, RpcBackend};
    use async_trait::async_trait;
    use macbot_memory::{
        AsyncMaintenanceProvider, MemoryAction, MemoryError, MemoryKind, MemoryRequest,
        MemorySource, MemoryTarget, ProjectRecord,
    };
    use macbot_providers::{Completion, MockProvider, ToolCall};
    use macbot_tools::{Tool, ToolContext};
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex as StdMutex};
    use tempfile::tempdir;
    use tokio::sync::Notify;

    struct TestMaintenance;

    #[async_trait]
    impl AsyncMaintenanceProvider for TestMaintenance {
        async fn summarize(&self, _: &str) -> Result<String, MemoryError> {
            Ok("maintenance summary".into())
        }
    }

    #[test]
    fn idle_memory_maintenance_excludes_main_bots_but_keeps_idle_workers() {
        let snapshot = json!({
            "bots": {
                "canonical-main": {"id":"main","is_main":true},
                "renamed-main": {"id":"secretary","is_main":true},
                "worker-idle": {"id":"worker-idle","is_main":false},
                "worker-active": {"id":"worker-active","is_main":false}
            }
        });
        let targets = idle_bot_memory_targets(
            &snapshot,
            &std::collections::HashSet::from(["worker-active".to_owned()]),
        );
        assert_eq!(targets, [MemoryTarget::bot("worker-idle")]);

        // A pre-marker persisted Bot with the canonical id follows the same
        // main-Bot fallback as engine construction.
        let legacy = json!({"bots":{"main":{"id":"main"}}});
        assert!(idle_bot_memory_targets(&legacy, &std::collections::HashSet::new()).is_empty());
    }

    #[derive(Default)]
    struct NullExecutionSink;

    #[async_trait]
    impl ExecutionSink for NullExecutionSink {
        async fn emit(&self, _: ExecutionEvent) {}

        async fn send_group_message(&self, _: Value) -> Result<Value, String> {
            Err("not used by project selection test".into())
        }

        async fn validate_send_msg_target(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }

        async fn approval_required(&self, _: Value) {}
    }

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

    #[tokio::test]
    async fn terminal_private_send_msg_fills_first_reply_and_keeps_followup_as_new_message() {
        for is_main in [true, false] {
            for intent in ["done", "blocked"] {
                let home = tempdir().unwrap();
                let gateway = Gateway::new(GatewayConfig {
                    home: home.path().to_path_buf(),
                    ..Default::default()
                });
                let backend = Arc::new(ProductionBackend::open(home.path()).unwrap());
                let (bot_id, chat_id) = if is_main {
                    ("main".to_owned(), "chat_main".to_owned())
                } else {
                    let created = backend
                        .call(
                            "bot.create",
                            json!({"name":"Private worker"}),
                            &gateway.state,
                        )
                        .await
                        .unwrap();
                    (
                        created["bot"]["id"].as_str().unwrap().to_owned(),
                        created["bot"]["dm_chat_id"].as_str().unwrap().to_owned(),
                    )
                };
                let run_id = if is_main {
                    if intent == "done" {
                        "run_terminal_main_done"
                    } else {
                        "run_terminal_main_blocked"
                    }
                } else {
                    if intent == "done" {
                        "run_terminal_worker_done"
                    } else {
                        "run_terminal_worker_blocked"
                    }
                };
                let source = backend
                    .call(
                        "chat.send",
                        json!({"chat_id":chat_id,"text":"请处理这件事"}),
                        &gateway.state,
                    )
                    .await
                    .expect("create source user message");
                let source_message_id = source["message"]["id"].as_str().unwrap().to_owned();
                let placeholder_id = format!(
                    "msg_{run_id}_reply_{}",
                    super::safe_component(&source_message_id)
                );
                let followup_id = format!("msg_{run_id}_terminal_report");
                let bridge = Arc::new(OrchestratorGroupBridge {
                    backend: backend.clone(),
                    state: gateway.state.clone(),
                    jobs: BashJobManager::default(),
                });
                let transport_sink = Arc::new(GatewayStateSink::new(
                    gateway.state.clone(),
                    backend.store.clone(),
                    bridge,
                ));
                let sink: Arc<dyn ExecutionSink> = Arc::new(OrchestratorSink {
                    inner: transport_sink,
                    orchestrator: backend.orchestrator.clone(),
                    store: backend.store.clone(),
                    backend: backend.clone(),
                    state: gateway.state.clone(),
                });
                let execution_state =
                    ExecutionState::with_durable(backend.store.clone(), backend.durable.clone());
                let engine = ExecutionEngine::new_with_usage_and_state(
                    backend.store.clone(),
                    Arc::new(MockProvider::new(vec![
                        Completion {
                            tool_calls: vec![ToolCall {
                                call_id: "first_reply".into(),
                                name: "send_msg".into(),
                                args: json!({
                                    "intent":"ack",
                                    "text":"收到，我开始处理。",
                                    "message_id":placeholder_id
                                }),
                            }],
                            stop_reason: "tool_calls".into(),
                            ..Default::default()
                        },
                        Completion {
                            tool_calls: vec![ToolCall {
                                call_id: "terminal_report".into(),
                                name: "send_msg".into(),
                                args: json!({
                                    "intent":intent,
                                    "text":"任务已完成"
                                }),
                            }],
                            stop_reason: "tool_calls".into(),
                            ..Default::default()
                        },
                    ])),
                    Vec::<Arc<dyn Tool>>::new(),
                    sink,
                    home.path(),
                    backend.usage.clone(),
                    execution_state,
                )
                .unwrap();
                let request: ExecutionRequest = serde_json::from_value(json!({
                    "run_id":run_id,
                    "assignment_id":null,
                    "chat_id":chat_id,
                    "bot_id":bot_id,
                    "source_message_id":source_message_id,
                    "model":"mock/model",
                    "instruction":"finish this private conversation",
                    "private":true
                }))
                .unwrap();

                let outcome = engine.run(request).await.unwrap();
                assert_eq!(
                    outcome.status,
                    if intent == "done" {
                        "done"
                    } else {
                        "suspended"
                    }
                );

                let messages = backend
                    .store
                    .read_jsonl::<Value>(format!("data/chats/{chat_id}/messages.jsonl"))
                    .unwrap();
                let first_reply_versions = messages
                    .iter()
                    .filter(|message| message["id"] == placeholder_id)
                    .collect::<Vec<_>>();
                assert!(
                    first_reply_versions.len() >= 2,
                    "the first reply fills the reserved placeholder: {first_reply_versions:?}"
                );
                let placeholder = first_reply_versions.first().unwrap();
                let first_reply = first_reply_versions.last().unwrap();
                for field in ["id", "chat_id", "sender", "created_at", "seq"] {
                    assert_eq!(
                        first_reply[field], placeholder[field],
                        "first reply replacement preserves placeholder {field}"
                    );
                }
                assert_eq!(placeholder["streaming"], true);
                assert_eq!(placeholder["source_message_id"], source_message_id);
                assert_eq!(first_reply["streaming"], false);
                assert_eq!(first_reply["intent"], "ack");
                assert_eq!(first_reply["fallback_text"], "收到，我开始处理。");

                let followup = messages
                    .iter()
                    .find(|message| message["id"] == followup_id)
                    .expect("terminal follow-up is a separate message");
                assert_eq!(followup["id"], followup_id);
                assert_eq!(followup["chat_id"], chat_id);
                assert_eq!(followup["sender"]["bot_id"], bot_id);
                assert_eq!(followup["source_message_id"], source_message_id);
                assert_eq!(followup["intent"], intent);
                assert_eq!(followup["streaming"], false);
                assert_eq!(followup["fallback_text"], "任务已完成");
                assert!(followup["seq"].as_u64().unwrap() > first_reply["seq"].as_u64().unwrap());

                let events = backend.store.events_since(0).unwrap();
                let first_reply_events = events
                    .iter()
                    .filter(|event| {
                        event.data.pointer("/message/id").and_then(Value::as_str)
                            == Some(placeholder_id.as_str())
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    first_reply_events
                        .iter()
                        .filter(|event| event.event == "message.created")
                        .count(),
                    1,
                    "the reserved first reply is created once"
                );
                let updates = first_reply_events
                    .iter()
                    .filter(|event| event.event == "message.updated")
                    .collect::<Vec<_>>();
                assert_eq!(updates.len(), 1, "the first send_msg fills the placeholder");
                assert_eq!(updates[0].data["message"]["streaming"], false);

                let followup_events = events
                    .iter()
                    .filter(|event| {
                        event.data.pointer("/message/id").and_then(Value::as_str)
                            == Some(followup_id.as_str())
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    followup_events
                        .iter()
                        .filter(|event| event.event == "message.created")
                        .count(),
                    1,
                    "the terminal follow-up gets a new created event"
                );
                assert_eq!(
                    followup_events
                        .iter()
                        .filter(|event| event.event == "message.updated")
                        .count(),
                    0,
                    "the terminal follow-up never overwrites the first reply"
                );

                let followup_created_seq = followup_events
                    .iter()
                    .find(|event| event.event == "message.created")
                    .unwrap()
                    .seq;
                let terminal_trace_type = if intent == "done" {
                    "run.end"
                } else {
                    "run.wait"
                };
                let terminal_seq = events
                    .iter()
                    .find(|event| {
                        event.event == "trace.item"
                            && event.data.pointer("/item/type").and_then(Value::as_str)
                                == Some(terminal_trace_type)
                            && event.data.pointer("/item/run_id").and_then(Value::as_str)
                                == Some(run_id)
                    })
                    .expect("terminal trace")
                    .seq;
                assert!(followup_created_seq < terminal_seq);

                let history = backend
                    .call("chat.history", json!({"chat_id":chat_id}), &gateway.state)
                    .await
                    .unwrap();
                let history_messages = history["messages"].as_array().unwrap();
                assert!(history_messages.iter().any(|message| {
                    message["id"] == source_message_id
                        && message.pointer("/sender/kind").and_then(Value::as_str) == Some("user")
                }));
                assert!(history_messages.iter().any(|message| {
                    message["id"] == placeholder_id && message["streaming"] == false
                }));
                assert!(history_messages.iter().any(|message| {
                    message["id"] == followup_id && message["streaming"] == false
                }));
            }
        }
    }

    #[tokio::test]
    async fn executor_question_publishes_only_canonical_card_and_resumes_after_answer() {
        let home = tempdir().unwrap();
        let gateway = Gateway::new(GatewayConfig {
            home: home.path().to_path_buf(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(home.path()).unwrap());
        let requests = Arc::new(StdMutex::new(Vec::<Value>::new()));
        let observed_requests = requests.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let requests = observed_requests.clone();
                async move {
                    requests.lock().unwrap().push(body);
                    (
                        [("content-type", "text/event-stream")],
                        "data: {\"choices\":[{\"delta\":{\"content\":\"已从 question.answer 恢复\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n",
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider_server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = backend
            .call(
                "provider.create",
                json!({
                    "name":"question resume test provider",
                    "api_kind":"openai-completions",
                    "base_url":format!("http://{address}/v1"),
                    "client_request_id":"question-resume-provider"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let provider_id = provider["provider"]["id"].as_str().unwrap().to_owned();
        let model = backend
            .call(
                "model.upsert",
                json!({
                    "provider_id":provider_id,
                    "model_id":"question-resume-model",
                    "client_request_id":"question-resume-model"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let model_ref = model["model"]["ref"].as_str().unwrap().to_owned();
        let run_id = "run_executor_question_canonical";
        let call_id = "executor_call_question_1";
        let message_id = format!("msg_question_{run_id}");
        let chat_id = "chat_main";
        let sink = {
            let bridge = Arc::new(OrchestratorGroupBridge {
                backend: backend.clone(),
                state: gateway.state.clone(),
                jobs: BashJobManager::default(),
            });
            let transport = Arc::new(GatewayStateSink::new(
                gateway.state.clone(),
                backend.store.clone(),
                bridge,
            ));
            Arc::new(OrchestratorSink {
                inner: transport,
                orchestrator: backend.orchestrator.clone(),
                store: backend.store.clone(),
                backend: backend.clone(),
                state: gateway.state.clone(),
            }) as Arc<dyn ExecutionSink>
        };
        let execution_state =
            ExecutionState::with_durable(backend.store.clone(), backend.durable.clone());
        let engine = ExecutionEngine::new_with_usage_and_state(
            backend.store.clone(),
            Arc::new(MockProvider::new(vec![Completion {
                tool_calls: vec![ToolCall {
                    call_id: call_id.into(),
                    name: "ask_user".into(),
                    args: json!({"question":"选哪个环境？"}),
                }],
                stop_reason: "tool_calls".into(),
                ..Default::default()
            }])),
            Vec::<Arc<dyn Tool>>::new(),
            sink,
            home.path(),
            backend.usage.clone(),
            execution_state,
        )
        .unwrap();
        let request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":run_id,
            "assignment_id":null,
            "chat_id":chat_id,
            "bot_id":"main",
            "provider_id":provider_id,
            "model":model_ref,
            "instruction":"需要用户选择",
            "private":true
        }))
        .unwrap();
        backend
            .store
            .write_snapshot(format!("data/run_requests/{run_id}.json"), &request)
            .unwrap();

        let suspended = engine.run(request.clone()).await.unwrap();
        assert_eq!(suspended.status, "suspended");
        let events = backend.store.events_since(0).unwrap();
        let question = events
            .iter()
            .find(|event| event.event == "question.asked")
            .expect("canonical question event")
            .data["question"]
            .clone();
        let question_id = question["id"].as_str().unwrap();
        assert_ne!(question_id, call_id);
        let message_events = events
            .iter()
            .filter(|event| {
                matches!(event.event.as_str(), "message.created" | "message.updated")
                    && event.data.pointer("/message/id").and_then(Value::as_str)
                        == Some(message_id.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(message_events.len(), 1, "no stale follow-up created/update");
        assert_eq!(message_events[0].event, "message.created");
        assert_eq!(
            message_events[0].data["message"]["blocks"][0]["question_id"],
            question_id
        );
        let asked_seq = events
            .iter()
            .find(|event| event.event == "question.asked")
            .unwrap()
            .seq;
        assert!(asked_seq < message_events[0].seq);

        let marker = backend
            .store
            .read_snapshot::<Value>(format!("data/waiting/{question_id}.json"))
            .unwrap()
            .unwrap();
        assert_eq!(marker["question_id"], question_id);
        assert_eq!(marker["call_id"], call_id);
        let history = backend
            .call("chat.history", json!({"chat_id":chat_id}), &gateway.state)
            .await
            .unwrap();
        let history_card = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == message_id)
            .unwrap();
        assert_eq!(history_card["blocks"][0]["question_id"], question_id);

        let composed = ComposedBackend::open(
            backend.clone(),
            gateway.state.clone(),
            home.path().to_path_buf(),
        )
        .unwrap();
        wait_for_startup_recovery(&composed).await;
        let answered = composed
            .call(
                "question.answer",
                json!({"question_id":question_id,"text":"生产环境"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(answered["question"]["id"], question_id);
        assert_eq!(answered["question"]["state"], "answered");
        wait_for_job_status(&backend, &suspended.job_id, macbot_durable::JobStatus::Done).await;
        {
            let provider_requests = requests.lock().unwrap();
            assert_eq!(
                provider_requests.len(),
                1,
                "answer should resume the model turn"
            );
            let messages = provider_requests[0]["messages"].as_array().unwrap();
            assert!(
                messages.iter().any(|message| {
                    message["role"] == "tool" && message["content"] == "生产环境"
                }),
                "the canonical question answer must remain in the tool result: {messages:?}"
            );
        }
        let resumed_job = backend
            .durable
            .lock()
            .await
            .job(&suspended.job_id)
            .unwrap()
            .clone();
        assert_eq!(resumed_job.checkpoint["text"], "已从 question.answer 恢复");
        provider_server.abort();
    }

    #[tokio::test]
    async fn project_find_bridge_selection_reaches_next_private_model_context() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::<PathBuf>::new()).unwrap());
        service
            .add_project_record(ProjectRecord {
                id: "project-atlas".into(),
                name: "Atlas project".into(),
                goal: "Build the Atlas service".into(),
                content: "Atlas event gateway".into(),
            })
            .unwrap();
        service
            .shared_memory
            .add(MemoryRequest {
                target: MemoryTarget::project("project-atlas"),
                action: MemoryAction::Add,
                content: "Atlas architecture uses an event gateway".into(),
                id: None,
                kind: Some(MemoryKind::Project),
                source: MemorySource::default(),
            })
            .unwrap();

        let mut run = FeatureRunContext::bot("worker-a", "local-user");
        run.private = true;
        run.run_id = Some("private-project-selection".into());
        run.chat_id = Some("private-chat".into());
        run.session_id = Some("private-chat".into());
        let runtime = FeatureToolRuntime::new(service, run);
        let bridge = ProjectFindToolBridge {
            runtime: runtime.clone(),
            visible_project_ids: vec!["project-atlas".into()],
        };
        let selected = bridge
            .call(
                &ToolContext::new("/tmp", "private-project-selection", "/tmp/runs"),
                json!({"query":"Atlas","project_id":"project-atlas"}),
            )
            .await;
        assert!(!selected.is_error);
        assert_eq!(selected.details["selected_project_id"], "project-atlas");

        let sink = runtime.execution_sink(Arc::new(NullExecutionSink));
        let request = ExecutionRequest {
            run_id: "private-project-selection".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "private-chat".into(),
            bot_id: "worker-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "work on Atlas".into(),
            messages: vec![json!({"role":"user","content":"work on Atlas"})],
            max_turns: 1,
            private: true,
            allow_unsafe: false,
            cwd: None,
            routine: false,
            price: None,
            resume_approved: false,
            continuation_id: None,
            continuation_payload: None,
            subagent: false,
            tools: None,
            phase: None,
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
            client_locale: None,
        };
        let context = sink.prepare_model_context(&request).await.unwrap().unwrap();
        assert!(context["system_blocks"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Atlas architecture uses an event gateway"));
    }

    #[tokio::test]
    async fn first_connection_greeting_is_idempotent_across_bootstrap_and_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let inner = Arc::new(ProductionBackend::open(&path).unwrap());
        let composed =
            ComposedBackend::open(inner.clone(), gateway.state.clone(), path.clone()).unwrap();
        wait_for_startup_recovery(&composed).await;

        for _ in 0..2 {
            composed
                .call("bootstrap", json!({}), &gateway.state)
                .await
                .expect("bootstrap should succeed");
        }
        let history = composed
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .expect("read greeted chat history");
        let greeting_count = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message.pointer("/sender/kind").and_then(Value::as_str) == Some("bot")
                    && message
                        .pointer("/fallback_text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains("你也可以跳过"))
            })
            .count();
        assert_eq!(greeting_count, 1);

        drop(composed);
        drop(inner);
        let reopened = Arc::new(ProductionBackend::open(&path).unwrap());
        let after_restart =
            ComposedBackend::open(reopened.clone(), gateway.state.clone(), path.clone()).unwrap();
        after_restart
            .call("bootstrap", json!({}), &gateway.state)
            .await
            .expect("bootstrap after restart should succeed");
        let history = reopened
            .call(
                "chat.history",
                json!({"chat_id":"chat_main","limit":100}),
                &gateway.state,
            )
            .await
            .expect("read restarted chat history");
        let greeting_count = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message.pointer("/sender/kind").and_then(Value::as_str) == Some("bot")
                    && message
                        .pointer("/fallback_text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains("你也可以跳过"))
            })
            .count();
        assert_eq!(greeting_count, 1);
    }

    #[tokio::test]
    async fn runtime_feature_access_authorizes_host_user_profile_for_private_and_group_runs() {
        let (inner, state, assignments) = seeded_backend().await;
        let snapshot = inner.orchestrator.snapshot().unwrap();
        let assignment = snapshot["assignments"][&assignments[0]].clone();
        let project_id = assignment["project_id"].as_str().unwrap();
        let bot_id = assignment["bot_id"].as_str().unwrap();
        let dir = tempdir().unwrap();
        let composed =
            ComposedBackend::open(inner, state.clone(), dir.path().to_path_buf()).unwrap();
        wait_for_startup_recovery(&composed).await;
        let private: ExecutionRequest = serde_json::from_value(json!({
            "run_id":"private-profile-access",
            "chat_id":"chat_main",
            "bot_id":"main",
            "model":"mock/model",
            "instruction":"test",
            "private":true
        }))
        .unwrap();
        let group: ExecutionRequest = serde_json::from_value(json!({
            "run_id":"group-profile-access",
            "chat_id":assignment["origin_chat_id"],
            "bot_id":bot_id,
            "model":"mock/model",
            "instruction":"test",
            "private":false,
            "project_id":project_id
        }))
        .unwrap();

        let private_access = composed.runtime.feature_access(&private);
        let group_access = composed.runtime.feature_access(&group);
        assert_eq!(private_access.user_id.as_deref(), Some("local-user"));
        assert_eq!(group_access.user_id.as_deref(), Some("local-user"));
        assert_eq!(group_access.project_id.as_deref(), Some(project_id));
        assert!(group_access.is_project_member(bot_id, project_id));
    }

    fn unwrap_arc<T>(value: Arc<T>) -> T {
        match Arc::try_unwrap(value) {
            Ok(value) => value,
            Err(_) => panic!("all persistence test tasks must be joined before reopen"),
        }
    }

    async fn seed_private_waiting_approval(
        backend: &Arc<ProductionBackend>,
        state: &crate::GatewayState,
        suffix: &str,
    ) -> (String, String, String) {
        let bot = backend
            .call(
                "bot.create",
                json!({"name":format!("deny-private-{suffix}")}),
                state,
            )
            .await
            .expect("create private bot");
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let args = json!({
            "action":"add",
            "content":format!("private-deny-{suffix}"),
            "kind":"bot_experience",
            "scope":"bot",
            "bot_id":bot_id.clone()
        });
        let approval = backend
            .call(
                "approval.request",
                json!({
                    "bot_id":bot_id,
                    "assignment_id":null,
                    "chat_id":chat_id,
                    "tool":"memory",
                    "risk":"write",
                    "summary":"private deny",
                    "detail":args.to_string()
                }),
                state,
            )
            .await
            .expect("create private approval");
        let approval_id = approval["approval"]["id"]
            .as_str()
            .or_else(|| approval["id"].as_str())
            .unwrap()
            .to_owned();
        let run_id = format!("run-private-deny-{suffix}");
        let call_id = format!("call-private-deny-{suffix}");
        let request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":run_id,
            "assignment_id":null,
            "chat_id":chat_id,
            "bot_id":bot_id,
            "model":"mock/model",
            "provider_id":"mock",
            "instruction":"private deny",
            "private":true,
            "allow_unsafe":true
        }))
        .unwrap();
        backend
            .store
            .append_jsonl(
                format!(
                    "data/chats/{}/messages.jsonl",
                    super::safe_component(&request.chat_id)
                ),
                &json!({
                    "id":format!("msg_{}", super::safe_component(&request.run_id)),
                    "chat_id":request.chat_id,
                    "seq":0,
                    "sender":{"kind":"bot","bot_id":request.bot_id},
                    "created_at":crate::now(),
                    "edited_at":null,
                    "deleted":false,
                    "reply_to":null,
                    "thread_count":0,
                    "mentions":[],
                    "blocks":[],
                    "fallback_text":"",
                    "intent":null,
                    "assignment_id":null,
                    "streaming":true,
                    "delivery":[],
                    "reactions":[]
                }),
            )
            .unwrap();
        backend
            .store
            .write_snapshot(
                format!("data/run_requests/{}.json", request.run_id),
                &request,
            )
            .unwrap();
        let checkpoint = json!({
            "run_id":request.run_id,
            "messages":[],
            "round":1,
            "pending_tool":{"name":"memory","call_id":call_id,"args":args},
            "pending_tools":[{"name":"memory","call_id":call_id,"args":args}],
            "approval_detail":args
        });
        let job = {
            let mut durable = backend.durable.lock().await;
            let queued = durable
                .create_job(&request.bot_id, "model_run", checkpoint.clone())
                .unwrap();
            durable
                .commit(
                    &queued.id,
                    macbot_durable::JobStatus::Waiting,
                    checkpoint,
                    true,
                )
                .unwrap()
        };
        backend
            .store
            .write_snapshot(
                format!("data/approval-map/{approval_id}.json"),
                &json!({"call_id":call_id}),
            )
            .unwrap();
        (approval_id, job.id, chat_id)
    }

    async fn wait_for_job_status(
        backend: &Arc<ProductionBackend>,
        job_id: &str,
        expected: macbot_durable::JobStatus,
    ) {
        for _ in 0..100 {
            if backend
                .durable
                .lock()
                .await
                .job(job_id)
                .is_some_and(|job| job.status == expected)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("job {job_id} did not reach {expected:?}");
    }

    async fn configure_recovery_model(
        backend: &Arc<ProductionBackend>,
        state: &crate::GatewayState,
        prefix: &str,
    ) {
        let provider = backend
            .call(
                "provider.create",
                json!({
                    "name":format!("{prefix} provider"),
                    "api_kind":"openai-completions",
                    "base_url":"http://127.0.0.1:1",
                    "client_request_id":format!("{prefix}-provider")
                }),
                state,
            )
            .await
            .unwrap();
        let provider_id = provider["provider"]["id"].as_str().unwrap();
        backend
            .call(
                "model.upsert",
                json!({
                    "provider_id":provider_id,
                    "model_id":format!("{prefix}-model"),
                    "client_request_id":format!("{prefix}-model")
                }),
                state,
            )
            .await
            .unwrap();
        let model_ref = format!("{provider_id}/{prefix}-model");
        backend
            .store
            .write_snapshot(
                "data/settings.json",
                &json!({"models":{"bot_default":model_ref,"main":null}}),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn chat_locale_is_per_message_profile_preferred_and_retry_stable() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let inner = Arc::new(ProductionBackend::open(&path).unwrap());
        configure_recovery_model(&inner, &gateway.state, "locale").await;
        let mut settings = inner
            .store
            .read_snapshot::<Value>("data/settings.json")
            .unwrap()
            .unwrap();
        settings["models"]["main"] = settings["models"]["bot_default"].clone();
        inner
            .store
            .write_snapshot("data/settings.json", &settings)
            .unwrap();
        let composed = ComposedBackend::open(inner.clone(), gateway.state.clone(), path).unwrap();
        let host_locale = host_default_locale();

        async fn send_request(
            composed: &ComposedBackend,
            inner: &ProductionBackend,
            state: &crate::GatewayState,
            locale: Option<&str>,
            request_id: &str,
        ) -> (Value, ExecutionRequest) {
            let mut params = json!({
                "chat_id":"chat_main",
                "text":format!("locale test {request_id}"),
                "client_request_id":request_id,
            });
            if let Some(locale) = locale {
                params["client_locale"] = json!(locale);
            }
            let result = inner
                .call("chat.send", params.clone(), state)
                .await
                .unwrap();
            let request = composed.request_for_chat(&params, &result).await.unwrap();
            (result, request)
        }
        fn system_prompt(request: &ExecutionRequest) -> String {
            request
                .messages
                .iter()
                .filter(|message| message["role"] == "system")
                .filter_map(|message| message["content"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let (_, french) = send_request(
            &composed,
            &inner,
            &gateway.state,
            Some("fr-FR"),
            "locale-fr",
        )
        .await;
        assert_eq!(french.client_locale.as_deref(), Some("fr-FR"));
        assert!(system_prompt(&french).contains("fr-FR"));

        let (_, japanese) = send_request(
            &composed,
            &inner,
            &gateway.state,
            Some("ja-JP"),
            "locale-ja",
        )
        .await;
        assert_eq!(japanese.client_locale.as_deref(), Some("ja-JP"));
        assert!(system_prompt(&japanese).contains("ja-JP"));

        let (_, fallback) =
            send_request(&composed, &inner, &gateway.state, None, "locale-host").await;
        assert!(system_prompt(&fallback).contains(&host_locale));

        composed.feature_service.user_memory_rpc(
            "memory.profile.update",
            json!({"language":"de-DE","expected_version":1,"client_request_id":"locale-profile"}),
            "Asia/Shanghai",
        ).unwrap();
        let (_, profile_preferred) = send_request(
            &composed,
            &inner,
            &gateway.state,
            Some("fr-FR"),
            "locale-profile-preferred",
        )
        .await;
        assert!(system_prompt(&profile_preferred).contains("de-DE"));
        assert!(!system_prompt(&profile_preferred).contains("fr-FR"));

        let params = json!({"chat_id":"chat_main","text":"retry locale","client_locale":"es-MX","client_request_id":"locale-retry"});
        let first = inner
            .call("chat.send", params.clone(), &gateway.state)
            .await
            .unwrap();
        let retried = inner
            .call("chat.send", params.clone(), &gateway.state)
            .await
            .unwrap();
        assert_eq!(first, retried);
        let retry_request = composed.request_for_chat(&params, &retried).await.unwrap();
        assert_eq!(retry_request.client_locale.as_deref(), Some("es-MX"));
        assert!(system_prompt(&retry_request).contains("de-DE"));
        let operations = inner
            .store
            .read_jsonl::<Value>("data/orchestrator/operations.jsonl")
            .unwrap();
        assert!(operations.iter().any(|operation| {
            operation["method"] == "chat.send"
                && operation["params"]["client_request_id"] == "locale-retry"
                && operation["params"]["client_locale"] == "es-MX"
        }));
    }

    #[tokio::test]
    async fn mentioning_worker_in_main_dm_does_not_change_private_reply_identity() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let inner = Arc::new(ProductionBackend::open(&path).unwrap());
        configure_recovery_model(&inner, &gateway.state, "private-route").await;
        let mut settings = inner
            .store
            .read_snapshot::<Value>("data/settings.json")
            .unwrap()
            .unwrap();
        settings["models"]["main"] = settings["models"]["bot_default"].clone();
        inner
            .store
            .write_snapshot("data/settings.json", &settings)
            .unwrap();

        let worker = inner
            .call(
                "bot.create",
                json!({"name":"private route worker"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let peer = inner
            .call(
                "bot.create",
                json!({"name":"private route peer"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let worker_id = worker["bot"]["id"].as_str().unwrap();
        let worker_dm = worker["bot"]["dm_chat_id"].as_str().unwrap();
        let peer_id = peer["bot"]["id"].as_str().unwrap();
        let params = json!({
            "chat_id":"chat_main",
            "text":"Keep the private reply in the Main Bot conversation.",
            "mentions":[{"kind":"bot","bot_id":worker_id}],
            "client_request_id":"private-route-main-mention"
        });
        let result = inner
            .call("chat.send", params.clone(), &gateway.state)
            .await
            .unwrap();

        let composed = ComposedBackend::open(inner.clone(), gateway.state.clone(), path).unwrap();
        wait_for_startup_recovery(&composed).await;
        let request = composed.request_for_chat(&params, &result).await.unwrap();
        assert_eq!(request.chat_id, "chat_main");
        assert_eq!(request.bot_id, "main");
        assert_eq!(request.assignment_id, None);
        assert!(request.private);

        // Even if a private user message contains another Bot mention, the
        // private conversation remains bound to its owning Bot incarnation.
        let peer_mention_params = json!({
            "chat_id":worker_dm,
            "text":"Keep the worker DM reply with its owning Bot.",
            "mentions":[{"kind":"bot","bot_id":peer_id}]
        });
        let peer_mention_result = json!({"message":{"id":"synthetic-worker-dm-message"}});
        let worker_request = composed
            .request_for_chat(&peer_mention_params, &peer_mention_result)
            .await
            .unwrap();
        assert_eq!(worker_request.chat_id, worker_dm);
        assert_eq!(worker_request.bot_id, worker_id);
        assert!(worker_request.private);

        let project = inner
            .call(
                "project.create",
                json!({
                    "name":"private route project",
                    "goal":"verify group-scoped Bot routing",
                    "member_bot_ids":[worker_id]
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let group_chat = project["chat"]["id"].as_str().unwrap();
        let group_params = json!({
            "chat_id":group_chat,
            "text":"Keep a group assignment in its bound group.",
            "mentions":[{"kind":"bot","bot_id":worker_id}],
            "client_request_id":"private-route-group-mention"
        });
        let group_result = inner
            .call("chat.send", group_params.clone(), &gateway.state)
            .await
            .unwrap();
        let group_assignment_id = group_result["message"]["delivery"][0]["assignment_id"]
            .as_str()
            .unwrap();
        let group_request = composed
            .request_for_chat(&group_params, &group_result)
            .await
            .unwrap();
        assert_eq!(group_request.chat_id, group_chat);
        assert_eq!(group_request.bot_id, worker_id);
        assert_eq!(
            group_request.assignment_id.as_deref(),
            Some(group_assignment_id)
        );
        assert!(!group_request.private);
    }

    fn private_run_id(message_id: &str) -> String {
        format!(
            "run_chat_{}",
            message_id
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect::<String>()
        )
    }

    async fn wait_for_startup_recovery(composed: &ComposedBackend) {
        if composed.recovery_done.load(Ordering::Acquire) {
            return;
        }
        let notified = composed.recovery_notify.notified();
        if composed.recovery_done.load(Ordering::Acquire) {
            return;
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), notified)
            .await
            .expect("startup recovery did not finish");
        assert!(composed.recovery_done.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn durable_assignment_recovery_persists_timestamp_and_updated_event() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let first_gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let first = Arc::new(ProductionBackend::open(&path).unwrap());
        let bot = first
            .call(
                "bot.create",
                json!({"name":"recover-marker"}),
                &first_gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let project = first
            .call(
                "project.create",
                json!({"name":"recover-marker-project","goal":"test recovery marker","member_bot_ids":[bot_id]}),
                &first_gateway.state,
            )
            .await
            .unwrap();
        let project_id = project["project"]["id"].as_str().unwrap().to_owned();
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let created = first
            .call(
                "assignment.create",
                json!({"project_id":project_id,"origin_chat_id":chat_id,"bot_id":bot_id,"title":"resume","instruction":"resume after restart","from":"main"}),
                &first_gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["status"], "working");

        let run_id = "run_recovered_assignment";
        let request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":run_id,
            "assignment_id":assignment_id,
            "chat_id":chat_id,
            "bot_id":bot_id,
            "model":"missing-provider/model",
            "instruction":"resume after restart",
            "private":false
        }))
        .unwrap();
        first
            .store
            .write_snapshot(format!("data/run_requests/{run_id}.json"), &request)
            .unwrap();
        let checkpoint = json!({"run_id":run_id});
        {
            let mut durable = first.durable.lock().await;
            let job = durable
                .create_job(&bot_id, "model_run", checkpoint.clone())
                .unwrap();
            durable
                .commit(
                    &job.id,
                    macbot_durable::JobStatus::Running,
                    checkpoint,
                    false,
                )
                .unwrap();
        }
        drop(first);
        drop(first_gateway);

        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let composed = ComposedBackend::open(backend.clone(), gateway.state.clone(), path).unwrap();
        wait_for_startup_recovery(&composed).await;

        let snapshot = backend.orchestrator.snapshot().unwrap();
        let recovered_at = snapshot["assignments"][&assignment_id]["recovered_at"]
            .as_str()
            .expect("recovered timestamp should be persisted");
        assert!(!recovered_at.is_empty());
        let events = backend.store.events_since(0).unwrap();
        assert!(events.iter().any(|event| {
            event.event == "assignment.updated"
                && event.data["assignment"]["id"] == assignment_id
                && event.data["assignment"]["recovered_at"] == recovered_at
        }));

        let normal = backend
            .call(
                "assignment.create",
                json!({"project_id":project_id,"origin_chat_id":chat_id,"bot_id":bot_id,"title":"new normal run","instruction":"no recovery marker","from":"main"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert!(normal["recovered_at"].is_null());
    }

    #[tokio::test]
    async fn denied_private_approval_cancels_waiting_job_without_tool_execution() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let composed = ComposedBackend::open(backend.clone(), gateway.state.clone(), path).unwrap();
        wait_for_startup_recovery(&composed).await;
        let (approval_id, job_id, chat_id) =
            seed_private_waiting_approval(&backend, &gateway.state, "live").await;
        let result = composed
            .call(
                "approval.decide",
                json!({"approval_id":approval_id,"decision":"deny"}),
                &gateway.state,
            )
            .await
            .unwrap();
        assert_eq!(result["approval"]["state"], "denied");
        wait_for_job_status(&backend, &job_id, macbot_durable::JobStatus::Cancelled).await;
        let trace = backend
            .store
            .read_jsonl::<Value>(format!("data/traces/{chat_id}.jsonl"))
            .unwrap();
        assert!(trace.iter().any(|item| {
            item["run_id"] == "run-private-deny-live"
                && item["type"] == "run.end"
                && item["data"]["status"] == "cancelled"
        }));
        assert!(!trace.iter().any(|item| {
            item["run_id"] == "run-private-deny-live"
                && item["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("tool."))
        }));
        let messages = backend
            .store
            .read_jsonl::<Value>(format!("data/chats/{chat_id}/messages.jsonl"))
            .unwrap();
        assert_eq!(messages.last().unwrap()["streaming"], false);
        assert_eq!(messages.last().unwrap()["fallback_text"], "已停止");
    }

    #[tokio::test]
    async fn denied_private_waiting_recovery_cancels_only_exact_job() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let backend = Arc::new(ProductionBackend::open(&path).unwrap());
        let (denied_id, denied_job, denied_chat) =
            seed_private_waiting_approval(&backend, &gateway.state, "recovery-denied").await;
        backend
            .call(
                "approval.decide",
                json!({"approval_id":denied_id,"decision":"deny"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let (pending_id, pending_job, _) =
            seed_private_waiting_approval(&backend, &gateway.state, "recovery-pending").await;
        let reopened = ComposedBackend::open(backend.clone(), gateway.state.clone(), path).unwrap();
        let _ = reopened;
        wait_for_job_status(&backend, &denied_job, macbot_durable::JobStatus::Cancelled).await;
        assert_eq!(
            backend
                .durable
                .lock()
                .await
                .job(&pending_job)
                .unwrap()
                .status,
            macbot_durable::JobStatus::Waiting
        );
        let trace = backend
            .store
            .read_jsonl::<Value>(format!("data/traces/{denied_chat}.jsonl"))
            .unwrap();
        assert!(trace.iter().any(|item| {
            item["run_id"] == "run-private-deny-recovery-denied"
                && item["type"] == "run.end"
                && item["data"]["status"] == "cancelled"
        }));
        let snapshot = backend.orchestrator.snapshot().unwrap();
        assert_eq!(snapshot["approvals"][&pending_id]["state"], "pending");
    }

    #[tokio::test]
    async fn startup_recovers_only_unread_private_message_without_durable_execution() {
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
                json!({"name":"orphan-recovery"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let provider = backend
            .call(
                "provider.create",
                json!({"name":"Recovery Provider","api_kind":"openai-completions","base_url":"http://127.0.0.1:1","client_request_id":"recovery-provider"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let provider_id = provider["provider"]["id"].as_str().unwrap();
        backend
            .call(
                "model.upsert",
                json!({"provider_id":provider_id,"model_id":"recovery-model","client_request_id":"recovery-model"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let model_ref = format!("{provider_id}/recovery-model");
        backend
            .store
            .write_snapshot(
                "data/settings.json",
                &json!({"models":{"bot_default":model_ref,"main":null}}),
            )
            .unwrap();
        let sent = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"recover this private turn"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let message_id = sent["message"]["id"].as_str().unwrap().to_owned();
        backend
            .store
            .write_snapshot(
                format!(
                    "data/chats/{}/metadata.json",
                    super::safe_component(&chat_id)
                ),
                &json!({"last_read_seq":0}),
            )
            .unwrap();
        let dispatch_gate = Arc::new(Notify::new());
        let composed = ComposedBackend::open_with_dispatch_gate(
            backend.clone(),
            gateway.state.clone(),
            path.clone(),
            dispatch_gate.clone(),
        )
        .unwrap();
        let run_id = format!(
            "run_chat_{}",
            message_id
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect::<String>()
        );
        for _ in 0..500 {
            if backend
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
            .unwrap()
            .is_some());
        // The real startup path is held immediately before assignment
        // dispatch.  Seeing the orphan request first proves this recovery is
        // independent of that later, potentially slow phase.
        assert!(!composed.recovery_done.load(Ordering::Acquire));
        dispatch_gate.notify_waiters();
        wait_for_startup_recovery(&composed).await;
        // The same recovery pass is idempotent once the request exists.
        composed.recover_orphan_private_messages().await;
        let durable = backend.durable.lock().await;
        assert_eq!(
            durable
                .jobs()
                .filter(|job| job.checkpoint.get("run_id").and_then(Value::as_str) == Some(&run_id))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn startup_does_not_replay_private_message_while_approval_is_pending() {
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
                json!({"name":"orphan-approval"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let provider = backend
            .call(
                "provider.create",
                json!({"name":"Approval Recovery Provider","api_kind":"openai-completions","base_url":"http://127.0.0.1:1","client_request_id":"approval-recovery-provider"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let provider_id = provider["provider"]["id"].as_str().unwrap();
        backend
            .call(
                "model.upsert",
                json!({"provider_id":provider_id,"model_id":"recovery-model","client_request_id":"approval-recovery-model"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let model_ref = format!("{provider_id}/recovery-model");
        backend
            .store
            .write_snapshot(
                "data/settings.json",
                &json!({"models":{"bot_default":model_ref,"main":null}}),
            )
            .unwrap();
        let sent = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"do not replay while approval is pending"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let message_id = sent["message"]["id"].as_str().unwrap().to_owned();
        backend
            .call(
                "approval.request",
                json!({
                    "bot_id":bot_id,
                    "assignment_id":null,
                    "chat_id":chat_id,
                    "tool":"memory",
                    "risk":"write",
                    "summary":"pending recovery guard",
                    "detail":"{}"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        backend
            .store
            .write_snapshot(
                format!(
                    "data/chats/{}/metadata.json",
                    super::safe_component(&chat_id)
                ),
                &json!({"last_read_seq":0}),
            )
            .unwrap();
        let composed = ComposedBackend::open(backend.clone(), gateway.state, path).unwrap();
        let run_id = format!(
            "run_chat_{}",
            message_id
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect::<String>()
        );
        wait_for_startup_recovery(&composed).await;
        assert!(backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn startup_skips_private_orphan_while_question_is_pending() {
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
                json!({"name":"question-recovery"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        configure_recovery_model(&backend, &gateway.state, "question-recovery").await;
        let message = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"wait for the question"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let run_id = private_run_id(message["message"]["id"].as_str().unwrap());
        let mut snapshot = backend.orchestrator.snapshot().unwrap();
        snapshot["questions"]["question-recovery"] = json!({
            "id":"question-recovery",
            "chat_id":chat_id,
            "bot_id":bot_id,
            "assignment_id":format!("dm_{chat_id}"),
            "text":"pending question",
            "options":[],
            "allow_free_text":true,
            "state":"pending"
        });
        backend.orchestrator.restore(snapshot).unwrap();
        backend
            .store
            .write_snapshot(
                format!(
                    "data/chats/{}/metadata.json",
                    super::safe_component(&chat_id)
                ),
                &json!({"last_read_seq":0}),
            )
            .unwrap();
        let composed = ComposedBackend::open(backend.clone(), gateway.state, path).unwrap();
        wait_for_startup_recovery(&composed).await;
        assert!(backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn startup_skips_private_orphan_while_same_chat_has_waiting_run() {
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
                json!({"name":"busy-recovery"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        configure_recovery_model(&backend, &gateway.state, "busy-recovery").await;
        // A non-terminal job without a run request is still unknown work for
        // this Bot; recovery must conservatively leave the chat untouched.
        let checkpoint = json!({"waiting_reason":"decision"});
        let existing_job = {
            let mut durable = backend.durable.lock().await;
            let job = durable
                .create_job(&bot_id, "model_run", checkpoint.clone())
                .unwrap();
            durable
                .commit(
                    &job.id,
                    macbot_durable::JobStatus::Waiting,
                    checkpoint,
                    false,
                )
                .unwrap()
        };
        let orphan = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"must wait behind existing run"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let orphan_run_id = private_run_id(orphan["message"]["id"].as_str().unwrap());
        backend
            .store
            .write_snapshot(
                format!(
                    "data/chats/{}/metadata.json",
                    super::safe_component(&chat_id)
                ),
                &json!({"last_read_seq":0}),
            )
            .unwrap();
        let composed = ComposedBackend::open(backend.clone(), gateway.state, path).unwrap();
        wait_for_startup_recovery(&composed).await;
        assert!(backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{orphan_run_id}.json"))
            .unwrap()
            .is_none());
        assert_eq!(
            backend
                .durable
                .lock()
                .await
                .job(&existing_job.id)
                .unwrap()
                .status,
            macbot_durable::JobStatus::Waiting
        );
    }

    #[tokio::test]
    async fn startup_claims_at_most_one_orphan_per_private_chat() {
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
                json!({"name":"multi-recovery"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        configure_recovery_model(&backend, &gateway.state, "multi-recovery").await;
        let first = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"first orphan"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let second = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"second orphan"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let first_run_id = private_run_id(first["message"]["id"].as_str().unwrap());
        let second_run_id = private_run_id(second["message"]["id"].as_str().unwrap());
        backend
            .store
            .write_snapshot(
                format!(
                    "data/chats/{}/metadata.json",
                    super::safe_component(&chat_id)
                ),
                &json!({"last_read_seq":0}),
            )
            .unwrap();
        let composed = ComposedBackend::open(backend.clone(), gateway.state.clone(), path).unwrap();
        for _ in 0..500 {
            if backend
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{first_run_id}.json"))
                .unwrap()
                .is_some()
                || backend
                    .store
                    .read_snapshot::<ExecutionRequest>(format!(
                        "data/run_requests/{second_run_id}.json"
                    ))
                    .unwrap()
                    .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let first_exists = backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{first_run_id}.json"))
            .unwrap()
            .is_some();
        let second_exists = backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{second_run_id}.json"))
            .unwrap()
            .is_some();
        assert_ne!(first_exists, second_exists);
        // A later explicit pass still cannot claim the second message in the
        // same process; it remains durable for a later recovery boundary.
        composed.recover_orphan_private_messages().await;
        assert_eq!(
            backend
                .store
                .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{first_run_id}.json"))
                .unwrap()
                .is_some() as u8
                + backend
                    .store
                    .read_snapshot::<ExecutionRequest>(format!(
                        "data/run_requests/{second_run_id}.json"
                    ))
                    .unwrap()
                    .is_some() as u8,
            1
        );
    }

    #[tokio::test]
    async fn startup_does_not_replay_message_with_terminal_execution_record() {
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
                json!({"name":"terminal-recovery"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let chat_id = bot["bot"]["dm_chat_id"].as_str().unwrap().to_owned();
        configure_recovery_model(&backend, &gateway.state, "terminal-recovery").await;
        let message = backend
            .call(
                "chat.send",
                json!({"chat_id":chat_id,"text":"already terminal"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let message_id = message["message"]["id"].as_str().unwrap();
        let run_id = private_run_id(message_id);
        let request: ExecutionRequest = serde_json::from_value(json!({
            "run_id":run_id,
            "assignment_id":null,
            "chat_id":chat_id,
            "bot_id":bot_id,
            "model":"unconfigured/model",
            "instruction":"already terminal",
            "private":true
        }))
        .unwrap();
        backend
            .store
            .write_snapshot(format!("data/run_requests/{run_id}.json"), &request)
            .unwrap();
        let checkpoint = json!({"run_id":run_id});
        {
            let mut durable = backend.durable.lock().await;
            let job = durable
                .create_job(&request.bot_id, "model_run", checkpoint.clone())
                .unwrap();
            durable
                .commit(&job.id, macbot_durable::JobStatus::Done, checkpoint, false)
                .unwrap();
        }
        backend
            .store
            .write_snapshot(
                format!(
                    "data/chats/{}/metadata.json",
                    super::safe_component(&chat_id)
                ),
                &json!({"last_read_seq":0}),
            )
            .unwrap();
        let composed = ComposedBackend::open(backend.clone(), gateway.state, path).unwrap();
        wait_for_startup_recovery(&composed).await;
        assert!(backend
            .store
            .read_snapshot::<ExecutionRequest>(format!("data/run_requests/{run_id}.json"))
            .unwrap()
            .is_some());
        assert_eq!(
            backend
                .durable
                .lock()
                .await
                .jobs()
                .filter(|job| job.checkpoint.get("run_id").and_then(Value::as_str) == Some(&run_id))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn approved_recovery_claim_has_one_owner_until_release() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let inner = Arc::new(ProductionBackend::open(&path).unwrap());
        let composed = ComposedBackend::open(inner, gateway.state, path).unwrap();
        assert!(composed.claim_approved_continuation("approval-1"));
        assert!(!composed.claim_approved_continuation("approval-1"));
        composed.release_approved_continuation("approval-1");
        assert!(composed.claim_approved_continuation("approval-1"));
        assert!(composed.claim_denied_cancellation("approval-2"));
        assert!(!composed.claim_denied_cancellation("approval-2"));
        composed.release_denied_cancellation("approval-2");
        assert!(composed.claim_denied_cancellation("approval-2"));
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
    async fn maintenance_report_publishes_durable_event_through_gateway_state() {
        let home = tempdir().unwrap();
        let path = home.path().to_path_buf();
        let gateway = Gateway::new(GatewayConfig {
            home: path.clone(),
            ..Default::default()
        });
        let inner = Arc::new(ProductionBackend::open(&path).unwrap());
        let composed = ComposedBackend::open(inner, gateway.state.clone(), path).unwrap();
        for content in ["first", "second"] {
            composed
                .feature_service
                .shared_memory
                .append_worklog(MemoryTarget::bot("bot-a"), content, MemorySource::default())
                .unwrap();
        }
        composed
            .feature_service
            .set_maintenance_provider(Some(Arc::new(TestMaintenance)))
            .unwrap();
        let mut live = gateway.state.events.subscribe();
        let report = composed
            .feature_service
            .maintenance_tick_with_events(&[MemoryTarget::bot("bot-a")])
            .await;
        let (committed, error) = composed.publish_maintenance_report(report).await;
        assert_eq!(committed, 1);
        assert!(error.is_none());
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), live.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["event"], "memory.updated");
        assert!(event["seq"].as_u64().is_some());
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
        assert!(gateway.state.browser.lock().await.state(&bot_id).is_err());
    }

    #[tokio::test]
    async fn screen_takeover_restore_reads_durable_active_state() {
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
                json!({"name":"screen-takeover-restore"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let bot_id = bot["bot"]["id"].as_str().unwrap().to_owned();
        let record = json!({
            "bot_id": bot_id,
            "assignment_id": format!("browser_takeover_{bot_id}"),
            "state": "active"
        });
        fs::create_dir_all(path.join("data/takeovers")).unwrap();
        fs::write(
            path.join("data/takeovers")
                .join(format!("browser_takeover_{bot_id}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        backend
            .reconcile_browser_takeover(&gateway.state, &bot_id)
            .await
            .unwrap();
        assert!(gateway.state.browser.lock().await.state(&bot_id).is_err());

        let mut released = record;
        released["state"] = json!("done");
        fs::write(
            path.join("data/takeovers")
                .join(format!("browser_takeover_{bot_id}.json")),
            serde_json::to_vec(&released).unwrap(),
        )
        .unwrap();
        backend
            .reconcile_browser_takeover(&gateway.state, &bot_id)
            .await
            .unwrap();
        assert!(gateway.state.browser.lock().await.state(&bot_id).is_err());
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
    async fn model_finish_project_requires_user_confirmation_before_summary() {
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
        assert!(backend
            .orchestrator
            .mark_project_review(&project_id)
            .is_err());
        backend
            .orchestrator
            .finish_assignment(&working_worker_id, "done")
            .unwrap();
        backend
            .orchestrator
            .finish_assignment(&queued_worker_id, "done")
            .unwrap();
        backend
            .orchestrator
            .mark_project_review(&project_id)
            .unwrap();
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
        assert!(runtime
            .finalize_confirmed_project_for_coordination(
                &json!({"project_id":project_id,"summary":"过早总结"}),
                None,
            )
            .await
            .is_err());
        assert_eq!(
            backend.orchestrator.snapshot().unwrap()["projects"][&project_id]["status"],
            "review"
        );
        backend
            .call(
                "project.confirm_done",
                json!({"project_id":project_id,"client_request_id":"user-confirm-model-finish"}),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment = backend
            .call(
                "assignment.create",
                json!({
                    "project_id":project_id,
                    "origin_chat_id":chat_id,
                    "bot_id":"main",
                    "title":"用户确认后的总结",
                    "instruction":"补充确认后的总结",
                    "from":"user"
                }),
                &gateway.state,
            )
            .await
            .unwrap();
        let assignment_id = assignment["id"].as_str().unwrap().to_owned();
        runtime
            .finalize_confirmed_project_for_coordination(
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
            "done"
        );
        assert_eq!(snapshot["assignments"][&queued_worker_id]["status"], "done");
        let entries = feature_service.shared_memory.entries().unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.id == format!("project-summary:{project_id}")));
        assert!(entries
            .iter()
            .any(|entry| { entry.id == format!("project-summary-worklog:{project_id}:main") }));
    }
}
