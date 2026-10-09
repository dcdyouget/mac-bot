//! Model-facing skill, memory and retrieval tools.
//!
//! Transport registration belongs to `backend.rs`; this module owns the actual
//! calls and carries the authenticated run context into `FeatureService`.

use crate::features::{FeatureError, FeatureService, MemoryAccess, MemoryActor};
use async_trait::async_trait;
use macbot_memory::{ContextPackage, ContextRequest, MemoryEntry, MemorySource, MemoryTarget};
use macbot_tools::{Risk, Tool, ToolContext, ToolResult};
use serde_json::{json, Map, Value};
use std::sync::Arc;

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
    /// Runtime supplied prompt metadata. Defaults are deliberately stable so
    /// older callers keep the same context shape until they provide richer
    /// Bot/project settings.
    pub model_context_window: usize,
    pub bot_identity: Option<String>,
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
            model_context_window: 128_000,
            bot_identity: None,
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
            model_context_window: 128_000,
            bot_identity: None,
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
        context
    }
}

/// Shared feature handle used to build the tools for one model run.
#[derive(Clone)]
pub struct FeatureToolRuntime {
    pub service: Arc<FeatureService>,
    pub run: FeatureRunContext,
}

impl FeatureToolRuntime {
    pub fn new(service: Arc<FeatureService>, run: FeatureRunContext) -> Self {
        Self { service, run }
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
    }

    pub fn run_started(&self, run_id: &str) -> Result<(), FeatureError> {
        self.service.begin_memory_run(run_id)
    }

    pub fn run_succeeded(&self, run_id: &str) -> Result<Vec<MemoryEntry>, FeatureError> {
        self.service.commit_memory_run(run_id)
    }

    pub fn run_succeeded_with_worklog(
        &self,
        run_id: &str,
        target: Option<MemoryTarget>,
        worklog: Option<&str>,
        source: MemorySource,
    ) -> Result<Vec<MemoryEntry>, FeatureError> {
        self.service
            .complete_run_with_worklog(run_id, target, worklog, source)
    }

    pub fn run_failed(&self, run_id: &str) -> Result<bool, FeatureError> {
        self.service.rollback_memory_run(run_id)
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
}

impl FeatureExecutionSink {
    pub fn new(
        delegate: Arc<dyn crate::execution::ExecutionSink>,
        runtime: FeatureToolRuntime,
    ) -> Self {
        Self { delegate, runtime }
    }

