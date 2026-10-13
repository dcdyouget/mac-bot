//! Model-facing skill, memory and retrieval tools.
//!
//! Transport registration belongs to `backend.rs`; this module owns the actual
//! calls and carries the authenticated run context into `FeatureService`.

use crate::features::{
    FeatureError, FeatureService, MemoryAccess, MemoryActor, UserProfileOnboardingInput,
};
use async_trait::async_trait;
use macbot_memory::{
    ContextPackage, ContextRequest, MemoryChange, MemoryChangeActorKind, MemoryChangeKind,
    MemoryOperation, MemoryScope, MemorySource, MemoryTarget,
};
use macbot_skills::SkillDraftFrom;
use macbot_store::Event;
use macbot_tools::{Risk, Tool, ToolContext, ToolResult};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

type PendingMemorySegment = (String, u64, usize);

/// Per-run identity and visibility. A runtime must be created with the
/// gateway's authenticated user and resolved project membership; tools never
/// infer those values from model arguments.
#[derive(Clone, Debug)]
pub struct FeatureRunContext {
    pub actor: MemoryActor,
    pub access: MemoryAccess,
    pub bot_id: Option<String>,
    pub run_id: Option<String>,
    pub private: bool,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub chat_id: Option<String>,
    pub allow_memory: bool,
    /// This run was admitted by the gateway's main-Bot private onboarding
    /// policy. Model arguments cannot turn this capability on.
    pub profile_onboarding: bool,
    /// Runtime supplied prompt metadata. Defaults are deliberately stable so
    /// older callers keep the same context shape until they provide richer
    /// Bot/project settings.
    pub model_context_window: usize,
    /// Current host timezone, independent of a segment-frozen profile.
    pub host_timezone: Option<String>,
    pub bot_identity: Option<String>,
    pub draft_from: Option<SkillDraftFrom>,
    pub announcement: String,
    pub references: Vec<String>,
    pub segment_summary: String,
    pub recent_context: Vec<String>,
    pub run_events: Vec<String>,
}

impl FeatureRunContext {
    pub fn bot(bot_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        let bot_id = bot_id.into();
        Self {
            actor: MemoryActor::bot(bot_id.clone()),
            access: MemoryAccess::user(user_id),
            bot_id: Some(bot_id),
            run_id: None,
            private: true,
            project_id: None,
            session_id: None,
            chat_id: None,
            allow_memory: true,
            profile_onboarding: false,
            model_context_window: 128_000,
            host_timezone: None,
            bot_identity: None,
            draft_from: None,
            announcement: String::new(),
            references: Vec::new(),
            segment_summary: String::new(),
            recent_context: Vec::new(),
            run_events: Vec::new(),
        }
    }

    pub fn main(bot_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        let bot_id = bot_id.into();
        Self {
            actor: MemoryActor::main(bot_id.clone()),
            access: MemoryAccess::user(user_id),
            bot_id: Some(bot_id),
            run_id: None,
            private: false,
            project_id: None,
            session_id: None,
            chat_id: None,
            allow_memory: true,
            profile_onboarding: false,
            model_context_window: 128_000,
            host_timezone: None,
            bot_identity: None,
            draft_from: None,
            announcement: String::new(),
            references: Vec::new(),
            segment_summary: String::new(),
            recent_context: Vec::new(),
            run_events: Vec::new(),
        }
    }

    /// Supply dynamic identity and prompt metadata resolved by the runtime.
    /// This keeps authenticated context construction outside model arguments.
    pub fn with_context_metadata(
        mut self,
        model_context_window: usize,
        bot_identity: impl Into<String>,
        announcement: impl Into<String>,
        references: Vec<String>,
    ) -> Self {
        self.model_context_window = model_context_window.max(1);
        self.bot_identity = Some(bot_identity.into());
        self.announcement = announcement.into();
        self.references = references;
        self
    }

    /// Build an authenticated feature context from the execution request.
    /// `access` must come from the gateway's authenticated session; this
    /// helper intentionally never trusts user/project values in model args.
    pub fn for_execution(
        request: &crate::execution::ExecutionRequest,
        access: MemoryAccess,
        is_main: bool,
    ) -> Self {
        let mut context = if is_main {
            Self::main(
                request.bot_id.clone(),
                access.user_id.clone().unwrap_or_default(),
            )
        } else {
            Self::bot(
                request.bot_id.clone(),
                access.user_id.clone().unwrap_or_default(),
            )
        };
        context.access = access;
        context.run_id = Some(request.run_id.clone());
        context.private = request.private;
        context.project_id = request.project_id.clone();
        context.session_id = Some(request.chat_id.clone());
        context.chat_id = Some(request.chat_id.clone());
        context.allow_memory = !is_main && !request.subagent;
        context.profile_onboarding = is_main
            && request.private
            && request.assignment_id.is_none()
            && request.project_id.is_none()
            && !request.routine
            && request
                .tools
                .as_ref()
                .is_some_and(|tools| tools.iter().any(|tool| tool == "set_user_profile"));
        context
    }
}

/// Shared feature handle used to build the tools for one model run.
#[derive(Clone)]
pub struct FeatureToolRuntime {
    pub service: Arc<FeatureService>,
    pub run: FeatureRunContext,
    selected_project_id: Arc<Mutex<Option<String>>>,
}

