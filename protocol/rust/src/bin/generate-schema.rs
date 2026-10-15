use macbot_protocol::*;
use schemars::{schema::RootSchema, schema_for};
use std::{env, fs, path::PathBuf};

// Keep the exported envelopes aligned with their custom serde dispatch without
// changing their wire properties or the definitions used by Kotlin generation.
#[derive(Clone)]
enum PayloadBinding {
    Reference(String),
    Fields(Vec<String>),
}
fn reference<T: schemars::JsonSchema>() -> PayloadBinding {
    PayloadBinding::Reference(T::schema_name())
}
fn fields(names: &[&str]) -> PayloadBinding {
    PayloadBinding::Fields(names.iter().map(|name| (*name).to_owned()).collect())
}
fn method_payload(method: Method) -> PayloadBinding {
    match method {
        Method::SessionResume => reference::<SessionResumeParams>(),
        Method::Ping => reference::<EmptyParams>(),
        Method::Bootstrap => reference::<EmptyParams>(),
        Method::DeviceRegister => reference::<DeviceRegisterParams>(),
        Method::ChatList => reference::<IncludeArchivedParams>(),
        Method::ChatGet => reference::<ChatIdParams>(),
        Method::ChatHistory => reference::<ChatHistoryParams>(),
        Method::ChatThread => reference::<ChatThreadParams>(),
        Method::ChatSend => reference::<ChatSendParams>(),
        Method::MessageDelete => reference::<MessageDeleteParams>(),
        Method::MessageEdit => reference::<MessageEditParams>(),
        Method::MessageSteer => reference::<MessageSteerParams>(),
        Method::ArtifactVersionsList => reference::<ArtifactVersionsParams>(),
        Method::ArtifactVersionGet => reference::<ArtifactVersionGetParams>(),
        Method::ChatMarkRead => reference::<ChatMarkReadParams>(),
        Method::ChatReact => reference::<ChatReactParams>(),
        Method::ChatSetPinned => reference::<ChatPinnedParams>(),
        Method::ChatSetMuted => reference::<ChatMutedParams>(),
        Method::BotList => reference::<BotListParams>(),
        Method::BotGet => reference::<BotIdParams>(),
        Method::BotCreate => reference::<BotCreateParams>(),
        Method::BotUpdate => reference::<BotUpdateParams>(),
        Method::BotDuplicate => reference::<BotDuplicateParams>(),
        Method::BotDelete => reference::<BotIdParams>(),
        Method::BotTemplates => reference::<EmptyParams>(),
        Method::BotCreateFromTemplate => reference::<CreateFromTemplateParams>(),
        Method::ProjectList => reference::<ProjectListParams>(),
        Method::ProjectGet => reference::<ProjectIdParams>(),
        Method::ProjectCreate => reference::<ProjectCreateParams>(),
        Method::ProjectUpdate => reference::<ProjectUpdateParams>(),
        Method::AnnouncementUpdate => reference::<AnnouncementUpdateParams>(),
        Method::ProjectAddMember => reference::<ProjectMemberParams>(),
        Method::ProjectRemoveMember => reference::<ProjectRemoveMemberParams>(),
        Method::ProjectConfirmDone => reference::<ProjectConfirmDoneParams>(),
        Method::ProjectRequestChanges => reference::<ProjectRequestChangesParams>(),
        Method::ProjectArchive => reference::<ProjectIdParams>(),
        Method::ProjectReopen => reference::<ProjectIdParams>(),
        Method::AssignmentList => reference::<AssignmentListParams>(),
        Method::AssignmentGet => reference::<AssignmentIdParams>(),
        Method::AssignmentStop => reference::<AssignmentIdParams>(),
        Method::AssignmentRetry => reference::<AssignmentRetryParams>(),
        Method::TraceHistory => reference::<TraceHistoryParams>(),
        Method::TraceSubscribe => reference::<TraceSubscribeParams>(),
        Method::TraceUnsubscribe => reference::<TraceUnsubscribeParams>(),
        Method::ApprovalList => reference::<ApprovalListParams>(),
        Method::ApprovalDecide => reference::<ApprovalDecideParams>(),
        Method::QuestionAnswer => reference::<QuestionAnswerParams>(),
        Method::QuestionList => reference::<QuestionListParams>(),
        Method::LoopResolve => reference::<LoopResolveParams>(),
        Method::TakeoverStart => reference::<TakeoverStartParams>(),
        Method::TakeoverRelease => reference::<TakeoverReleaseParams>(),
        Method::WorkbenchGet => reference::<EmptyParams>(),
        Method::SkillList => reference::<EmptyParams>(),
        Method::SkillGet => reference::<SkillNameParams>(),
        Method::SkillCreate => reference::<SkillCreateParams>(),
        Method::SkillUpdate => reference::<SkillCreateParams>(),
        Method::SkillDelete => reference::<SkillNameParams>(),
        Method::SkillSetEnabled => reference::<SkillSetEnabledParams>(),
        Method::SkillPublish => reference::<SkillNameParams>(),
        Method::SkillImport => reference::<SkillImportParams>(),
        Method::RoutineList => reference::<RoutineListParams>(),
        Method::RoutineCreate => reference::<RoutineCreateParams>(),
        Method::RoutineUpdate => reference::<RoutineUpdateParams>(),
        Method::RoutineDelete => reference::<RoutineIdParams>(),
        Method::RoutineSetEnabled => reference::<RoutineEnabledParams>(),
        Method::RoutineTestRun => reference::<RoutineIdParams>(),
        Method::RoutineRuns => reference::<RoutineIdParams>(),
        Method::ProviderList => reference::<EmptyParams>(),
        Method::ProviderCreate => reference::<ProviderCreateParams>(),
        Method::ProviderUpdate => reference::<ProviderUpdateParams>(),
        Method::ProviderDelete => reference::<ProviderIdParams>(),
        Method::ProviderTest => reference::<ProviderIdParams>(),
        Method::ModelRefresh => reference::<ProviderIdParams>(),
        Method::ModelUpsert => reference::<ModelUpsertParams>(),
        Method::ModelDelete => reference::<ModelDeleteParams>(),
        Method::SettingsGet => reference::<EmptyParams>(),
        Method::HostPermissions => reference::<EmptyParams>(),
        Method::SettingsUpdate => reference::<SettingsUpdateParams>(),
        Method::MemoryUserList => reference::<EmptyParams>(),
        Method::MemoryUserCreate => reference::<MemoryUserCreateParams>(),
        Method::MemoryUserUpdate => reference::<MemoryUserUpdateParams>(),
        Method::MemoryUserDelete => reference::<MemoryUserDeleteParams>(),
        Method::MemoryProfileGet => reference::<EmptyParams>(),
        Method::MemoryProfileUpdate => reference::<MemoryProfileUpdateParams>(),
        Method::UsageSummary => reference::<UsagePeriodParams>(),
        Method::UsageHeatmap => reference::<UsageHeatmapParams>(),
        Method::UsageTimeseries => reference::<UsageTimeseriesParams>(),
        Method::UsageBreakdown => reference::<UsageBreakdownParams>(),
        Method::Search => reference::<SearchParams>(),
    }
}
fn event_payload(event: EventName) -> PayloadBinding {
    match event {
        EventName::ChatCreated | EventName::ChatUpdated => fields(&["chat"]),
        EventName::ChatDeleted => fields(&["chat_id"]),
        EventName::ReadUpdated => fields(&["chat_id", "last_read_seq"]),
        EventName::MessageCreated | EventName::MessageUpdated => fields(&["message"]),
        EventName::MessageDeleted => fields(&["chat_id", "message_id"]),
        EventName::BotCreated | EventName::BotUpdated => fields(&["bot"]),
        EventName::BotDeleted => fields(&["bot_id"]),
        EventName::ProjectCreated | EventName::ProjectUpdated => fields(&["project"]),
        EventName::AnnouncementUpdated => fields(&["announcement"]),
        EventName::AssignmentCreated | EventName::AssignmentUpdated => fields(&["assignment"]),
        EventName::ArtifactRegistered => fields(&["artifact"]),
        EventName::ApprovalRequested | EventName::ApprovalResolved => fields(&["approval"]),
        EventName::QuestionAsked | EventName::QuestionAnswered => fields(&["question"]),
        EventName::SkillUpdated => fields(&["skill"]),
        EventName::SkillDeleted => fields(&["name"]),
        EventName::RoutineUpdated => fields(&["routine"]),
        EventName::RoutineDeleted => fields(&["routine_id"]),
        EventName::RoutineRun => fields(&["run"]),
        EventName::ProviderUpdated => fields(&["provider", "models"]),
        EventName::ProviderDeleted => fields(&["provider_id"]),
        EventName::SettingsUpdated => fields(&["settings"]),
        EventName::MemoryUserUpdated => fields(&["entry", "quota"]),
        EventName::MemoryUserDeleted => fields(&["entry_id", "quota"]),
        EventName::MemoryProfileUpdated => fields(&["profile"]),
        EventName::Hello => reference::<Hello>(),
        EventName::SyncDone => reference::<SeqEvent>(),
        EventName::SyncCursor => reference::<SeqEvent>(),
        EventName::MessageDelta => reference::<MessageDelta>(),
        EventName::Typing => reference::<TypingEvent>(),
        EventName::BotStatus => reference::<BotStatusEvent>(),
        EventName::UsageTick => reference::<UsageTick>(),
        EventName::HostStatus => reference::<HostStatus>(),
        EventName::TraceItem => fields(&["stream", "item"]),
        EventName::TraceDelta => fields(&["stream", "request_id", "channel", "call_id", "text"]),
        EventName::TraceToolOutput => fields(&["stream", "call_id", "chunk"]),
    }
}
fn trace_payload(kind: TraceType) -> PayloadBinding {
    match kind {
        TraceType::RunStart => fields(&["phase", "model", "parent_run_id", "subagent_task"]),
        TraceType::LlmRequest => fields(&["request_id", "model", "context", "tools", "prompt_ref"]),
        TraceType::LlmResponse => fields(&[
            "request_id",
            "text",
            "thinking",
            "tool_calls",
            "stop_reason",
            "usage",
            "latency_ms",
            "ttft_ms",
        ]),
        TraceType::ToolStart => fields(&["call_id", "name", "args"]),
        TraceType::ToolEnd => fields(&[
            "call_id",
            "is_error",
            "preview",
            "details",
            "truncated",
            "full_output",
            "duration_ms",
        ]),
        TraceType::SendMsg => fields(&["call_id", "intent", "message_id", "chat_id"]),
        TraceType::QueueReview => fields(&["message_ids", "count"]),
        TraceType::Steer => fields(&["message_id", "text", "from"]),
        TraceType::RunWait => fields(&["reason", "message_id"]),
        TraceType::Compaction => fields(&["reason", "before_tokens", "after_tokens"]),
        TraceType::RunEnd => fields(&["status", "error"]),
        TraceType::RunResume => fields(&["by_message_id"]),
    }
}
fn bound_payload(
    document: &serde_json::Value,
    union: &str,
    binding: PayloadBinding,
) -> serde_json::Value {
    use serde_json::json;
    match binding {
        PayloadBinding::Reference(name) => {
            assert!(
                document["definitions"].get(&name).is_some(),
                "missing payload definition {name}"
            );
            json!({"$ref": format!("#/definitions/{name}")})
        }
        PayloadBinding::Fields(mut names) => {
            names.sort();
            let candidates = document["definitions"][union]["anyOf"]
                .as_array()
                .expect("untagged payload union");
            let matches: Vec<_> = candidates
                .iter()
                .filter(|candidate| {
                    let mut actual: Vec<_> = candidate["properties"]
                        .as_object()
                        .map(|properties| properties.keys().cloned().collect())
                        .unwrap_or_default();
                    actual.sort();
                    actual == names
                })
                .collect();
            assert_eq!(
                matches.len(),
                1,
                "ambiguous or missing {union} payload fields {names:?}"
            );
            matches[0].clone()
        }
    }
}
fn bind_envelopes(document: &mut serde_json::Value) {
    use serde_json::{json, Value};
    for (title, discriminator, payload, enum_name, union) in [
        ("RpcRequest", "method", "params", "Method", "MethodParams"),
        ("EventFrame", "event", "data", "EventName", "EventData"),
        ("TraceItem", "type", "data", "TraceType", "TraceData"),
    ] {
        let root = document["title"] == title;
        let nested = document["definitions"].get(title).is_some();
        if !root && !nested {
            continue;
        }
        let names = document["definitions"][enum_name]["enum"]
            .as_array()
            .expect("discriminator enum");
        let constraints: Vec<Value> = names.iter().map(|name| {
            let binding = match title {
                "RpcRequest" => method_payload(serde_json::from_value(name.clone()).unwrap()),
                "EventFrame" => event_payload(serde_json::from_value(name.clone()).unwrap()),
                "TraceItem" => trace_payload(serde_json::from_value(name.clone()).unwrap()),
                _ => unreachable!(),
            };
            let shape = bound_payload(document, union, binding);
            json!({
                "if": {"properties": {(discriminator): {"const": name}}, "required": [discriminator]},
                "then": {"properties": {(payload): shape}, "required": [payload]}
            })
        }).collect();
        if root {
            document["allOf"] = json!(constraints);
        }
        if nested {
            document["definitions"][title]["allOf"] = json!(constraints);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../schema"));
    fs::create_dir_all(&out)?;
    let roots: Vec<(&str, RootSchema)> = vec![
        ("hello", schema_for!(Hello)),
        ("bot", schema_for!(Bot)),
        ("chat", schema_for!(Chat)),
        ("message", schema_for!(Message)),
        ("block", schema_for!(Block)),
        ("project", schema_for!(Project)),
        ("artifact", schema_for!(Artifact)),
        ("announcement", schema_for!(Announcement)),
        ("assignment", schema_for!(Assignment)),
        ("approval", schema_for!(Approval)),
        ("question", schema_for!(Question)),
        ("skill", schema_for!(Skill)),
        ("skill_detail", schema_for!(SkillDetail)),
        ("routine", schema_for!(Routine)),
        ("routine_run", schema_for!(RoutineRun)),
        ("provider", schema_for!(Provider)),
        ("model", schema_for!(Model)),
        ("settings", schema_for!(Settings)),
        ("memory_entry", schema_for!(MemoryEntry)),
        ("memory_quota", schema_for!(MemoryQuota)),
        ("user_profile", schema_for!(UserProfile)),
        ("device", schema_for!(Device)),
        ("usage_totals", schema_for!(UsageTotals)),
        ("usage_heatmap", schema_for!(HeatmapResult)),
        ("workbench", schema_for!(Workbench)),
        ("method", schema_for!(Method)),
        ("method_params", schema_for!(MethodParams)),
        ("method_result", schema_for!(MethodResult)),
        ("event_name", schema_for!(EventName)),
        ("event_data", schema_for!(EventData)),
        ("trace_data", schema_for!(TraceData)),
        ("write_meta", schema_for!(WriteMeta)),
        ("trace_item", schema_for!(TraceItem)),
        ("rpc_request", schema_for!(RpcRequest)),
        ("rpc_response", schema_for!(RpcResponse)),
        ("event_frame", schema_for!(EventFrame)),
        ("screen_state", schema_for!(ScreenState)),
        ("screen_client_frame", schema_for!(ScreenClientFrame)),
        ("screen_frame_header", schema_for!(ScreenFrameHeader)),
    ];
    let count = roots.len();
    for (name, schema) in roots {
        let mut schema = serde_json::to_value(schema)?;
        bind_envelopes(&mut schema);
        let schema: RootSchema = serde_json::from_value(schema)?;
        fs::write(
            out.join(format!("{name}.json")),
            serde_json::to_string_pretty(&schema)? + "\n",
        )?;
    }
    println!("wrote {} schemas to {}", count, out.display());
    Ok(())
}