    fn context_request(&self, request: &crate::execution::ExecutionRequest) -> ContextRequest {
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
        targets.push(MemoryTarget::bot(request.bot_id.clone()));
        if let Some(project_id) = &request.project_id {
            if self.runtime.run.actor.is_main()
                || self
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
        let recent_messages: Vec<String> = request
            .messages
            .iter()
            .filter_map(|message| message.get("content").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let recent_context = if self.runtime.run.recent_context.is_empty() {
            recent_messages.clone()
        } else {
            self.runtime.run.recent_context.clone()
        };
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
        let run_end = if event.event == "run.end" {
            Some(&event.data)
        } else if event.event == "trace.item"
            && event.data.pointer("/item/type").and_then(Value::as_str) == Some("run.end")
        {
            event.data.pointer("/item/data")
        } else {
            None
        };
        if let Some(run_end) = run_end {
            if run_end.get("status").and_then(Value::as_str) != Some("done") {
                let _ = self.runtime.run_failed(
                    run_end
                        .get("run_id")
                        .and_then(Value::as_str)
                        .or(self.runtime.run.run_id.as_deref())
                        .unwrap_or_default(),
                );
            }
        }
        self.delegate.emit(event).await;
    }

    async fn send_group_message(&self, message: Value) -> Result<Value, String> {
        self.delegate.send_group_message(message).await
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
        let mut context_request = self.context_request(request);
        let mut package = self
            .runtime
            .context(&context_request)
            .map_err(|error| error.to_string())?;
        // The first assembly intentionally creates a fresh snapshot. Reuse
        // that snapshot for the same request so a large task can then be
        // classified by the 80% boundary instead of being hidden behind the
        // one-time `memory_snapshot_stale` segment marker.
        if package.segment_reason.as_deref() == Some("memory_snapshot_stale") {
            context_request.previous_snapshot = Some(package.snapshot.clone());
            package = self
                .runtime
                .context(&context_request)
                .map_err(|error| error.to_string())?;
        }
        if package.segment_reason.as_deref() == Some("context_80_percent") {
            let summary = self
                .runtime
                .compact_context(&context_request)
                .await
                .map_err(|error| format!("context compaction failed: {error}"))?
                .ok_or_else(|| "context compaction produced no summary".to_owned())?;
            // Replace the oversized L3 segment before sending it to the
            // model. Stage the summary with the current run so a failed
            // run cannot leave an orphaned Bot worklog entry.
            if let Some(layer) = package.layers.iter_mut().find(|layer| layer.level == 3) {
                layer.content = format!("Compacted segment:\n{summary}");
            }
            let source = MemorySource {
                bot_id: Some(request.bot_id.clone()),
                run_id: Some(request.run_id.clone()),
                session_id: Some(request.chat_id.clone()),
            };
            if let Err(error) = self.runtime.service.stage_context_summary(
                &self.runtime.run.actor,
                &self.runtime.run.access,
                &request.run_id,
                MemoryTarget::bot(request.bot_id.clone()),
                &summary,
                source,
            ) {
                tracing::warn!(run_id = %request.run_id, %error, "failed to stage compacted context");
            }
        }
        let text = package
            .layers
            .iter()
            .map(|layer| format!("L{}:\n{}", layer.level, layer.content))
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(Some(Value::String(text)))
    }

    async fn commit_succeeded(&self, request: &crate::execution::ExecutionRequest, data: Value) {
        if request.subagent {
            // RuntimeExecution opens a staging slot for every run. Subagents
            // are read-only, so successful completion must discard the slot.
            let _ = self.runtime.run_failed(&request.run_id);
            return;
        }
        let text = data.get("text").and_then(Value::as_str).unwrap_or_default();
        let target = (!text.trim().is_empty()).then(|| MemoryTarget::bot(request.bot_id.clone()));
        let source = MemorySource {
            bot_id: Some(request.bot_id.clone()),
            run_id: Some(request.run_id.clone()),
            session_id: Some(request.chat_id.clone()),
        };
        if let Err(error) = self.runtime.run_succeeded_with_worklog(
            &request.run_id,
            target,
            (!text.trim().is_empty()).then_some(text),
            source,
        ) {
            tracing::error!(run_id = %request.run_id, %error, "failed to commit run memory");
        }
    }
}

struct SkillTool {
    runtime: FeatureToolRuntime,
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
            let detail = {
                let registry = self
                    .runtime
                    .service
                    .skill_registry
                    .read()
                    .map_err(|_| FeatureError::Invalid("skill lock poisoned".into()))?;
                if let Some(text) = object.get("text").and_then(Value::as_str) {
                    registry.load_for_text(text, bot_id)?.ok_or_else(|| {
                        FeatureError::Invalid("no enabled /skill invocation".into())
                    })?
                } else {
                    let name = required(&object, "name")?;
                    if !registry.model_invocation_allowed(name)? {
                        return Err(FeatureError::Invalid(
                            "skill requires explicit /skill-name invocation".into(),
                        ));
                    }
                    (registry.load_for_bot(name, bot_id)?, String::new())
                }
            };
            let name = detail.0.skill.name.clone();
            self.runtime
                .service
                .record_skill_invocation(&name, bot_id)?;
            Ok(json!({"skill":detail.0.skill,"content":detail.0.content,"instruction":detail.1}))
        })();
        into_result(result)
    }
}