impl FeatureToolRuntime {
    pub fn new(service: Arc<FeatureService>, run: FeatureRunContext) -> Self {
        Self {
            service,
            run,
            selected_project_id: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn select_project_id(&self, project_id: String) -> Result<(), FeatureError> {
        if !self.run.private {
            return Err(FeatureError::NotAllowed(
                "project memory can only be loaded into a private chat".into(),
            ));
        }
        *self
            .selected_project_id
            .lock()
            .map_err(|_| FeatureError::Invalid("project selection lock poisoned".into()))? =
            Some(project_id);
        Ok(())
    }

    /// Construct the per-run tools while retaining the gateway's shared
    /// `FeatureService` handle.  Runtime wiring should call this once per run.
    pub fn for_execution(
        service: Arc<FeatureService>,
        request: &crate::execution::ExecutionRequest,
        access: MemoryAccess,
        is_main: bool,
    ) -> Self {
        Self::new(
            service,
            FeatureRunContext::for_execution(request, access, is_main),
        )
    }

    /// Wrap the normal execution sink with context assembly and transactional
    /// memory hooks.  The delegate remains responsible for event fan-out.
    pub fn execution_sink(
        &self,
        delegate: Arc<dyn crate::execution::ExecutionSink>,
    ) -> FeatureExecutionSink {
        FeatureExecutionSink::new(delegate, self.clone())
    }

    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = if self.run.actor.is_main() {
            Vec::new()
        } else {
            vec![
                Arc::new(SkillTool {
                    runtime: self.clone(),
                }),
                Arc::new(MemoryTool {
                    runtime: self.clone(),
                }),
                Arc::new(MemorySearchTool {
                    runtime: self.clone(),
                }),
                Arc::new(SessionSearchTool {
                    runtime: self.clone(),
                }),
                Arc::new(ProjectFindTool {
                    runtime: self.clone(),
                }),
                Arc::new(ChatHistoryTool {
                    runtime: self.clone(),
                }),
            ]
        };
        // Main is a coordination-only Bot. Draft creation is a work-side
        // mutation and must never be exposed in its model tool set.
        if !self.run.actor.is_main() {
            tools.push(Arc::new(SkillDraftTool {
                runtime: self.clone(),
            }));
        }
        if self.run.profile_onboarding {
            tools.push(Arc::new(SetUserProfileTool {
                runtime: self.clone(),
            }));
        }
        tools
    }

    pub fn context(&self, request: &ContextRequest) -> Result<ContextPackage, FeatureError> {
        self.service.context(request)
    }

    /// Context assembly reports the 80% boundary. The caller may await this
    /// hook after assembling a context to produce a durable segment summary.
    pub async fn compact_context(
        &self,
        request: &ContextRequest,
    ) -> Result<Option<String>, FeatureError> {
        self.service.compact_context(request).await
    }
}

/// Adapter for the gateway execution engine. It keeps transport/event fan-out
/// in the existing sink while making memory context and successful-run commit
/// durable in this feature module.
pub struct FeatureExecutionSink {
    delegate: Arc<dyn crate::execution::ExecutionSink>,
    runtime: FeatureToolRuntime,
    skill_draft_calls: Arc<Mutex<HashSet<String>>>,
    memory_calls: Arc<Mutex<HashSet<String>>>,
    pending_segments: Arc<Mutex<HashMap<String, PendingMemorySegment>>>,
    run_seen_revisions: Arc<Mutex<HashMap<String, u64>>>,
}

impl FeatureExecutionSink {
    pub fn new(
        delegate: Arc<dyn crate::execution::ExecutionSink>,
        runtime: FeatureToolRuntime,
    ) -> Self {
        Self {
            delegate,
            runtime,
            skill_draft_calls: Arc::new(Mutex::new(HashSet::new())),
            memory_calls: Arc::new(Mutex::new(HashSet::new())),
            pending_segments: Arc::new(Mutex::new(HashMap::new())),
            run_seen_revisions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn skill_updated_event(&self, event: &crate::execution::ExecutionEvent) -> Option<Value> {
        if event.event != "trace.item" {
            return None;
        }
        let item = event.data.get("item")?;
        let data = item.get("data")?;
        match item.get("type").and_then(Value::as_str) {
            Some("tool.start")
                if data.get("name").and_then(Value::as_str) == Some("skill_draft") =>
            {
                if let Some(call_id) = data.get("call_id").and_then(Value::as_str) {
                    self.skill_draft_calls
                        .lock()
                        .ok()?
                        .insert(call_id.to_owned());
                }
                None
            }
            Some("tool.end") => {
                let call_id = data.get("call_id").and_then(Value::as_str)?;
                let was_skill_draft = self.skill_draft_calls.lock().ok()?.remove(call_id);
                if !was_skill_draft || data.get("is_error").and_then(Value::as_bool) != Some(false)
                {
                    return None;
                }
                data.get("details")?.get("_skill_updated").cloned()
            }
            _ => None,
        }
    }

    fn memory_failed_event(&self, event: &crate::execution::ExecutionEvent) -> Option<Value> {
        if event.event != "trace.item" {
            return None;
        }
        let item = event.data.get("item")?;
        let data = item.get("data")?;
        let call_id = data.get("call_id")?.as_str()?;
        match item.get("type").and_then(Value::as_str) {
            Some("tool.start") if data.get("name").and_then(Value::as_str) == Some("memory") => {
                self.memory_calls.lock().ok()?.insert(call_id.to_owned());
                None
            }
            Some("tool.end") => {
                let was_memory = self.memory_calls.lock().ok()?.remove(call_id);
                if !was_memory || data.get("is_error").and_then(Value::as_bool) != Some(true) {
                    return None;
                }
                Some(json!({
                    "run_id": self.runtime.run.run_id,
                    "bot_id": self.runtime.run.bot_id,
                    "call_id": call_id,
                    "result": data.get("details").cloned().unwrap_or(Value::Null),
                }))
            }
            _ => None,
        }
    }

    fn persisted_memory_events(&self, event: &crate::execution::ExecutionEvent) -> Vec<Event> {
        if event.event != "trace.item"
            || event.data.pointer("/item/type").and_then(Value::as_str) != Some("tool.end")
            || event
                .data
                .pointer("/item/data/name")
                .and_then(Value::as_str)
                != Some("memory")
        {
            return Vec::new();
        }
        event
            .data
            .pointer("/item/data/details/_persisted_events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| serde_json::from_value(value.clone()).ok())
            .collect()
    }

    async fn record_context_write_failure(
        &self,
        request: &crate::execution::ExecutionRequest,
        error: macbot_memory::MemoryError,
    ) -> Value {
        let call_id = format!("memory-context-{}", uuid::Uuid::new_v4());
        let result = into_memory_result(Err(FeatureError::Memory(error)));
        let payload = result.details;
        self.delegate
            .emit(crate::execution::ExecutionEvent {
                event: "memory.failed".into(),
                data: json!({
                    "run_id": request.run_id,
                    "bot_id": request.bot_id,
                    "call_id": call_id,
                    "stage": "snapshot_cache_put",
                    "result": payload,
                }),
                persistent: true,
            })
            .await;
        json!({"call_id":call_id,"result":payload})
    }

    async fn record_automatic_memory_failure(
        &self,
        request: &crate::execution::ExecutionRequest,
        stage: &'static str,
        error: FeatureError,
    ) {
        let code = error.code();
        tracing::warn!(
            run_id = %request.run_id,
            bot_id = %request.bot_id,
            stage,
            code,
            %error,
            "automatic memory write failed; continuing model execution"
        );
        self.delegate
            .emit(crate::execution::ExecutionEvent {
                event: "memory.failed".into(),
                data: json!({
                    "run_id":request.run_id,
                    "bot_id":request.bot_id,
                    "stage":stage,
                    "code":code,
                    "message":error.to_string(),
                }),
                persistent: true,
            })
            .await;
    }

    fn context_request(&self, request: &crate::execution::ExecutionRequest) -> ContextRequest {
        let is_main = self.runtime.run.actor.is_main();
        let kind = if request.private {
            macbot_memory::ConversationKind::Private
        } else if request.project_id.is_some() {
            macbot_memory::ConversationKind::Group
        } else {
            macbot_memory::ConversationKind::Scheduled
        };
        let mut targets = Vec::new();
        if let Some(user_id) = &self.runtime.run.access.user_id {
            targets.push(MemoryTarget::user(user_id.clone()));
        }
        // The main Bot is a coordinator. Its prompt may use the user's profile
        // and preferences, but should not inherit its own work history or any
        // project context that can pull the coordination thread into doing
        // the underlying work itself.
        if !is_main {
            targets.push(MemoryTarget::bot(request.bot_id.clone()));
        }
        if !is_main && request.private {
            if let Some(project_id) = self
                .runtime
                .selected_project_id
                .lock()
                .ok()
                .and_then(|selected| selected.clone())
            {
                targets.push(MemoryTarget::project(project_id));
            }
        }
        if !is_main {
            if let Some(project_id) = &request.project_id {
                if self
                    .runtime
                    .run
                    .access
                    .project_member_bot_ids
                    .iter()
                    .any(|id| id == &request.bot_id)
                {
                    targets.push(MemoryTarget::project(project_id.clone()));
                }
            }
        }
        let recent_messages: Vec<String> = request
            .messages
            .iter()
            .filter(|message| matches!(message["role"].as_str(), Some("user" | "tool")))
            .filter_map(|message| message.get("content").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let mut recent_context = self.runtime.run.recent_context.clone();
        recent_context.extend(recent_messages.iter().cloned());
        ContextRequest {
            kind,
            memory_targets: targets,
            l0_platform_rules: self.runtime.service.l0_platform_rules().into(),
            l1_bot_identity: self
                .runtime
                .run
                .bot_identity
                .clone()
                .unwrap_or_else(|| format!("Bot id: {}", request.bot_id)),
            announcement: self.runtime.run.announcement.clone(),
            task: request.instruction.clone(),
            trigger: request.instruction.clone(),
            references: self.runtime.run.references.clone(),
            recent_messages,
            segment_summary: self.runtime.run.segment_summary.clone(),
            recent_context,
            run_events: self.runtime.run.run_events.clone(),
            model_context_window: self.runtime.run.model_context_window,
            previous_snapshot: None,
            now: chrono::Utc::now(),
        }
    }
}

#[async_trait]
impl crate::execution::ExecutionSink for FeatureExecutionSink {
    async fn emit(&self, event: crate::execution::ExecutionEvent) {
        let skill_updated = self.skill_updated_event(&event);
        let memory_failed = self.memory_failed_event(&event);
        let memory_events = self.persisted_memory_events(&event);
        self.delegate.emit(event).await;
        for persisted in memory_events {
            self.delegate.publish_persisted(persisted).await;
        }
        if let Some(data) = memory_failed {
            self.delegate
                .emit(crate::execution::ExecutionEvent {
                    event: "memory.failed".into(),
                    data,
                    persistent: true,
                })
                .await;
        }
        if let Some(data) = skill_updated {
            // The tool mutation has already been persisted by the shared
            // SkillRegistry. Route the live update through the same sink as
            // RPC skill mutations so connected clients refresh immediately.
            self.delegate
                .emit(crate::execution::ExecutionEvent {
                    event: "skill.updated".into(),
                    data,
                    persistent: true,
                })
                .await;
        }
    }

    async fn publish_persisted(&self, event: Event) {
        self.delegate.publish_persisted(event).await;
    }

    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        self.delegate.send_group_message(message).await
    }

    async fn validate_send_msg_target(&self, message: &Value) -> Result<(), String> {
        self.delegate.validate_send_msg_target(message).await
    }

    async fn approval_required(&self, data: Value) {
        self.delegate.approval_required(data).await;
    }

    async fn usage_tick(&self, assignment_id: &str, usage: Value) {
        self.delegate.usage_tick(assignment_id, usage).await;
    }

    async fn assignment_usage(&self, assignment_id: &str, usage: Value) {
        self.delegate.assignment_usage(assignment_id, usage).await;
    }

    async fn prepare_model_context(
        &self,
        request: &crate::execution::ExecutionRequest,
    ) -> Result<Option<Value>, String> {
        let thread_id = format!("{}::{}", request.bot_id, request.chat_id);
        let current_revision = self
            .runtime
            .service
            .shared_memory
            .revision()
            .map_err(|error| error.to_string())?;
        let existing_segment = self
            .runtime
            .service
            .shared_memory
            .current_segment_id(&thread_id)
            .map_err(|error| error.to_string())?;
        let had_segment = existing_segment.is_some();
        let mut segment_id = match existing_segment {
            Some(id) => id,
            None => self
                .runtime
                .service
                .shared_memory
                .open_segment(&thread_id, current_revision)
                .map_err(|error| error.to_string())?,
        };
        let mut cache_key = format!("{thread_id}:{segment_id}");
        let mut context_write_errors = Vec::new();
        let mut context_request = self.context_request(request);
        context_request.previous_snapshot = self
            .runtime
            .service
            .shared_memory
            .snapshot_cache_get(&cache_key)
            .map_err(|error| error.to_string())?;
        let cache_missing = context_request.previous_snapshot.is_none();
        let mut package = self
            .runtime
            .context(&context_request)
            .map_err(|error| error.to_string())?;
        if let Err(error) = self
            .runtime
            .service
            .shared_memory
            .snapshot_cache_put(cache_key.clone(), package.snapshot.clone())
        {
            context_write_errors.push(self.record_context_write_failure(request, error).await);
        }

        let segment_highwater = self
            .runtime
            .service
            .shared_memory
            .segment_seen_revision(&segment_id)
            .map_err(|error| error.to_string())?
            .unwrap_or(package.snapshot.revision);
        let baseline_revision = context_request
            .previous_snapshot
            .as_ref()
            .map_or(package.snapshot.revision, |snapshot| snapshot.revision);
        let run_cursor = self
            .run_seen_revisions
            .lock()
            .map_err(|_| "run memory revision lock poisoned".to_owned())?
            .get(&request.run_id)
            .copied()
            .unwrap_or(baseline_revision);
        let changes = self.runtime.service.shared_memory.changes_since(run_cursor);
        let changelog_gap = matches!(
            &changes,
            Err(macbot_memory::MemoryError::ChangeLogGap { .. })
        );
        let changes = match changes {
            Ok(changes) => changes,
            Err(macbot_memory::MemoryError::ChangeLogGap { .. }) => Vec::new(),
            Err(error) => return Err(error.to_string()),
        };
        let update = render_memory_updates(
            changes.clone(),
            &context_request.memory_targets,
            &request.bot_id,
            &request.run_id,
        );
        let newly_observed = changes
            .into_iter()
            .filter(|change| change.revision > segment_highwater)
            .collect();
        let new_delta_chars = render_memory_updates(
            newly_observed,
            &context_request.memory_targets,
            &request.bot_id,
            &request.run_id,
        )
        .map_or(0, |text| text.chars().count());
        let already_delta_chars = self
            .runtime
            .service
            .shared_memory
            .segment_delta_chars(&segment_id)
            .map_err(|error| error.to_string())?;
        let over_delta_limit = already_delta_chars.saturating_add(new_delta_chars) > 1_000;
        let context_window = package.segment_reason.as_deref() == Some("context_80_percent");
        let project_loaded = request.private
            && context_request
                .previous_snapshot
                .as_ref()
                .is_some_and(|snapshot| {
                    context_request.memory_targets.iter().any(|target| {
                        target.scope == MemoryScope::Project && !snapshot.targets.contains(target)
                    })
                });
        let restart_rebuild = had_segment && cache_missing;
        let needs_new_segment = context_window
            || over_delta_limit
            || project_loaded
            || changelog_gap
            || restart_rebuild;
        let mut compaction_summary = None;
        if needs_new_segment {
            let extraction_transcript = maintenance_transcript(request, false);
            let source = MemorySource {
                bot_id: Some(request.bot_id.clone()),
                run_id: Some(request.run_id.clone()),
                session_id: Some(request.chat_id.clone()),
            };
            if !self.runtime.run.actor.is_main() && (had_segment || context_window) {
                let targets = context_request
                    .memory_targets
                    .iter()
                    .filter(|target| {
                        matches!(target.scope, MemoryScope::Bot | MemoryScope::Project)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let extraction = self
                    .runtime
                    .service
                    .maintenance_extract_for_segment(
                        &format!("{}:{}", request.run_id, segment_id),
                        &targets,
                        &extraction_transcript,
                        source.clone(),
                    )
                    .await;
                match extraction {
                    Ok((_, events)) => {
                        for event in events {
                            self.delegate.publish_persisted(event).await;
                        }
                    }
                    Err(error) if error.code() == "quota_full" => {
                        self.record_automatic_memory_failure(request, "segment_extract", error)
                            .await;
                    }
                    Err(error) => return Err(format!("memory extraction failed: {error}")),
                }
            }
            let transcript = maintenance_transcript(request, true);
            let summary = self
                .runtime
                .service
                .summarize_segment(&transcript)
                .await
                .map_err(|error| format!("context summarization failed: {error}"))?;
            if !self.runtime.run.actor.is_main() {
                let persistence = self.runtime.service.persist_context_summary_immediate(
                    &self.runtime.run.actor,
                    &self.runtime.run.access,
                    &request.run_id,
                    MemoryTarget::bot(request.bot_id.clone()),
                    &summary,
                    source,
                );
                match persistence {
                    Ok(events) => {
                        for event in events {
                            self.delegate.publish_persisted(event).await;
                        }
                    }
                    Err(error) if error.code() == "quota_full" => {
                        self.record_automatic_memory_failure(request, "context_summary", error)
                            .await;
                    }
                    Err(error) => {
                        return Err(format!("context summary persistence failed: {error}"));
                    }
                }
            }
            segment_id = self
                .runtime
                .service
                .shared_memory
                .open_segment(
                    &thread_id,
                    self.runtime
                        .service
                        .shared_memory
                        .revision()
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
            cache_key = format!("{thread_id}:{segment_id}");
            context_request.previous_snapshot = None;
            context_request.segment_summary = summary.clone();
            package = self
                .runtime
                .context(&context_request)
                .map_err(|error| error.to_string())?;
            if let Err(error) = self
                .runtime
                .service
                .shared_memory
                .snapshot_cache_put(cache_key, package.snapshot.clone())
            {
                context_write_errors.push(self.record_context_write_failure(request, error).await);
            }
            compaction_summary = Some(summary);
        }
        let (system_a, system_b) = prompt_blocks(
            &package,
            chrono::Utc::now(),
            self.runtime.run.host_timezone.as_deref(),
        );
        let output_update = if needs_new_segment { None } else { update };
        let pending = (
            segment_id,
            current_revision.max(package.snapshot.revision),
            if needs_new_segment {
                0
            } else {
                new_delta_chars
            },
        );
        self.pending_segments
            .lock()
            .map_err(|_| "pending memory segment lock poisoned".to_owned())?
            .insert(request.run_id.clone(), pending);
        Ok(Some(json!({
            "system_blocks":[
                {"level":2,"content":system_a,"cache_control":{"type":"ephemeral"}},
                {"level":3,"content":system_b}
            ],
            "memory_update":output_update,
            "compaction_summary":compaction_summary,
            "memory_errors":context_write_errors
        })))
    }

    async fn model_context_checkpointed(&self, request: &crate::execution::ExecutionRequest) {
        let pending = self
            .pending_segments
            .lock()
            .ok()
            .and_then(|mut state| state.remove(&request.run_id));
        if let Some((segment_id, revision, delta_chars)) = pending {
            let memory = &self.runtime.service.shared_memory;
            if delta_chars > 0 {
                if let Err(error) = memory.add_segment_delta_chars(&segment_id, delta_chars) {
                    tracing::error!(run_id = %request.run_id, %error, "failed to persist memory delta size");
                    self.delegate.emit(crate::execution::ExecutionEvent {
                        event: "memory.failed".into(),
                        data: json!({"run_id":request.run_id,"bot_id":request.bot_id,"code":"segment_checkpoint","message":error.to_string()}),
                        persistent: true,
                    }).await;
                    return;
                }
            }
            if let Err(error) = memory.record_segment_seen(&segment_id, revision) {
                tracing::error!(run_id = %request.run_id, %error, "failed to advance memory segment revision");
                self.delegate.emit(crate::execution::ExecutionEvent {
                    event: "memory.failed".into(),
                    data: json!({"run_id":request.run_id,"bot_id":request.bot_id,"code":"segment_checkpoint","message":error.to_string()}),
                    persistent: true,
                }).await;
                return;
            }
            if let Err(error) = memory.record_segment_seen(&request.run_id, revision) {
                tracing::error!(run_id = %request.run_id, %error, "failed to persist run memory revision");
                self.delegate.emit(crate::execution::ExecutionEvent {
                    event: "memory.failed".into(),
                    data: json!({"run_id":request.run_id,"bot_id":request.bot_id,"code":"run_segment_checkpoint","message":error.to_string()}),
                    persistent: true,
                }).await;
                return;
            }
            if let Ok(mut cursors) = self.run_seen_revisions.lock() {
                cursors.insert(request.run_id.clone(), revision);
            }
        }
    }

    async fn commit_succeeded(&self, request: &crate::execution::ExecutionRequest, data: Value) {
        if request.subagent || self.runtime.run.actor.is_main() {
            return;
        }
        let text = data.get("text").and_then(Value::as_str).unwrap_or_default();
        if text.trim().is_empty() {
            return;
        }
        let summary = self
            .runtime
            .service
            .summarize_worklog(&request.instruction, text)
            .await
            .unwrap_or_else(|_| compact_worklog_summary(&request.instruction, text));
        let source = MemorySource {
            bot_id: Some(request.bot_id.clone()),
            run_id: Some(request.run_id.clone()),
            session_id: Some(request.chat_id.clone()),
        };
        match self.runtime.service.append_worklog_immediate_events(
            &request.run_id,
            MemoryTarget::bot(request.bot_id.clone()),
            &summary,
            source,
        ) {
            Ok((_, events)) => {
                for event in events {
                    self.delegate.publish_persisted(event).await;
                }
            }
            Err(error) => {
                tracing::error!(run_id = %request.run_id, %error, "failed to commit run memory");
                let code = error.code();
                self.delegate
                    .emit(crate::execution::ExecutionEvent {
                        event: "memory.failed".into(),
                        data: json!({"run_id":request.run_id,"bot_id":request.bot_id,"stage":"worklog_write","code":code,"message":error.to_string()}),
                        persistent: true,
                    })
                    .await;
            }
        }
    }
}

fn compact_worklog_summary(instruction: &str, outcome: &str) -> String {
    let action = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
    let result = outcome.split_whitespace().collect::<Vec<_>>().join(" ");
    let action = action.chars().take(180).collect::<String>();
    let result = result.chars().take(180).collect::<String>();
    format!("做了什么：{action}\n结果如何：{result}")
}

struct SkillTool {
    runtime: FeatureToolRuntime,
}

struct SkillDraftTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for SkillDraftTool {
    fn name(&self) -> &str {
        "skill_draft"
    }

    fn description(&self) -> &str {
        "When the user asks to turn the just-completed approach into a skill, create a draft with valid SKILL.md frontmatter (matching name and description) for review. Never publish or enable it automatically."
    }

    fn schema(&self) -> Value {
        json!({
            "type":"object",
            "required":["name","content"],
            "properties":{
                "name":{"type":"string","minLength":1},
                "content":{"type":"string","minLength":1}
            }
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Write
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        if self.runtime.run.actor.is_main() {
            return ToolResult::error("skill_draft is unavailable to the coordination Bot");
        }
        if let Err(error) = validate_skill_draft_tool_args(&args) {
            return ToolResult::error(error);
        }
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            let name = required(&object, "name")?;
            let content = required(&object, "content")?;
            let skill = self.runtime.service.create_skill_draft_with_origin(
                name,
                content,
                self.runtime.run.draft_from.clone(),
            )?;
            // FeatureExecutionSink consumes this private result metadata and
            // emits the normal durable/live skill.updated event after the
            // trace item has been recorded.
            Ok(json!({
                "skill": skill,
                "_skill_updated": {"skill": skill}
            }))
        })();
        into_result(result)
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load a skill's full SKILL.md on demand. A /skill-name prefix is an explicit invocation."
    }

    fn schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "name":{"type":"string"},
                "text":{"type":"string","description":"Original user text; supports /skill-name instruction"}
            }
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }

    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.allow_memory {
            return ToolResult::error("memory tools are unavailable in this run");
        }
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            let bot_id = self.runtime.run.bot_id.as_deref();
            let (detail, instruction) = {
                if let Some(text) = object.get("text").and_then(Value::as_str) {
                    self.runtime
                        .service
                        .load_skill_for_text(text, bot_id)?
                        .ok_or_else(|| {
                            FeatureError::Invalid("no enabled /skill invocation".into())
                        })?
                } else {
                    let name = required(&object, "name")?;
                    if !self
                        .runtime
                        .service
                        .skill_registry
                        .read()
                        .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?
                        .model_invocation_allowed(name)?
                    {
                        return Err(FeatureError::Invalid(
                            "skill requires explicit /skill-name invocation".into(),
                        ));
                    }
                    (
                        self.runtime.service.load_skill_for_bot(name, bot_id)?,
                        String::new(),
                    )
                }
            };
            Ok(json!({"skill":detail.skill,"content":detail.content,"instruction":instruction}))
        })();
        into_result(result)
    }
}

struct MemoryTool {
    runtime: FeatureToolRuntime,
}

struct SetUserProfileTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for SetUserProfileTool {
    fn name(&self) -> &str {
        "set_user_profile"
    }