struct MemoryTool {
    runtime: FeatureToolRuntime,
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Stage a user, private Bot, or project memory change for the current run."
    }

    fn schema(&self) -> Value {
        json!({"type":"object","required":["scope","action","content"],"properties":{
            "scope":{"enum":["user","bot","project"]},"action":{"enum":["add","replace","remove"]},
            "content":{"type":"string"},"id":{"type":"string"},"user_id":{"type":"string"},
            "bot_id":{"type":"string"},"project_id":{"type":"string"},"kind":{"type":"string"}
        }})
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Write
    }

    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        if !self.runtime.run.allow_memory {
            return ToolResult::error("memory tools are unavailable in this run");
        }
        into_result(self.runtime.service.memory_rpc_with_access(
            &self.runtime.run.actor,
            &self.runtime.run.access,
            with_run_id(args, &ctx.run_id),
        ))
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
        "Search durable memory visible to this Bot and user/group context."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"},"scope":{"enum":["user","bot","project"]},"user_id":{"type":"string"},"bot_id":{"type":"string"},"project_id":{"type":"string"}}})
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
            let query = required(&object, "query")?;
            let target = target_from_object(&object)?;
            Ok(
                json!({"entries":self.runtime.service.memory_search_for_access(
                &self.runtime.run.actor,
                &self.runtime.run.access,
                query,
                target.as_ref(),
            )?}),
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
        json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"}}})
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
            Ok(json!({"projects":projects}))
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

fn with_run_id(mut args: Value, run_id: &str) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.insert("run_id".into(), Value::String(run_id.to_string()));
    }
    args
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
            )))
        }
    };
    Ok(Some(target))
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use macbot_memory::{ConversationKind, MemoryScope};
    use tempfile::tempdir;

    fn runtime() -> FeatureToolRuntime {
        let home = tempdir().unwrap().keep();
        let service = Arc::new(FeatureService::open(home, Vec::new()).unwrap());
        FeatureToolRuntime::new(service, FeatureRunContext::bot("bot-a", "user-a"))
    }

    #[test]
    fn execution_context_and_shared_handle_are_stable() {
        let home = tempdir().unwrap().keep();
        let service = FeatureService::open_shared(home, Vec::new()).unwrap();
        let request = crate::execution::ExecutionRequest {
            run_id: "run-shared".into(),
            assignment_id: None,
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

    #[tokio::test]
    async fn memory_tool_stages_then_run_commit_persists() {
        let runtime = runtime();
        let memory = runtime
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "memory")
            .unwrap();
        let context = ToolContext::new("/tmp", "run-1", "/tmp/runs");
        runtime.run_started("run-1").unwrap();
        let result = memory
            .call(
                &context,
                json!({"scope":"user","user_id":"user-a","action":"add","kind":"user_preference","content":"likes tests"}),
            )
            .await;
        assert!(!result.is_error, "{}", result.content.len());
        assert_eq!(runtime.service.shared_memory.entries().unwrap().len(), 0);
        runtime.run_succeeded("run-1").unwrap();
        assert_eq!(runtime.service.shared_memory.entries().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn memory_search_tool_does_not_leak_other_users_or_bots() {
        let runtime = runtime();
        runtime.service.begin_memory_run("seed").unwrap();
        runtime
            .service
            .stage_memory(
                &MemoryActor::System,
                "seed",
                macbot_memory::MemoryRequest {
                    target: MemoryTarget::user("user-other"),
                    action: macbot_memory::MemoryAction::Add,
                    content: "secret other user".into(),
                    id: None,
                    kind: Some(macbot_memory::MemoryKind::UserPreference),
                    source: MemorySource::default(),
                },
            )
            .unwrap();
        runtime
            .service
            .stage_memory(
                &MemoryActor::System,
                "seed",
                macbot_memory::MemoryRequest {
                    target: MemoryTarget::bot("bot-b"),
                    action: macbot_memory::MemoryAction::Add,
                    content: "secret other bot".into(),
                    id: None,
                    kind: Some(macbot_memory::MemoryKind::BotExperience),
                    source: MemorySource::default(),
                },
            )
            .unwrap();
        runtime.service.commit_memory_run("seed").unwrap();
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
    async fn cross_session_memory_commits_on_finish_and_rolls_back_failures() {
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
        first.run_started("run-a").unwrap();
        assert!(!memory
            .call(
                &context,
                json!({"scope":"user","user_id":"user-a","action":"add","kind":"user_preference","content":"likes concise"}),
            )
            .await
            .is_error);
        first.run_succeeded("run-a").unwrap();

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

        first.run_started("run-failed").unwrap();
        assert!(!memory
            .call(
                &ToolContext::new("/tmp", "run-failed", "/tmp/runs"),
                json!({"scope":"user","user_id":"user-a","action":"add","kind":"user_preference","content":"must rollback"}),
            )
            .await
            .is_error);
        first.run_failed("run-failed").unwrap();
        assert!(service
            .memory_search("must rollback", Some(&MemoryTarget::user("user-a")))
            .unwrap()
            .is_empty());
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
        assert_eq!(result.details["instruction"], "use the project home");
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
}