    fn description(&self) -> &str {
        "Complete the first private-chat profile setup for the user, or mark onboarding skipped. Ask for name, preferred language, and identity; never ask for or change the Host timezone. Use only after the user answers or asks to skip."
    }

    fn schema(&self) -> Value {
        json!({
            "type":"object",
            "required":["expected_version"],
            "properties":{
                "expected_version":{"type":"integer","minimum":1},
                "name":{"type":"string","maxLength":20},
                "language":{"type":"string","maxLength":35},
                "identity":{"type":"string","maxLength":100},
                "complete":{"type":"boolean","description":"Set true once the user has finished providing profile fields."},
                "skip":{"type":"boolean","description":"Set true when the user explicitly skips onboarding; this marks the profile onboarded without changing its fields."}
            },
            "additionalProperties":false
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Write
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.profile_onboarding
            || !matches!(self.runtime.run.actor, MemoryActor::MainBot { .. })
            || !self.runtime.run.private
            || self.runtime.run.project_id.is_some()
            || self.runtime.run.access.user_id.is_none()
        {
            return ToolResult::error("profile onboarding is unavailable in this run");
        }
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "expected_version" | "name" | "language" | "identity" | "complete" | "skip"
                )
            }) {
                return Err(FeatureError::Invalid(
                    "unknown set_user_profile argument".into(),
                ));
            }
            let expected_version = object
                .get("expected_version")
                .and_then(Value::as_u64)
                .filter(|version| *version > 0)
                .ok_or_else(|| FeatureError::Invalid("expected_version is required".into()))?;
            let skip = object.get("skip").and_then(Value::as_bool).unwrap_or(false);
            let complete = object
                .get("complete")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let field = |key: &str| -> Result<Option<String>, FeatureError> {
                match object.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(Value::String(value)) => Ok(Some(value.clone())),
                    Some(_) => Err(FeatureError::Invalid(format!("{key} must be a string"))),
                }
            };
            let bot_id = match &self.runtime.run.actor {
                MemoryActor::MainBot { bot_id } => bot_id,
                _ => unreachable!("actor checked before parsing"),
            };
            let run_id = self
                .runtime
                .run
                .run_id
                .as_deref()
                .filter(|run_id| *run_id == ctx.run_id)
                .ok_or_else(|| FeatureError::Invalid("onboarding run identity mismatch".into()))?;
            let response =
                self.runtime
                    .service
                    .set_user_profile_onboarding(UserProfileOnboardingInput {
                        run_id: run_id.to_owned(),
                        bot_id: bot_id.to_owned(),
                        expected_version,
                        name: field("name")?,
                        language: field("language")?,
                        identity: field("identity")?,
                        complete,
                        skip,
                    })?;
            Ok(response.result)
        })();
        into_result(result)
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Immediately add, replace, or remove Bot/project memory. Replacement and removal require the entry's expected_version. User preferences are written only by the user or curator. Project and Bot targets require explicit project_id or bot_id."
    }

    fn schema(&self) -> Value {
        json!({
            "type":"object",
            "description":"Project and Bot targets require explicit non-empty project_id or bot_id; never infer target IDs from context.",
            "required":["scope"],
            "properties":{
                "scope":{"enum":["bot","project"]},"action":{"enum":["add","replace","remove"],"description":"Defaults to add when omitted."},
                "content":{"type":"string"},"id":{"type":"string","minLength":1},"expected_version":{"type":"integer","minimum":1},"user_id":{"type":"string","minLength":1},
                "bot_id":{"type":"string","minLength":1},"project_id":{"type":"string","minLength":1},"kind":{"type":"string","enum":["user_preference","bot_experience","bot_worklog","project"]}
            },
            "allOf":[
                {"if":{"required":["scope"],"properties":{"scope":{"const":"bot"}}},"then":{"required":["bot_id"]}},
                {"if":{"required":["scope"],"properties":{"scope":{"const":"project"}}},"then":{"required":["project_id"]}},
                {"if":{"required":["action"],"properties":{"action":{"enum":["add","replace"]}}},"then":{"required":["content"]}},
                {"if":{"required":["action"],"properties":{"action":{"enum":["replace","remove"]}}},"then":{"required":["id","expected_version"]}}
            ]
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Write
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.allow_memory {
            return ToolResult::error("memory tools are unavailable in this run");
        }
        if let Err(error) = validate_memory_tool_args(&args) {
            return ToolResult::error(error);
        }
        let mutation = self.runtime.service.memory_mutation_with_access(
            &self.runtime.run.actor,
            &self.runtime.run.access,
            with_run_id(args, &ctx.run_id),
        );
        match mutation {
            Ok((payload, events)) => {
                let mut result = into_memory_result(Ok(payload));
                result.details["_persisted_events"] =
                    serde_json::to_value(events).unwrap_or_else(|_| json!([]));
                result
            }
            Err(error) => into_memory_result(Err(error)),
        }
    }
}

struct MemorySearchTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }
    fn description(&self) -> &str {
        "Search durable memory visible to this Bot and user/group context. Project and Bot targets require explicit project_id or bot_id; IDs are never inferred from run context."
    }
    fn schema(&self) -> Value {
        json!({
            "type":"object",
            "description":"When scope is project or bot, provide an explicit non-empty project_id or bot_id; never infer target IDs from context.",
            "required":["query"],
            "properties":{"query":{"type":"string"},"scope":{"enum":["user","bot","project"]},"user_id":{"type":"string","minLength":1},"bot_id":{"type":"string","minLength":1},"project_id":{"type":"string","minLength":1}},
            "allOf":[
                {"if":{"required":["scope"],"properties":{"scope":{"const":"bot"}}},"then":{"required":["bot_id"]}},
                {"if":{"required":["scope"],"properties":{"scope":{"const":"project"}}},"then":{"required":["project_id"]}}
            ]
        })
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.allow_memory {
            return ToolResult::error("memory tools are unavailable in this run");
        }
        if let Err(error) = validate_memory_search_tool_args(&args) {
            return ToolResult::error(error);
        }
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            let query = required(&object, "query")?;
            let target = target_from_object(&object)?;
            Ok(
                json!({"entries":self.runtime.service.memory_search_for_access(
                &self.runtime.run.actor,
                &self.runtime.run.access,
                query,
                target.as_ref(),
            )?.into_iter().map(|entry| {
                let mut value = serde_json::to_value(&entry).unwrap_or(Value::Null);
                if let Some(object) = value.as_object_mut() {
                    object.insert("version".into(), json!(self.runtime.service.shared_memory.entry_version(&entry.id)?.unwrap_or(1)));
                }
                Ok(value)
            }).collect::<Result<Vec<_>, FeatureError>>()?}),
            )
        })();
        into_result(result)
    }
}

struct SessionSearchTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }
    fn description(&self) -> &str {
        "Search messages in the current session."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["session_id","query"],"properties":{"session_id":{"type":"string"},"query":{"type":"string"}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.allow_memory {
            return ToolResult::error("memory tools are unavailable in this run");
        }
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            let session_id = required(&object, "session_id")?;
            if !self.runtime.run.actor.is_main() {
                let Some(visible) = self.runtime.run.session_id.as_deref() else {
                    return Err(FeatureError::Invalid(
                        "session_search requires the current session context".into(),
                    ));
                };
                if visible != session_id {
                    return Err(FeatureError::Invalid(
                        "session_search is limited to the current session".into(),
                    ));
                }
            }
            let query = required(&object, "query")?;
            Ok(json!({"messages":self.runtime.service.session_search(session_id, query)?}))
        })();
        into_result(result)
    }
}

struct ProjectFindTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for ProjectFindTool {
    fn name(&self) -> &str {
        "project_find"
    }
    fn description(&self) -> &str {
        "Find projects relevant to the current task."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"},"project_id":{"type":"string","description":"Optional id from a returned project result to load its memory into the current private chat."}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.allow_memory {
            return ToolResult::error("memory tools are unavailable in this run");
        }
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            let projects = self.runtime.service.project_find_for_access(
                &self.runtime.run.actor,
                &self.runtime.run.access,
                required(&object, "query")?,
            )?;
            let selected = object.get("project_id").and_then(Value::as_str);
            if let Some(project_id) = selected {
                if !projects.iter().any(|project| project.id == project_id) {
                    return Err(FeatureError::NotAllowed(
                        "project id must be selected from authorized project_find results".into(),
                    ));
                }
                self.runtime.select_project_id(project_id.to_owned())?;
            }
            Ok(json!({"projects":projects,"selected_project_id":selected}))
        })();
        into_result(result)
    }
}

struct ChatHistoryTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for ChatHistoryTool {
    fn name(&self) -> &str {
        "chat_history"
    }
    fn description(&self) -> &str {
        "Read bounded durable history for a chat."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["chat_id"],"properties":{"chat_id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, _ctx: &ToolContext, args: Value) -> ToolResult {
        let result = (|| -> Result<Value, FeatureError> {
            let object = object(&args)?;
            let chat_id = required(&object, "chat_id")?;
            if !self.runtime.run.actor.is_main() {
                let Some(visible) = self.runtime.run.chat_id.as_deref() else {
                    return Err(FeatureError::Invalid(
                        "chat_history requires the current chat context".into(),
                    ));
                };
                if visible != chat_id {
                    return Err(FeatureError::Invalid(
                        "chat_history is limited to the current chat".into(),
                    ));
                }
            }
            let limit = object.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
            Ok(json!({"messages":self.runtime.service.chat_history(chat_id,limit)?}))
        })();
        into_result(result)
    }
}

fn object(value: &Value) -> Result<Map<String, Value>, FeatureError> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| FeatureError::Invalid("tool arguments must be an object".into()))
}

fn required<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, FeatureError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FeatureError::Invalid(format!("{key} is required")))
}

/// Validate draft arguments before the execution engine creates a write
/// approval. Registry-specific checks (name normalization and filesystem
/// safety) remain in `SkillRegistry::create_draft`.
pub fn validate_skill_draft_tool_args(args: &Value) -> Result<(), String> {
    let object = args
        .as_object()
        .ok_or_else(|| "tool arguments must be an object".to_owned())?;
    let name = required(object, "name").map_err(|error| error.to_string())?;
    if name.trim().is_empty() {
        return Err("name is required".into());
    }
    let content = required(object, "content").map_err(|error| error.to_string())?;
    if content.trim().is_empty() {
        return Err("content is required".into());
    }
    Ok(())
}

fn with_run_id(mut args: Value, run_id: &str) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.insert("run_id".into(), Value::String(run_id.to_string()));
        object
            .entry("operation_id")
            .or_insert_with(|| json!(format!("{run_id}:{}", uuid::Uuid::now_v7())));
    }
    args
}

fn render_memory_updates(
    changes: Vec<MemoryChange>,
    visible_targets: &[MemoryTarget],
    bot_id: &str,
    run_id: &str,
) -> Option<String> {
    let mut final_changes = std::collections::BTreeMap::<String, MemoryChange>::new();
    for change in changes {
        if visible_targets.contains(&change.target) {
            let replace = final_changes
                .get(&change.entry_id)
                .is_none_or(|current| current.revision < change.revision);
            if replace {
                final_changes.insert(change.entry_id.clone(), change);
            }
        }
    }
    let mut lines = Vec::new();
    let mut revision = 0;
    for change in final_changes.into_values() {
        revision = revision.max(change.revision);
        if change.actor.kind == MemoryChangeActorKind::Bot
            && change.actor.bot_id.as_deref() == Some(bot_id)
            && change.actor.run_id.as_deref() == Some(run_id)
        {
            continue;
        }
        if change.op == MemoryOperation::Remove {
            lines.push(format!("- remove {}", change.entry_id));
            continue;
        }
        let operation = match change.op {
            MemoryOperation::Add => "add",
            MemoryOperation::Replace => "replace",
            MemoryOperation::Remove => unreachable!(),
        };
        let kind = match change.kind {
            MemoryChangeKind::UserPreference => "user_preference",
            MemoryChangeKind::UserProfile => "user_profile",
            MemoryChangeKind::BotExperience => "bot_experience",
            MemoryChangeKind::ProjectKnowledge => "project_knowledge",
            MemoryChangeKind::Worklog => "worklog",
        };
        let content = if matches!(
            change.kind,
            MemoryChangeKind::Worklog | MemoryChangeKind::ProjectKnowledge
        ) {
            let topic = change
                .content
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("(no topic)")
                .chars()
                .take(80)
                .collect::<String>();
            format!("{operation} {} [{kind}] topic: {topic}", change.entry_id)
        } else {
            format!(
                "{operation} {} [{kind}]: {}",
                change.entry_id, change.content
            )
        };
        lines.push(format!("- {content}"));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "<memory_update revision=\"{revision}\">\n{}\n</memory_update>",
            lines.join("\n")
        )
    })
}

fn prompt_blocks(
    package: &ContextPackage,
    now: chrono::DateTime<chrono::Utc>,
    host_timezone: Option<&str>,
) -> (String, String) {
    let mut l2_lines = package
        .layers
        .iter()
        .find(|layer| layer.level == 2)
        .map(|layer| layer.content.lines().map(str::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    let snapshot_timezone = l2_lines
        .iter()
        .find_map(|line| line.strip_prefix("Host 时区：").map(str::to_owned))
        .unwrap_or_else(|| "UTC".to_owned());
    let timezone = host_timezone.unwrap_or(&snapshot_timezone);
    l2_lines.retain(|line| !line.starts_with("Host 时区："));
    let mut system_a = package
        .layers
        .iter()
        .filter(|layer| layer.level < 2)
        .map(|layer| format!("L{}:\n{}", layer.level, layer.content))
        .collect::<Vec<_>>();
    let memory_text = l2_lines.join("\n");
    if !memory_text.is_empty() {
        system_a.push(format!("L2:\n{memory_text}"));
    }
    if !package.large_entries.is_empty() {
        let catalog = package
            .large_entries
            .iter()
            .map(|entry| format!("- {}: {}", entry.entry_id, entry.topic))
            .collect::<Vec<_>>()
            .join("\n");
        system_a.push(format!(
            "大块记忆目录（正文按需 memory_search）：\n{catalog}"
        ));
    }
    let mut system_b = vec![format!(
        "当前时间：{}\nHost 时区：{timezone}",
        now.to_rfc3339()
    )];
    system_b.extend(
        package
            .layers
            .iter()
            .filter(|layer| layer.level >= 3)
            .map(|layer| format!("L{}:\n{}", layer.level, layer.content)),
    );
    (system_a.join("\n\n"), system_b.join("\n\n"))
}

fn maintenance_transcript(
    request: &crate::execution::ExecutionRequest,
    trim_old_tools: bool,
) -> String {
    let messages = request
        .messages
        .iter()
        .filter(|message| message["role"] != "system")
        .collect::<Vec<_>>();
    let keep_from = messages.len().saturating_sub(4);
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let role = message["role"].as_str().unwrap_or("user");
            let content = message["content"].as_str()?;
            let content = if trim_old_tools && role == "tool" && index < keep_from {
                content.chars().take(240).collect::<String>()
            } else {
                content.to_owned()
            };
            Some(format!("{role}: {content}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn target_from_object(object: &Map<String, Value>) -> Result<Option<MemoryTarget>, FeatureError> {
    let Some(scope) = object.get("scope").and_then(Value::as_str) else {
        return Ok(None);
    };
    let target = match scope {
        "user" => MemoryTarget::user(
            object
                .get("user_id")
                .or_else(|| object.get("owner_id"))
                .and_then(Value::as_str)
                .ok_or_else(|| FeatureError::Invalid("user_id is required".into()))?,
        ),
        "bot" => MemoryTarget::bot(required(object, "bot_id")?),
        "project" => MemoryTarget::project(required(object, "project_id")?),
        _ => {
            return Err(FeatureError::Invalid(format!(
                "unknown memory scope: {scope}"
            )));
        }
    };
    Ok(Some(target))
}

/// Validate model-facing memory arguments before the execution engine creates
/// an approval or opens a durable staging request.  This is deliberately a
/// pure shape/enum check: ownership and membership remain the authenticated
/// `FeatureService` decision, and target IDs are never filled from context.
pub fn validate_memory_tool_args(args: &Value) -> Result<(), String> {
    let object = args
        .as_object()
        .ok_or_else(|| "tool arguments must be an object".to_owned())?;
    let scope = object
        .get("scope")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "scope is required".to_owned())?;
    for key in ["user_id", "owner_id", "bot_id", "project_id"] {
        if object.contains_key(key) {
            required(object, key).map_err(|error| error.to_string())?;
        }
    }
    match scope {
        "user" => return Err("user preferences are written only by the user or curator".into()),
        "bot" => {
            required(object, "bot_id").map_err(|error| error.to_string())?;
        }
        "project" => {
            required(object, "project_id").map_err(|error| error.to_string())?;
        }
        other => return Err(format!("unknown memory scope: {other}")),
    }

    let action = object
        .get("action")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("add");
    if !matches!(action, "add" | "replace" | "remove") {
        return Err(format!("unknown memory action: {action}"));
    }
    if matches!(action, "replace" | "remove") {
        required(object, "id").map_err(|error| error.to_string())?;
        let version = object.get("expected_version").and_then(Value::as_u64);
        if version.is_none() {
            return Err("replace/remove require a non-negative expected_version".into());
        }
    }
    if action != "remove" {
        let content = object
            .get("content")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        if content.is_none() {
            return Err("content is required for add/replace".into());
        }
    }

    if let Some(kind) = object.get("kind") {
        let kind = kind
            .as_str()
            .ok_or_else(|| "kind must be a string".to_owned())?;
        let valid = match scope {
            "user" => kind == "user_preference",
            "bot" => matches!(kind, "bot_experience" | "bot_worklog"),
            "project" => kind == "project",
            _ => false,
        };
        if !valid {
            return Err(format!("memory kind {kind} is not valid for scope {scope}"));
        }
    }
    Ok(())
}

/// Pure validation for the read-only memory search tool.  Unlike writes,
/// search has no action; an omitted scope means the authenticated visible
/// set.  An explicit scope must carry its own owner ID and is never inferred.
pub fn validate_memory_search_tool_args(args: &Value) -> Result<(), String> {
    let object = args
        .as_object()
        .ok_or_else(|| "tool arguments must be an object".to_owned())?;
    required(object, "query").map_err(|error| error.to_string())?;
    for key in ["user_id", "owner_id", "bot_id", "project_id"] {
        if object.contains_key(key) {
            required(object, key).map_err(|error| error.to_string())?;
        }
    }
    let Some(scope) = object.get("scope") else {
        return Ok(());
    };
    let scope = scope
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "scope must be a non-empty string".to_owned())?;
    match scope {
        "user" => {
            object
                .get("user_id")
                .or_else(|| object.get("owner_id"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "user_id is required for scoped search".to_owned())?;
        }
        "bot" => {
            required(object, "bot_id").map_err(|error| error.to_string())?;
        }
        "project" => {
            required(object, "project_id").map_err(|error| error.to_string())?;
        }
        other => return Err(format!("unknown memory scope: {other}")),
    }
    Ok(())
}

fn into_result<T: serde::Serialize>(result: Result<T, FeatureError>) -> ToolResult {
    match result {
        Ok(value) => {
            let details = serde_json::to_value(&value).unwrap_or(Value::Null);
            let text = serde_json::to_string(&details).unwrap_or_else(|_| "{}".into());
            ToolResult {
                content: vec![macbot_tools::Part::Text { text }],
                details,
                is_error: false,
            }
        }
        Err(error) => ToolResult::error(error),
    }
}

fn into_memory_result(result: Result<Value, FeatureError>) -> ToolResult {
    let payload = match result {
        Ok(value) => value,
        Err(error) => match error {
            FeatureError::MemoryMutationConflict(details) => json!({
                "ok":false,
                "code":"conflict",
                "message":"memory entry changed since it was read",
                "current":details.get("current").cloned().unwrap_or(Value::Null),
            }),
            FeatureError::MemoryToolQuotaFull(details) => json!({
                "ok":false,
                "code":"quota_full",
                "message":"memory quota is full; merge or replace an existing entry",
                "entries":details.get("entries").cloned().unwrap_or_else(|| json!([])),
            }),
            FeatureError::Memory(macbot_memory::MemoryError::NotFound(id)) => json!({
                "ok":false,"code":"not_found","message":format!("memory entry not found: {id}"),
            }),
            FeatureError::NotAllowed(message) => json!({
                "ok":false,"code":"not_allowed","message":message,
            }),
            other => json!({"ok":false,"code":"invalid","message":other.to_string()}),
        },
    };
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    let is_error = payload.get("ok") == Some(&Value::Bool(false));
    ToolResult {
        content: vec![macbot_tools::Part::Text { text }],
        details: payload,
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::ExecutionSink;
    use chrono::Utc;
    use macbot_memory::{
        ConversationKind, MemoryAction, MemoryKind, MemoryRequest, MemoryScope, MemorySource,
        MemoryTarget,
    };
    use macbot_providers::{Completion, ModelEvent, ModelProvider, ModelRequest, ToolCall};
    use macbot_tools::{Risk, Tool, ToolContext, ToolResult};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    fn memory_change(value: Value) -> MemoryChange {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn memory_tool_preserves_not_allowed_error_code() {
        let result = into_memory_result(Err(FeatureError::NotAllowed("scope is forbidden".into())));
        assert_eq!(result.details["code"], "not_allowed");
        assert!(result.is_error);
    }

    #[test]
    fn incremental_messages_keep_final_entry_state_filter_self_and_catalog_large_items() {
        let changes = vec![
            memory_change(json!({
                "operation_id":"a1","revision":1,"op":"add","entry_id":"u1",
                "target":{"scope":"user","owner_id":"local-user"},"kind":"user_preference","content":"old",
                "actor":{"kind":"user"},"source":{},"at":"2026-10-12T00:00:00Z"
            })),
            memory_change(json!({
                "operation_id":"a2","revision":2,"op":"replace","entry_id":"u1",
                "target":{"scope":"user","owner_id":"local-user"},"kind":"user_preference","content":"final preference",
                "actor":{"kind":"user"},"source":{},"at":"2026-10-12T00:00:01Z"
            })),
            memory_change(json!({
                "operation_id":"a3","revision":3,"op":"add","entry_id":"b1",
                "target":{"scope":"bot","owner_id":"bot-a"},"kind":"bot_experience","content":"current bot own write",
                "actor":{"kind":"bot","bot_id":"bot-a","run_id":"run-current"},"source":{},"at":"2026-10-12T00:00:02Z"
            })),
            memory_change(json!({
                "operation_id":"a4","revision":4,"op":"add","entry_id":"p1",
                "target":{"scope":"project","owner_id":"project-a"},"kind":"project_knowledge","content":"Decision: ship in two phases\nFurther detail",
                "actor":{"kind":"bot","bot_id":"bot-b","run_id":"run-other"},"source":{},"at":"2026-10-12T00:00:03Z"
            })),
        ];
        let update = render_memory_updates(
            changes,
            &[
                MemoryTarget::user("local-user"),
                MemoryTarget::bot("bot-a"),
                MemoryTarget::project("project-a"),
            ],
            "bot-a",
            "run-current",
        )
        .unwrap();
        assert!(update.contains("revision=\"4\""));
        assert_eq!(update.matches("u1").count(), 1);
        assert!(update.contains("final preference"));
        assert!(!update.contains("old"));
        assert!(!update.contains("current bot own write"));
        assert!(update.contains("p1 [project_knowledge] topic: Decision: ship in two phases"));
        assert!(!update.contains("Further detail"));
    }

    fn runtime() -> FeatureToolRuntime {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        FeatureToolRuntime::new(service, FeatureRunContext::bot("bot-a", "user-a"))
    }

    #[test]
    fn memory_schemas_require_explicit_project_and_bot_ids() {
        let runtime = runtime();
        for tool_name in ["memory", "memory_search"] {
            let schema = runtime
                .tools()
                .into_iter()
                .find(|tool| tool.name() == tool_name)
                .unwrap()
                .schema();
            assert_eq!(schema["properties"]["bot_id"]["minLength"], 1);
            assert_eq!(schema["properties"]["project_id"]["minLength"], 1);
            assert!(schema["description"]
                .as_str()
                .unwrap()
                .contains("never infer"));
            let conditions = schema["allOf"].as_array().unwrap();
            assert!(conditions.iter().any(|condition| {
                condition["if"]["properties"]["scope"]["const"] == "bot"
                    && condition["then"]["required"] == json!(["bot_id"])
            }));
            assert!(conditions.iter().any(|condition| {
                condition["if"]["properties"]["scope"]["const"] == "project"
                    && condition["then"]["required"] == json!(["project_id"])
            }));
        }
    }

    #[test]
    fn memory_argument_validator_rejects_invalid_kind_before_authorization() {
        let error = validate_memory_tool_args(&json!({
            "scope": "project",
            "project_id": "project-a",
            "action": "replace",
            "id": "marker",
            "expected_version": 1,
            "kind": "project_status",
            "content": "done"
        }))
        .unwrap_err();
        assert!(error.contains("not valid for scope project"), "{error}");
    }

    #[test]
    fn memory_argument_validator_matches_supported_actions_and_kinds() {
        assert!(validate_memory_tool_args(&json!({
            "scope": "user",
            "content": "bot tools cannot write user preferences"
        }))
        .is_err());
        assert!(validate_memory_tool_args(&json!({
            "scope": "bot",
            "bot_id": "bot-a",
            "action": "add",
            "kind": "bot_worklog",
            "content": "worked"
        }))
        .is_ok());
        assert!(validate_memory_tool_args(&json!({
            "scope": "project",
            "project_id": "project-a",
            "action": "remove",
            "id": "entry-a",
            "expected_version": 2
        }))
        .is_ok());
        assert!(validate_memory_tool_args(&json!({
            "scope": "bot",
            "bot_id": "bot-a",
            "action": "replace",
            "id": "entry-a",
            "version": 1,
            "content": "legacy alias"
        }))
        .is_err());
        assert!(validate_memory_tool_args(&json!({
            "scope": "bot",
            "bot_id": "bot-a",
            "action": "remove",
            "id": "entry-a"
        }))
        .is_err());
        assert_eq!(
            validate_memory_tool_args(&json!({
                "scope": "project",
                "project_id": "project-a",
                "action": "replace",
                "content": "new",
                "expected_version": 1
            }))
            .unwrap_err(),
            "invalid feature request: id is required"
        );
        assert!(validate_memory_tool_args(&json!({
            "scope": "user",
            "user_id": "user-a",
            "action": "add",
            "kind": "bot_experience",
            "content": "wrong"
        }))
        .unwrap_err()
        .contains("written only by the user or curator"));
    }

    #[test]
    fn memory_search_argument_validator_requires_explicit_scoped_owner() {
        assert!(validate_memory_search_tool_args(&json!({
            "query": "visible"
        }))
        .is_ok());
        assert!(validate_memory_search_tool_args(&json!({
            "scope": "project",
            "project_id": "project-a",
            "query": "visible"
        }))
        .is_ok());
        assert_eq!(
            validate_memory_search_tool_args(&json!({
                "scope": "project",
                "query": "visible"
            }))
            .unwrap_err(),
            "invalid feature request: project_id is required"
        );
        assert_eq!(
            validate_memory_search_tool_args(&json!({
                "scope": "project",
                "project_id": "project-a"
            }))
            .unwrap_err(),
            "invalid feature request: query is required"
        );
    }

    #[test]
    fn execution_context_and_shared_handle_are_stable() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open_shared(home, Vec::new()).unwrap();
        let request = crate::execution::ExecutionRequest {
            run_id: "run-shared".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "chat-a".into(),
            bot_id: "bot-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: Some("project-a".into()),
            instruction: "test".into(),
            messages: Vec::new(),
            max_turns: 1,
            private: false,
            allow_unsafe: false,
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
            client_locale: None,
        };
        let runtime = FeatureToolRuntime::for_execution(
            Arc::clone(&service),
            &request,
            MemoryAccess::group("project-a", vec!["bot-a".into()]),
            false,
        );
        assert!(Arc::ptr_eq(&runtime.service, &service));
        assert_eq!(runtime.run.run_id.as_deref(), Some("run-shared"));
        assert_eq!(runtime.run.session_id.as_deref(), Some("chat-a"));
        assert_eq!(runtime.run.project_id.as_deref(), Some("project-a"));
        assert!(!runtime.run.private);
    }

    #[test]
    fn profile_onboarding_tool_is_private_main_only_and_requires_gateway_admission() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open_shared(home, Vec::new()).unwrap();
        let make_request = |private: bool, project_id: Option<&str>| {
            serde_json::from_value::<crate::execution::ExecutionRequest>(json!({
                "run_id":"run-onboarding",
                "chat_id":"chat-main",
                "bot_id":"main",
                "model":"mock/model",
                "instruction":"hello",
                "private":private,
                "project_id":project_id,
                "tools":["set_user_profile"]
            }))
            .unwrap()
        };

        let private_main = make_request(true, None);
        let main_runtime = FeatureToolRuntime::for_execution(
            Arc::clone(&service),
            &private_main,
            MemoryAccess::user("local-user"),
            true,
        );
        let main_tools = main_runtime
            .tools()
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(main_tools, ["set_user_profile"]);
        assert!(!main_runtime.run.allow_memory);

        let group_request = make_request(false, Some("project-a"));
        let group_runtime = FeatureToolRuntime::for_execution(
            Arc::clone(&service),
            &group_request,
            MemoryAccess::group("project-a", vec!["main".into()]),
            true,
        );
        assert!(!group_runtime
            .tools()
            .iter()
            .any(|tool| tool.name() == "set_user_profile"));

        let bot_runtime = FeatureToolRuntime::for_execution(
            service,
            &private_main,
            MemoryAccess::user("local-user"),
            false,
        );
        assert!(!bot_runtime
            .tools()
            .iter()
            .any(|tool| tool.name() == "set_user_profile"));
        assert!(bot_runtime.run.allow_memory);
        assert!(bot_runtime
            .tools()
            .iter()
            .any(|tool| tool.name() == "memory"));
    }

    #[tokio::test]
    async fn memory_tool_commits_immediately_and_uses_compare_and_swap() {
        let runtime = runtime();
        let memory = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory")
            .unwrap();
        let context = ToolContext::new("/tmp", "run-1", "/tmp/runs");
        let result = memory
            .call(
                &context,
                json!({"scope":"bot","bot_id":"bot-a","action":"add","kind":"bot_experience","content":"likes tests"}),
            )
            .await;
        assert!(!result.is_error, "{}", result.content.len());
        assert_eq!(result.details["ok"], true);
        assert_eq!(result.details["version"], 1);
        assert!(runtime
            .service
            .shared_store
            .events_since(0)
            .unwrap()
            .iter()
            .any(|event| event.event == "memory.updated"));
        assert_eq!(runtime.service.shared_memory.entries().unwrap().len(), 1);
        assert_eq!(runtime.service.shared_memory.entries().unwrap().len(), 1);

        let current = runtime.service.shared_memory.entries().unwrap().remove(0);
        let entry_id = current.id.clone();
        let search = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory_search")
            .unwrap()
            .call(&context, json!({"query":"likes tests"}))
            .await;
        assert_eq!(search.details["entries"][0]["version"], 1);
        let stale = memory.call(
            &context,
            json!({"scope":"bot","bot_id":"bot-a","action":"replace","id":entry_id,"expected_version":0,"content":"stale"}),
        ).await;
        assert!(stale.is_error);
        assert_eq!(stale.details["code"], "conflict");
        assert_eq!(stale.details["current"]["version"], 1);
        let updated = memory.call(
            &context,
            json!({"scope":"bot","bot_id":"bot-a","action":"replace","id":entry_id,"expected_version":1,"content":"new preference"}),
        ).await;
        assert!(!updated.is_error);
        assert_eq!(updated.details["version"], 2);
        let deleted = memory.call(
            &context,
            json!({"scope":"bot","bot_id":"bot-a","action":"remove","id":entry_id,"expected_version":2}),
        ).await;
        assert!(!deleted.is_error);
        assert_eq!(deleted.details["version"], 3);
        assert!(runtime.service.shared_memory.entries().unwrap().is_empty());
    }

    #[tokio::test]
    async fn memory_tool_returns_quota_full_with_existing_entry_directory() {
        let runtime = runtime();
        let memory = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory")
            .unwrap();
        let context = ToolContext::new("/tmp", "run-quota", "/tmp/runs");
        for _ in 0..4 {
            let result = memory
                .call(
                    &context,
                    json!({"scope":"bot","bot_id":"bot-a","kind":"bot_experience","content":"x".repeat(500)}),
                )
                .await;
            assert!(!result.is_error);
        }
        let full = memory
            .call(
                &context,
                json!({"scope":"bot","bot_id":"bot-a","kind":"bot_experience","content":"x"}),
            )
            .await;
        assert!(full.is_error);
        assert_eq!(full.details["code"], "quota_full");
        assert_eq!(full.details["entries"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn memory_search_tool_does_not_leak_other_users_or_bots() {
        let runtime = runtime();
        for target in [MemoryTarget::user("user-other"), MemoryTarget::bot("bot-b")] {
            runtime
                .service
                .shared_memory
                .add(macbot_memory::MemoryRequest {
                    target: target.clone(),
                    action: macbot_memory::MemoryAction::Add,
                    content: format!("secret {}", target.owner_id),
                    id: None,
                    kind: Some(if target.scope == MemoryScope::User {
                        macbot_memory::MemoryKind::UserPreference
                    } else {
                        macbot_memory::MemoryKind::BotExperience
                    }),
                    source: MemorySource::default(),
                })
                .unwrap();
        }
        let search = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory_search")
            .unwrap();
        let result = search
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"query":"secret"}),
            )
            .await;
        assert!(!result.is_error);
        assert_eq!(result.details["entries"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn bot_memory_is_private_and_tool_writes_survive_run_failure() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        let first = FeatureToolRuntime::new(
            Arc::clone(&service),
            FeatureRunContext::bot("bot-a", "user-a"),
        );
        let mut second_context = FeatureRunContext::bot("bot-b", "user-b");
        second_context.session_id = Some("session-b".into());
        let second = FeatureToolRuntime::new(Arc::clone(&service), second_context);
        let memory = first
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory")
            .unwrap();
        let context = ToolContext::new("/tmp", "run-a", "/tmp/runs");
        assert!(!memory
            .call(
                &context,
                json!({"scope":"bot","bot_id":"bot-a","action":"add","kind":"bot_experience","content":"likes concise"}),
            )
            .await
            .is_error);

        let search = second
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory_search")
            .unwrap();
        let hidden = search
            .call(
                &ToolContext::new("/tmp", "run-b", "/tmp/runs"),
                json!({"query":"concise"}),
            )
            .await;
        assert!(!hidden.is_error);
        assert_eq!(hidden.details["entries"].as_array().unwrap().len(), 0);

        assert!(!memory
            .call(
                &ToolContext::new("/tmp", "run-failed", "/tmp/runs"),
                json!({"scope":"bot","bot_id":"bot-a","action":"add","kind":"bot_experience","content":"must persist"}),
            )
            .await
            .is_error);
        assert!(
            service
                .memory_search("must persist", Some(&MemoryTarget::bot("bot-a")))
                .unwrap()
                .len()
                == 1
        );
    }

    #[tokio::test]
    async fn skill_tool_loads_forced_invocation_and_records_it() {
        let runtime = runtime();
        runtime
            .service
            .skill_rpc(
                "skill.create",
                json!({"name":"explicit-only","content":"---\nname: explicit-only\ndescription: explicit\ndisable-model-invocation: true\n---\n# explicit"}),
            )
            .unwrap();
        let skill = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "skill")
            .unwrap();
        let result = skill
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"text":"/project-home use the project home"}),
            )
            .await;
        assert!(!result.is_error);
        assert_eq!(result.details["skill"]["name"], "project-home");
        assert!(result.details["skill"]["last_invoked_at"].is_string());
        assert_eq!(result.details["instruction"], "use the project home");
        let model_loaded = skill
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"name":"project-home"}),
            )
            .await;
        assert!(!model_loaded.is_error);
        assert!(model_loaded.details["skill"]["last_invoked_at"].is_string());
        assert_eq!(model_loaded.details["skill"]["invocations_7d"]["total"], 2);
        let denied = skill
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"name":"explicit-only"}),
            )
            .await;
        assert!(denied.is_error);
        let forced = skill
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"text":"/explicit-only now"}),
            )
            .await;
        assert!(!forced.is_error);
    }

    #[tokio::test]
    async fn skill_draft_tool_validates_registers_and_stays_unpublished() {
        let runtime = runtime();
        let draft = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "skill_draft")
            .expect("worker must receive skill_draft");
        assert_eq!(draft.risk(&json!({})), Risk::Write);
        assert!(
            draft
                .call(
                    &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                    json!({"name":"draft-only"})
                )
                .await
                .is_error
        );
        assert!(validate_skill_draft_tool_args(&json!({
            "name": " ",
            "content": "body"
        }))
        .is_err());

        let result = draft
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"name":"draft-only","content":"---\nname: draft-only\ndescription: Draft\n---\n# Draft"}),
            )
            .await;
        assert!(!result.is_error, "{:?}", result.content);
        assert_eq!(result.details["skill"]["source"], "draft");
        let registry = runtime.service.skill_registry.read().unwrap();
        let error = registry
            .load_for_bot("draft-only", Some("bot-a"))
            .unwrap_err();
        assert!(error.to_string().contains("must be published"), "{error}");
        drop(registry);

        let published = runtime
            .service
            .skill_rpc("skill.publish", json!({"name":"draft-only"}))
            .unwrap();
        assert_eq!(published.result["skill"]["source"], "user");
        assert!(runtime
            .service
            .skill_registry
            .read()
            .unwrap()
            .load_for_bot("draft-only", Some("bot-a"))
            .is_ok());
    }

    #[test]
    fn main_runtime_does_not_register_skill_draft() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        let runtime = FeatureToolRuntime::new(service, FeatureRunContext::main("main", "user-a"));
        assert!(runtime
            .tools()
            .into_iter()
            .all(|tool| tool.name() != "skill_draft"));
    }

    #[tokio::test]
    async fn main_context_ignores_legacy_bot_and_project_memory_without_blocking_run() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        service
            .user_memory_rpc("memory.profile.get", json!({}), "Asia/Shanghai")
            .unwrap();
        service
            .user_memory_rpc(
                "memory.profile.update",
                json!({"name":"Ari","language":"中文","expected_version":1}),
                "UTC",
            )
            .unwrap();
        service
            .user_memory_rpc(
                "memory.user.create",
                json!({"content":"偏好简洁、直接的答复"}),
                "UTC",
            )
            .unwrap();

        let seed_memory = |target, kind, content: String| {
            service
                .shared_memory
                .add(MemoryRequest {
                    target,
                    action: MemoryAction::Add,
                    content,
                    id: None,
                    kind: Some(kind),
                    source: MemorySource::default(),
                })
                .unwrap();
        };
        // Reproduce the legacy state that previously made every main run
        // exceed the worklog quota when it tried to append another summary.
        seed_memory(
            MemoryTarget::bot("main"),
            MemoryKind::BotWorklog,
            format!("MAIN_BOT_WORKLOG_MUST_STAY_PRIVATE {}", "w".repeat(1_340)),
        );
        seed_memory(
            MemoryTarget::bot("main"),
            MemoryKind::BotExperience,
            "MAIN_BOT_EXPERIENCE_MUST_STAY_PRIVATE".into(),
        );
        seed_memory(
            MemoryTarget::project("project-private"),
            MemoryKind::Project,
            "PROJECT_WORK_MUST_STAY_OUT_OF_MAIN_CONTEXT".into(),
        );

        let calls = Arc::new(StdMutex::new(Vec::new()));
        let summarized = Arc::new(StdMutex::new(String::new()));
        service
            .set_maintenance_provider(Some(Arc::new(OrderedMaintenance {
                calls: calls.clone(),
                summarized,
            })))
            .unwrap();
        let mut main_context = FeatureRunContext::main("main", "local-user");
        main_context.model_context_window = 2;
        main_context.run_id = Some("main-quota-run".into());
        main_context.chat_id = Some("main-chat".into());
        main_context.session_id = Some("main-chat".into());
        let main_sink = FeatureExecutionSink::new(
            Arc::new(EventRecordingSink::default()),
            FeatureToolRuntime::new(service.clone(), main_context),
        );
        let main_request = serde_json::from_value::<crate::execution::ExecutionRequest>(json!({
            "run_id":"main-quota-run",
            "chat_id":"main-chat",
            "bot_id":"main",
            "model":"mock/model",
            "instruction":"coordinate this request",
            "private":true,
            "project_id":"project-private"
        }))
        .unwrap();

        let context = main_sink
            .prepare_model_context(&main_request)
            .await
            .expect("legacy main memory must not prevent context assembly")
            .unwrap();
        let system = context["system_blocks"][0]["content"].as_str().unwrap();
        assert!(system.contains("称呼：Ari"));
        assert!(system.contains("语言：中文"));
        assert!(system.contains("偏好简洁、直接的答复"));
        assert!(!system.contains("MAIN_BOT_EXPERIENCE_MUST_STAY_PRIVATE"));
        assert!(!system.contains("MAIN_BOT_WORKLOG_MUST_STAY_PRIVATE"));
        assert!(!system.contains("PROJECT_WORK_MUST_STAY_OUT_OF_MAIN_CONTEXT"));
        assert!(!system.contains("project-private"));
        assert_eq!(*calls.lock().unwrap(), vec!["summarize"]);

        // A completed main run must not try to append another worklog to the
        // already-full legacy bot:main scope.
        main_sink
            .commit_succeeded(&main_request, json!({"text":"x".repeat(1_146)}))
            .await;
        assert_eq!(
            service
                .shared_memory
                .entries()
                .unwrap()
                .iter()
                .filter(|entry| {
                    entry.target == MemoryTarget::bot("main")
                        && entry.kind == MemoryKind::BotWorklog
                })
                .map(|entry| entry.content.chars().count())
                .sum::<usize>(),
            1_375
        );

        // Worker Bots still receive their own experience and large-memory
        // directory, and keep the normal successful-run worklog behavior.
        seed_memory(
            MemoryTarget::bot("worker-a"),
            MemoryKind::BotExperience,
            "WORKER_EXPERIENCE_REMAINS_AVAILABLE".into(),
        );
        seed_memory(
            MemoryTarget::bot("worker-a"),
            MemoryKind::BotWorklog,
            "worker prior work: shipped the search screen".into(),
        );
        let mut worker_context = FeatureRunContext::bot("worker-a", "local-user");
        worker_context.run_id = Some("worker-quota-run".into());
        worker_context.chat_id = Some("worker-chat".into());
        let worker_runtime = FeatureToolRuntime::new(service.clone(), worker_context);
        worker_runtime
            .select_project_id("project-private".into())
            .unwrap();
        let worker_sink =
            FeatureExecutionSink::new(Arc::new(EventRecordingSink::default()), worker_runtime);
        let worker_request = serde_json::from_value::<crate::execution::ExecutionRequest>(json!({
            "run_id":"worker-quota-run",
            "chat_id":"worker-chat",
            "bot_id":"worker-a",
            "model":"mock/model",
            "instruction":"continue the implementation",
            "private":true
        }))
        .unwrap();
        let worker_prepared = worker_sink
            .prepare_model_context(&worker_request)
            .await
            .unwrap()
            .unwrap();
        let worker_system = worker_prepared["system_blocks"][0]["content"]
            .as_str()
            .unwrap();
        assert!(worker_system.contains("WORKER_EXPERIENCE_REMAINS_AVAILABLE"));
        assert!(worker_system.contains("worker prior work: shipped the search screen"));
        assert!(worker_system.contains("PROJECT_WORK_MUST_STAY_OUT_OF_MAIN_CONTEXT"));
        worker_sink
            .commit_succeeded(
                &worker_request,
                json!({"text":"worker finished the requested implementation"}),
            )
            .await;
        assert_eq!(
            service
                .shared_memory
                .entries()
                .unwrap()
                .iter()
                .filter(|entry| {
                    entry.target == MemoryTarget::bot("worker-a")
                        && entry.kind == MemoryKind::BotWorklog
                })
                .count(),
            2
        );
    }

    #[derive(Default)]
    struct EventRecordingSink {
        events: StdMutex<Vec<crate::execution::ExecutionEvent>>,
    }

    #[async_trait]
    impl crate::execution::ExecutionSink for EventRecordingSink {
        async fn emit(&self, event: crate::execution::ExecutionEvent) {
            self.events.lock().unwrap().push(event);
        }

        async fn send_group_message(&self, message: Value) -> Result<Value, String> {
            Ok(message)
        }

        async fn validate_send_msg_target(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }

        async fn approval_required(&self, _: Value) {}
    }

    struct DurableEventSink {
        store: macbot_store::Store,
    }

    #[async_trait]
    impl crate::execution::ExecutionSink for DurableEventSink {
        async fn emit(&self, event: crate::execution::ExecutionEvent) {
            if event.persistent {
                self.store.append_event(event.event, event.data).unwrap();
            }
        }

        async fn publish_persisted(&self, _event: macbot_store::Event) {}

        async fn send_group_message(&self, message: Value) -> Result<Value, String> {
            Ok(message)
        }

        async fn validate_send_msg_target(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }

        async fn approval_required(&self, _: Value) {}
    }

    struct SnapshotFailureProvider {
        requests: Arc<StdMutex<Vec<ModelRequest>>>,
    }

    #[async_trait]
    impl ModelProvider for SnapshotFailureProvider {
        async fn stream(
            &self,
            request: ModelRequest,
            _: tokio::sync::mpsc::Sender<ModelEvent>,
        ) -> macbot_providers::Result<Completion> {
            let mut requests = self.requests.lock().unwrap();
            let first_request = requests.is_empty();
            requests.push(request);
            Ok(if first_request {
                Completion {
                    tool_calls: vec![ToolCall {
                        call_id: "break-memory-state".into(),
                        name: "break_memory_state".into(),
                        args: json!({}),
                    }],
                    stop_reason: "tool_calls".into(),
                    ..Default::default()
                }
            } else {
                Completion {
                    text: "continued after memory error".into(),
                    stop_reason: "stop".into(),
                    ..Default::default()
                }
            })
        }
    }

    struct BreakMemoryStateTool {
        path: PathBuf,
    }

    #[async_trait]
    impl Tool for BreakMemoryStateTool {
        fn name(&self) -> &str {
            "break_memory_state"
        }

        fn description(&self) -> &str {
            "Inject a persistent memory state write failure for the regression test."
        }

        fn schema(&self) -> Value {
            json!({"type":"object","properties":{}})
        }

        fn risk(&self, _: &Value) -> Risk {
            Risk::Read
        }

        async fn call(&self, _: &ToolContext, _: Value) -> ToolResult {
            match fs::create_dir(&self.path) {
                Ok(()) => ToolResult::text("injected persistent memory state failure"),
                Err(error) => ToolResult::error(error),
            }
        }
    }

    struct OrderedMaintenance {
        calls: Arc<StdMutex<Vec<String>>>,
        summarized: Arc<StdMutex<String>>,
    }

    #[async_trait]
    impl macbot_memory::AsyncMaintenanceProvider for OrderedMaintenance {
        async fn summarize(&self, text: &str) -> Result<String, macbot_memory::MemoryError> {
            self.calls.lock().unwrap().push("summarize".into());
            *self.summarized.lock().unwrap() = text.to_owned();
            Ok("compact durable conversation".into())
        }

        async fn extract(
            &self,
            _text: &str,
        ) -> Result<Vec<macbot_memory::MemoryDraft>, macbot_memory::MemoryError> {
            self.calls.lock().unwrap().push("extract".into());
            Ok(Vec::new())
        }
    }

    struct QuotaMaintenance;

    #[async_trait]
    impl macbot_memory::AsyncMaintenanceProvider for QuotaMaintenance {
        async fn summarize(&self, _text: &str) -> Result<String, macbot_memory::MemoryError> {
            Ok("s".repeat(440))
        }

        async fn extract_for_targets(
            &self,
            _text: &str,
            targets: &[MemoryTarget],
        ) -> Result<Vec<macbot_memory::MemoryDraft>, macbot_memory::MemoryError> {
            let target = targets
                .iter()
                .find(|target| target.scope == MemoryScope::Bot)
                .cloned()
                .ok_or_else(|| {
                    macbot_memory::MemoryError::Invalid(
                        "test expected a Bot extraction target".into(),
                    )
                })?;
            Ok(vec![macbot_memory::MemoryDraft {
                target,
                kind: macbot_memory::MemoryKind::BotWorklog,
                content: "e".repeat(440),
                source: MemorySource::default(),
            }])
        }
    }

    struct RecordingFinalProvider {
        requests: Arc<StdMutex<Vec<ModelRequest>>>,
    }

    #[async_trait]
    impl ModelProvider for RecordingFinalProvider {
        async fn stream(
            &self,
            request: ModelRequest,
            _events: tokio::sync::mpsc::Sender<ModelEvent>,
        ) -> macbot_providers::Result<Completion> {
            self.requests.lock().unwrap().push(request);
            Ok(Completion {
                text: "task completed despite full automatic memory".into(),
                stop_reason: "stop".into(),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn quota_full_automatic_memory_writes_do_not_block_worker_model_run() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(&home, Vec::new()).unwrap());
        service
            .shared_memory
            .add(MemoryRequest {
                target: MemoryTarget::bot("worker-a"),
                action: MemoryAction::Add,
                content: "w".repeat(1_401),
                id: None,
                kind: Some(macbot_memory::MemoryKind::BotWorklog),
                source: MemorySource::default(),
            })
            .unwrap();
        service
            .shared_memory
            .add(MemoryRequest {
                target: MemoryTarget::bot("worker-a"),
                action: MemoryAction::Add,
                content: "KEEP_WORKER_L2_AFTER_AUTOMATIC_QUOTA_FAILURE".into(),
                id: None,
                kind: Some(macbot_memory::MemoryKind::BotExperience),
                source: MemorySource::default(),
            })
            .unwrap();
        service
            .set_maintenance_provider(Some(Arc::new(QuotaMaintenance)))
            .unwrap();

        let request = crate::execution::ExecutionRequest {
            run_id: "quota-full-worker-run".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "quota-full-worker-chat".into(),
            bot_id: "worker-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "complete a small worker task".into(),
            messages: vec![json!({"role":"user","content":"please proceed"})],
            max_turns: 1,
            private: true,
            allow_unsafe: false,
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
            client_locale: None,
        };
        let mut run = FeatureRunContext::bot("worker-a", "user-a");
        run.run_id = Some(request.run_id.clone());
        run.chat_id = Some(request.chat_id.clone());
        run.session_id = Some(request.chat_id.clone());
        run.model_context_window = 2;
        let runtime = FeatureToolRuntime::new(service.clone(), run);
        let sink = FeatureExecutionSink::new(
            Arc::new(DurableEventSink {
                store: service.shared_store.clone(),
            }),
            runtime.clone(),
        );
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let engine = crate::execution::ExecutionEngine::new(
            service.shared_store.clone(),
            Arc::new(RecordingFinalProvider {
                requests: requests.clone(),
            }),
            runtime.tools(),
            Arc::new(sink),
            home.clone(),
        )
        .unwrap();

        let outcome = engine.run(request).await.unwrap();
        assert_eq!(outcome.status, "done");
        assert_eq!(outcome.text, "task completed despite full automatic memory");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "the worker must reach llm.request");
        assert!(requests[0].messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"].as_str().is_some_and(|text| {
                    text.contains("KEEP_WORKER_L2_AFTER_AUTOMATIC_QUOTA_FAILURE")
                })
        }));
        drop(requests);

        let events = service.shared_store.events_since(0).unwrap();
        let failures = events
            .iter()
            .filter(|event| event.event == "memory.failed")
            .map(|event| event.data.clone())
            .collect::<Vec<_>>();
        for stage in ["segment_extract", "context_summary", "worklog_write"] {
            assert!(
                events.iter().any(|event| {
                    event.event == "memory.failed"
                        && event.data["run_id"] == "quota-full-worker-run"
                        && event.data["stage"] == stage
                        && event.data["code"] == "quota_full"
                }),
                "missing observable quota failure for {stage}; failures={failures:?}"
            );
        }
        assert!(events.iter().any(|event| {
            event.event == "trace.item"
                && event.data.pointer("/item/type").and_then(Value::as_str) == Some("llm.request")
        }));
        assert!(events.iter().any(|event| {
            event.event == "trace.item"
                && event.data.pointer("/item/type").and_then(Value::as_str) == Some("run.end")
                && event.data.pointer("/item/data/status") == Some(&json!("done"))
        }));
    }

    #[tokio::test]
    async fn context_window_segment_extracts_trims_summarizes_then_rebuilds_l2() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let summarized = Arc::new(StdMutex::new(String::new()));
        service
            .set_maintenance_provider(Some(Arc::new(OrderedMaintenance {
                calls: calls.clone(),
                summarized: summarized.clone(),
            })))
            .unwrap();
        let mut warm_run = FeatureRunContext::bot("bot-a", "user-a");
        warm_run.model_context_window = 100_000;
        warm_run.run_id = Some("warm-run".into());
        warm_run.chat_id = Some("chat-segment".into());
        warm_run.session_id = Some("chat-segment".into());
        let delegate = Arc::new(EventRecordingSink::default());
        let sink = FeatureExecutionSink::new(
            delegate.clone(),
            FeatureToolRuntime::new(service.clone(), warm_run),
        );
        let request = crate::execution::ExecutionRequest {
            run_id: "warm-run".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "chat-segment".into(),
            bot_id: "bot-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "finish the setup".into(),
            messages: vec![json!({"role":"user","content":"starting"})],
            max_turns: 1,
            private: true,
            allow_unsafe: false,
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
            client_locale: None,
        };
        sink.prepare_model_context(&request).await.unwrap().unwrap();
        sink.model_context_checkpointed(&request).await;

        let mut compact_run = FeatureRunContext::bot("bot-a", "user-a");
        compact_run.model_context_window = 2;
        compact_run.run_id = Some("compact-run".into());
        compact_run.chat_id = Some("chat-segment".into());
        compact_run.session_id = Some("chat-segment".into());
        let sink =
            FeatureExecutionSink::new(delegate, FeatureToolRuntime::new(service, compact_run));
        let mut compact_request = request;
        compact_request.run_id = "compact-run".into();
        compact_request.messages = vec![
            json!({"role":"tool","content":"z".repeat(500)}),
            json!({"role":"user","content":"u1"}),
            json!({"role":"assistant","content":"a1"}),
            json!({"role":"tool","content":"recent tool output"}),
            json!({"role":"user","content":"current question"}),
        ];
        let context = sink
            .prepare_model_context(&compact_request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["extract", "summarize"]);
        assert!(summarized.lock().unwrap().contains(&"z".repeat(240)));
        assert!(!summarized.lock().unwrap().contains(&"z".repeat(241)));
        assert!(context["compaction_summary"].as_str().is_some());
        assert_eq!(context["system_blocks"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn persistent_snapshot_write_failure_is_returned_and_persisted() {
        let directory = tempdir().unwrap();
        let home = directory.path();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        let delegate = Arc::new(DurableEventSink {
            store: service.shared_store.clone(),
        });
        let mut run = FeatureRunContext::bot("bot-a", "user-a");
        run.run_id = Some("snapshot-io-run".into());
        run.chat_id = Some("snapshot-io-chat".into());
        run.session_id = Some("snapshot-io-chat".into());
        let sink =
            FeatureExecutionSink::new(delegate, FeatureToolRuntime::new(service.clone(), run));
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let request = crate::execution::ExecutionRequest {
            run_id: "snapshot-io-run".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "snapshot-io-chat".into(),
            bot_id: "bot-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "continue after memory storage failure".into(),
            messages: vec![json!({"role":"user","content":"continue"})],
            max_turns: 2,
            private: true,
            allow_unsafe: false,
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
            client_locale: None,
        };
        let runtime = FeatureToolRuntime::for_execution(
            service.clone(),
            &request,
            MemoryAccess::default(),
            false,
        );
        let mut tools = runtime.tools();
        tools.push(Arc::new(BreakMemoryStateTool {
            path: home.join("data/memory/state.tmp"),
        }));
        let engine = crate::execution::ExecutionEngine::new(
            service.shared_store.clone(),
            Arc::new(SnapshotFailureProvider {
                requests: requests.clone(),
            }),
            tools,
            Arc::new(sink),
            home,
        )
        .unwrap();
        let outcome = engine.run(request.clone()).await.unwrap();
        assert_eq!(outcome.status, "done");

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let messages = &requests[1].messages;
        let assistant = messages
            .iter()
            .find(|message| {
                message["role"] == "assistant"
                    && message["tool_calls"][0]["function"]["arguments"]
                        .as_str()
                        .is_some_and(|args| args.contains("snapshot_cache_put"))
            })
            .unwrap();
        let call_id = assistant["tool_calls"][0]["id"].as_str().unwrap();
        assert!(call_id.starts_with("memory-context-"));
        assert_eq!(
            assistant["tool_calls"][0]["function"]["arguments"],
            "{\"_gateway_context_error\":\"snapshot_cache_put\"}"
        );
        let tool_result = messages
            .iter()
            .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
            .unwrap();
        assert_eq!(tool_result["is_error"], true);
        let payload: Value =
            serde_json::from_str(tool_result["content"].as_str().unwrap()).unwrap();
        assert_eq!(payload["ok"], false);
        assert_eq!(payload["code"], "invalid");
        assert!(payload["message"].as_str().unwrap().contains("I/O error"));

        let events = service.shared_store.events_since(0).unwrap();
        let failed = events
            .iter()
            .filter(|event| {
                event.event == "memory.failed" && event.data["stage"] == "snapshot_cache_put"
            })
            .collect::<Vec<_>>();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].data["call_id"], call_id);
        assert_eq!(failed[0].data["result"], payload);
        let run_end = events
            .iter()
            .find(|event| {
                event.event == "trace.item"
                    && event.data.pointer("/item/type").and_then(Value::as_str) == Some("run.end")
            })
            .unwrap();
        assert_eq!(
            run_end.data.pointer("/item/data/status"),
            Some(&json!("done"))
        );
        let seqs = events.iter().map(|event| event.seq).collect::<HashSet<_>>();
        assert_eq!(seqs.len(), events.len());
    }

    #[tokio::test]
    async fn oversized_first_context_extracts_before_summarizing() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let summarized = Arc::new(StdMutex::new(String::new()));
        service
            .set_maintenance_provider(Some(Arc::new(OrderedMaintenance {
                calls: calls.clone(),
                summarized: summarized.clone(),
            })))
            .unwrap();
        let mut run = FeatureRunContext::bot("bot-a", "user-a");
        run.model_context_window = 2;
        run.run_id = Some("first-oversized-run".into());
        run.chat_id = Some("first-oversized-chat".into());
        run.session_id = Some("first-oversized-chat".into());
        let sink = FeatureExecutionSink::new(
            Arc::new(EventRecordingSink::default()),
            FeatureToolRuntime::new(service, run),
        );
        let request = crate::execution::ExecutionRequest {
            run_id: "first-oversized-run".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "first-oversized-chat".into(),
            bot_id: "bot-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "summarize current work".into(),
            messages: vec![json!({"role":"user","content":"x".repeat(500)})],
            max_turns: 1,
            private: true,
            allow_unsafe: false,
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
            client_locale: None,
        };
        let context = sink.prepare_model_context(&request).await.unwrap().unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["extract", "summarize"]);
        assert!(summarized.lock().unwrap().contains(&"x".repeat(240)));
        assert_eq!(context["system_blocks"].as_array().unwrap().len(), 2);
        assert!(context["compaction_summary"].as_str().is_some());
    }

    struct OutOfScopeMaintenance;

    #[async_trait]
    impl macbot_memory::AsyncMaintenanceProvider for OutOfScopeMaintenance {
        async fn summarize(&self, _: &str) -> Result<String, macbot_memory::MemoryError> {
            Ok("compacted context".into())
        }

        async fn extract_for_targets(
            &self,
            _: &str,
            targets: &[MemoryTarget],
        ) -> Result<Vec<macbot_memory::MemoryDraft>, macbot_memory::MemoryError> {
            let allowed_bot = targets
                .iter()
                .find(|target| target.scope == macbot_memory::MemoryScope::Bot)
                .cloned()
                .expect("the private context has an eligible Bot target");
            Ok(vec![
                macbot_memory::MemoryDraft {
                    target: MemoryTarget::bot("unrelated-bot"),
                    kind: MemoryKind::BotExperience,
                    content: "must be discarded".into(),
                    source: macbot_memory::MemorySource::default(),
                },
                macbot_memory::MemoryDraft {
                    target: allowed_bot,
                    kind: MemoryKind::BotExperience,
                    content: "eligible durable experience".into(),
                    source: macbot_memory::MemorySource::default(),
                },
            ])
        }
    }

    #[tokio::test]
    async fn out_of_scope_maintenance_target_is_ignored_during_normal_context_build() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        service
            .set_maintenance_provider(Some(Arc::new(OutOfScopeMaintenance)))
            .unwrap();
        let mut run = FeatureRunContext::bot("bot-a", "user-a");
        run.model_context_window = 2;
        run.run_id = Some("out-of-scope-extract-run".into());
        run.chat_id = Some("out-of-scope-extract-chat".into());
        run.session_id = Some("out-of-scope-extract-chat".into());
        let sink = FeatureExecutionSink::new(
            Arc::new(EventRecordingSink::default()),
            FeatureToolRuntime::new(service.clone(), run),
        );
        let request = crate::execution::ExecutionRequest {
            run_id: "out-of-scope-extract-run".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "out-of-scope-extract-chat".into(),
            bot_id: "bot-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "continue the conversation".into(),
            messages: vec![json!({"role":"user","content":"x".repeat(500)})],
            max_turns: 1,
            private: true,
            allow_unsafe: false,
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
            client_locale: None,
        };

        let context = sink.prepare_model_context(&request).await.unwrap().unwrap();

        assert!(context["compaction_summary"].as_str().is_some());
        let entries = service.shared_memory.entries().unwrap();
        assert!(entries
            .iter()
            .all(|entry| entry.target == MemoryTarget::bot("bot-a")));
        assert!(entries
            .iter()
            .any(|entry| entry.content == "eligible durable experience"));
        assert!(!entries
            .iter()
            .any(|entry| entry.content == "must be discarded"));
    }

    #[tokio::test]
    async fn private_project_find_selection_opens_segment_and_loads_project_catalog() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let summarized = Arc::new(StdMutex::new(String::new()));
        service
            .set_maintenance_provider(Some(Arc::new(OrderedMaintenance {
                calls: calls.clone(),
                summarized: summarized.clone(),
            })))
            .unwrap();
        service
            .add_project_record(macbot_memory::ProjectRecord {
                id: "project-atlas".into(),
                name: "Atlas project".into(),
                goal: "Build the Atlas service".into(),
                content: "Atlas service architecture".into(),
            })
            .unwrap();
        service
            .memory_rpc(
                &MemoryActor::main("main-bot"),
                json!({
                    "operation_id":"project-memory-seed",
                    "run_id":"project-seed-run",
                    "scope":"project",
                    "project_id":"project-atlas",
                    "action":"add",
                    "kind":"project",
                    "content":"Atlas architecture: event-driven gateway"
                }),
            )
            .unwrap();

        let mut run = FeatureRunContext::bot("bot-a", "local-user");
        run.access =
            MemoryAccess::group_for_user("local-user", "project-atlas", vec!["bot-a".into()]);
        run.private = true;
        run.run_id = Some("private-project-run".into());
        run.chat_id = Some("private-project-chat".into());
        run.session_id = Some("private-project-chat".into());
        let runtime = FeatureToolRuntime::new(service, run);
        let sink = runtime.execution_sink(Arc::new(EventRecordingSink::default()));
        let request = crate::execution::ExecutionRequest {
            run_id: "private-project-run".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "private-project-chat".into(),
            bot_id: "bot-a".into(),
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
            subagent: false,
            tools: None,
            phase: None,
            parent_run_id: None,
            subagent_task: None,
            save_full_requests: false,
            resume_message: None,
            client_locale: None,
        };
        sink.prepare_model_context(&request).await.unwrap();
        sink.model_context_checkpointed(&request).await;

        let project_find = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "project_find")
            .unwrap();
        let context = ToolContext::new("/tmp", "private-project-run", "/tmp/runs");
        let found = project_find.call(&context, json!({"query":"Atlas"})).await;
        assert!(!found.is_error);
        assert_eq!(found.details["projects"][0]["id"], "project-atlas");
        let selected = project_find
            .call(
                &context,
                json!({"query":"Atlas","project_id":"project-atlas"}),
            )
            .await;
        assert!(!selected.is_error);
        assert_eq!(selected.details["selected_project_id"], "project-atlas");

        let next_context = sink.prepare_model_context(&request).await.unwrap().unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["extract", "summarize"]);
        assert!(next_context["system_blocks"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Atlas architecture"));
    }

    #[tokio::test]
    async fn memory_delta_is_visible_again_in_a_later_run_of_the_same_segment() {
        let home = tempdir().unwrap().keep();
        let run_checkpoint_path = home.join("data/memory/segments/run-one.json");
        let service = Arc::new(FeatureService::open(home.clone(), Vec::new()).unwrap());
        let bot_entry = service
            .memory_rpc(
                &MemoryActor::bot("bot-a"),
                json!({
                    "operation_id":"seed-entry",
                    "run_id":"seed-run",
                    "scope":"bot",
                    "bot_id":"bot-a",
                    "action":"add",
                    "kind":"bot_experience",
                    "content":"old durable guidance"
                }),
            )
            .unwrap();
        let delegate = Arc::new(EventRecordingSink::default());
        let mut run_context = FeatureRunContext::bot("bot-a", "local-user");
        run_context.run_id = Some("run-one".into());
        run_context.chat_id = Some("same-chat".into());
        run_context.session_id = Some("same-chat".into());
        run_context.host_timezone = Some("America/Los_Angeles".into());
        let sink = FeatureExecutionSink::new(
            delegate.clone(),
            FeatureToolRuntime::new(service.clone(), run_context),
        );
        let mut request = crate::execution::ExecutionRequest {
            run_id: "run-one".into(),
            assignment_id: None,
            source_message_id: None,
            chat_id: "same-chat".into(),
            bot_id: "bot-a".into(),
            model: "mock/model".into(),
            provider_id: "mock".into(),
            project_id: None,
            instruction: "help me".into(),
            messages: vec![json!({"role":"user","content":"hello"})],
            max_turns: 1,
            private: true,
            allow_unsafe: false,
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
            client_locale: None,
        };
        let initial_context = sink.prepare_model_context(&request).await.unwrap().unwrap();
        assert!(initial_context["system_blocks"][1]["content"]
            .as_str()
            .unwrap()
            .contains("America/Los_Angeles"));
        sink.model_context_checkpointed(&request).await;
        assert!(run_checkpoint_path.exists());
        let run_checkpoint: Value =
            serde_json::from_slice(&std::fs::read(&run_checkpoint_path).unwrap()).unwrap();
        assert_eq!(
            run_checkpoint["seen_revision"],
            service.shared_memory.revision().unwrap()
        );
        service
            .user_memory_rpc(
                "memory.user.create",
                json!({"content":"prefers concise replies"}),
                "UTC",
            )
            .unwrap();
        service
            .memory_rpc(
                &MemoryActor::bot("bot-a"),
                json!({
                    "operation_id":"replace-entry",
                    "run_id":"update-run",
                    "scope":"bot",
                    "bot_id":"bot-a",
                    "action":"replace",
                    "kind":"bot_experience",
                    "id":bot_entry["entry_id"],
                    "expected_version":bot_entry["version"],
                    "content":"new durable guidance"
                }),
            )
            .unwrap();
        let first_update = sink.prepare_model_context(&request).await.unwrap().unwrap();
        assert!(first_update["memory_update"]
            .as_str()
            .unwrap()
            .contains("prefers concise replies"));
        sink.model_context_checkpointed(&request).await;

        request.run_id = "run-two".into();
        request.messages = vec![json!({"role":"user","content":"another question"})];
        let mut second_context = FeatureRunContext::bot("bot-a", "local-user");
        second_context.run_id = Some("run-two".into());
        second_context.chat_id = Some("same-chat".into());
        second_context.session_id = Some("same-chat".into());
        second_context.host_timezone = Some("Asia/Tokyo".into());
        let second_sink =
            FeatureExecutionSink::new(delegate, FeatureToolRuntime::new(service, second_context));
        let second_update = second_sink
            .prepare_model_context(&request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            initial_context["system_blocks"][0]["content"],
            second_update["system_blocks"][0]["content"]
        );
        assert!(second_update["memory_update"]
            .as_str()
            .unwrap()
            .contains("prefers concise replies"));
        assert!(second_update["system_blocks"][0]["content"]
            .as_str()
            .unwrap()
            .contains("old durable guidance"));
        assert!(second_update["system_blocks"][1]["content"]
            .as_str()
            .unwrap()
            .contains("Asia/Tokyo"));
        assert!(!second_update["system_blocks"][0]["content"]
            .as_str()
            .unwrap()
            .contains("new durable guidance"));
    }

    #[tokio::test]
    async fn successful_skill_draft_trace_emits_skill_updated_event() {
        let runtime = runtime();
        let draft = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "skill_draft")
            .unwrap();
        let result = draft
            .call(
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"name":"event-draft","content":"---\nname: event-draft\ndescription: Draft\n---\n# Draft"}),
            )
            .await;
        assert!(!result.is_error);
        let delegate = Arc::new(EventRecordingSink::default());
        let sink = FeatureExecutionSink::new(delegate.clone(), runtime);
        sink.emit(crate::execution::ExecutionEvent {
            event: "trace.item".into(),
            data: json!({
                "item": {
                    "type": "tool.start",
                    "data": {"call_id":"draft-call","name":"skill_draft"}
                }
            }),
            persistent: true,
        })
        .await;
        sink.emit(crate::execution::ExecutionEvent {
            event: "trace.item".into(),
            data: json!({
                "item": {
                    "type": "tool.end",
                    "data": {"call_id":"draft-call","is_error": false, "details": result.details}
                }
            }),
            persistent: true,
        })
        .await;
        let events = delegate.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].event, "skill.updated");
        assert_eq!(events[2].data["skill"]["name"], "event-draft");
    }

    #[tokio::test]
    async fn skill_updated_metadata_from_other_tool_is_ignored() {
        let runtime = runtime();
        let delegate = Arc::new(EventRecordingSink::default());
        let sink = FeatureExecutionSink::new(delegate.clone(), runtime);
        sink.emit(crate::execution::ExecutionEvent {
            event: "trace.item".into(),
            data: json!({
                "item": {
                    "type": "tool.start",
                    "data": {"call_id":"read-call","name":"read"}
                }
            }),
            persistent: true,
        })
        .await;
        sink.emit(crate::execution::ExecutionEvent {
            event: "trace.item".into(),
            data: json!({
                "item": {
                    "type": "tool.end",
                    "data": {
                        "call_id":"read-call",
                        "is_error":false,
                        "details":{"_skill_updated":{"skill":{"name":"spoof"}}}
                    }
                }
            }),
            persistent: true,
        })
        .await;
        let events = delegate.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.event != "skill.updated"));
    }

    #[test]
    fn context_hook_keeps_l0_and_group_boundary() {
        let runtime = runtime();
        let package = runtime
            .context(&ContextRequest {
                kind: ConversationKind::Group,
                memory_targets: vec![MemoryTarget::user("user-a")],
                l0_platform_rules: runtime.service.l0_platform_rules().into(),
                l1_bot_identity: "bot".into(),
                announcement: String::new(),
                task: "task".into(),
                trigger: String::new(),
                references: Vec::new(),
                recent_messages: (0..40).map(|n| format!("m{n}")).collect(),
                segment_summary: String::new(),
                recent_context: Vec::new(),
                run_events: vec!["send_msg".into()],
                model_context_window: 100,
                previous_snapshot: None,
                now: Utc::now(),
            })
            .unwrap();
        assert_eq!(package.layers[0].level, 0);
        assert!(package.layers[3].content.contains("m10"));
        assert!(!package.layers[3].content.contains("m0"));
        assert!(package.new_segment);
        assert_eq!(MemoryScope::User.to_string(), "user");
    }

    #[test]
    fn runtime_user_and_project_group_targets_receive_only_the_authorized_profile() {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        service
            .user_memory_rpc("memory.profile.get", json!({}), "Asia/Shanghai")
            .unwrap();
        service
            .user_memory_rpc(
                "memory.profile.update",
                json!({"name":"Initial Name","expected_version":1}),
                "Asia/Shanghai",
            )
            .unwrap();

        let make_request = |bot_id: &str, private: bool, project_id: Option<&str>| {
            serde_json::from_value::<crate::execution::ExecutionRequest>(json!({
                "run_id":"profile-context-run",
                "chat_id":"profile-context-chat",
                "bot_id":bot_id,
                "model":"mock/model",
                "instruction":"continue",
                "private":private,
                "project_id":project_id
            }))
            .unwrap()
        };
        let context = |request: &crate::execution::ExecutionRequest, access: MemoryAccess| {
            let runtime =
                FeatureToolRuntime::for_execution(Arc::clone(&service), request, access, false);
            let sink = FeatureExecutionSink::new(Arc::new(EventRecordingSink::default()), runtime);
            let request = sink.context_request(request);
            sink.runtime.context(&request).unwrap()
        };

        let private_request = make_request("worker-a", true, None);
        let private = context(&private_request, MemoryAccess::user("user"));
        assert!(private.layers[2]
            .content
            .starts_with("用户档案（优先参考）"));
        assert!(private.layers[2].content.contains("称呼：Initial Name"));

        let group_request = make_request("worker-b", false, Some("project-a"));
        let group = context(
            &group_request,
            MemoryAccess::group_for_user("user", "project-a", vec!["worker-b".into()]),
        );
        assert!(group.layers[2].content.starts_with("用户档案（优先参考）"));
        assert!(group.layers[2].content.contains("称呼：Initial Name"));

        service
            .user_memory_rpc(
                "memory.profile.update",
                json!({"name":"Updated Name","expected_version":2}),
                "Asia/Shanghai",
            )
            .unwrap();
        let updated_group = context(
            &group_request,
            MemoryAccess::group_for_user("user", "project-a", vec!["worker-b".into()]),
        );
        assert!(updated_group.layers[2]
            .content
            .contains("称呼：Updated Name"));
        assert!(!updated_group.layers[2]
            .content
            .contains("称呼：Initial Name"));

        let other_user = context(&private_request, MemoryAccess::user("user-other"));
        assert!(!other_user.layers[2].content.contains("用户档案"));
        assert!(!other_user.layers[2].content.contains("Updated Name"));
    }
}
