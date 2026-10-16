//! Deterministic protocol mock.
//!
//! This module is deliberately stateful and typed at the JSON boundary. It is
//! used by both clients before a real runtime backend is configured. Every
//! mutating method returns the object it changed and appends the corresponding
//! protocol event to the gateway event log.

use super::{id, now, rpc_error, GatewayState, MockState, RpcResult};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde_json::{json, Value};
use std::time::Duration as StdDuration;

pub(crate) async fn mock_call(
    method: &str,
    params: Value,
    state: &GatewayState,
    chat_reply_lifecycle_step_delay: Option<StdDuration>,
) -> RpcResult {
    let mut data = state.inner.write().await;
    let previous_seq = data.seq;
    let request_id = params
        .get("client_request_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let signature = request_signature(method, &params);
    if let Some(key) = &request_id {
        if let Some(previous) = extra_get(&data, "idempotency_meta")
            .into_iter()
            .find(|entry| entry.get("key").and_then(Value::as_str) == Some(key))
        {
            let previous_method = previous
                .get("signature")
                .and_then(Value::as_str)
                .and_then(|signature| signature.split_once(':').map(|(method, _)| method));
            if previous_method != Some(method) {
                return Err(rpc_error(
                    "conflict",
                    "client_request_id was already used for another method",
                    None,
                ));
            }
            if let Some(value) = data.idempotency.get(key) {
                return Ok(value.clone());
            }
        } else if let Some(value) = data.idempotency.get(key) {
            // Preserve entries created by an older mock state format.
            return Ok(value.clone());
        }
    }

    let mut result = dispatch(
        method,
        params,
        &mut data,
        state,
        chat_reply_lifecycle_step_delay,
    )
    .await?;
    ensure_loaded_skills_defaults(&mut result);
    if method.starts_with("skill.") {
        persist_mock_skills(&state.home, &data)
            .map_err(|_| rpc_error("internal", "failed to persist mock skills", None))?;
    }
    if let Some(key) = request_id {
        data.idempotency.insert(key.clone(), result.clone());
        let mut meta = extra_get(&data, "idempotency_meta");
        meta.push(json!({"key":key,"signature":signature}));
        extra_set(&mut data, "idempotency_meta", meta);
    }
    for event in data
        .events
        .iter()
        .filter(|event| event.get("seq").and_then(Value::as_u64).unwrap_or(0) > previous_seq)
    {
        let _ = state.events.send(event.clone());
    }
    Ok(result)
}

async fn dispatch(
    method: &str,
    params: Value,
    state: &mut MockState,
    gateway: &GatewayState,
    chat_reply_lifecycle_step_delay: Option<StdDuration>,
) -> RpcResult {
    ensure_mock_defaults(state, &gateway.home);
    match method {
        "ping" => Ok(json!({"server_time":now()})),
        "session.resume" => Ok(json!({"mode":"replay"})),
        "bootstrap" => {
            ensure_mock_profile_greeting(state);
            let hello = json!({
                "protocol":1,"server_version":"0.1.0","node_id":gateway.node_id.read().await.clone(),
                "host_name":gateway.host_name.read().await.clone(),"server_time":now(),
                "last_seq":state.seq,"timezone":"Asia/Shanghai","currency":"CNY",
                "features":["mock","browser"]
            });
            Ok(
                json!({"seq":state.seq,"hello":hello,"bots":state.bots,"chats":state.chats,
                "projects":state.projects,"settings":state.settings,"pending":pending(state)}),
            )
        }
        "device.register" => register_device(state, &params),
        "chat.list" => chat_list(state, &params),
        "chat.get" => find_wrapped(&state.chats, params.get("chat_id"), "chat"),
        "chat.history" => chat_history(state, &params),
        "chat.thread" => chat_thread(state, &params),
        "chat.send" => {
            let lifecycle_target = chat_reply_lifecycle_step_delay
                .map(|step_delay| prepare_chat_reply_lifecycle_target(state, &params, step_delay))
                .transpose()?;
            let result = send_chat(state, &params, &gateway.home)?;
            if let Some(target) = lifecycle_target {
                let source_message_id = result
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        rpc_error("internal", "mock user message is missing an id", None)
                    })?
                    .to_owned();
                let message_id = create_chat_reply_placeholder(
                    state,
                    &target.chat_id,
                    &source_message_id,
                    &target.bot_id,
                    target.requested_message_id.as_deref(),
                )?;
                schedule_chat_reply_lifecycle(
                    gateway.clone(),
                    target.step_delay,
                    target.chat_id,
                    target.bot_id,
                    source_message_id,
                    message_id,
                );
            }
            Ok(result)
        }
        "mock.replay_chat_reply_lifecycle" if chat_reply_lifecycle_step_delay.is_some() => {
            replay_chat_reply_lifecycle_from_rpc(
                state,
                &params,
                gateway,
                chat_reply_lifecycle_step_delay.expect("guarded by scenario option"),
            )
        }
        // Acceptance-only mock RPC. It deliberately does not exist in the
        // protocol or production adapter; it creates a canonical Bot DM event
        // so clients can exercise notification delivery and deep links.
        "mock.trigger_bot_dm" => trigger_bot_dm(state, &params),
        // Acceptance-only group counterpart used to exercise Bot-authored
        // project notifications without changing the production contract.
        "mock.trigger_bot_group_message" => trigger_bot_group_message(state, &params),
        // Acceptance-only test control for an empty main-chat state. Never
        // expose this operation through protocol or production adapters.
        "mock.clear_main_chat" => clear_mock_main_chat(state, &params),
        "mock.use_complete_profile" => use_complete_mock_profile(state),
        "mock.set_memory_scenario" => set_mock_memory_scenario(state, &params),
        "message.delete" => message_delete(state, &params),
        "message.edit" => message_edit(state, &params),
        "message.steer" => message_steer(state, &params),
        "chat.mark_read" => mark_read(state, &params),
        "chat.react" => react(state, &params),
        "chat.set_pinned" => set_chat_flag(state, &params, "pinned"),
        "chat.set_muted" => set_chat_flag(state, &params, "muted"),
        "bot.list" => bot_list(state, &params),
        "bot.get" => find_wrapped(&state.bots, params.get("bot_id"), "bot"),
        "bot.create" => create_bot(state, &params),
        "bot.update" => update_bot(state, &params),
        "bot.duplicate" => duplicate_bot(state, &params),
        "bot.delete" => delete_bot(state, &params),
        "bot.templates" => Ok(json!({"templates":templates()})),
        "bot.create_from_template" => create_from_template(state, &params),
        "project.list" => project_list(state, &params),
        "project.get" => project_get(state, &params),
        "project.create" => create_project(state, &params),
        "project.update" => project_update(state, &params),
        "project.add_member" => project_member(state, &params, true),
        "project.remove_member" => project_member(state, &params, false),
        "project.confirm_done" => project_status(state, &params, "done"),
        "project.request_review" => project_request_review(state, &params),
        "project.archive" => project_status(state, &params, "archived"),
        "project.reopen" => project_status(state, &params, "active"),
        "project.request_changes" => project_request_changes(state, &params, &gateway.home),
        "artifact.versions.list" => artifact_versions_list(state, &params),
        "artifact.version.get" => artifact_version_get(state, &params),
        "assignment.list" => assignment_list(state, &params),
        "assignment.get" => find_wrapped(
            &state.assignments,
            params.get("assignment_id"),
            "assignment",
        ),
        "assignment.stop" => assignment_stop(state, &params),
        "assignment.retry" => assignment_retry(state, &params),
        "assignment.create" | "assign" => assignment_create(state, &params, false),
        "delegate" => assignment_create(state, &params, true),
        "trace.history" => trace_history(state, &params),
        "trace.subscribe" => Ok(json!({"stream":id("stream"),"in_flight":[]})),
        "trace.unsubscribe" => Ok(json!({})),
        "approval.list" => filtered_list(state, "approvals", &params, "state", "approvals"),
        "approval.request" => approval_request(state, &params),
        "approval.decide" => approval_decide(state, &params),
        "question.ask" => question_ask(state, &params),
        "question.list" => question_list(state, &params),
        "question.answer" => question_answer(state, &params),
        "memory.user.list" => user_memory_list(state),
        "memory.user.create" => user_memory_create(state, &params),
        "memory.user.update" => user_memory_update(state, &params),
        "memory.user.delete" => user_memory_delete(state, &params),
        "memory.profile.get" => user_profile_get(state),
        "memory.profile.update" => user_profile_update(state, &params),
        "loop.resolve" => Ok(json!({})),
        "takeover.start" => takeover_start(state, &params),
        "takeover.release" => takeover_release(state, &params),
        "workbench.get" => workbench(state),
        "skill.list" => {
            let skills = extra_get(state, "skills")
                .into_iter()
                .map(|mut skill| {
                    if let Some(object) = skill.as_object_mut() {
                        object.remove("content");
                        object.entry("last_invoked_at").or_insert(Value::Null);
                    }
                    skill
                })
                .collect::<Vec<_>>();
            Ok(json!({"skills":skills}))
        }
        "skill.get" => skill_get(state, &params),
        "skill.create" => skill_create(state, &params, "user"),
        "skill.update" => skill_update(state, &params),
        "skill.delete" => skill_delete(state, &params),
        "skill.set_enabled" => skill_set_enabled(state, &params),
        "skill.publish" => skill_publish(state, &params),
        "skill.import" => skill_import(state, &params),
        "routine.list" => routine_list(state, &params),
        "routine.create" => routine_create(state, &params),
        "routine.update" => routine_update(state, &params),
        "routine.delete" => routine_delete(state, &params),
        "routine.set_enabled" => routine_set_enabled(state, &params),
        "routine.test_run" => routine_test_run(state, &params),
        "routine.runs" => routine_runs(state, &params),
        "provider.list" => {
            Ok(json!({"providers":extra_get(state,"providers"),"models":extra_get(state,"models")}))
        }
        "provider.create" => provider_create(state, &params),
        "provider.update" => provider_update(state, &params),
        "provider.delete" => provider_delete(state, &params),
        "provider.test" => {
            let provider_id = params
                .get("provider_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !extra_get(state, "providers")
                .iter()
                .any(|provider| provider.get("id").and_then(Value::as_str) == Some(provider_id))
            {
                return Err(rpc_error("not_found", "provider not found", None));
            }
            Ok(json!({"ok":true,"latency_ms":1,"error":null}))
        }
        "model.refresh" => {
            let provider_id = params
                .get("provider_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !extra_get(state, "providers")
                .iter()
                .any(|provider| provider.get("id").and_then(Value::as_str) == Some(provider_id))
            {
                return Err(rpc_error("not_found", "provider not found", None));
            }
            Ok(json!({"models":extra_get(state,"models")}))
        }
        "model.upsert" => model_upsert(state, &params),
        "model.delete" => model_delete(state, &params),
        "settings.get" => Ok(json!({"settings":state.settings})),
        "host.permissions" => Ok(json!({
            "screen_recording":"unknown",
            "accessibility":"unknown",
            "full_disk_access":"unknown",
            "automation_chrome":"unknown"
        })),
        "settings.update" => settings_update(state, &params),
        "usage.summary" => usage_summary(&params),
        "usage.heatmap" => usage_heatmap(&params),
        "usage.timeseries" => usage_timeseries(&params),
        "usage.breakdown" => usage_breakdown(&params),
        "search" => search(state, &params),
        _ => Err(rpc_error(
            "invalid_params",
            &format!("unknown method: {method}"),
            None,
        )),
    }
}

const LIFECYCLE_FIRST_REPLY: &str = "收到，我来处理。";
const LIFECYCLE_FOLLOWUP_REPLY: &str = "这是后续独立消息。";

struct ChatReplyLifecycleTarget {
    chat_id: String,
    bot_id: String,
    requested_message_id: Option<String>,
    step_delay: StdDuration,
}

fn prepare_chat_reply_lifecycle_target(
    state: &MockState,
    params: &Value,
    step_delay: StdDuration,
) -> Result<ChatReplyLifecycleTarget, super::RpcError> {
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or("chat_main")
        .to_owned();
    let bot_id = chat_reply_bot_for_chat(
        state,
        &chat_id,
        params.get("mock_reply_bot_id").and_then(Value::as_str),
    )?;
    let requested_message_id =
        match params.get("mock_reply_message_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if !value.trim().is_empty() => {
                if state.messages.values().flatten().any(|message| {
                    message.get("id").and_then(Value::as_str) == Some(value.as_str())
                }) {
                    return Err(rpc_error(
                        "conflict",
                        "mock_reply_message_id is already in use",
                        None,
                    ));
                }
                Some(value.clone())
            }
            _ => {
                return Err(rpc_error(
                    "invalid_params",
                    "mock_reply_message_id must be a non-empty string or null",
                    None,
                ));
            }
        };
    Ok(ChatReplyLifecycleTarget {
        chat_id,
        bot_id,
        requested_message_id,
        step_delay,
    })
}

fn chat_reply_bot_for_chat(
    state: &MockState,
    chat_id: &str,
    requested_bot_id: Option<&str>,
) -> Result<String, super::RpcError> {
    let chat = state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
        .ok_or_else(|| rpc_error("not_found", "chat not found", None))?;
    let kind = chat.get("kind").and_then(Value::as_str).unwrap_or("");
    let members = chat
        .get("member_bot_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let associated_bot = chat.get("bot_id").and_then(Value::as_str);
    let bot_id = if let Some(requested_bot_id) = requested_bot_id {
        let is_member = members
            .iter()
            .any(|member| member.as_str() == Some(requested_bot_id));
        if (kind == "project" && !is_member)
            || (kind != "project"
                && associated_bot.is_some_and(|associated| associated != requested_bot_id))
        {
            return Err(rpc_error(
                "invalid_params",
                "mock reply Bot must belong to the selected chat",
                None,
            ));
        }
        requested_bot_id.to_owned()
    } else if kind == "project" {
        members
            .iter()
            .filter_map(Value::as_str)
            .find(|member| {
                state
                    .bots
                    .iter()
                    .any(|bot| bot.get("id").and_then(Value::as_str) == Some(member))
            })
            .map(str::to_owned)
            .ok_or_else(|| rpc_error("not_found", "project chat has no member Bot", None))?
    } else {
        associated_bot
            .map(str::to_owned)
            .or_else(|| {
                state
                    .bots
                    .iter()
                    .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                    .and_then(|bot| bot.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
            })
            .ok_or_else(|| rpc_error("not_found", "chat has no Bot", None))?
    };
    if !state
        .bots
        .iter()
        .any(|bot| bot.get("id").and_then(Value::as_str) == Some(&bot_id))
    {
        return Err(rpc_error("not_found", "bot not found", None));
    }
    Ok(bot_id)
}

fn create_chat_reply_placeholder(
    state: &mut MockState,
    chat_id: &str,
    source_message_id: &str,
    bot_id: &str,
    requested_message_id: Option<&str>,
) -> Result<String, super::RpcError> {
    let message_id = requested_message_id
        .map(str::to_owned)
        .unwrap_or_else(|| id("msg"));
    if state
        .messages
        .values()
        .flatten()
        .any(|message| message.get("id").and_then(Value::as_str) == Some(message_id.as_str()))
    {
        return Err(rpc_error("conflict", "message id is already in use", None));
    }
    let created_at = now();
    let seq = next_chat_message_seq(state, chat_id);
    let message = json!({
        "id":message_id,
        "chat_id":chat_id,
        "seq":seq,
        "sender":{"kind":"bot","bot_id":bot_id},
        "created_at":created_at,
        "edited_at":null,
        "deleted":false,
        "reply_to":null,
        "quote":null,
        "thread_count":0,
        "mentions":[],
        "blocks":[],
        "fallback_text":"",
        "intent":null,
        "assignment_id":null,
        "source_message_id":source_message_id,
        "streaming":true,
        "delivery":[],
        "loaded_skills":[],
        "reactions":[]
    });
    state
        .messages
        .entry(chat_id.to_owned())
        .or_default()
        .push(message.clone());
    state.emit("message.created", json!({"message":message.clone()}));
    if let Some(chat_update) = update_chat_preview(state, chat_id, &message) {
        state.emit("chat.updated", json!({"chat":chat_update}));
    }
    Ok(message_id)
}

fn replay_chat_reply_lifecycle_from_rpc(
    state: &mut MockState,
    params: &Value,
    gateway: &GatewayState,
    step_delay: StdDuration,
) -> RpcResult {
    if params
        .get("client_request_id")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(rpc_error(
            "invalid_params",
            "client_request_id is required",
            None,
        ));
    }
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "chat_id is required", None))?;
    let source_message_id = params
        .get("source_message_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "source_message_id is required", None))?;
    let source = state
        .messages
        .get(chat_id)
        .and_then(|messages| {
            messages.iter().find(|message| {
                message.get("id").and_then(Value::as_str) == Some(source_message_id)
            })
        })
        .ok_or_else(|| rpc_error("not_found", "source message not found in this chat", None))?;
    if source.pointer("/sender/kind").and_then(Value::as_str) != Some("user") {
        return Err(rpc_error(
            "invalid_params",
            "source_message_id must identify a user message in this chat",
            None,
        ));
    }
    let bot_id =
        chat_reply_bot_for_chat(state, chat_id, params.get("bot_id").and_then(Value::as_str))?;
    let requested_message_id = match params.get("message_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.as_str()),
        _ => {
            return Err(rpc_error(
                "invalid_params",
                "message_id must be a non-empty string or null",
                None,
            ));
        }
    };
    let message_id = if let Some(message_id) = requested_message_id {
        let placeholder = state
            .messages
            .get(chat_id)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
            })
            .ok_or_else(|| rpc_error("not_found", "placeholder not found in this chat", None))?;
        if placeholder.pointer("/sender/kind").and_then(Value::as_str) != Some("bot")
            || placeholder
                .pointer("/sender/bot_id")
                .and_then(Value::as_str)
                != Some(&bot_id)
            || placeholder.get("source_message_id").and_then(Value::as_str)
                != Some(source_message_id)
            || placeholder.get("streaming").and_then(Value::as_bool) != Some(true)
        {
            return Err(rpc_error(
                "conflict",
                "message_id must identify this source message's active Bot placeholder",
                None,
            ));
        }
        message_id.to_owned()
    } else {
        create_chat_reply_placeholder(state, chat_id, source_message_id, &bot_id, None)?
    };
    schedule_chat_reply_lifecycle(
        gateway.clone(),
        step_delay,
        chat_id.to_owned(),
        bot_id.clone(),
        source_message_id.to_owned(),
        message_id.clone(),
    );
    Ok(json!({
        "chat_id":chat_id,
        "source_message_id":source_message_id,
        "bot_id":bot_id,
        "message_id":message_id,
        "scheduled":true
    }))
}

fn schedule_chat_reply_lifecycle(
    gateway: GatewayState,
    step_delay: StdDuration,
    chat_id: String,
    bot_id: String,
    source_message_id: String,
    message_id: String,
) {
    tokio::spawn(async move {
        tokio::time::sleep(step_delay).await;
        gateway
            .publish_temporary(
                "message.delta",
                json!({"chat_id":chat_id,"message_id":message_id,"text":"收到，"}),
            )
            .await;
        tokio::time::sleep(step_delay).await;
        gateway
            .publish_temporary(
                "message.delta",
                json!({"chat_id":chat_id,"message_id":message_id,"text":"我来处理。"}),
            )
            .await;
        tokio::time::sleep(step_delay).await;
        if !finish_chat_reply_placeholder(
            &gateway,
            &chat_id,
            &bot_id,
            &source_message_id,
            &message_id,
        )
        .await
        {
            return;
        }
        tokio::time::sleep(step_delay).await;
        create_chat_reply_followup(&gateway, &chat_id, &bot_id, &source_message_id).await;
    });
}

async fn finish_chat_reply_placeholder(
    gateway: &GatewayState,
    chat_id: &str,
    bot_id: &str,
    source_message_id: &str,
    message_id: &str,
) -> bool {
    let _event_lock = gateway.event_lock.lock().await;
    let (message_event, chat_event) = {
        let mut state = gateway.inner.write().await;
        let Some(message) = state.messages.get_mut(chat_id).and_then(|messages| {
            messages
                .iter_mut()
                .find(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
        }) else {
            return false;
        };
        if message.pointer("/sender/bot_id").and_then(Value::as_str) != Some(bot_id)
            || message.get("source_message_id").and_then(Value::as_str) != Some(source_message_id)
            || message.get("streaming").and_then(Value::as_bool) != Some(true)
        {
            return false;
        }
        message["blocks"] = json!([{"type":"text","markdown":LIFECYCLE_FIRST_REPLY}]);
        message["fallback_text"] = json!(LIFECYCLE_FIRST_REPLY);
        message["intent"] = json!("ack");
        message["streaming"] = json!(false);
        let message = message.clone();
        let message_event = state.emit("message.updated", json!({"message":message.clone()}));
        let chat_event = update_chat_preview(&mut state, chat_id, &message)
            .map(|chat| state.emit("chat.updated", json!({"chat":chat})));
        (message_event, chat_event)
    };
    let _ = gateway.events.send(message_event);
    if let Some(chat_event) = chat_event {
        let _ = gateway.events.send(chat_event);
    }
    true
}

async fn create_chat_reply_followup(
    gateway: &GatewayState,
    chat_id: &str,
    bot_id: &str,
    source_message_id: &str,
) {
    let _event_lock = gateway.event_lock.lock().await;
    let (message_event, chat_event) = {
        let mut state = gateway.inner.write().await;
        let created_at = now();
        let seq = next_chat_message_seq(&state, chat_id);
        let message = json!({
            "id":id("msg"),
            "chat_id":chat_id,
            "seq":seq,
            "sender":{"kind":"bot","bot_id":bot_id},
            "created_at":created_at,
            "edited_at":null,
            "deleted":false,
            "reply_to":null,
            "quote":null,
            "thread_count":0,
            "mentions":[],
            "blocks":[{"type":"text","markdown":LIFECYCLE_FOLLOWUP_REPLY}],
            "fallback_text":LIFECYCLE_FOLLOWUP_REPLY,
            "intent":"ack",
            "assignment_id":null,
            "source_message_id":source_message_id,
            "streaming":false,
            "delivery":[],
            "loaded_skills":[],
            "reactions":[]
        });
        state
            .messages
            .entry(chat_id.to_owned())
            .or_default()
            .push(message.clone());
        let message_event = state.emit("message.created", json!({"message":message.clone()}));
        let chat_event = update_chat_preview(&mut state, chat_id, &message)
            .map(|chat| state.emit("chat.updated", json!({"chat":chat})));
        (message_event, chat_event)
    };
    let _ = gateway.events.send(message_event);
    if let Some(chat_event) = chat_event {
        let _ = gateway.events.send(chat_event);
    }
}

fn update_chat_preview(state: &mut MockState, chat_id: &str, message: &Value) -> Option<Value> {
    let message_seq = message.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let chat = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))?;
    let last_seq = chat.get("last_seq").and_then(Value::as_u64).unwrap_or(0);
    if last_seq > message_seq {
        return None;
    }
    chat["last_message"] = json!({
        "message_id":message.get("id"),
        "sender":message.get("sender"),
        "text":message.get("fallback_text").and_then(Value::as_str).unwrap_or(""),
        "created_at":message.get("created_at")
    });
    chat["last_seq"] = json!(message_seq);
    chat["updated_at"] = message.get("created_at").cloned().unwrap_or(Value::Null);
    Some(chat.clone())
}

#[cfg(test)]
#[path = "mock_chat_reply_lifecycle_test.rs"]
mod chat_reply_lifecycle_tests;

fn search(state: &MockState, params: &Value) -> RpcResult {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| rpc_error("invalid_params", "query is required", None))?
        .to_lowercase();
    let kinds = params
        .get("kinds")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_lowercase)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100) as usize;
    let accepts = |kind: &str| kinds.is_empty() || kinds.contains(kind);
    let mut hits = Vec::new();
    let mut add = |kind: &str,
                   id: Option<&str>,
                   chat_id: Option<&str>,
                   title: String,
                   snippet: String,
                   at: Option<&str>| {
        let Some(id) = id.filter(|value| !value.is_empty()) else {
            return;
        };
        if !accepts(kind) {
            return;
        }
        let haystack = format!("{} {} {}", id, title, snippet).to_lowercase();
        if !query.is_empty() && !haystack.contains(&query) {
            return;
        }
        hits.push(json!({
            "kind":kind,
            "id":id,
            "chat_id":chat_id,
            "title":title,
            "snippet":snippet,
            "at":at,
        }));
    };

    for bot in &state.bots {
        let title = bot
            .get("name")
            .or_else(|| bot.get("label"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let snippet = bot
            .get("description")
            .or_else(|| bot.get("label"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        add(
            "bot",
            bot.get("id").and_then(Value::as_str),
            bot.get("dm_chat_id").and_then(Value::as_str),
            title,
            snippet,
            bot.get("updated_at").and_then(Value::as_str),
        );
    }
    for chat in &state.chats {
        let title = chat
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let snippet = chat
            .get("last_message")
            .and_then(|message| message.get("fallback_text").or(Some(message)))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        add(
            "chat",
            chat.get("id").and_then(Value::as_str),
            chat.get("id").and_then(Value::as_str),
            title,
            snippet,
            chat.get("updated_at").and_then(Value::as_str),
        );
    }
    let mut artifact_ids = std::collections::HashSet::new();
    for (chat_id, messages) in &state.messages {
        for message in messages {
            let snippet = message
                .get("fallback_text")
                .or_else(|| message.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            add(
                "message",
                message.get("id").and_then(Value::as_str),
                Some(chat_id),
                snippet.clone(),
                snippet,
                message.get("created_at").and_then(Value::as_str),
            );
            if let Some(blocks) = message.get("blocks").and_then(Value::as_array) {
                for block in blocks {
                    for artifact in block
                        .get("artifacts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let id = artifact
                            .get("id")
                            .or_else(|| artifact.get("artifact_id"))
                            .and_then(Value::as_str);
                        if id.is_some_and(|value| artifact_ids.insert(value.to_owned())) {
                            let title = artifact
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            let path = artifact
                                .get("path_or_url")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            add(
                                "artifact",
                                id,
                                Some(chat_id),
                                title,
                                path,
                                artifact
                                    .get("updated_at")
                                    .or_else(|| artifact.get("created_at"))
                                    .and_then(Value::as_str),
                            );
                        }
                    }
                }
            }
        }
    }
    for announcement in extra_get(state, "announcements") {
        for artifact in announcement
            .get("artifacts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = artifact.get("id").and_then(Value::as_str);
            if id.is_some_and(|value| artifact_ids.insert(value.to_owned())) {
                add(
                    "artifact",
                    id,
                    None,
                    artifact
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    artifact
                        .get("path_or_url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    artifact
                        .get("updated_at")
                        .or_else(|| artifact.get("created_at"))
                        .and_then(Value::as_str),
                );
            }
        }
    }
    for routine in extra_get(state, "routines") {
        add(
            "routine",
            routine.get("id").and_then(Value::as_str),
            None,
            routine
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            routine
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            routine.get("updated_at").and_then(Value::as_str),
        );
    }
    hits.sort_by(|left, right| {
        left.get("kind")
            .and_then(Value::as_str)
            .cmp(&right.get("kind").and_then(Value::as_str))
            .then_with(|| {
                left.get("id")
                    .and_then(Value::as_str)
                    .cmp(&right.get("id").and_then(Value::as_str))
            })
    });
    hits.truncate(limit);
    Ok(json!({"results":hits}))
}

fn takeover_bot_id(params: &Value) -> String {
    params
        .get("bot_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or("bot_main")
        .to_owned()
}

fn takeover_start(state: &mut MockState, params: &Value) -> RpcResult {
    let bot_id = takeover_bot_id(params);
    let assignment_id = params
        .get("assignment_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            state
                .assignments
                .iter()
                .find(|assignment| {
                    assignment.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str())
                        && assignment.get("status").and_then(Value::as_str) == Some("working")
                })
                .and_then(|assignment| assignment.get("id").and_then(Value::as_str))
                .map(str::to_owned)
        })
        .or_else(|| {
            state
                .assignments
                .iter()
                .find(|assignment| {
                    assignment.get("status").and_then(Value::as_str) == Some("working")
                })
                .and_then(|assignment| assignment.get("id").and_then(Value::as_str))
                .map(str::to_owned)
        })
        .ok_or_else(|| rpc_error("not_found", "no working assignment", None))?;
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("user takeover")
        .to_owned();
    let mut takeovers = extra_get(state, "takeovers");
    takeovers.retain(|item| item.get("bot_id").and_then(Value::as_str) != Some(bot_id.as_str()));
    takeovers.push(json!({"bot_id":bot_id,"assignment_id":assignment_id,"reason":reason,"state":"active","at":now()}));
    extra_set(state, "takeovers", takeovers);
    let tabs = vec![
        json!({"bot_id":bot_id,"tab_id":"tab_mock_1","assignment_id":assignment_id,
            "title":"Mock 登录页","url":"https://example.test/login","active":true}),
        json!({"bot_id":bot_id,"tab_id":"tab_mock_2","assignment_id":assignment_id,
            "title":"Mock 工作台","url":"https://example.test/workbench","active":false}),
    ];
    extra_set(state, &format!("screen_tabs:{bot_id}"), tabs);
    extra_set(
        state,
        &format!("screen_driver:{bot_id}"),
        vec![json!({"bot_id":bot_id,"driver":"user"})],
    );
    Ok(json!({}))
}

fn takeover_release(state: &mut MockState, params: &Value) -> RpcResult {
    let bot_id = takeover_bot_id(params);
    if extra_get(state, &format!("screen_tabs:{bot_id}")).is_empty() {
        takeover_start(state, params)?;
    }
    let note = params.get("note").cloned().unwrap_or(Value::Null);
    let mut takeovers = extra_get(state, "takeovers");
    for item in &mut takeovers {
        if item.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str()) {
            item["state"] = json!("done");
            item["note"] = note.clone();
            item["released_at"] = json!(now());
        }
    }
    extra_set(state, "takeovers", takeovers);
    extra_set(
        state,
        &format!("screen_driver:{bot_id}"),
        vec![json!({"bot_id":bot_id,"driver":"bot"})],
    );
    Ok(json!({}))
}

fn request_signature(method: &str, params: &Value) -> String {
    let mut params = params.clone();
    if let Some(object) = params.as_object_mut() {
        object.remove("client_request_id");
    }
    format!("{method}:{}", params)
}

fn ensure_loaded_skills_defaults(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("id") && object.contains_key("sender") {
                object.entry("loaded_skills").or_insert(json!([]));
            }
            for child in object.values_mut() {
                ensure_loaded_skills_defaults(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                ensure_loaded_skills_defaults(item);
            }
        }
        _ => {}
    }
}

fn extra_get(state: &MockState, key: &str) -> Vec<Value> {
    state.extra.get(key).cloned().unwrap_or_default()
}
fn extra_set(state: &mut MockState, key: &str, values: Vec<Value>) {
    state.extra.insert(key.to_string(), values);
}

fn persist_mock_skills(home: &std::path::Path, state: &MockState) -> std::io::Result<()> {
    let directory = home.join("data");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("mock-skills.json");
    let temporary = directory.join(format!("mock-skills-{}.tmp", std::process::id()));
    let content = serde_json::to_vec(&extra_get(state, "skills")).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, path)
}

fn working_assignment_context(state: &MockState) -> Option<(String, String, String)> {
    state
        .assignments
        .iter()
        .find(|assignment| assignment.get("status").and_then(Value::as_str) == Some("working"))
        .and_then(|assignment| {
            let assignment_id = assignment.get("id").and_then(Value::as_str)?;
            let bot_id = assignment.get("bot_id").and_then(Value::as_str)?;
            let chat_id = state
                .bots
                .iter()
                .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
                .and_then(|bot| bot.get("dm_chat_id").and_then(Value::as_str))?;
            Some((
                assignment_id.to_owned(),
                bot_id.to_owned(),
                chat_id.to_owned(),
            ))
        })
}

fn login_member_bot_ids(state: &MockState) -> Vec<String> {
    state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some("chat_login"))
        .and_then(|chat| chat.get("member_bot_ids").and_then(Value::as_array))
        .map(|members| {
            members
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .filter(|members: &Vec<String>| !members.is_empty())
        .unwrap_or_else(|| {
            state
                .bots
                .iter()
                .filter_map(|bot| bot.get("id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
}

fn bot_role_note(state: &MockState, bot_id: &str) -> String {
    state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
        .and_then(|bot| bot.get("label").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned()
}

fn login_project_members(
    state: &MockState,
    member_ids: &[String],
    joined_at: &Value,
) -> Vec<Value> {
    member_ids
        .iter()
        .map(|bot_id| {
            json!({
                "bot_id": bot_id,
                "role_note": bot_role_note(state, bot_id),
                "joined_at": joined_at
            })
        })
        .collect()
}

fn login_announcement_members(
    state: &MockState,
    project_id: &str,
    member_ids: &[String],
) -> Vec<Value> {
    member_ids
        .iter()
        .map(|bot_id| {
            let assignment = state
                .assignments
                .iter()
                .filter(|assignment| {
                    assignment.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str())
                        && assignment.get("project_id").and_then(Value::as_str) == Some(project_id)
                })
                .find(|assignment| {
                    matches!(
                        assignment.get("status").and_then(Value::as_str),
                        Some("working")
                            | Some("waiting_user")
                            | Some("waiting_bot")
                            | Some("blocked")
                    )
                })
                .or_else(|| {
                    state.assignments.iter().find(|assignment| {
                        assignment.get("bot_id").and_then(Value::as_str) == Some(bot_id.as_str())
                            && assignment.get("status").and_then(Value::as_str) == Some("done")
                    })
                });
            let (state_name, current_assignment_id, since) = match assignment {
                Some(assignment) => match assignment.get("status").and_then(Value::as_str) {
                    Some("working") => (
                        "working",
                        assignment.get("id").cloned().unwrap_or(Value::Null),
                        assignment.get("started_at").cloned().unwrap_or(Value::Null),
                    ),
                    Some("waiting_user") => (
                        "waiting_user",
                        assignment.get("id").cloned().unwrap_or(Value::Null),
                        assignment.get("started_at").cloned().unwrap_or(Value::Null),
                    ),
                    Some("waiting_bot") => (
                        "waiting_bot",
                        assignment.get("id").cloned().unwrap_or(Value::Null),
                        assignment.get("started_at").cloned().unwrap_or(Value::Null),
                    ),
                    Some("blocked") => (
                        "blocked",
                        assignment.get("id").cloned().unwrap_or(Value::Null),
                        assignment.get("started_at").cloned().unwrap_or(Value::Null),
                    ),
                    Some("done") => (
                        "done",
                        Value::Null,
                        assignment
                            .get("finished_at")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    _ => ("idle", Value::Null, Value::Null),
                },
                None => ("idle", Value::Null, Value::Null),
            };
            json!({
                "bot_id": bot_id,
                "role_note": bot_role_note(state, bot_id),
                "state": state_name,
                "current_assignment_id": current_assignment_id,
                "since": since
            })
        })
        .collect()
}

fn repair_login_members(state: &mut MockState) {
    let member_ids = login_member_bot_ids(state);
    let joined_at = state
        .projects
        .iter()
        .find(|project| project.get("id").and_then(Value::as_str) == Some("prj_login"))
        .and_then(|project| project.get("created_at"))
        .cloned()
        .unwrap_or_else(|| json!(now()));
    let project_members = login_project_members(state, &member_ids, &joined_at);
    for project in &mut state.projects {
        if project.get("id").and_then(Value::as_str) == Some("prj_login") {
            project["members"] = json!(project_members);
        }
    }
    let mut announcements = extra_get(state, "announcements");
    for announcement in &mut announcements {
        if announcement.get("project_id").and_then(Value::as_str) == Some("prj_login") {
            announcement["members"] =
                json!(login_announcement_members(state, "prj_login", &member_ids));
        }
    }
    if !announcements.is_empty() {
        extra_set(state, "announcements", announcements);
    }
}

fn repair_waiting_references(state: &mut MockState, home: &std::path::Path) {
    let context = working_assignment_context(state);
    let mut approvals = extra_get(state, "approvals");
    let mut questions = extra_get(state, "questions");
    if let Some((assignment_id, bot_id, chat_id)) = context {
        if approvals.is_empty() {
            approvals.push(json!({
                "id":"apr_mock_pending",
                "bot_id":bot_id,
                "assignment_id":assignment_id,
                "chat_id":chat_id,
                "tool":"bash",
                "risk":"exec",
                "summary":"Run the mock verification command",
                "detail":"The deterministic mock keeps one pending approval for the workbench.",
                "state":"pending",
                "created_at":now(),
                "decided_at":null
            }));
        }
        if questions.is_empty() {
            questions.push(json!({
                "id":"q_mock_pending",
                "bot_id":bot_id,
                "assignment_id":assignment_id,
                "chat_id":chat_id,
                "text":"Which deterministic mock path should continue?",
                "options":["safe","fast"],
                "allow_free_text":true,
                "state":"pending",
                "answer":null
            }));
        }
        for item in approvals
            .iter_mut()
            .filter(|item| item.get("id").and_then(Value::as_str) == Some("apr_mock_pending"))
        {
            item["bot_id"] = json!(bot_id);
            item["assignment_id"] = json!(assignment_id);
            item["chat_id"] = json!(chat_id);
        }
        for item in questions
            .iter_mut()
            .filter(|item| item.get("id").and_then(Value::as_str) == Some("q_mock_pending"))
        {
            item["bot_id"] = json!(bot_id);
            item["assignment_id"] = json!(assignment_id);
            item["chat_id"] = json!(chat_id);
        }
    } else {
        approvals.retain(|item| item.get("id").and_then(Value::as_str) != Some("apr_mock_pending"));
        questions.retain(|item| {
            item.get("id").and_then(Value::as_str) != Some("q_mock_pending")
                || item.get("state").and_then(Value::as_str) != Some("pending")
        });
    }
    for approval in &mut approvals {
        let supported = mock_always_allow_supported(approval, state, home);
        approval["always_allow_supported"] = json!(supported);
    }
    extra_set(state, "approvals", approvals);
    extra_set(state, "questions", questions);
}

fn mock_always_allow_supported(
    approval: &Value,
    state: &MockState,
    home: &std::path::Path,
) -> bool {
    let tool = approval.get("tool").and_then(Value::as_str).unwrap_or("");
    let detail = approval
        .get("detail")
        .map(|detail| match detail {
            Value::String(serialized) => {
                serde_json::from_str::<Value>(serialized).unwrap_or_else(|_| detail.clone())
            }
            _ => detail.clone(),
        })
        .unwrap_or(Value::Null);
    let assignment = approval
        .get("assignment_id")
        .and_then(Value::as_str)
        .and_then(|id| {
            state
                .assignments
                .iter()
                .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some(id))
        });
    let bot_id = approval
        .get("bot_id")
        .and_then(Value::as_str)
        .or_else(|| assignment.and_then(|item| item.get("bot_id").and_then(Value::as_str)))
        .unwrap_or("bot_main");
    let cwd = assignment
        .and_then(|item| item.get("project_id"))
        .and_then(Value::as_str)
        .and_then(|project_id| {
            state
                .projects
                .iter()
                .find(|project| project.get("id").and_then(Value::as_str) == Some(project_id))
                .and_then(|project| project.get("slug").and_then(Value::as_str))
        })
        .map(|slug| home.join("projects").join(mock_safe_component(slug)))
        .unwrap_or_else(|| home.join("bots").join(mock_safe_component(bot_id)));
    !mock_builtin_requires_approval(tool, &detail, &cwd, home)
}

fn mock_safe_component(value: &str) -> String {
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

fn mock_builtin_requires_approval(
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
    let Some(command) = args
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| args.as_str())
    else {
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
            let resolved = if *token == "~" || token.starts_with("~/") {
                let Some(user_home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
                    return true;
                };
                if *token == "~" {
                    user_home
                } else {
                    user_home.join(&token[2..])
                }
            } else if target.is_absolute() {
                target.to_path_buf()
            } else {
                cwd.join(target)
            };
            if !mock_lexically_normalize(&resolved).starts_with(mock_lexically_normalize(home)) {
                return true;
            }
        }
    }
    false
}

fn mock_lexically_normalize(path: &std::path::Path) -> std::path::PathBuf {
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

fn ensure_mock_defaults(state: &mut MockState, home: &std::path::Path) {
    if !state.settings.get("tools").is_some_and(Value::is_object) {
        state.settings["tools"] = json!({});
    }
    if !state
        .settings
        .pointer("/tools/defaults")
        .is_some_and(Value::is_object)
    {
        state.settings["tools"]["defaults"] = json!({
            "files":true,
            "bash":true,
            "browser":true,
            "subagent":true,
            "web":true,
            "mcp":false
        });
    } else {
        let defaults = state.settings["tools"]["defaults"]
            .as_object_mut()
            .expect("defaults was checked as an object");
        for (key, value) in [
            ("files", true),
            ("bash", true),
            ("browser", true),
            ("subagent", true),
            ("web", true),
            ("mcp", false),
        ] {
            defaults.entry(key).or_insert(json!(value));
        }
    }
    ensure_mock_skill_fixtures(state, home);
    ensure_mock_skill_attachment_fixtures(state, home);
    if extra_get(state, "providers").is_empty() {
        extra_set(
            state,
            "providers",
            vec![json!({
                "id":"prv_mock",
                "name":"Mock Provider",
                "api_kind":"openai-completions",
                "base_url":"mock://local",
                "has_key":false,
                "headers":{},
                "created_at":now(),
                "updated_at":now()
            })],
        );
    }
    if extra_get(state, "models").is_empty() {
        extra_set(
            state,
            "models",
            vec![json!({
                "ref":"prv_mock/mock-model",
                "provider_id":"prv_mock",
                "model_id":"mock-model",
                "display_name":"Mock Model",
                "context_window":128000,
                "max_output":8192,
                "caps":{"vision":false,"tools":true,"reasoning":false},
                "price":null,
                "enabled":true
            })],
        );
    }
    ensure_mock_usage_models(state);
    repair_login_members(state);
    repair_waiting_references(state, home);
    ensure_pending_review_fixture(state);
    ensure_question_approval_history_fixture(state);
    ensure_mock_user_memory(state);
    ensure_message_block_scenarios(state);
    ensure_mock_web_trace(state);
    ensure_login_question_history_fixture(state);
    ensure_code_dm_history_fixture(state);
    // The pending-review fixture creates its project and artifact references
    // above, so seed its files only after that fixture is present.
    seed_mock_artifact_files(state, home);
    ensure_mock_artifact_history(state, home);
    ensure_mock_routines(state);
    ensure_login_question_operation_scenarios(state);
}

/// Keep usage sample model IDs resolvable through provider.list, including
/// when an existing mock home already has a provider or a partial model list.
fn ensure_mock_usage_models(state: &mut MockState) {
    let provider_id = extra_get(state, "providers")
        .first()
        .and_then(|provider| provider.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("prv_mock")
        .to_owned();
    let mut models = extra_get(state, "models");
    for (model_id, display_name, vision) in [
        ("claude-sonnet", "Claude Sonnet", false),
        ("deepseek-chat", "DeepSeek Chat", false),
        ("claude-haiku", "Claude Haiku", false),
        ("free-model", "免费模型", false),
    ] {
        let model_ref = format!("{provider_id}/{model_id}");
        if models
            .iter()
            .any(|model| model.get("ref").and_then(Value::as_str) == Some(&model_ref))
        {
            continue;
        }
        models.push(json!({
            "ref":model_ref,
            "provider_id":provider_id,
            "model_id":model_id,
            "display_name":display_name,
            "context_window":128000,
            "max_output":8192,
            "caps":{"vision":vision,"tools":true,"reasoning":false},
            "price":null,
            "enabled":true
        }));
    }
    extra_set(state, "models", models);
}

/// Give the active web-redesign assignment a useful, deterministic trace for
/// client acceptance. The fixture remains a working task: its last bash tool
/// call is intentionally still open, matching the assignment status and the
/// workbench's active-task relationship.
fn ensure_mock_web_trace(state: &mut MockState) {
    const ASSIGNMENT_ID: &str = "asg_code_web";
    const CHAT_ID: &str = "chat_web";
    const RUN_ID: &str = "run_code_web";

    if !state.assignments.iter().any(|assignment| {
        assignment.get("id").and_then(Value::as_str) == Some(ASSIGNMENT_ID)
            && assignment.get("status").and_then(Value::as_str) == Some("working")
            && assignment.get("origin_chat_id").and_then(Value::as_str) == Some(CHAT_ID)
    }) {
        return;
    }

    let items = [
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":2,
            "at":"2026-10-09T10:19:04.000Z","type":"llm.request",
            "data":{"request_id":"req_code_web_1","model":"prv_mock/mock-model",
                "context":{"l0":3200,"l1":8400,"l2":1600,"l3":12000,"l4":5800,"total":31000},
                "tools":["files.read","files.write","bash","subagent"],"prompt_ref":null}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":3,
            "at":"2026-10-09T10:19:06.000Z","type":"llm.response",
            "data":{"request_id":"req_code_web_1","text":"我先检查现有首页、路由和登录入口。",
                "thinking":"确认当前导航结构，再分开处理页面布局和构建验证。",
                "tool_calls":[],"stop_reason":"stop",
                "usage":{"input_tokens":4200,"output_tokens":180,"cache_read_tokens":900,"cache_write_tokens":0,"cost":0.01,"requests":1},
                "latency_ms":1800,"ttft_ms":420}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":4,
            "at":"2026-10-09T10:19:07.000Z","type":"tool.start",
            "data":{"call_id":"call_web_read","name":"files.read","args":{"path":"src/routes/index.tsx"}}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":5,
            "at":"2026-10-09T10:19:08.000Z","type":"tool.end",
            "data":{"call_id":"call_web_read","is_error":false,"preview":"18 routes · 官网入口使用旧版表单组件",
                "details":{"lines":212},"truncated":false,"full_output":null,"duration_ms":14}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":"run_code_web_sub1","aseq":6,
            "at":"2026-10-09T10:19:10.000Z","type":"run.start",
            "data":{"phase":"subagent","model":"prv_mock/mock-model","parent_run_id":RUN_ID,
                "subagent_task":"检查官网登录入口的移动端布局与可访问性"}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":"run_code_web_sub1","aseq":7,
            "at":"2026-10-09T10:19:11.000Z","type":"llm.request",
            "data":{"request_id":"req_code_web_sub1","model":"prv_mock/mock-model",
                "context":{"l0":300,"l1":700,"l2":100,"l3":0,"l4":0,"total":1100},
                "tools":["files.read"],"prompt_ref":null}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":"run_code_web_sub1","aseq":8,
            "at":"2026-10-09T10:19:14.000Z","type":"llm.response",
            "data":{"request_id":"req_code_web_sub1","text":"移动窄屏下按钮需要改为纵向排列。",
                "thinking":null,"tool_calls":[],"stop_reason":"stop",
                "usage":{"input_tokens":700,"output_tokens":90,"cache_read_tokens":0,"cache_write_tokens":0,"cost":0.0,"requests":1},
                "latency_ms":1100,"ttft_ms":250}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":"run_code_web_sub1","aseq":9,
            "at":"2026-10-09T10:19:14.500Z","type":"run.end",
            "data":{"status":"done","error":null}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":10,
            "at":"2026-10-09T10:23:20.000Z","type":"llm.request",
            "data":{"request_id":"req_code_web_2","model":"prv_mock/mock-model",
                "context":{"l0":4100,"l1":9800,"l2":3200,"l3":18000,"l4":7000,"total":42100},
                "tools":["files.write","bash"],"prompt_ref":null}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":11,
            "at":"2026-10-09T10:23:23.000Z","type":"llm.response",
            "data":{"request_id":"req_code_web_2","text":"已完成首页导航与登录表单调整，准备跑测试。",
                "thinking":null,"tool_calls":[{"call_id":"call_web_test","name":"bash","args":{"command":"npm test"}}],
                "stop_reason":"tool_calls",
                "usage":{"input_tokens":6100,"output_tokens":340,"cache_read_tokens":1800,"cache_write_tokens":0,"cost":0.02,"requests":1},
                "latency_ms":2100,"ttft_ms":330}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":12,
            "at":"2026-10-09T10:23:24.000Z","type":"tool.start",
            "data":{"call_id":"call_web_test","name":"bash","args":{"command":"npm test"}}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":13,
            "at":"2026-10-09T10:23:37.000Z","type":"tool.end",
            "data":{"call_id":"call_web_test","is_error":false,"preview":"18 tests passed",
                "details":{"exit_code":0},"truncated":false,"full_output":null,"duration_ms":12840}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":14,
            "at":"2026-10-09T10:34:02.000Z","type":"llm.request",
            "data":{"request_id":"req_code_web_3","model":"prv_mock/mock-model",
                "context":{"l0":5000,"l1":12000,"l2":4000,"l3":24000,"l4":9000,"total":54000},
                "tools":["bash"],"prompt_ref":null}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":15,
            "at":"2026-10-09T10:34:03.000Z","type":"llm.response",
            "data":{"request_id":"req_code_web_3","text":"单元测试通过，正在生成部署构建。",
                "thinking":null,"tool_calls":[{"call_id":"call_web_deploy","name":"bash","args":{"command":"npm run deploy"}}],
                "stop_reason":"tool_calls",
                "usage":{"input_tokens":7800,"output_tokens":460,"cache_read_tokens":2400,"cache_write_tokens":0,"cost":0.03,"requests":1},
                "latency_ms":2400,"ttft_ms":360}
        }),
        json!({
            "assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,"run_id":RUN_ID,"aseq":16,
            "at":"2026-10-09T10:34:04.000Z","type":"tool.start",
            "data":{"call_id":"call_web_deploy","name":"bash","args":{"command":"npm run deploy"}}
        }),
    ];

    let trace = state.traces.entry(ASSIGNMENT_ID.to_owned()).or_default();
    for item in items {
        let aseq = item["aseq"].as_u64().unwrap_or_default();
        if !trace
            .iter()
            .any(|existing| existing.get("aseq").and_then(Value::as_u64) == Some(aseq))
        {
            trace.push(item);
        }
    }
    trace.sort_by_key(|item| item.get("aseq").and_then(Value::as_u64).unwrap_or_default());

    // Scenario trace events are indexed both by assignment and by chat.
    // Preserve that same lookup behavior for the deterministic acceptance tail.
    let assignment_items = trace.clone();
    let chat_trace = state.traces.entry(CHAT_ID.to_owned()).or_default();
    for item in assignment_items {
        let assignment = item.get("assignment_id").and_then(Value::as_str);
        let aseq = item.get("aseq").and_then(Value::as_u64);
        if !chat_trace.iter().any(|existing| {
            existing.get("assignment_id").and_then(Value::as_str) == assignment
                && existing.get("aseq").and_then(Value::as_u64) == aseq
        }) {
            chat_trace.push(item);
        }
    }
    chat_trace.sort_by_key(|item| {
        (
            item.get("at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("aseq").and_then(Value::as_u64).unwrap_or_default(),
        )
    });
}

fn ensure_mock_skill_fixtures(state: &mut MockState, home: &std::path::Path) {
    if !extra_get(state, "mock_skills_loaded").is_empty() {
        return;
    }

    let path = home.join("data/mock-skills.json");
    if let Ok(raw) = std::fs::read(&path) {
        if let Ok(skills) = serde_json::from_slice::<Vec<Value>>(&raw) {
            extra_set(state, "skills", skills);
            extra_set(state, "mock_skills_loaded", vec![json!({"loaded":true})]);
            return;
        }
    }

    let mut skills = extra_get(state, "skills");
    if skills.is_empty() {
        skills.extend([
            skill_default(
                "agent-browser",
                "浏览器自动化和页面交互",
                "builtin",
                Some("# agent-browser\nUse the browser tool for web interactions."),
            ),
            skill_default(
                "macbot-collab",
                "Mac Bot 多 Bot 协作",
                "builtin",
                Some("# macbot-collab\nCoordinate work through the main bot."),
            ),
            skill_default(
                "project-home",
                "项目目录和文件约定",
                "builtin",
                Some("# project-home\nKeep project artifacts under the project home."),
            ),
        ]);
    }

    if !skills
        .iter()
        .any(|skill| skill.get("name").and_then(Value::as_str) == Some("web-auth-patterns"))
    {
        let mut skill = skill_default(
            "web-auth-patterns",
            "登录、注册和找回密码的实现规范",
            "user",
            Some("---\nname: web-auth-patterns\ndescription: 登录、注册和找回密码的实现规范\n---\n# 登录实现规范\n使用 bcrypt，检查会话安全，并为邮件验证与密码重置补齐测试。"),
        );
        let last_invoked_at = (Utc::now() - Duration::hours(2)).to_rfc3339();
        skill["path"] = json!("~/MacBot/skills/web-auth-patterns");
        skill["invocations_7d"] = json!({
            "total":14,
            "by_bot":[{"bot_id":"bot_code","count":12},{"bot_id":"bot_test","count":2}]
        });
        skill["last_invoked_at"] = json!(last_invoked_at);
        skills.push(skill);
    }

    if !skills
        .iter()
        .any(|skill| skill.get("name").and_then(Value::as_str) == Some("prd-template"))
    {
        let mut draft = skill_default(
            "prd-template",
            "登录功能产品需求文档模板",
            "draft",
            Some("---\nname: prd-template\ndescription: 登录功能产品需求文档模板\n---\n# 产品需求文档\n## 背景\n## 用户故事\n## 验收标准"),
        );
        draft["path"] = json!("~/MacBot/skills/.drafts/prd-template");
        draft["enabled"] = json!(false);
        draft["draft_from"] = json!({
            "bot_id":"bot_product",
            "bot_name":"产品",
            "project_id":"prj_login",
            "project_name":"登录功能"
        });
        skills.push(draft);
    }

    extra_set(state, "skills", skills);
    extra_set(state, "mock_skills_loaded", vec![json!({"loaded":true})]);
}

/// Keep the canonical skills screen example backed by files the mock can use,
/// including mock homes created before the attachment fixture was added.
fn ensure_mock_skill_attachment_fixtures(state: &mut MockState, home: &std::path::Path) {
    const ATTACHMENTS: [(&str, &str); 2] = [
        (
            "scripts/gen-test-users.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' '{\"users\":[{\"email\":\"alice@example.test\"},{\"email\":\"bob@example.test\"}]}'\n",
        ),
        (
            "references/owasp.md",
            "# Authentication security checklist\n\n- Store passwords with a modern password hash and per-password salt.\n- Set session cookies to HttpOnly, Secure, and SameSite.\n- Rate-limit login, recovery, and verification endpoints.\n- Use single-use, expiring tokens for email verification and password reset.\n",
        ),
    ];

    let mut skills = extra_get(state, "skills");
    let Some(skill) = skills.iter_mut().find(|skill| {
        skill.get("name").and_then(Value::as_str) == Some("web-auth-patterns")
            && skill.get("path").and_then(Value::as_str)
                == Some("~/MacBot/skills/web-auth-patterns")
    }) else {
        return;
    };

    let files = skill
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if files.is_empty() {
        skill["files"] = json!(ATTACHMENTS.iter().map(|(path, _)| path).collect::<Vec<_>>());
        extra_set(state, "skills", skills);
        let _ = persist_mock_skills(home, state);
    }

    let directory = home.join("skills/web-auth-patterns");
    for (relative_path, contents) in ATTACHMENTS {
        seed_mock_file(&directory.join(relative_path), contents);
    }
}

fn seed_mock_artifact_files(state: &MockState, home: &std::path::Path) {
    let slug = state
        .projects
        .iter()
        .find(|project| project.get("id").and_then(Value::as_str) == Some("prj_login"))
        .and_then(|project| project.get("slug"))
        .and_then(Value::as_str)
        .filter(|slug| {
            let mut components = std::path::Path::new(slug).components();
            matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
        })
        .unwrap_or("login");
    let product_dir = home.join("projects").join(slug).join("product");
    let files = [
        (
            product_dir.join("prd.md"),
            "# 登录功能 PRD\n\n版本 2：仅做邮箱登录，不做手机号；密码至少 8 位并包含字母和数字。\n",
        ),
        (
            product_dir.join("prototype/index.html"),
            "<!doctype html>\n<html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>登录原型</title><body><main><h1>登录</h1><p>邮箱登录原型</p></main></body></html>\n",
        ),
    ];
    for (path, contents) in files {
        seed_mock_file(&path, contents);
    }

    let review_slug = state
        .projects
        .iter()
        .find(|project| project.get("id").and_then(Value::as_str) == Some("prj_mock_review"))
        .and_then(|project| project.get("slug"))
        .and_then(Value::as_str)
        .filter(|slug| {
            let mut components = std::path::Path::new(slug).components();
            matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
        })
        .unwrap_or("mock-review");
    let review_dir = home.join("projects").join(review_slug);
    let review_files = [
        (
            review_dir.join("docs/brief.md"),
            "# 项目说明\n\nMock 待验收项目：交付示例文件，用于验证项目目录和文件预览。\n",
        ),
        (
            review_dir.join("src/main.rs"),
            "fn main() {\n    println!(\"mock review artifact\");\n}\n",
        ),
        (
            review_dir.join("reports/acceptance.md"),
            "# 验收报告\n\nMock 场景示例：项目说明、实现代码和验收报告均已交付。\n",
        ),
    ];
    for (path, contents) in review_files {
        seed_mock_file(&path, contents);
    }
}

fn seed_mock_file(path: &std::path::Path, contents: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    // Mock startup may run repeatedly. Only create missing files so an
    // existing project artifact or user edit is never replaced.
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return;
    };
    let _ = std::io::Write::write_all(&mut file, contents.as_bytes());
}

fn ensure_mock_routines(state: &mut MockState) {
    if !extra_get(state, "mock_routines_seeded").is_empty() {
        return;
    }
    if !extra_get(state, "routines").is_empty() {
        extra_set(state, "mock_routines_seeded", vec![json!({"seeded":true})]);
        return;
    }

    let now = Utc::now();
    let at = |minutes_ago: i64| {
        (now - Duration::minutes(minutes_ago)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let first_started = at(8);
    let first_finished = at(6);
    let second_started = at(5 * 24 * 60 + 55);
    let second_finished = at(5 * 24 * 60 + 53);
    let third_started = at(6 * 24 * 60 + 50);
    let third_finished = at(6 * 24 * 60 + 49);
    let next_weekday = next_mock_routine_run(now, 1, true);
    let next_daily = next_mock_routine_run(now, 1, false);

    let routines = vec![
        json!({
            "id":"rtn_mock_x_scout", "bot_id":"bot_test", "project_id":null,
            "name":"每日 X 科技侦察",
            "instructions":"浏览 X 时间线，挑出 3 条科技新闻，每条用两句话总结，并附上原链接。",
            "schedules":[{"cron":"0 9 * * 1-5","label":"工作日 09:00"}],
            "timezone":"Asia/Shanghai", "enabled":true, "next_run_at":next_weekday,
            "last_run":{
                "id":"rrn_mock_x_scout_today", "routine_id":"rtn_mock_x_scout",
                "assignment_id":null, "trigger":"schedule", "status":"done",
                "started_at":first_started, "finished_at":first_finished, "error":null
            },
            "created_at":at(7 * 24 * 60), "updated_at":at(0)
        }),
        json!({
            "id":"rtn_mock_morning_brief", "bot_id":"bot_main", "project_id":null,
            "name":"晨报", "instructions":"汇总今天需要关注的会话和待处理事项。",
            "schedules":[{"cron":"0 9 * * *","label":"每天 09:00"}],
            "timezone":"Asia/Shanghai", "enabled":true, "next_run_at":next_daily,
            "last_run":null, "created_at":at(14 * 24 * 60), "updated_at":at(0)
        }),
        json!({
            "id":"rtn_mock_weekly_summary", "bot_id":"bot_product", "project_id":null,
            "name":"周报汇总", "instructions":"整理本周项目进展和下周待办。",
            "schedules":[{"cron":"0 18 * * 5","label":"周五 18:00"}],
            "timezone":"Asia/Shanghai", "enabled":false, "next_run_at":null,
            "last_run":{
                "id":"rrn_mock_weekly_summary", "routine_id":"rtn_mock_weekly_summary",
                "assignment_id":null, "trigger":"schedule", "status":"failed",
                "started_at":third_started, "finished_at":third_finished, "error":"示例：登录失效"
            },
            "created_at":at(21 * 24 * 60), "updated_at":at(0)
        }),
    ];
    let runs = vec![
        json!({
            "id":"rrn_mock_x_scout_today", "routine_id":"rtn_mock_x_scout",
            "assignment_id":null, "trigger":"schedule", "status":"done",
            "started_at":first_started, "finished_at":first_finished, "error":null
        }),
        json!({
            "id":"rrn_mock_x_scout_20261007", "routine_id":"rtn_mock_x_scout",
            "assignment_id":null, "trigger":"schedule", "status":"done",
            "started_at":second_started, "finished_at":second_finished, "error":null
        }),
        json!({
            "id":"rrn_mock_x_scout_20261006", "routine_id":"rtn_mock_x_scout",
            "assignment_id":null, "trigger":"schedule", "status":"failed",
            "started_at":third_started, "finished_at":third_finished, "error":"登录失效"
        }),
    ];
    extra_set(state, "routines", routines);
    extra_set(state, "routine_runs", runs);
    extra_set(state, "mock_routines_seeded", vec![json!({"seeded":true})]);
}

fn next_mock_routine_run(now: DateTime<Utc>, hour_utc: u32, weekdays_only: bool) -> String {
    let mut next = now
        .date_naive()
        .and_hms_opt(hour_utc, 0, 0)
        .expect("valid mock routine time")
        .and_utc();
    loop {
        let is_weekend = matches!(next.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        if next > now && (!weekdays_only || !is_weekend) {
            break;
        }
        next += Duration::days(1);
    }
    next.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn ensure_mock_user_memory(state: &mut MockState) {
    if !mock_main_chat_is_cleared(state) {
        upsert_mock_message(
            state,
            "chat_main",
            "msg_main_welcome",
            json!({"kind":"user"}),
            json!([{"type":"text","markdown":"我不喜欢长篇报告：先给结论，细节放后面。"}]),
            "我不喜欢长篇报告：先给结论，细节放后面。".into(),
            None,
        );
    }
    if !state.extra.contains_key("memory_user_entries") {
        extra_set(
            state,
            "memory_user_entries",
            vec![
                json!({"id":"mem_mock_1","content":"回复用中文，代码和注释用英文。","version":1,"source":{"by":{"kind":"system"},"chat_id":"chat_login","message_id":"msg_opening"},"updated_by":{"kind":"system"},"created_at":"2026-10-09T10:20:00Z","updated_at":"2026-10-09T10:20:00Z"}),
                json!({"id":"mem_mock_2","content":"不喜欢长篇报告：先给结论，细节放后面。","version":2,"source":{"by":{"kind":"system"},"chat_id":"chat_main","message_id":"msg_main_welcome"},"updated_by":{"kind":"system"},"created_at":"2026-10-08T21:40:00Z","updated_at":"2026-10-10T09:15:00Z"}),
                json!({"id":"mem_mock_3","content":"周末不要推送非紧急通知。","version":1,"source":{"by":{"kind":"user"},"chat_id":null,"message_id":null},"updated_by":{"kind":"user"},"created_at":"2026-10-06T09:15:00Z","updated_at":"2026-10-06T09:15:00Z"}),
                json!({"id":"mem_mock_4","content":"旧版本 Bot 记录：讨论时先说明结论和依据。","version":1,"source":{"by":{"kind":"bot","bot_id":"bot_product"},"chat_id":"chat_login","message_id":"msg_legacy_memory"},"updated_by":{"kind":"system"},"created_at":"2026-10-03T08:00:00Z","updated_at":"2026-10-03T08:00:00Z"}),
                json!({"id":"mem_mock_conflict","content":"冲突演示：此条版本已在其他设备更新。","version":1,"source":{"by":{"kind":"system"},"chat_id":"chat_main","message_id":"msg_main_welcome"},"updated_by":{"kind":"system"},"created_at":"2026-10-04T08:00:00Z","updated_at":"2026-10-10T09:15:00Z"}),
                json!({"id":"mem_mock_quota_1","content":format!("{}{}", "长期偏好：回答尽量先给直接结论，再说明必要依据。" , "；".repeat(400)),"version":1,"source":{"by":{"kind":"system"},"chat_id":"chat_main","message_id":"msg_main_welcome"},"updated_by":{"kind":"system"},"created_at":"2026-10-02T08:00:00Z","updated_at":"2026-10-02T08:00:00Z"}),
                json!({"id":"mem_mock_quota_2","content":format!("{}{}", "文档偏好：结构清楚、少用冗长背景，代码示例保持简洁。" , "、".repeat(400)),"version":1,"source":{"by":{"kind":"system"},"chat_id":"chat_main","message_id":"msg_main_welcome"},"updated_by":{"kind":"system"},"created_at":"2026-10-01T08:00:00Z","updated_at":"2026-10-01T08:00:00Z"}),
                json!({"id":"mem_mock_quota_3","content":format!("{}{}", "协作偏好：任务完成后说明结果以及后续动作。" , "，".repeat(200)),"version":1,"source":{"by":{"kind":"system"},"chat_id":"chat_login","message_id":"msg_opening"},"updated_by":{"kind":"system"},"created_at":"2026-09-30T08:00:00Z","updated_at":"2026-09-30T08:00:00Z"}),
            ],
        );
        // The seeded list intentionally sits close to its 1,500-character
        // allowance. This switch makes the quota-full path deterministic even
        // when a user edits one of the long examples first.
        extra_set(state, "mock_force_quota_full", vec![json!(true)]);
        extra_set(state, "mock_conflict_once", vec![json!(true)]);
    }
    if extra_get(state, "memory_user_profile").is_empty() {
        extra_set(
            state,
            "memory_user_profile",
            vec![json!({
                "name":null,
                "language":null,
                "identity":null,
                "timezone":"Asia/Shanghai",
                "onboarded":false,
                "version":1,
                "updated_by":{"kind":"system"},
                "updated_at":now()
            })],
        );
    }
    if extra_get(state, "memory_user_profile_complete").is_empty() {
        extra_set(
            state,
            "memory_user_profile_complete",
            vec![json!({
                "name":"小明",
                "language":"zh-CN",
                "identity":"产品经理",
                "timezone":"Asia/Shanghai",
                "onboarded":true,
                "version":2,
                "updated_by":{"kind":"user"},
                "updated_at":"2026-10-10T09:15:00Z",
                "evidence":{"chat_id":"chat_main","message_id":"msg_main_welcome","text":"我不喜欢长篇报告：先给结论，细节放后面。"}
            })],
        );
    }
}

fn ensure_mock_profile_greeting(state: &mut MockState) {
    if mock_main_chat_is_cleared(state) {
        return;
    }
    let profile = extra_get(state, "memory_user_profile")
        .into_iter()
        .next()
        .unwrap_or(Value::Null);
    let is_empty = profile.get("onboarded").and_then(Value::as_bool) != Some(true)
        && ["name", "language", "identity"]
            .iter()
            .all(|field| profile.get(field).is_none_or(Value::is_null));
    if is_empty {
        upsert_mock_message(
            state,
            "chat_main",
            "msg_profile_onboarding_greeting_v1",
            json!({"kind":"bot","bot_id":"bot_main"}),
            json!([{"type":"text","markdown":"你好，我是秘书长。为了之后更好地配合你，想先了解你希望我怎么称呼你、使用什么语言，以及你的身份或角色；你也可以跳过。"}]),
            "你好，我是秘书长。为了之后更好地配合你，想先了解你希望我怎么称呼你、使用什么语言，以及你的身份或角色；你也可以跳过。".into(),
            None,
        );
    }
}

fn use_complete_mock_profile(state: &mut MockState) -> RpcResult {
    ensure_mock_user_memory(state);
    let profile = extra_get(state, "memory_user_profile_complete")
        .into_iter()
        .next()
        .ok_or_else(|| rpc_error("not_found", "complete mock profile is unavailable", None))?;
    extra_set(state, "memory_user_profile", vec![profile.clone()]);
    state.emit("memory.profile.updated", json!({"profile":profile}));
    Ok(json!({"profile":profile}))
}

fn set_mock_memory_scenario(state: &mut MockState, params: &Value) -> RpcResult {
    for (param, key) in [
        ("quota_full", "mock_force_quota_full"),
        ("conflict_once", "mock_conflict_once"),
    ] {
        if let Some(enabled) = params.get(param).and_then(Value::as_bool) {
            extra_set(state, key, vec![json!(enabled)]);
        }
    }
    Ok(json!({
        "quota_full":extra_get(state, "mock_force_quota_full").first().and_then(Value::as_bool).unwrap_or(false),
        "conflict_once":extra_get(state, "mock_conflict_once").first().and_then(Value::as_bool).unwrap_or(false),
    }))
}

fn mock_main_chat_is_cleared(state: &MockState) -> bool {
    extra_get(state, "mock_main_chat_cleared")
        .first()
        .and_then(Value::as_bool)
        == Some(true)
}

fn user_memory_quota(state: &MockState) -> Value {
    let used_chars = extra_get(state, "memory_user_entries")
        .iter()
        .filter_map(|entry| entry.get("content").and_then(Value::as_str))
        .map(|content| content.chars().count() as u64)
        .sum::<u64>();
    json!({"used_chars":used_chars,"limit_chars":1500})
}

fn user_memory_quota_entries(state: &MockState) -> Vec<Value> {
    extra_get(state, "memory_user_entries")
        .into_iter()
        .map(|mut entry| {
            if let Some(object) = entry.as_object_mut() {
                let entry_id = object.get("id").cloned().unwrap_or(Value::Null);
                object.insert("entry_id".into(), entry_id);
                object.insert("scope".into(), json!("user"));
                object.insert("kind".into(), json!("user_preference"));
                object.entry("version").or_insert(json!(1));
            }
            entry
        })
        .collect()
}

fn user_memory_list(state: &MockState) -> RpcResult {
    let mut entries = extra_get(state, "memory_user_entries");
    entries.sort_by(|left, right| {
        right
            .get("updated_at")
            .and_then(Value::as_str)
            .cmp(&left.get("updated_at").and_then(Value::as_str))
    });
    Ok(json!({"entries":entries,"quota":user_memory_quota(state)}))
}

fn user_memory_create(state: &mut MockState, params: &Value) -> RpcResult {
    let request: macbot_protocol::MemoryUserCreateParams =
        serde_json::from_value(params.clone())
            .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?;
    let content = request.content;
    if content.is_empty() || content.chars().count() > 500 {
        return Err(rpc_error(
            "invalid_params",
            "content must contain 1 to 500 characters",
            None,
        ));
    }
    if extra_get(state, "mock_force_quota_full")
        .first()
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(rpc_error(
            "quota_full",
            "user memory quota is full",
            Some(json!({"entries":user_memory_quota_entries(state)})),
        ));
    }
    let quota = user_memory_quota(state);
    let used = quota["used_chars"].as_u64().unwrap_or_default();
    if used + content.chars().count() as u64 > 1500 {
        return Err(rpc_error("quota_full", "user memory quota is full", None));
    }
    let timestamp = now();
    let entry = json!({"id":id("mem"),"content":content,"version":1,
        "source":{"by":{"kind":"user"},"chat_id":null,"message_id":null},
        "updated_by":{"kind":"user"},"created_at":timestamp,"updated_at":timestamp});
    let mut entries = extra_get(state, "memory_user_entries");
    entries.push(entry.clone());
    extra_set(state, "memory_user_entries", entries);
    let quota = user_memory_quota(state);
    state.emit("memory.user.updated", json!({"entry":entry,"quota":quota}));
    Ok(json!({"entry":entry,"quota":quota}))
}

fn user_memory_update(state: &mut MockState, params: &Value) -> RpcResult {
    let request: macbot_protocol::MemoryUserUpdateParams =
        serde_json::from_value(params.clone())
            .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?;
    let content = request.content;
    if content.is_empty() || content.chars().count() > 500 {
        return Err(rpc_error(
            "invalid_params",
            "content must contain 1 to 500 characters",
            None,
        ));
    }
    let mut entries = extra_get(state, "memory_user_entries");
    let index = entries
        .iter()
        .position(|entry| entry["id"] == request.entry_id)
        .ok_or_else(|| rpc_error("not_found", "memory entry not found", None))?;
    if request.entry_id == "mem_mock_conflict"
        && extra_get(state, "mock_conflict_once")
            .first()
            .and_then(Value::as_bool)
            == Some(true)
    {
        let current_version = entries[index]["version"].as_u64().unwrap_or(1);
        entries[index]["version"] = json!(current_version.saturating_add(1));
        let current = entries[index].clone();
        extra_set(state, "memory_user_entries", entries);
        extra_set(state, "mock_conflict_once", vec![json!(false)]);
        return Err(rpc_error(
            "conflict",
            "memory entry version conflict",
            Some(json!({"current":current})),
        ));
    }
    let current = entries[index].clone();
    if current["version"].as_u64() != Some(request.expected_version) {
        return Err(rpc_error(
            "conflict",
            "memory entry version conflict",
            Some(json!({"current":current})),
        ));
    }
    if extra_get(state, "mock_force_quota_full")
        .first()
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(rpc_error(
            "quota_full",
            "user memory quota is full",
            Some(json!({"entries":user_memory_quota_entries(state)})),
        ));
    }
    let used = user_memory_quota(state)["used_chars"]
        .as_u64()
        .unwrap_or_default();
    let old_chars = current["content"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .count() as u64;
    if used.saturating_sub(old_chars) + content.chars().count() as u64 > 1500 {
        return Err(rpc_error("quota_full", "user memory quota is full", None));
    }
    let entry = &mut entries[index];
    entry["content"] = json!(content);
    entry["version"] = json!(request.expected_version.saturating_add(1));
    entry["updated_by"] = json!({"kind":"user"});
    entry["updated_at"] = json!(now());
    let entry = entries[index].clone();
    extra_set(state, "memory_user_entries", entries);
    let quota = user_memory_quota(state);
    state.emit("memory.user.updated", json!({"entry":entry,"quota":quota}));
    Ok(json!({"entry":entry,"quota":quota}))
}

fn user_memory_delete(state: &mut MockState, params: &Value) -> RpcResult {
    let request: macbot_protocol::MemoryUserDeleteParams =
        serde_json::from_value(params.clone())
            .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?;
    let mut entries = extra_get(state, "memory_user_entries");
    let index = entries
        .iter()
        .position(|entry| entry["id"] == request.entry_id)
        .ok_or_else(|| rpc_error("not_found", "memory entry not found", None))?;
    if entries[index]["version"].as_u64() != Some(request.expected_version) {
        let current = entries[index].clone();
        return Err(rpc_error(
            "conflict",
            "memory entry version conflict",
            Some(json!({"current":current})),
        ));
    }
    entries.remove(index);
    extra_set(state, "memory_user_entries", entries);
    let quota = user_memory_quota(state);
    state.emit(
        "memory.user.deleted",
        json!({"entry_id":request.entry_id,"quota":quota}),
    );
    Ok(json!({"quota":quota}))
}

fn user_profile_get(state: &MockState) -> RpcResult {
    Ok(
        json!({"profile":extra_get(state, "memory_user_profile").into_iter().next().unwrap_or(Value::Null)}),
    )
}

fn user_profile_update(state: &mut MockState, params: &Value) -> RpcResult {
    let request: macbot_protocol::MemoryProfileUpdateParams =
        serde_json::from_value(params.clone())
            .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?;
    let mut profile = extra_get(state, "memory_user_profile").into_iter().next()
        .unwrap_or_else(|| json!({"name":null,"language":null,"identity":null,"timezone":"Asia/Shanghai","onboarded":false,"version":1,"updated_by":{"kind":"system"},"updated_at":now()}));
    if profile["version"].as_u64() != Some(request.expected_version) {
        return Err(rpc_error(
            "conflict",
            "user profile version conflict",
            Some(json!({"current":profile})),
        ));
    }
    for (key, value) in [
        ("name", request.name),
        ("language", request.language),
        ("identity", request.identity),
    ] {
        match value {
            macbot_protocol::Patch::Unset => {}
            macbot_protocol::Patch::Null => profile[key] = Value::Null,
            macbot_protocol::Patch::Value(value) => {
                profile[key] = if key == "language" {
                    json!(macbot_memory::normalize_profile_language(&value)
                        .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?)
                } else {
                    json!(value)
                };
            }
        }
    }
    if profile["name"]
        .as_str()
        .is_some_and(|value| value.chars().count() > 20)
        || profile["identity"]
            .as_str()
            .is_some_and(|value| value.chars().count() > 100)
    {
        return Err(rpc_error(
            "invalid_params",
            "profile field exceeds its maximum length",
            None,
        ));
    }
    profile["version"] = json!(request.expected_version.saturating_add(1));
    profile["updated_by"] = json!({"kind":"user"});
    profile["updated_at"] = json!(now());
    extra_set(state, "memory_user_profile", vec![profile.clone()]);
    state.emit("memory.profile.updated", json!({"profile":profile}));
    Ok(json!({"profile":profile}))
}

fn ensure_mock_artifact_history(state: &mut MockState, home: &std::path::Path) {
    if !extra_get(state, "mock_artifact_history_seeded").is_empty() {
        return;
    }

    let mut versions = extra_get(state, "artifact_versions");
    let version_one = "# 登录功能 PRD\n\n版本 1：邮箱登录，支持邮箱验证码。";
    let version_two =
        "# 登录功能 PRD\n\n版本 2：仅做邮箱登录，不做手机号；密码至少 8 位并包含字母和数字。";
    if !versions.iter().any(|entry| {
        entry
            .pointer("/version/artifact_id")
            .and_then(Value::as_str)
            == Some("art_prd")
    }) {
        versions.extend([
            json!({
                "version":{"artifact_id":"art_prd","version":1,"title":"PRD","path_or_url":"product/prd.md","bot_id":"bot_product","assignment_id":"asg_product","kind":"file","created_at":"2026-10-09T10:18:00Z","byte_size":version_one.len(),"mime":"text/markdown"},
                "content":version_one
            }),
            json!({
                "version":{"artifact_id":"art_prd","version":2,"title":"PRD","path_or_url":"product/prd.md","bot_id":"bot_product","assignment_id":"asg_product","kind":"file","created_at":"2026-10-09T10:24:00Z","byte_size":version_two.len(),"mime":"text/markdown"},
                "content":version_two
            }),
        ]);
    }

    let review_artifacts = extra_get(state, "announcements")
        .into_iter()
        .find(|announcement| {
            announcement.get("project_id").and_then(Value::as_str) == Some("prj_mock_review")
        })
        .and_then(|announcement| announcement.get("artifacts").cloned())
        .and_then(|artifacts| artifacts.as_array().cloned())
        .unwrap_or_default();
    let review_slug = state
        .projects
        .iter()
        .find(|project| project.get("id").and_then(Value::as_str) == Some("prj_mock_review"))
        .and_then(|project| project.get("slug"))
        .and_then(Value::as_str)
        .filter(|slug| {
            let mut components = std::path::Path::new(slug).components();
            matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
        })
        .unwrap_or("mock-review");
    for (artifact_id, relative_path, fallback_content) in [
        (
            "art_mock_brief",
            "docs/brief.md",
            "# 项目说明\n\nMock 待验收项目：交付示例文件，用于验证项目目录和文件预览。\n",
        ),
        (
            "art_mock_report",
            "reports/acceptance.md",
            "# 验收报告\n\nMock 场景示例：项目说明、实现代码和验收报告均已交付。\n",
        ),
    ] {
        if versions.iter().any(|entry| {
            entry
                .pointer("/version/artifact_id")
                .and_then(Value::as_str)
                == Some(artifact_id)
        }) {
            continue;
        }
        let Some(artifact) = review_artifacts
            .iter()
            .find(|artifact| artifact.get("id").and_then(Value::as_str) == Some(artifact_id))
        else {
            continue;
        };
        let file_path = home.join("projects").join(review_slug).join(relative_path);
        let content =
            std::fs::read_to_string(file_path).unwrap_or_else(|_| fallback_content.to_owned());
        versions.push(json!({
            "version":{
                "artifact_id":artifact_id,
                "version":1,
                "title":artifact["title"],
                "path_or_url":artifact["path_or_url"],
                "bot_id":artifact["bot_id"],
                "assignment_id":artifact["assignment_id"],
                "kind":"file",
                "created_at":artifact["created_at"],
                "byte_size":content.len(),
                "mime":"text/markdown"
            },
            "content":content
        }));
    }

    extra_set(state, "artifact_versions", versions);
    extra_set(
        state,
        "mock_artifact_history_seeded",
        vec![json!({"seeded":true})],
    );
}

fn artifact_versions_list(state: &MockState, params: &Value) -> RpcResult {
    let request: macbot_protocol::ArtifactVersionsParams =
        serde_json::from_value(params.clone())
            .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?;
    let artifact_id = request.artifact_id.as_str();
    if !mock_artifact_exists(state, artifact_id) {
        return Err(rpc_error("not_found", "artifact not found", None));
    }
    let mut versions = extra_get(state, "artifact_versions")
        .into_iter()
        .filter(|entry| {
            entry
                .pointer("/version/artifact_id")
                .and_then(Value::as_str)
                == Some(artifact_id)
        })
        .collect::<Vec<_>>();
    if let Some(before) = request.before_version {
        versions.retain(|entry| {
            entry.pointer("/version/version").and_then(Value::as_u64) < Some(before)
        });
    }
    versions.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .pointer("/version/version")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        )
    });
    let limit = request.limit.unwrap_or(20).clamp(1, 100) as usize;
    let has_more = versions.len() > limit;
    versions.truncate(limit);
    Ok(json!({
        "versions":versions.into_iter().filter_map(|entry| entry.get("version").cloned()).collect::<Vec<_>>(),
        "has_more":has_more
    }))
}

fn artifact_version_get(state: &MockState, params: &Value) -> RpcResult {
    let request: macbot_protocol::ArtifactVersionGetParams = serde_json::from_value(params.clone())
        .map_err(|error| rpc_error("invalid_params", &error.to_string(), None))?;
    let artifact_id = request.artifact_id.as_str();
    let version = request.version;
    if !mock_artifact_exists(state, artifact_id) {
        return Err(rpc_error("not_found", "artifact not found", None));
    }
    let entry = extra_get(state, "artifact_versions")
        .into_iter()
        .find(|entry| {
            entry
                .pointer("/version/artifact_id")
                .and_then(Value::as_str)
                == Some(artifact_id)
                && entry.pointer("/version/version").and_then(Value::as_u64) == Some(version)
        })
        .ok_or_else(|| rpc_error("not_found", "artifact version not found", None))?;
    Ok(json!({
        "version":entry["version"],
        "content":entry["content"],
        "content_encoding":"utf8"
    }))
}

fn mock_artifact_exists(state: &MockState, artifact_id: &str) -> bool {
    let matches_artifact = |artifact: &Value| {
        artifact.get("id").and_then(Value::as_str) == Some(artifact_id)
            || artifact.get("artifact_id").and_then(Value::as_str) == Some(artifact_id)
    };
    extra_get(state, "artifacts").iter().any(matches_artifact)
        || extra_get(state, "announcements")
            .iter()
            .any(|announcement| {
                announcement
                    .get("artifacts")
                    .and_then(Value::as_array)
                    .is_some_and(|artifacts| artifacts.iter().any(matches_artifact))
            })
        || state.messages.values().any(|messages| {
            messages.iter().any(|message| {
                message
                    .get("blocks")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block
                                .get("artifacts")
                                .and_then(Value::as_array)
                                .is_some_and(|artifacts| artifacts.iter().any(matches_artifact))
                        })
                    })
            })
        })
}

fn ensure_message_block_scenarios(state: &mut MockState) {
    const FIXTURES: [(&str, &str, &str, &str); 5] = [
        ("asg_mock_queued", "queued", "等待执行示例", "chat_login"),
        (
            "asg_mock_waiting",
            "waiting_user",
            "等待确认示例",
            "chat_login",
        ),
        ("asg_mock_working", "working", "执行中示例", "chat_login"),
        ("asg_mock_done", "done", "已完成示例", "chat_login"),
        ("asg_mock_failed", "failed", "失败任务示例", "chat_login"),
    ];
    let Some(template) = state
        .assignments
        .iter()
        .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some("asg_code"))
        .cloned()
    else {
        return;
    };
    for (assignment_id, status, title, chat_id) in FIXTURES {
        if !state
            .assignments
            .iter()
            .any(|assignment| assignment.get("id").and_then(Value::as_str) == Some(assignment_id))
        {
            let mut assignment = template.clone();
            assignment["id"] = json!(assignment_id);
            assignment["status"] = json!(status);
            assignment["title"] = json!(title);
            assignment["instruction"] = json!(title);
            assignment["origin_chat_id"] = json!(chat_id);
            assignment["project_id"] = json!("prj_login");
            assignment["queue_reason"] = if status == "queued" {
                json!("serial_in_project")
            } else {
                Value::Null
            };
            assignment["wait"] = if status == "waiting_user" {
                json!({"reason":"decision","message_id":"msg_mock_waiting"})
            } else {
                Value::Null
            };
            assignment["finished_at"] = if matches!(status, "failed" | "done") {
                json!(now())
            } else {
                Value::Null
            };
            state.assignments.push(assignment);
        }
    }
    if !state.assignments.iter().any(|assignment| {
        assignment.get("id").and_then(Value::as_str) == Some("asg_mock_delegation")
    }) {
        let mut assignment = template;
        assignment["id"] = json!("asg_mock_delegation");
        assignment["title"] = json!("快速调研");
        assignment["instruction"] = json!("查找登录竞品的邮箱验证流程");
        assignment["bot_id"] = json!("bot_product");
        assignment["project_id"] = Value::Null;
        assignment["origin_chat_id"] = json!("chat_main");
        assignment["status"] = json!("working");
        assignment["queue_reason"] = Value::Null;
        assignment["wait"] = Value::Null;
        state.assignments.push(assignment);
    }
    let fixtures = state.assignments.clone();
    for assignment in fixtures {
        let assignment_id = assignment.get("id").and_then(Value::as_str).unwrap_or("");
        if !matches!(
            assignment_id,
            "asg_mock_queued"
                | "asg_mock_waiting"
                | "asg_mock_working"
                | "asg_mock_done"
                | "asg_mock_failed"
                | "asg_mock_delegation"
        ) {
            continue;
        }
        let chat_id = assignment
            .get("origin_chat_id")
            .and_then(Value::as_str)
            .unwrap_or("chat_login");
        if chat_id == "chat_main" && mock_main_chat_is_cleared(state) {
            continue;
        }
        let is_delegation = assignment_id == "asg_mock_delegation";
        let message_id = if is_delegation {
            "msg_mock_delegation"
        } else {
            match assignment_id {
                "asg_mock_queued" => "msg_mock_task_queued",
                "asg_mock_waiting" => "msg_mock_task_waiting",
                "asg_mock_working" => "msg_mock_task_working",
                "asg_mock_done" => "msg_mock_task_done",
                "asg_mock_failed" => "msg_mock_task_failed",
                _ => continue,
            }
        };
        upsert_mock_message(
            state,
            chat_id,
            message_id,
            json!({"kind":"system"}),
            json!([if is_delegation {
                json!({"type":"delegation","bot_id":assignment["bot_id"],"assignment_id":assignment_id})
            } else {
                json!({"type":"task_card","assignment_id":assignment_id})
            }]),
            if is_delegation {
                format!("已委派给 {}：{}", assignment["bot_id"], assignment["title"])
            } else {
                format!("任务：{}", assignment["title"])
            },
            Some(assignment_id),
        );
    }
    upsert_mock_message(
        state,
        "chat_login",
        "msg_mock_system",
        json!({"kind":"system"}),
        json!([{"type":"system","code":"announcement_updated","text":"秘书长 更新了公告"}]),
        "秘书长 更新了公告".into(),
        None,
    );
}

fn ensure_login_question_history_fixture(state: &mut MockState) {
    const CHAT_ID: &str = "chat_login";
    const ASSIGNMENT_ID: &str = "asg_mock_waiting";
    if !state
        .assignments
        .iter()
        .any(|assignment| assignment.get("id").and_then(Value::as_str) == Some(ASSIGNMENT_ID))
        || !state
            .chats
            .iter()
            .any(|chat| chat.get("id").and_then(Value::as_str) == Some(CHAT_ID))
    {
        return;
    }
    let Some(bot_id) = state
        .assignments
        .iter()
        .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some(ASSIGNMENT_ID))
        .and_then(|assignment| assignment.get("bot_id"))
        .cloned()
    else {
        return;
    };

    let mut questions = extra_get(state, "questions");
    for question in [
        json!({
            "id":"q_mock_login_pending","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"验证邮件用哪个发件地址？","options":["noreply@app.com","support@app.com"],"multi":false,
            "allow_free_text":false,"state":"pending","answer":null
        }),
        json!({
            "id":"q_mock_login_answered","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"登录失败 5 次后要锁账号吗？","options":["锁定 15 分钟","不锁，只加验证码"],"multi":false,
            "allow_free_text":true,"state":"answered",
            "answer":{"option_index":1,"option_indexes":[1],"text":null,"at":"2026-10-10T10:40:00Z"}
        }),
        json!({
            "id":"q_mock_login_expired","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"登录页要不要放「记住我」？","options":["要","不要"],"multi":false,
            "allow_free_text":true,"state":"expired","answer":null
        }),
    ] {
        if !questions
            .iter()
            .any(|existing| existing.get("id") == question.get("id"))
        {
            questions.push(question);
        }
    }
    extra_set(state, "questions", questions);

    if state
        .messages
        .get(CHAT_ID)
        .into_iter()
        .flatten()
        .any(|message| {
            message.get("id").and_then(Value::as_str) == Some("msg_mock_login_questions")
        })
    {
        return;
    }
    let seq = next_chat_message_seq(state, CHAT_ID);
    let message = json!({
        "id":"msg_mock_login_questions","chat_id":CHAT_ID,"seq":seq,
        "sender":{"kind":"bot","bot_id":bot_id},"created_at":"2026-10-10T10:39:00Z",
        "edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[],
        "blocks":[
            {"type":"text","markdown":"登录方案有 3 个问题："},
            {"type":"question","question_id":"q_mock_login_pending"},
            {"type":"question","question_id":"q_mock_login_answered"},
            {"type":"question","question_id":"q_mock_login_expired"}
        ],
        "fallback_text":"登录方案有 3 个问题：验证邮件发件地址、失败锁定策略和记住我。",
        "intent":null,"assignment_id":ASSIGNMENT_ID,"streaming":false,"delivery":[],"reactions":[]
    });
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(CHAT_ID))
    {
        chat["last_message"] = json!({
            "message_id":message["id"],"sender":message["sender"],"text":message["fallback_text"],
            "created_at":message["created_at"]
        });
        chat["last_seq"] = json!(seq);
        chat["updated_at"] = message["created_at"].clone();
    }
    state
        .messages
        .entry(CHAT_ID.to_owned())
        .or_default()
        .push(message.clone());
    state.emit("message.created", json!({"message":message}));
}

/// Provide an independent, answerable three-question deck and single-question
/// history cards for prototype acceptance. Keep the older mixed-state fixture
/// above intact so pending, answered, and expired history remains available.
fn ensure_login_question_operation_scenarios(state: &mut MockState) {
    const CHAT_ID: &str = "chat_login";
    const FLOW_ASSIGNMENT_ID: &str = "asg_mock_waiting";

    let assignment_bot = |assignment_id: &str| {
        state
            .assignments
            .iter()
            .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some(assignment_id))
            .and_then(|assignment| assignment.get("bot_id"))
            .cloned()
    };
    let Some(flow_bot_id) = assignment_bot(FLOW_ASSIGNMENT_ID) else {
        return;
    };
    let Some(test_bot_id) = assignment_bot("asg_test") else {
        return;
    };
    let Some(product_bot_id) = assignment_bot("asg_product") else {
        return;
    };
    if !state
        .chats
        .iter()
        .any(|chat| chat.get("id").and_then(Value::as_str) == Some(CHAT_ID))
    {
        return;
    }

    let mut questions = extra_get(state, "questions");
    for question in [
        json!({
            "id":"q_mock_login_flow_1","bot_id":flow_bot_id,"assignment_id":FLOW_ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"登录失败 5 次后要锁账号吗？","options":["锁定 15 分钟","不锁，只加验证码","永久锁定，需要邮件解锁"],"multi":false,
            "allow_free_text":true,"state":"pending","answer":null
        }),
        json!({
            "id":"q_mock_login_flow_2","bot_id":flow_bot_id,"assignment_id":FLOW_ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"验证邮件用哪个发件地址？","options":["noreply@app.com","support@app.com"],"multi":false,
            "allow_free_text":true,"state":"pending","answer":null
        }),
        json!({
            "id":"q_mock_login_flow_3","bot_id":flow_bot_id,"assignment_id":FLOW_ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"本期要做哪些登录方式？","options":["邮箱 + 密码","邮箱验证码","Apple 登录"],"multi":true,
            "allow_free_text":false,"state":"pending","answer":null
        }),
        json!({
            "id":"q_mock_login_single_browsers","bot_id":test_bot_id,"assignment_id":"asg_test","chat_id":CHAT_ID,
            "text":"回归测试覆盖哪些浏览器？","options":["Chrome","Safari","Firefox"],"multi":true,
            "allow_free_text":false,"state":"answered",
            "answer":{"option_index":null,"option_indexes":[0,1],"text":null,"at":"2026-10-10T10:50:00Z"}
        }),
        json!({
            "id":"q_mock_login_single_remember","bot_id":product_bot_id,"assignment_id":"asg_product","chat_id":CHAT_ID,
            "text":"登录页要不要放「记住我」？","options":["要","不要"],"multi":false,
            "allow_free_text":true,"state":"answered",
            "answer":{"option_index":null,"option_indexes":[],"text":"先不要，下期再说","at":"2026-10-10T10:52:00Z"}
        }),
    ] {
        if !questions
            .iter()
            .any(|existing| existing.get("id") == question.get("id"))
        {
            questions.push(question);
        }
    }
    extra_set(state, "questions", questions);

    upsert_mock_message(
        state,
        CHAT_ID,
        "msg_mock_login_question_flow",
        json!({"kind":"bot","bot_id":flow_bot_id}),
        json!([
            {"type":"text","markdown":"实现前有 3 个问题需要你定一下："},
            {"type":"question","question_id":"q_mock_login_flow_1"},
            {"type":"question","question_id":"q_mock_login_flow_2"},
            {"type":"question","question_id":"q_mock_login_flow_3"}
        ]),
        "实现前有 3 个问题需要你定一下：登录失败策略、验证邮件发件地址和本期登录方式。".into(),
        Some(FLOW_ASSIGNMENT_ID),
    );
    upsert_mock_message(
        state,
        CHAT_ID,
        "msg_mock_login_single_browsers",
        json!({"kind":"bot","bot_id":test_bot_id}),
        json!([{"type":"question","question_id":"q_mock_login_single_browsers"}]),
        "回归测试覆盖哪些浏览器？已回答：Chrome、Safari。".into(),
        Some("asg_test"),
    );
    upsert_mock_message(
        state,
        CHAT_ID,
        "msg_mock_login_single_remember",
        json!({"kind":"bot","bot_id":product_bot_id}),
        json!([{"type":"question","question_id":"q_mock_login_single_remember"}]),
        "登录页要不要放「记住我」？已回答：先不要，下期再说。".into(),
        Some("asg_product"),
    );
}

fn ensure_code_dm_history_fixture(state: &mut MockState) {
    const CHAT_ID: &str = "chat_code";
    let Some(bot_id) = state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some("bot_code"))
        .map(|bot| json!(bot["id"]))
    else {
        return;
    };
    if !state
        .chats
        .iter()
        .any(|chat| chat.get("id").and_then(Value::as_str) == Some(CHAT_ID))
    {
        return;
    }
    upsert_mock_message(
        state,
        CHAT_ID,
        "msg_mock_code_dm_user",
        json!({"kind":"user"}),
        json!([{"type":"text","markdown":"登录那边的改动进展怎么样？"}]),
        "登录那边的改动进展怎么样？".into(),
        None,
    );
    upsert_mock_message(
        state,
        CHAT_ID,
        "msg_mock_code_dm_reply",
        json!({"kind":"bot","bot_id":bot_id}),
        json!([
            {"type":"text","markdown":"邮箱登录流程已经实现，正在跑测试。"},
            {"type":"project_card","project_id":"prj_login","context":"mentioned","note":"我在群里负责实现登录"}
        ]),
        "邮箱登录流程已经实现，正在跑测试。登录功能：我在群里负责实现登录。".into(),
        Some("asg_code"),
    );

    if extra_get(state, "approvals")
        .iter()
        .any(|approval| approval.get("id").and_then(Value::as_str) == Some("apr_mock_pending"))
    {
        upsert_mock_message(
            state,
            CHAT_ID,
            "msg_mock_code_dm_approval",
            json!({"kind":"bot","bot_id":bot_id}),
            json!([{"type":"text","markdown":"部署前需要你确认："},{"type":"approval","approval_id":"apr_mock_pending"}]),
            "部署前需要你确认执行发布命令。".into(),
            Some("asg_code_web"),
        );
    }
}

fn upsert_mock_message(
    state: &mut MockState,
    chat_id: &str,
    message_id: &str,
    sender: Value,
    blocks: Value,
    fallback_text: String,
    assignment_id: Option<&str>,
) {
    let seq = next_chat_message_seq(state, chat_id);
    let messages = state.messages.entry(chat_id.to_owned()).or_default();
    if messages
        .iter()
        .any(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
    {
        return;
    }
    let created_at = now();
    let message = json!({
        "id":message_id,"chat_id":chat_id,"seq":seq,"sender":sender,"created_at":created_at,
        "edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,"mentions":[],
        "blocks":blocks,"fallback_text":fallback_text,"intent":null,"assignment_id":assignment_id,
        "streaming":false,"delivery":[],"reactions":[]
    });
    messages.push(message.clone());
    state.emit("message.created", json!({"message":message.clone()}));
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
    {
        chat["last_message"] = json!({"message_id":message_id,"sender":sender,"text":fallback_text,"created_at":created_at});
        chat["last_seq"] = json!(seq);
        chat["updated_at"] = json!(created_at);
    }
}

fn next_chat_message_seq(state: &MockState, chat_id: &str) -> u64 {
    let message_seq = state
        .messages
        .get(chat_id)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("seq").and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    let chat_seq = state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
        .and_then(|chat| chat.get("last_seq"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    message_seq.max(chat_seq).saturating_add(1)
}

fn ensure_question_approval_history_fixture(state: &mut MockState) {
    const ASSIGNMENT_ID: &str = "asg_mock_questions";
    const CHAT_ID: &str = "chat_test";
    if !state
        .assignments
        .iter()
        .any(|assignment| assignment.get("id").and_then(Value::as_str) == Some(ASSIGNMENT_ID))
    {
        if let Some(mut assignment) = state
            .assignments
            .iter()
            .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some("asg_test"))
            .cloned()
        {
            assignment["id"] = json!(ASSIGNMENT_ID);
            assignment["title"] = json!("等待问题确认（Mock）");
            assignment["instruction"] = json!("用于展示历史问题卡片");
            assignment["status"] = json!("waiting_user");
            assignment["wait"] =
                json!({"reason":"decision","message_id":"msg_mock_question_history"});
            assignment["origin_chat_id"] = json!(CHAT_ID);
            assignment["project_id"] = Value::Null;
            assignment["finished_at"] = Value::Null;
            state.assignments.push(assignment);
        }
    }
    if !state
        .assignments
        .iter()
        .any(|assignment| assignment.get("id").and_then(Value::as_str) == Some(ASSIGNMENT_ID))
        || !state
            .chats
            .iter()
            .any(|chat| chat.get("id").and_then(Value::as_str) == Some(CHAT_ID))
    {
        return;
    }
    let Some(assignment) = state
        .assignments
        .iter()
        .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some(ASSIGNMENT_ID))
    else {
        return;
    };
    let Some(bot_id) = assignment
        .get("bot_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    let dm_chat_id = state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id.as_str()))
        .and_then(|bot| bot.get("dm_chat_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(dm_chat_id) = dm_chat_id else {
        return;
    };
    let mut questions = extra_get(state, "questions");
    for question in [
        json!({
            "id":"q_mock_history_answered","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"回归测试覆盖哪些浏览器？","options":["Chrome","Safari","Firefox"],"multi":true,
            "allow_free_text":false,"state":"answered",
            "answer":{"option_index":null,"option_indexes":[0,1],"text":null,"at":"2026-10-10T10:50:00Z"}
        }),
        json!({
            "id":"q_mock_history_expired","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"登录页要不要放「记住我」？","options":["要","不要"],"multi":false,
            "allow_free_text":true,"state":"expired",
            "answer":null
        }),
        json!({
            "id":"q_mock_history_pending","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":CHAT_ID,
            "text":"发件地址选哪个？","options":["noreply@app.com","support@app.com"],"multi":false,
            "allow_free_text":false,"state":"pending","answer":null
        }),
    ] {
        if !questions
            .iter()
            .any(|item| item.get("id") == question.get("id"))
        {
            questions.push(question);
        }
    }
    extra_set(state, "questions", questions);

    let mut approvals = extra_get(state, "approvals");
    if !approvals
        .iter()
        .any(|item| item.get("id").and_then(Value::as_str) == Some("apr_mock_history"))
    {
        approvals.push(json!({
            "id":"apr_mock_history","bot_id":bot_id,"assignment_id":ASSIGNMENT_ID,"chat_id":dm_chat_id,
            "tool":"bash","risk":"exec","summary":"执行发布命令","detail":"git push origin main",
            "matched_rule_text":"git push 先问我","always_allow_supported":false,
            "state":"pending","created_at":"2026-10-10T10:55:00Z","decided_at":null
        }));
    }
    extra_set(state, "approvals", approvals);

    if !state
        .messages
        .get(CHAT_ID)
        .into_iter()
        .flatten()
        .any(|message| {
            message.get("id").and_then(Value::as_str) == Some("msg_mock_question_history")
        })
    {
        let seq = next_chat_message_seq(state, CHAT_ID);
        let message = json!({
            "id":"msg_mock_question_history","chat_id":CHAT_ID,"seq":seq,"sender":{"kind":"bot","bot_id":bot_id},
            "created_at":"2026-10-10T10:50:00Z","edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
            "mentions":[],"blocks":[
                {"type":"text","markdown":"有 3 个问题需要确认："},
                {"type":"question","question_id":"q_mock_history_answered"},
                {"type":"question","question_id":"q_mock_history_expired"},
                {"type":"question","question_id":"q_mock_history_pending"}
            ],"fallback_text":"有 3 个问题需要确认：回归测试覆盖浏览器，是否记住我，发件地址。",
            "intent":null,"assignment_id":ASSIGNMENT_ID,"streaming":false,"delivery":[],"reactions":[]
        });
        if let Some(chat) = state
            .chats
            .iter_mut()
            .find(|chat| chat.get("id").and_then(Value::as_str) == Some(CHAT_ID))
        {
            chat["last_message"] = json!({
                "message_id":message["id"],
                "sender":message["sender"],
                "text":message["fallback_text"],
                "created_at":message["created_at"]
            });
            chat["last_seq"] = json!(seq);
            chat["updated_at"] = message["created_at"].clone();
        }
        state
            .messages
            .entry(CHAT_ID.to_owned())
            .or_default()
            .push(message.clone());
        state.emit("message.created", json!({"message":message}));
    }

    if !state
        .messages
        .get(&dm_chat_id)
        .into_iter()
        .flatten()
        .any(|message| {
            message.get("id").and_then(Value::as_str) == Some("msg_mock_approval_history")
        })
    {
        let seq = next_chat_message_seq(state, &dm_chat_id);
        let message = json!({
            "id":"msg_mock_approval_history","chat_id":dm_chat_id,"seq":seq,"sender":{"kind":"bot","bot_id":bot_id},
            "created_at":"2026-10-10T10:55:00Z","edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
            "mentions":[],"blocks":[{"type":"approval","approval_id":"apr_mock_history"}],
            "fallback_text":"需要批准执行发布命令。","intent":null,"assignment_id":ASSIGNMENT_ID,
            "streaming":false,"delivery":[],"reactions":[]
        });
        if let Some(chat) = state
            .chats
            .iter_mut()
            .find(|chat| chat.get("id").and_then(Value::as_str) == Some(dm_chat_id.as_str()))
        {
            chat["last_message"] = json!({
                "message_id":message["id"],
                "sender":message["sender"],
                "text":message["fallback_text"],
                "created_at":message["created_at"]
            });
            chat["last_seq"] = json!(seq);
            chat["updated_at"] = message["created_at"].clone();
        }
        state
            .messages
            .entry(dm_chat_id.to_owned())
            .or_default()
            .push(message.clone());
        state.emit("message.created", json!({"message":message}));
    }
}

fn ensure_pending_review_fixture(state: &mut MockState) {
    const PROJECT_ID: &str = "prj_mock_review";
    let fixture_is_complete = state
        .projects
        .iter()
        .any(|project| project.get("id").and_then(Value::as_str) == Some(PROJECT_ID))
        && state
            .messages
            .get("chat_mock_review")
            .is_some_and(|messages| {
                [
                    "msg_mock_review_brief",
                    "msg_mock_review_progress",
                    "msg_mock_review_complete",
                ]
                .iter()
                .all(|message_id| {
                    messages.iter().any(|message| {
                        message.get("id").and_then(Value::as_str) == Some(message_id)
                    })
                })
            })
        && extra_get(state, "announcements")
            .iter()
            .any(|announcement| {
                announcement.get("project_id").and_then(Value::as_str) == Some(PROJECT_ID)
                    && announcement
                        .get("artifacts")
                        .and_then(Value::as_array)
                        .is_some_and(|artifacts| artifacts.len() == 3)
            });
    if fixture_is_complete {
        return;
    }
    let Some(mut project) = state
        .projects
        .iter()
        .find(|project| project.get("id").and_then(Value::as_str) == Some("prj_login"))
        .cloned()
    else {
        return;
    };
    let Some(mut chat) = state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some("chat_login"))
        .cloned()
    else {
        return;
    };
    let updated_at = now();
    let chat_id = "chat_mock_review";
    project["id"] = json!(PROJECT_ID);
    project["chat_id"] = json!(chat_id);
    project["name"] = json!("Mock 待验收项目");
    project["slug"] = json!("mock-review");
    project["home_path"] = json!("~/MacBot/projects/mock-review/");
    project["status"] = json!("review");
    project["done_at"] = Value::Null;
    project["updated_at"] = json!(updated_at);
    project["highlights"] = json!([{"at":updated_at,"text":"示例交付已完成，等待验收"}]);
    chat["id"] = json!(chat_id);
    chat["title"] = project["name"].clone();
    chat["project_id"] = json!(PROJECT_ID);
    chat["last_message"] = Value::Null;
    chat["last_seq"] = json!(0);
    chat["last_read_seq"] = json!(0);
    chat["unread"] = json!(0);
    chat["attention"] = json!("none");
    chat["updated_at"] = json!(updated_at);
    state
        .projects
        .retain(|existing| existing.get("id").and_then(Value::as_str) != Some(PROJECT_ID));
    state
        .chats
        .retain(|existing| existing.get("id").and_then(Value::as_str) != Some(chat_id));
    state.projects.push(project);
    state.chats.push(chat);

    let mut announcements = extra_get(state, "announcements");
    if let Some(mut announcement) = announcements
        .iter()
        .find(|item| item.get("project_id").and_then(Value::as_str) == Some("prj_login"))
        .cloned()
    {
        announcement["project_id"] = json!(PROJECT_ID);
        announcement["text"] = json!("# Mock 待验收项目\n\n示例交付已完成，请检查成果。\n");
        announcement["artifacts"] = json!([
            {
                "id":"art_mock_brief",
                "project_id":PROJECT_ID,
                "assignment_id":"asg_mock_done",
                "bot_id":"bot_product",
                "kind":"file",
                "title":"项目说明",
                "path_or_url":"docs/brief.md",
                "created_at":updated_at,
                "updated_at":updated_at
            },
            {
                "id":"art_mock_build",
                "project_id":PROJECT_ID,
                "assignment_id":"asg_mock_done",
                "bot_id":"bot_code",
                "kind":"dir",
                "title":"实现代码",
                "path_or_url":"src/",
                "created_at":updated_at,
                "updated_at":updated_at
            },
            {
                "id":"art_mock_report",
                "project_id":PROJECT_ID,
                "assignment_id":"asg_mock_done",
                "bot_id":"bot_test",
                "kind":"file",
                "title":"验收报告",
                "path_or_url":"reports/acceptance.md",
                "created_at":updated_at,
                "updated_at":updated_at
            }
        ]);
        announcement["highlights"] = json!([{"at":updated_at,"text":"示例交付已完成，等待验收"}]);
        announcement["version"] = json!(1);
        announcement["updated_at"] = json!(updated_at);
        announcements
            .retain(|item| item.get("project_id").and_then(Value::as_str) != Some(PROJECT_ID));
        announcements.push(announcement);
        extra_set(state, "announcements", announcements);
    }
    let _ = set_review_card_mock(
        state,
        PROJECT_ID,
        "pending",
        json!([]),
        "Mock 待验收项目已完成，请检查交付。",
    );
    upsert_mock_message(
        state,
        chat_id,
        "msg_mock_review_brief",
        json!({"kind":"user"}),
        json!([{"type":"text","markdown":"请完成并验收这个示例项目。"}]),
        "请完成并验收这个示例项目。".into(),
        None,
    );
    upsert_mock_message(
        state,
        chat_id,
        "msg_mock_review_progress",
        json!({"kind":"bot","bot_id":"bot_main"}),
        json!([{"type":"progress","text":"已汇总项目说明、实现代码和验收报告。"}]),
        "已汇总项目说明、实现代码和验收报告。".into(),
        None,
    );
    upsert_mock_message(
        state,
        chat_id,
        "msg_mock_review_complete",
        json!({"kind":"bot","bot_id":"bot_main"}),
        json!([{
            "type":"completion",
            "summary":"Mock 示例项目已完成，等待验收。",
            "artifacts":[
                {"artifact_id":"art_mock_brief","title":"项目说明","path_or_url":"docs/brief.md"},
                {"artifact_id":"art_mock_build","title":"实现代码","path_or_url":"src/"},
                {"artifact_id":"art_mock_report","title":"验收报告","path_or_url":"reports/acceptance.md"}
            ],
            "next":[],
            "notify_main":false
        }]),
        "Mock 示例项目已完成，等待验收。".into(),
        None,
    );
}
fn pending(state: &MockState) -> Value {
    let reviews = state
        .projects
        .iter()
        .filter(|project| project.get("status").and_then(Value::as_str) == Some("review"))
        .filter_map(|project| project.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    json!({
        "approvals":extra_get(state,"approvals").iter().filter(|x| x.get("state").and_then(Value::as_str)==Some("pending")).cloned().collect::<Vec<_>>(),
        "questions":extra_get(state,"questions").iter().filter(|x| x.get("state").and_then(Value::as_str)==Some("pending")).cloned().collect::<Vec<_>>(),
        "reviews":reviews
    })
}
#[derive(Clone)]
struct UsageSample {
    ts: DateTime<Utc>,
    bot_id: &'static str,
    project_id: Option<&'static str>,
    model_id: &'static str,
    phase: &'static str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cost: Option<f64>,
    task_done: bool,
}

fn usage_seed(now: DateTime<Utc>) -> Vec<UsageSample> {
    vec![
        UsageSample {
            ts: now - Duration::hours(12),
            bot_id: "bot_main",
            project_id: Some("prj_login"),
            model_id: "claude-sonnet",
            phase: "work",
            input_tokens: 1_000,
            output_tokens: 400,
            cache_read_tokens: 100,
            cache_write_tokens: 20,
            cost: Some(0.12),
            task_done: true,
        },
        UsageSample {
            ts: now - Duration::days(1) - Duration::hours(3),
            bot_id: "bot_code",
            project_id: Some("prj_login"),
            model_id: "deepseek-chat",
            phase: "memory",
            input_tokens: 600,
            output_tokens: 200,
            cache_read_tokens: 50,
            cache_write_tokens: 5,
            cost: None,
            task_done: false,
        },
        UsageSample {
            ts: now - Duration::days(2),
            bot_id: "bot_main",
            project_id: Some("prj_web"),
            model_id: "claude-sonnet",
            phase: "compact",
            input_tokens: 1_200,
            output_tokens: 300,
            cache_read_tokens: 0,
            cache_write_tokens: 50,
            cost: Some(0.20),
            task_done: false,
        },
        UsageSample {
            ts: now - Duration::days(3) - Duration::hours(4),
            bot_id: "bot_test",
            project_id: Some("prj_web"),
            model_id: "claude-haiku",
            phase: "work",
            input_tokens: 800,
            output_tokens: 500,
            cache_read_tokens: 80,
            cache_write_tokens: 10,
            cost: Some(0.35),
            task_done: true,
        },
        UsageSample {
            ts: now - Duration::days(4),
            bot_id: "bot_main",
            project_id: Some("prj_login"),
            model_id: "deepseek-chat",
            phase: "coordinate",
            input_tokens: 450,
            output_tokens: 180,
            cache_read_tokens: 30,
            cache_write_tokens: 0,
            cost: Some(0.07),
            task_done: true,
        },
        UsageSample {
            ts: now - Duration::days(5) - Duration::hours(2),
            bot_id: "bot_code",
            project_id: Some("prj_web"),
            model_id: "deepseek-chat",
            phase: "work",
            input_tokens: 700,
            output_tokens: 250,
            cache_read_tokens: 40,
            cache_write_tokens: 15,
            cost: Some(0.11),
            task_done: false,
        },
        UsageSample {
            ts: now - Duration::days(6),
            bot_id: "bot_product",
            project_id: Some("prj_login"),
            model_id: "free-model",
            phase: "memory",
            input_tokens: 300,
            output_tokens: 100,
            cache_read_tokens: 25,
            cache_write_tokens: 5,
            cost: None,
            task_done: false,
        },
        // Falls into the previous interval for the default seven-day window.
        UsageSample {
            ts: now - Duration::days(9),
            bot_id: "bot_main",
            project_id: Some("prj_login"),
            model_id: "claude-sonnet",
            phase: "work",
            input_tokens: 500,
            output_tokens: 160,
            cache_read_tokens: 20,
            cache_write_tokens: 0,
            cost: Some(0.05),
            task_done: true,
        },
        // The prior 30-day dashboard window must contain resolvable activity
        // so its comparison cards never render as an empty baseline.
        UsageSample {
            ts: now - Duration::days(35),
            bot_id: "bot_code",
            project_id: Some("prj_login"),
            model_id: "deepseek-chat",
            phase: "work",
            input_tokens: 320,
            output_tokens: 120,
            cache_read_tokens: 20,
            cache_write_tokens: 5,
            cost: Some(0.04),
            task_done: true,
        },
        UsageSample {
            ts: now - Duration::days(44) - Duration::hours(2),
            bot_id: "bot_test",
            project_id: Some("prj_web"),
            model_id: "claude-haiku",
            phase: "verify",
            input_tokens: 280,
            output_tokens: 90,
            cache_read_tokens: 10,
            cache_write_tokens: 0,
            cost: Some(0.03),
            task_done: true,
        },
        UsageSample {
            ts: now - Duration::days(56),
            bot_id: "bot_product",
            project_id: Some("prj_login"),
            model_id: "free-model",
            phase: "coordinate",
            input_tokens: 190,
            output_tokens: 70,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: None,
            task_done: false,
        },
    ]
}

fn usage_window(params: &Value) -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    let from = params
        .get("from")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or(now - Duration::days(7));
    let to = params
        .get("to")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or(now);
    (from, to.max(from + Duration::milliseconds(1)))
}

fn usage_samples(params: &Value) -> (Vec<UsageSample>, DateTime<Utc>, DateTime<Utc>) {
    let (from, to) = usage_window(params);
    let samples = usage_seed(Utc::now())
        .into_iter()
        .filter(|sample| sample.ts >= from && sample.ts < to)
        .collect();
    (samples, from, to)
}

fn usage_totals(samples: &[UsageSample]) -> Value {
    let input_tokens = samples.iter().map(|x| x.input_tokens).sum::<u64>();
    let output_tokens = samples.iter().map(|x| x.output_tokens).sum::<u64>();
    let cache_read_tokens = samples.iter().map(|x| x.cache_read_tokens).sum::<u64>();
    let cache_write_tokens = samples.iter().map(|x| x.cache_write_tokens).sum::<u64>();
    let cost = samples.iter().filter_map(|x| x.cost).sum::<f64>();
    json!({
        "input_tokens":input_tokens,
        "output_tokens":output_tokens,
        "cache_read_tokens":cache_read_tokens,
        "cache_write_tokens":cache_write_tokens,
        "requests":samples.len() as u64,
        "cost":if samples.iter().any(|x| x.cost.is_some()) { json!(cost) } else { Value::Null }
    })
}

fn usage_metric(params: &Value) -> &'static str {
    match params
        .get("metric")
        .and_then(Value::as_str)
        .unwrap_or("tokens")
    {
        "cost" => "cost",
        "requests" => "requests",
        _ => "tokens",
    }
}

fn metric_value(sample: &UsageSample, metric: &str) -> f64 {
    match metric {
        "cost" => sample.cost.unwrap_or(0.0),
        "requests" => 1.0,
        _ => (sample.input_tokens + sample.output_tokens) as f64,
    }
}

fn usage_summary(params: &Value) -> RpcResult {
    let (current, from, _) = usage_samples(params);
    let span = usage_window(params).1 - from;
    let previous_params = json!({"from":(from - span).to_rfc3339(),"to":from.to_rfc3339()});
    let (previous, _, _) = usage_samples(&previous_params);
    Ok(
        json!({"current":usage_totals_with_tasks(&current),"previous":usage_totals_with_tasks(&previous)}),
    )
}

fn usage_totals_with_tasks(samples: &[UsageSample]) -> Value {
    let mut totals = usage_totals(samples);
    totals["tasks_done"] = json!(samples.iter().filter(|x| x.task_done).count() as u64);
    totals
}

fn usage_heatmap(params: &Value) -> RpcResult {
    let (samples, from, to) = usage_samples(params);
    let metric = usage_metric(params);
    let mode = params
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("calendar");
    if mode == "weekhour" {
        let mut matrix = vec![vec![0.0; 24]; 7];
        for sample in &samples {
            let local = sample.ts;
            matrix[local.weekday().num_days_from_monday() as usize][local.hour() as usize] +=
                metric_value(sample, metric);
        }
        let values = matrix
            .iter()
            .flatten()
            .copied()
            .filter(|x| *x > 0.0)
            .collect::<Vec<_>>();
        return Ok(json!({"matrix":matrix,"thresholds":thresholds(&values)}));
    }
    let mut days = Vec::new();
    let mut day = from.date_naive();
    while day <= to.date_naive() && days.len() < 370 {
        let day_samples = samples
            .iter()
            .filter(|sample| sample.ts.date_naive() == day)
            .collect::<Vec<_>>();
        let value = day_samples
            .iter()
            .map(|sample| metric_value(sample, metric))
            .sum::<f64>();
        let top_bot_id = day_samples
            .iter()
            .max_by(|a, b| metric_value(a, metric).total_cmp(&metric_value(b, metric)))
            .map(|sample| sample.bot_id);
        days.push(json!({"date":day.to_string(),"value":value,"tokens":day_samples.iter().map(|x| x.input_tokens+x.output_tokens).sum::<u64>(),"cost":day_samples.iter().filter_map(|x| x.cost).sum::<f64>(),"requests":day_samples.len() as u64,"top_bot_id":top_bot_id}));
        day = day.succ_opt().unwrap_or(day);
    }
    let values = days
        .iter()
        .filter_map(|day| day["value"].as_f64())
        .filter(|x| *x > 0.0)
        .collect::<Vec<_>>();
    Ok(json!({"days":days,"thresholds":thresholds(&values)}))
}

fn thresholds(values: &[f64]) -> [f64; 3] {
    if values.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    [
        values[0],
        values[values.len() / 2],
        *values.last().unwrap_or(&0.0),
    ]
}

fn usage_timeseries(params: &Value) -> RpcResult {
    let (samples, from, to) = usage_samples(params);
    let metric = usage_metric(params);
    let requested = params
        .get("granularity")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let granularity = match requested {
        "auto" if to - from <= Duration::days(7) => "hour",
        "auto" if to - from <= Duration::days(90) => "day",
        "auto" => "week",
        "hour" | "day" | "week" => requested,
        _ => return Err(rpc_error("invalid_params", "invalid granularity", None)),
    };
    let step = match granularity {
        "hour" => Duration::hours(1),
        "week" => Duration::weeks(1),
        _ => Duration::days(1),
    };
    let mut buckets = Vec::new();
    let mut cursor = if granularity == "hour" {
        from.with_minute(0)
            .and_then(|x| x.with_second(0))
            .and_then(|x| x.with_nanosecond(0))
            .unwrap_or(from)
    } else {
        from.date_naive()
            .and_hms_opt(0, 0, 0)
            .map(|x| DateTime::<Utc>::from_naive_utc_and_offset(x, Utc))
            .unwrap_or(from)
    };
    while cursor < to && buckets.len() < 370 {
        buckets.push(cursor);
        cursor += step;
    }
    let dimension = params
        .get("dimension")
        .and_then(Value::as_str)
        .unwrap_or("model");
    let mut keys = std::collections::BTreeSet::new();
    for sample in &samples {
        keys.insert(sample_dimension(sample, dimension).to_string());
    }
    let mut keys = keys.into_iter().collect::<Vec<_>>();
    let top = params
        .get("top")
        .and_then(Value::as_u64)
        .unwrap_or(keys.len() as u64) as usize;
    let mut ranked = keys
        .iter()
        .map(|key| {
            (
                key.clone(),
                samples
                    .iter()
                    .filter(|sample| sample_dimension(sample, dimension) == key)
                    .map(|sample| metric_value(sample, metric))
                    .sum::<f64>(),
            )
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    keys = ranked
        .iter()
        .take(top.max(1))
        .map(|x| x.0.clone())
        .collect();
    if ranked.len() > keys.len() {
        keys.push("other".into());
    }
    let split_io = params
        .get("split_io")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let series = keys.into_iter().map(|key| {
        let mut values = Vec::new(); let mut input = Vec::new(); let mut output = Vec::new();
        for bucket in &buckets {
            let end = *bucket + step;
            let rows = samples.iter().filter(|sample| sample.ts >= *bucket && sample.ts < end && (key == "other" || sample_dimension(sample, dimension) == key)).collect::<Vec<_>>();
            values.push(rows.iter().map(|x| metric_value(x, metric)).sum::<f64>());
            input.push(rows.iter().map(|x| x.input_tokens as f64).sum::<f64>());
            output.push(rows.iter().map(|x| x.output_tokens as f64).sum::<f64>());
        }
        json!({"key":key,"label":key,"values":values,"input_values":if split_io {json!(input)} else {Value::Null},"output_values":if split_io {json!(output)} else {Value::Null},"total":values.iter().sum::<f64>()})
    }).collect::<Vec<_>>();
    Ok(
        json!({"granularity":granularity,"buckets":buckets.into_iter().map(|x|x.to_rfc3339()).collect::<Vec<_>>(),"series":series}),
    )
}

fn sample_dimension(sample: &UsageSample, dimension: &str) -> &'static str {
    match dimension {
        "bot" => sample.bot_id,
        "project" => sample.project_id.unwrap_or("unassigned"),
        _ => sample.model_id,
    }
}

fn usage_breakdown(params: &Value) -> RpcResult {
    let (samples, from, to) = usage_samples(params);
    let metric = usage_metric(params);
    let dimension = params
        .get("dimension")
        .and_then(Value::as_str)
        .unwrap_or("bot");
    let drill = params.get("drill");
    let samples = samples
        .into_iter()
        .filter(|sample| {
            drill.is_none_or(|drill| {
                drill
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .is_none_or(|id| sample.bot_id == id)
                    && drill
                        .get("project_id")
                        .and_then(Value::as_str)
                        .is_none_or(|id| sample.project_id == Some(id))
            })
        })
        .collect::<Vec<_>>();
    let mut keys = std::collections::BTreeSet::new();
    for sample in &samples {
        keys.insert(sample_dimension(sample, dimension).to_string());
    }
    let mut rows = Vec::new();
    for key in keys {
        let group = samples
            .iter()
            .filter(|sample| sample_dimension(sample, dimension) == key)
            .collect::<Vec<_>>();
        let mut phases = std::collections::BTreeMap::new();
        for sample in &group {
            *phases.entry(sample.phase).or_insert(0.0) += metric_value(sample, metric);
        }
        let mut sparkline = Vec::new();
        let mut day = from.date_naive();
        while day <= to.date_naive() && sparkline.len() < 370 {
            sparkline.push(
                group
                    .iter()
                    .filter(|sample| sample.ts.date_naive() == day)
                    .map(|sample| metric_value(sample, metric))
                    .sum::<f64>(),
            );
            day = day.succ_opt().unwrap_or(day);
        }
        rows.push(json!({"key":key,"label":key,"usage":usage_totals(&group.iter().map(|x| (*x).clone()).collect::<Vec<_>>()),"sparkline":sparkline,"phases":phases}));
    }
    Ok(json!({"rows":rows}))
}

fn find_wrapped(items: &[Value], id_value: Option<&Value>, kind: &str) -> RpcResult {
    let id = id_value.and_then(Value::as_str);
    items
        .iter()
        .find(|v| id.is_some() && v.get("id").and_then(Value::as_str) == id)
        .cloned()
        .map(|value| json!({kind:value}))
        .ok_or_else(|| rpc_error("not_found", &format!("{kind} not found"), None))
}

fn paged(
    items: &[Value],
    params: &Value,
    seq_key: &str,
    default: usize,
    max: usize,
) -> (Vec<Value>, bool) {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(default as u64)
        .clamp(1, max as u64) as usize;
    let before = params.get("before_seq").and_then(Value::as_u64);
    let after = params.get("after_seq").and_then(Value::as_u64);
    let filtered: Vec<_> = items
        .iter()
        .filter(|item| {
            let seq = item.get(seq_key).and_then(Value::as_u64).unwrap_or(0);
            before.is_none_or(|v| seq < v) && after.is_none_or(|v| seq > v)
        })
        .cloned()
        .collect();
    let has_more = filtered.len() > limit;
    (filtered.into_iter().take(limit).collect(), has_more)
}

fn chat_list(state: &MockState, params: &Value) -> RpcResult {
    let include_archived = params
        .get("include_archived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let chats = state
        .chats
        .iter()
        .filter(|chat| {
            include_archived || chat.get("archived").and_then(Value::as_bool) != Some(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({"chats":chats}))
}

fn chat_history(state: &MockState, params: &Value) -> RpcResult {
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or("chat_main");
    let mut messages = state.messages.get(chat_id).cloned().unwrap_or_default();
    for message in &mut messages {
        message
            .as_object_mut()
            .expect("mock messages are objects")
            .entry("loaded_skills")
            .or_insert(json!([]));
    }
    let (messages, has_more) = paged(&messages, params, "seq", 50, 100);
    Ok(json!({"messages":messages,"has_more":has_more}))
}

fn chat_thread(state: &MockState, params: &Value) -> RpcResult {
    let chat_id = params.get("chat_id").and_then(Value::as_str).unwrap_or("");
    let root_id = params
        .get("root_message_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let messages = state.messages.get(chat_id).cloned().unwrap_or_default();
    let root = messages
        .iter()
        .find(|message| message.get("id").and_then(Value::as_str) == Some(root_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "message not found", None))?;
    let replies = messages
        .iter()
        .filter(|message| message.get("reply_to").and_then(Value::as_str) == Some(root_id))
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({"root":root,"replies":replies}))
}

/// Acceptance-only mock action for a real Bot-authored private message.
/// Callers must use an authenticated mock RPC connection and a unique
/// `client_request_id`; the generic mock dispatcher stores the result for
/// retries. This method must never be added to protocol or production RPCs.
fn trigger_bot_dm(state: &mut MockState, params: &Value) -> RpcResult {
    let request_id = params
        .get("client_request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .ok_or_else(|| {
            rpc_error(
                "invalid_params",
                "client_request_id must contain 1 to 128 bytes",
                None,
            )
        })?;
    let _ = request_id;
    let bot_id = params
        .get("bot_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "bot_id is required", None))?;
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "chat_id is required", None))?;
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "text is required", None))?;

    let bot = state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "bot not found", None))?;
    let chat = state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "chat not found", None))?;
    let is_bot_dm = bot.get("dm_chat_id").and_then(Value::as_str) == Some(chat_id)
        && chat.get("bot_id").and_then(Value::as_str) == Some(bot_id)
        && matches!(
            chat.get("kind").and_then(Value::as_str),
            Some("main" | "direct" | "bot_dm")
        );
    if !is_bot_dm {
        return Err(rpc_error(
            "invalid_params",
            "chat_id must be the selected Bot's private chat",
            None,
        ));
    }

    let seq = next_chat_message_seq(state, chat_id);
    let messages = state.messages.entry(chat_id.to_owned()).or_default();
    let created_at = now();
    let message = json!({
        "id":id("msg"),"chat_id":chat_id,"seq":seq,
        "sender":{"kind":"bot","bot_id":bot_id},"sender_name":bot.get("name").cloned().unwrap_or(json!("Bot")),
        "created_at":created_at,"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
        "mentions":[],"blocks":[{"type":"text","markdown":text}],"fallback_text":text,
        "intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[]
    });
    messages.push(message.clone());
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
    {
        chat["last_message"] = json!({
            "message_id":message["id"],"sender":message["sender"],
            "text":text,"created_at":created_at
        });
        chat["last_seq"] = json!(seq);
        chat["updated_at"] = json!(created_at);
    }
    state.emit("message.created", json!({"message":message.clone()}));
    Ok(json!({"message":message}))
}

/// Acceptance-only mock action for a Bot-authored project-group message.
/// Generic mock RPC authentication and `client_request_id` caching apply.
/// This method must never be added to the protocol or production adapter.
fn trigger_bot_group_message(state: &mut MockState, params: &Value) -> RpcResult {
    let request_id = params
        .get("client_request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .ok_or_else(|| {
            rpc_error(
                "invalid_params",
                "client_request_id must contain 1 to 128 bytes",
                None,
            )
        })?;
    let _ = request_id;
    let bot_id = params
        .get("bot_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "bot_id is required", None))?;
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "chat_id is required", None))?;
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "text is required", None))?;

    let bot = state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "bot not found", None))?;
    let chat = state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "chat not found", None))?;
    let project_id = chat
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|_| chat.get("kind").and_then(Value::as_str) == Some("project"))
        .ok_or_else(|| rpc_error("invalid_params", "chat_id must be a project group", None))?;
    let project = state
        .projects
        .iter()
        .find(|project| {
            project.get("id").and_then(Value::as_str) == Some(project_id)
                && project.get("chat_id").and_then(Value::as_str) == Some(chat_id)
        })
        .ok_or_else(|| rpc_error("not_found", "project chat not found", None))?;
    let chat_member = chat
        .get("member_bot_ids")
        .and_then(Value::as_array)
        .is_some_and(|members| members.iter().any(|member| member.as_str() == Some(bot_id)));
    let project_member = project
        .get("members")
        .and_then(Value::as_array)
        .is_some_and(|members| {
            members
                .iter()
                .any(|member| member.get("bot_id").and_then(Value::as_str) == Some(bot_id))
        });
    if !chat_member || !project_member {
        return Err(rpc_error(
            "invalid_params",
            "Bot must be a member of the project group",
            None,
        ));
    }

    let seq = next_chat_message_seq(state, chat_id);
    let messages = state.messages.entry(chat_id.to_owned()).or_default();
    let created_at = now();
    let message = json!({
        "id":id("msg"),"chat_id":chat_id,"seq":seq,
        "sender":{"kind":"bot","bot_id":bot_id},"sender_name":bot.get("name").cloned().unwrap_or(json!("Bot")),
        "created_at":created_at,"edited_at":null,"deleted":false,"reply_to":null,"thread_count":0,
        "mentions":[],"blocks":[{"type":"text","markdown":text}],"fallback_text":text,
        "intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[]
    });
    messages.push(message.clone());
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
    {
        chat["last_message"] = json!({
            "message_id":message["id"],"sender":message["sender"],
            "text":text,"created_at":created_at
        });
        chat["last_seq"] = json!(seq);
        chat["updated_at"] = json!(created_at);
    }
    state.emit("message.created", json!({"message":message.clone()}));
    Ok(json!({"message":message}))
}

fn clear_mock_main_chat(state: &mut MockState, params: &Value) -> RpcResult {
    let request_id = params
        .get("client_request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .ok_or_else(|| {
            rpc_error(
                "invalid_params",
                "client_request_id must contain 1 to 128 bytes",
                None,
            )
        })?;
    let _ = request_id;
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "chat_id is required", None))?;
    if chat_id != "chat_main" {
        return Err(rpc_error(
            "invalid_params",
            "only the current Host main chat can be cleared",
            None,
        ));
    }
    let exists = state.chats.iter().any(|chat| {
        chat.get("id").and_then(Value::as_str) == Some("chat_main")
            && chat.get("kind").and_then(Value::as_str) == Some("main")
            && chat.get("bot_id").and_then(Value::as_str) == Some("bot_main")
    });
    if !exists {
        return Err(rpc_error("not_found", "main chat not found", None));
    }

    let messages = state.messages.remove("chat_main").unwrap_or_default();
    let had_state = !mock_main_chat_is_cleared(state)
        || !messages.is_empty()
        || state.chats.iter().any(|chat| {
            chat.get("id").and_then(Value::as_str) == Some("chat_main")
                && (chat
                    .get("last_message")
                    .is_some_and(|message| !message.is_null())
                    || chat.get("unread").and_then(Value::as_u64).unwrap_or(0) != 0)
        });
    extra_set(state, "mock_main_chat_cleared", vec![json!(true)]);
    for message in messages {
        if let Some(message_id) = message.get("id").and_then(Value::as_str) {
            state.emit(
                "message.deleted",
                json!({"chat_id":"chat_main","message_id":message_id}),
            );
        }
    }
    let mut updated_chat = None;
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some("chat_main"))
    {
        chat["last_message"] = Value::Null;
        chat["last_read_seq"] = json!(0);
        chat["unread"] = json!(0);
        if had_state {
            chat["updated_at"] = json!(now());
        }
        updated_chat = Some(chat.clone());
    }
    let updated_chat =
        updated_chat.ok_or_else(|| rpc_error("not_found", "main chat not found", None))?;
    if had_state {
        state.emit("chat.updated", json!({"chat":updated_chat.clone()}));
    }
    Ok(json!({"chat":updated_chat,"cleared":true}))
}

fn send_chat(state: &mut MockState, params: &Value, home: &std::path::Path) -> RpcResult {
    crate::validate_client_locale(params)?;
    let chat_id = params
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or("chat_main")
        .to_string();
    if !state
        .chats
        .iter()
        .any(|chat| chat.get("id").and_then(Value::as_str) == Some(&chat_id))
    {
        return Err(rpc_error("not_found", "chat not found", None));
    }
    let seq = next_chat_message_seq(state, &chat_id);
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let attachments = params
        .get("attachments")
        .map(|value| {
            value.as_array().ok_or_else(|| {
                rpc_error("invalid_params", "attachments must be an array", None)
            })
        })
        .transpose()?
        .into_iter()
        .flatten()
        .map(|attachment| {
            let upload_id = attachment.as_str().ok_or_else(|| {
                rpc_error("invalid_params", "attachment ids must be strings", None)
            })?;
            if upload_id.is_empty()
                || !upload_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(rpc_error("invalid_params", "invalid upload_id", None));
            }
            let metadata_path = home
                .join("data/uploads")
                .join(format!("{upload_id}.json"));
            let metadata = std::fs::read(&metadata_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .ok_or_else(|| {
                    rpc_error(
                        "not_found",
                        &format!("upload {upload_id} metadata not found"),
                        None,
                    )
                })?;
            let file = metadata.get("file").unwrap_or(&metadata);
            Ok(json!({
                "type":"file",
                "file":{
                    "root":"upload",
                    "root_id":upload_id,
                    "path":"",
                    "name":file.get("name").and_then(Value::as_str).unwrap_or(upload_id),
                    "size":file.get("size").and_then(Value::as_u64).unwrap_or(0),
                    "mime":file.get("mime").and_then(Value::as_str).unwrap_or("application/octet-stream")
                }
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut mentions = params
        .get("mentions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let quote_id = params
        .get("quote")
        .or_else(|| params.get("reply_to"))
        .and_then(Value::as_str);
    let quote = if let Some(quote_id) = quote_id {
        let quote_mode = params
            .get("quote_mode")
            .and_then(Value::as_str)
            .unwrap_or("reply");
        if !matches!(quote_mode, "reply" | "reference") {
            return Err(rpc_error(
                "invalid_argument",
                "quote_mode must be reply or reference",
                None,
            ));
        }
        let source = state
            .messages
            .get(&chat_id)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message.get("id").and_then(Value::as_str) == Some(quote_id))
            })
            .ok_or_else(|| rpc_error("not_found", "quoted message not found in this chat", None))?;
        let sender = source
            .get("sender")
            .cloned()
            .unwrap_or_else(|| json!({"kind":"system"}));
        if quote_mode == "reply" && sender.get("kind").and_then(Value::as_str) == Some("bot") {
            if let Some(bot_id) = sender.get("bot_id").and_then(Value::as_str) {
                if !bot_id.is_empty()
                    && !mentions.iter().any(|mention| {
                        mention.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                    })
                {
                    mentions.push(json!({"kind":"bot","bot_id":bot_id,"instruction":null}));
                }
            }
        }
        let excerpt = source
            .get("fallback_text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();
        json!({"message_id":quote_id,"sender":sender,"excerpt":excerpt})
    } else {
        Value::Null
    };
    let mentions = Value::Array(mentions);
    let created_at = now();
    // In a project chat, an unaddressed user message waits for each Bot task
    // already working in that same conversation. A later message.steer RPC
    // can promote one of these individual deliveries.
    let is_unaddressed_project_message = mentions.as_array().is_some_and(Vec::is_empty)
        && state.chats.iter().any(|chat| {
            chat.get("id").and_then(Value::as_str) == Some(&chat_id)
                && chat.get("kind").and_then(Value::as_str) == Some("project")
        });
    let delivery = if is_unaddressed_project_message {
        state
            .assignments
            .iter()
            .filter(|assignment| {
                assignment.get("origin_chat_id").and_then(Value::as_str) == Some(&chat_id)
                    && assignment.get("status").and_then(Value::as_str) == Some("working")
            })
            .filter_map(|assignment| {
                Some(json!({
                    "bot_id":assignment.get("bot_id")?,
                    "assignment_id":assignment.get("id")?,
                    "state":"queued",
                    "at":created_at
                }))
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let fallback_text = if text.trim().is_empty() && !attachments.is_empty() {
        attachments
            .iter()
            .filter_map(|block| block.pointer("/file/name").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        text.clone()
    };
    let mut blocks = Vec::with_capacity(usize::from(!text.is_empty()) + attachments.len());
    if !text.is_empty() {
        blocks.push(json!({"type":"text","markdown":text}));
    }
    blocks.extend(attachments);
    let message = json!({"id":id("msg"),"chat_id":chat_id,"seq":seq,"sender":{"kind":"user"},"created_at":created_at,"edited_at":null,"deleted":false,"reply_to":null,"quote":quote,"thread_count":0,"mentions":mentions,"blocks":blocks,"fallback_text":fallback_text,"intent":null,"assignment_id":null,"streaming":false,"delivery":delivery,"loaded_skills":[],"reactions":[]});
    if let Some(locale) = params.get("client_locale").and_then(Value::as_str) {
        state.client_locales.insert(
            message["id"].as_str().unwrap().to_owned(),
            locale.to_owned(),
        );
    }
    state
        .messages
        .entry(chat_id.clone())
        .or_default()
        .push(message.clone());
    state.emit("message.created", json!({"message":message.clone()}));
    let loaded_skills = mock_explicit_skill_loads(state, &chat_id, &text, &mentions);
    let mut message = message;
    if !loaded_skills.is_empty() {
        message["loaded_skills"] = json!(loaded_skills);
        if let Some(messages) = state.messages.get_mut(&chat_id) {
            if let Some(existing) = messages
                .iter_mut()
                .find(|item| item.get("id") == message.get("id"))
            {
                *existing = message.clone();
            }
        }
        state.emit("message.updated", json!({"message":message.clone()}));
    }
    let updated_chat = if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(&chat_id))
    {
        chat["last_message"] = json!({"message_id":message["id"],"sender":message["sender"],"text":message["fallback_text"],"created_at":message["created_at"]});
        chat["last_seq"] = json!(seq);
        chat["updated_at"] = message["created_at"].clone();
        Some(chat.clone())
    } else {
        None
    };
    if let Some(chat) = updated_chat {
        state.emit("chat.updated", json!({"chat":chat}));
    }
    for assignment in refresh_pending_message_counts(state) {
        state.emit("assignment.updated", json!({"assignment":assignment}));
    }
    Ok(json!({"message":message}))
}

/// The mock treats chat.send as one successful model-context admission for
/// each explicitly addressed Bot, allowing clients to verify the durable
/// loaded-skill contract without a model provider.
fn mock_explicit_skill_loads(
    state: &MockState,
    chat_id: &str,
    text: &str,
    mentions: &Value,
) -> Vec<Value> {
    let Some((name, _)) = macbot_skills::forced_invocation(text) else {
        return Vec::new();
    };
    let Some(skill) = extra_get(state, "skills")
        .into_iter()
        .find(|skill| skill.get("name").and_then(Value::as_str) == Some(name))
    else {
        return Vec::new();
    };
    if skill.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let mut bot_ids = mentions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mention| mention.get("kind").and_then(Value::as_str) == Some("bot"))
        .filter_map(|mention| mention.get("bot_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if bot_ids.is_empty() {
        let private_bot_id = state
            .chats
            .iter()
            .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
            .filter(|chat| chat.get("kind").and_then(Value::as_str) != Some("project"))
            .and_then(|chat| chat.get("bot_id").and_then(Value::as_str))
            .or_else(|| {
                state
                    .bots
                    .iter()
                    .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
                    .and_then(|bot| bot.get("id").and_then(Value::as_str))
            });
        if let Some(bot_id) = private_bot_id {
            bot_ids.push(bot_id.to_owned());
        }
    }
    bot_ids.sort();
    bot_ids.dedup();
    let disabled = skill
        .get("disabled_bot_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let loaded_at = now();
    bot_ids
        .into_iter()
        .filter(|bot_id| !disabled.iter().any(|id| id.as_str() == Some(bot_id)))
        .map(|bot_id| {
            json!({
                "name":name,
                "bot_id":bot_id,
                "assignment_id":null,
                "loaded_at":loaded_at
            })
        })
        .collect()
}

fn message_delete(state: &mut MockState, params: &Value) -> RpcResult {
    let message_id = params
        .get("message_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "message_id is required", None))?;
    let position = state
        .messages
        .iter()
        .find_map(|(chat_id, messages)| {
            messages
                .iter()
                .position(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
                .map(|index| (chat_id.clone(), index))
        })
        .ok_or_else(|| rpc_error("not_found", "message not found", None))?;
    let (chat_id, index) = position;
    let message = &mut state
        .messages
        .get_mut(&chat_id)
        .expect("message chat exists")[index];
    if message.pointer("/sender/kind").and_then(Value::as_str) != Some("user") {
        return Err(rpc_error(
            "forbidden",
            "only user messages can be deleted",
            None,
        ));
    }
    if message.get("deleted").and_then(Value::as_bool) == Some(true) {
        return Ok(json!({}));
    }
    message["deleted"] = json!(true);
    message["blocks"] = json!([]);
    message["fallback_text"] = json!("");
    message["mentions"] = json!([]);
    message["quote"] = Value::Null;
    message["reactions"] = json!([]);
    message["edited_at"] = json!(now());
    state.emit(
        "message.deleted",
        json!({"chat_id":chat_id,"message_id":message_id}),
    );
    for assignment in refresh_pending_message_counts(state) {
        state.emit("assignment.updated", json!({"assignment":assignment}));
    }
    Ok(json!({}))
}

fn message_edit(state: &mut MockState, params: &Value) -> RpcResult {
    let message_id = params
        .get("message_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "message_id is required", None))?;
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            rpc_error(
                "invalid_params",
                "text is required and must not be blank",
                None,
            )
        })?;
    let position = state
        .messages
        .iter()
        .find_map(|(chat_id, messages)| {
            messages
                .iter()
                .position(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
                .map(|index| (chat_id.clone(), index))
        })
        .ok_or_else(|| rpc_error("not_found", "message not found", None))?;
    let (chat_id, index) = position;
    let message = &mut state
        .messages
        .get_mut(&chat_id)
        .expect("message chat exists")[index];
    if message.pointer("/sender/kind").and_then(Value::as_str) != Some("user") {
        return Err(rpc_error(
            "forbidden",
            "only user messages can be edited",
            None,
        ));
    }
    if message.get("deleted").and_then(Value::as_bool) == Some(true)
        || message.get("streaming").and_then(Value::as_bool) == Some(true)
    {
        return Err(rpc_error(
            "conflict",
            "deleted or streaming messages cannot be edited",
            None,
        ));
    }
    let blocks = message
        .get("blocks")
        .and_then(Value::as_array)
        .ok_or_else(|| rpc_error("conflict", "only plain text messages can be edited", None))?;
    let no_mentions = message
        .get("mentions")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    if blocks.len() != 1
        || blocks[0].get("type").and_then(Value::as_str) != Some("text")
        || !no_mentions
    {
        return Err(rpc_error(
            "conflict",
            "only plain text messages can be edited",
            None,
        ));
    }
    let edited_at = now();
    message["blocks"][0]["markdown"] = json!(text);
    message["fallback_text"] = json!(text);
    message["edited_at"] = json!(edited_at);
    let result = message.clone();
    state.emit("message.updated", json!({"message":result.clone()}));
    Ok(json!({"message":result}))
}

fn message_steer(state: &mut MockState, params: &Value) -> RpcResult {
    let message_id = params
        .get("message_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let bot_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
    let assignment_id = params
        .get("assignment_id")
        .and_then(Value::as_str)
        .unwrap_or("");

    let message_position = state
        .messages
        .iter()
        .find_map(|(chat_id, messages)| {
            messages
                .iter()
                .position(|message| message.get("id").and_then(Value::as_str) == Some(message_id))
                .map(|position| (chat_id.clone(), position))
        })
        .ok_or_else(|| rpc_error("not_found", "message not found", None))?;
    let (chat_id, message_index) = message_position;
    let message = state.messages[&chat_id][message_index].clone();

    if message.pointer("/sender/kind").and_then(Value::as_str) != Some("user") {
        return Err(rpc_error(
            "forbidden",
            "only a user message can be changed into a steer",
            None,
        ));
    }
    if message.get("deleted").and_then(Value::as_bool) == Some(true) {
        return Err(rpc_error(
            "conflict",
            "deleted messages cannot be changed into a steer",
            None,
        ));
    }
    if message
        .get("mentions")
        .and_then(Value::as_array)
        .is_some_and(|mentions| !mentions.is_empty())
    {
        return Err(rpc_error(
            "conflict",
            "addressed messages cannot be promoted from the waiting queue",
            None,
        ));
    }
    let assignment_index = state
        .assignments
        .iter()
        .position(|assignment| assignment.get("id").and_then(Value::as_str) == Some(assignment_id))
        .ok_or_else(|| rpc_error("not_found", "assignment not found", None))?;
    let assignment = &state.assignments[assignment_index];
    if assignment.get("bot_id").and_then(Value::as_str) != Some(bot_id)
        || assignment.get("origin_chat_id").and_then(Value::as_str) != Some(&chat_id)
    {
        return Err(rpc_error(
            "forbidden",
            "message is not queued for this Bot task",
            None,
        ));
    }

    let assignment = &mut state.assignments[assignment_index];
    let already_promoted = assignment
        .get("steers")
        .and_then(Value::as_array)
        .is_some_and(|steers| {
            steers
                .iter()
                .any(|steer| steer.get("message_id").and_then(Value::as_str) == Some(message_id))
        });
    if already_promoted {
        let assignment = state.assignments[assignment_index].clone();
        let message = state.messages[&chat_id][message_index].clone();
        return Ok(json!({"message":message,"assignment":assignment}));
    }

    if assignment.get("status").and_then(Value::as_str) != Some("working") {
        return Err(rpc_error("conflict", "Bot task is no longer working", None));
    }

    // The user's chat read cursor is independent from a Bot task's queued
    // delivery. The assignment-scoped queued state below is authoritative.
    let delivery_is_queued = message
        .get("delivery")
        .and_then(Value::as_array)
        .is_some_and(|deliveries| {
            deliveries.iter().any(|delivery| {
                delivery.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                    && delivery.get("assignment_id").and_then(Value::as_str) == Some(assignment_id)
                    && delivery.get("state").and_then(Value::as_str) == Some("queued")
            })
        });
    if !delivery_is_queued {
        return Err(rpc_error(
            "conflict",
            "message is not waiting for this Bot",
            None,
        ));
    }

    let steers = assignment
        .get_mut("steers")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| rpc_error("internal", "assignment has no steer list", None))?;
    if !steers
        .iter()
        .any(|steer| steer.get("message_id").and_then(Value::as_str) == Some(message_id))
    {
        steers.push(json!({
            "message_id":message_id,
            "text":message.get("fallback_text").cloned().unwrap_or(json!("")),
            "at":now(),
            "applied_at":null
        }));
    }
    let message = state.messages[&chat_id][message_index].clone();
    let changed_assignments = refresh_pending_message_counts(state);
    let assignment = state.assignments[assignment_index].clone();
    let target_emitted = changed_assignments
        .iter()
        .any(|changed| changed.get("id").and_then(Value::as_str) == Some(assignment_id));
    for changed in changed_assignments {
        state.emit("assignment.updated", json!({"assignment":changed}));
    }
    if !target_emitted {
        state.emit(
            "assignment.updated",
            json!({"assignment":assignment.clone()}),
        );
    }
    Ok(json!({"message":message,"assignment":assignment}))
}

/// Keep the wire-visible pending count derived from canonical message
/// deliveries. A message delivered to multiple working assignments counts
/// once for each queued assignment, and changing one delivery leaves the
/// other assignments untouched.
fn refresh_pending_message_counts(state: &mut MockState) -> Vec<Value> {
    let counts = state
        .assignments
        .iter()
        .map(|assignment| {
            let assignment_id = assignment.get("id").and_then(Value::as_str).unwrap_or("");
            let bot_id = assignment
                .get("bot_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let chat_id = assignment
                .get("origin_chat_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let count = state
                .messages
                .values()
                .flatten()
                .filter(|message| {
                    message.pointer("/sender/kind").and_then(Value::as_str) == Some("user")
                        && message.get("deleted").and_then(Value::as_bool) != Some(true)
                        && message
                            .get("mentions")
                            .and_then(Value::as_array)
                            .is_none_or(Vec::is_empty)
                        && message.get("chat_id").and_then(Value::as_str) == Some(chat_id)
                        && message
                            .get("delivery")
                            .and_then(Value::as_array)
                            .is_some_and(|deliveries| {
                                deliveries.iter().any(|delivery| {
                                    delivery.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                                        && delivery.get("assignment_id").and_then(Value::as_str)
                                            == Some(assignment_id)
                                        && delivery.get("state").and_then(Value::as_str)
                                            == Some("queued")
                                })
                            })
                })
                .count();
            (assignment_id.to_owned(), count)
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut changed = Vec::new();
    for assignment in &mut state.assignments {
        let assignment_id = assignment.get("id").and_then(Value::as_str).unwrap_or("");
        let count = counts.get(assignment_id).copied().unwrap_or(0);
        if assignment.get("pending_messages").and_then(Value::as_u64) != Some(count as u64) {
            assignment["pending_messages"] = json!(count);
            changed.push(assignment.clone());
        }
    }
    changed
}

fn mark_read(state: &mut MockState, params: &Value) -> RpcResult {
    let chat_id = params.get("chat_id").and_then(Value::as_str).unwrap_or("");
    let seq = params.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let chat = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id))
        .ok_or_else(|| rpc_error("not_found", "chat not found", None))?;
    chat["last_read_seq"] = json!(seq);
    state.emit(
        "read.updated",
        json!({"chat_id":chat_id,"last_read_seq":seq}),
    );
    Ok(json!({}))
}

fn react(state: &mut MockState, params: &Value) -> RpcResult {
    let message_id = params
        .get("message_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    for messages in state.messages.values_mut() {
        if let Some(message) = messages
            .iter_mut()
            .find(|m| m.get("id").and_then(Value::as_str) == Some(message_id))
        {
            let emoji = params.get("emoji").cloned().unwrap_or(json!(""));
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
            let reactions = message["reactions"].as_array_mut().unwrap();
            if on {
                reactions.retain(|r| r.get("emoji") != Some(&emoji));
                reactions.push(json!({"emoji":emoji,"by":[{"kind":"user"}]}));
            } else {
                reactions.retain(|r| r.get("emoji") != Some(&emoji));
            }
            let result = message.clone();
            state.emit("message.updated", json!({"message":result.clone()}));
            return Ok(json!({"message":result}));
        }
    }
    Err(rpc_error("not_found", "message not found", None))
}

fn set_chat_flag(state: &mut MockState, params: &Value, key: &str) -> RpcResult {
    let chat_id = params.get("chat_id").and_then(Value::as_str).unwrap_or("");
    let chat = state
        .chats
        .iter_mut()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(chat_id))
        .ok_or_else(|| rpc_error("not_found", "chat not found", None))?;
    chat[key] = json!(params.get(key).and_then(Value::as_bool).unwrap_or(false));
    let result = chat.clone();
    state.emit("chat.updated", json!({"chat":result.clone()}));
    Ok(json!({"chat":result}))
}

fn register_device(state: &mut MockState, params: &Value) -> RpcResult {
    let device_id = params
        .get("device_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if device_id.is_empty() {
        return Err(rpc_error("invalid_params", "device_id is required", None));
    }
    let device = json!({"id":device_id,"platform":params.get("platform").cloned().unwrap_or(json!("macos")),"app_version":params.get("app_version").cloned().unwrap_or(json!("")),"device_name":params.get("device_name").cloned().unwrap_or(json!("")),"push_token":params.get("push_token").cloned().unwrap_or(Value::Null),"last_seen_at":now()});
    state.devices.insert(device_id, device.clone());
    Ok(json!({"device":device}))
}

fn bot_list(state: &MockState, params: &Value) -> RpcResult {
    let include = params
        .get("include_hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(
        json!({"bots":state.bots.iter().filter(|b| include || b.get("hidden").and_then(Value::as_bool)!=Some(true)).cloned().collect::<Vec<_>>() }),
    )
}

fn create_bot(state: &mut MockState, params: &Value) -> RpcResult {
    let now = now();
    let bot_id = id("bot");
    let chat_id = id("chat");
    let name = params.get("name").and_then(Value::as_str).unwrap_or("Bot");
    let tools = params
        .get("tools")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| state.settings["tools"]["defaults"].clone());
    let bot = json!({"id":bot_id,"name":name,"label":params.get("label").and_then(Value::as_str).unwrap_or(""),"description":params.get("description").and_then(Value::as_str).unwrap_or(""),"avatar":params.get("avatar").cloned().unwrap_or(json!({"kind":"bean","color":0})),"is_main":false,"model":params.get("model").cloned().unwrap_or(Value::Null),"max_parallel":params.get("max_parallel").and_then(Value::as_u64).unwrap_or(2),"tools":tools,"browser_mode":params.get("browser_mode").cloned().unwrap_or(json!("headless")),"pinned":false,"hidden":false,"notifications":true,"dm_chat_id":chat_id,"created_at":now,"updated_at":now,"status":{"summary":"idle","active":0,"queued":0,"waiting":0}});
    let chat = json!({"id":chat_id,"kind":"direct","title":name,"bot_id":bot_id,"project_id":null,"member_bot_ids":[],"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":false,"muted":false,"updated_at":now});
    state.bots.push(bot.clone());
    state.chats.push(chat.clone());
    state.emit("bot.created", json!({"bot":bot.clone()}));
    state.emit("chat.created", json!({"chat":chat.clone()}));
    Ok(json!({"bot":bot,"dm_chat":chat}))
}

fn update_bot(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
    let bot = state
        .bots
        .iter_mut()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "bot not found", None))?;
    merge_patch(bot, params.get("patch"));
    bot["updated_at"] = json!(now());
    let result = bot.clone();
    state.emit("bot.updated", json!({"bot":result.clone()}));
    Ok(json!({"bot":result}))
}

fn duplicate_bot(state: &mut MockState, params: &Value) -> RpcResult {
    let source_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
    let source = state
        .bots
        .iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(source_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "bot not found", None))?;
    let input = json!({"name":params.get("name").cloned().unwrap_or(source["name"].clone()),"label":source["label"],"description":source["description"],"avatar":source["avatar"],"model":source["model"],"max_parallel":source["max_parallel"],"tools":source["tools"],"browser_mode":source["browser_mode"]});
    create_bot(state, &input)
}

fn delete_bot(state: &mut MockState, params: &Value) -> RpcResult {
    let bot_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
    if bot_id == "bot_main" {
        return Err(rpc_error(
            "forbidden",
            "the main bot cannot be deleted",
            None,
        ));
    }
    if !state
        .bots
        .iter()
        .any(|x| x.get("id").and_then(Value::as_str) == Some(bot_id))
    {
        return Err(rpc_error("not_found", "bot not found", None));
    }
    state
        .bots
        .retain(|x| x.get("id").and_then(Value::as_str) != Some(bot_id));
    state.emit("bot.deleted", json!({"bot_id":bot_id}));
    Ok(json!({}))
}

fn templates() -> Vec<Value> {
    vec![
        json!({"id":"product-engineering-test","name":"产品 + 编码 + 测试","description":"标准交付团队","bots":[{"name":"产品","label":"产品负责人","description":"定义目标"},{"name":"编码","label":"工程师","description":"实现方案"},{"name":"测试","label":"测试工程师","description":"验证结果"}]}),
    ]
}
fn create_from_template(state: &mut MockState, params: &Value) -> RpcResult {
    let template_id = params
        .get("template_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let template = templates()
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(template_id))
        .ok_or_else(|| rpc_error("not_found", "template not found", None))?;
    let mut bots = Vec::new();
    let mut chats = Vec::new();
    for row in template["bots"].as_array().cloned().unwrap_or_default() {
        let result = create_bot(
            state,
            &json!({"name":row["name"],"label":row["label"],"description":row["description"]}),
        )?;
        bots.push(result["bot"].clone());
        chats.push(result["dm_chat"].clone());
    }
    Ok(json!({"bots":bots,"dm_chats":chats}))
}

fn project_list(state: &MockState, params: &Value) -> RpcResult {
    let statuses = params.get("status").and_then(Value::as_array);
    let projects = state
        .projects
        .iter()
        .filter(|p| {
            statuses.is_none_or(|values| {
                values
                    .iter()
                    .any(|v| v.as_str() == p.get("status").and_then(Value::as_str))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({"projects":projects}))
}
fn project_get(state: &MockState, params: &Value) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let project = state
        .projects
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
    let highlights = project.get("highlights").cloned().unwrap_or(json!([]));
    let announcement = extra_get(state, "announcements")
        .into_iter()
        .find(|item| item.get("project_id").and_then(Value::as_str) == Some(project_id))
        .unwrap_or_else(|| {
            let updated_at = now();
            json!({
                "project_id":project_id,
                "text":"",
                "version":0,
                "text_updated_by":{"kind":"user"},
                "text_updated_at":updated_at,
                "members":[],
                "artifacts":[],
                "highlights":highlights,
                "updated_at":updated_at
            })
        });
    Ok(json!({"project":project,"announcement":announcement}))
}
fn create_project(state: &mut MockState, params: &Value) -> RpcResult {
    let requested_members = params
        .get("member_bot_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| rpc_error("invalid_params", "member_bot_ids is required", None))?;
    if requested_members.is_empty() || requested_members.len() > 6 {
        return Err(rpc_error(
            "invalid_params",
            "invalid parameters: a project needs 1-6 member bots",
            None,
        ));
    }
    let main_bot_id = mock_main_bot_id(state);
    let mut worker_ids = Vec::with_capacity(requested_members.len());
    let mut seen = std::collections::HashSet::new();
    for value in requested_members {
        let bot_id = value
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| rpc_error("invalid_params", "bot_id is required", None))?;
        if is_mock_main_id(state, bot_id) {
            return Err(rpc_error(
                "invalid_params",
                "invalid parameters: main Bot is added to every project automatically",
                None,
            ));
        }
        let bot = state
            .bots
            .iter()
            .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
            .ok_or_else(|| rpc_error("not_found", "bot not found", None))?;
        let canonical_id = bot["id"].as_str().unwrap_or(bot_id);
        if !seen.insert(canonical_id.to_owned()) {
            return Err(rpc_error(
                "invalid_params",
                &format!(
                    "invalid parameters: Bot {canonical_id} appears more than once in project members"
                ),
                None,
            ));
        }
        worker_ids.push(canonical_id.to_owned());
    }
    let now = now();
    let project_id = id("prj");
    let chat_id = id("chat");
    let name = params.get("name").cloned().unwrap_or(json!("新项目"));
    let slug = name
        .as_str()
        .unwrap_or("project")
        .to_lowercase()
        .replace(' ', "-");
    let mut member_ids = vec![main_bot_id.clone()];
    member_ids.extend(worker_ids);
    let role_notes = member_ids
        .iter()
        .map(|bot_id| {
            (
                bot_id.clone(),
                if bot_id == &main_bot_id {
                    "负责人".to_owned()
                } else {
                    bot_role_note(state, bot_id)
                },
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let project_members = member_ids
        .iter()
        .map(|bot_id| {
            json!({
                "bot_id":bot_id,
                "role_note":role_notes.get(bot_id).cloned().unwrap_or_default(),
                "joined_at":now
            })
        })
        .collect::<Vec<_>>();
    let chat_members = member_ids.iter().map(|id| json!(id)).collect::<Vec<_>>();
    let project = json!({"id":project_id,"chat_id":chat_id,"name":name,"slug":slug,"goal":params.get("goal").cloned().unwrap_or(json!("")),"flow":params.get("flow").cloned().unwrap_or(json!([])),"deadline":params.get("deadline").cloned().unwrap_or(Value::Null),"home_path":format!("~/MacBot/projects/{slug}/"),"status":"active","lead_bot_id":main_bot_id,"members":project_members,"created_by":{"kind":"user"},"created_at":now,"updated_at":now,"done_at":null,"highlights":[]});
    let chat = json!({"id":chat_id,"kind":"project","title":project["name"],"bot_id":null,"project_id":project_id,"member_bot_ids":chat_members,"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":false,"muted":false,"updated_at":now});
    state.projects.push(project.clone());
    state.chats.push(chat.clone());
    state.emit("project.created", json!({"project":project.clone()}));
    state.emit("chat.created", json!({"chat":chat.clone()}));
    upsert_mock_message(
        state,
        "chat_main",
        &format!("msg_project_card_{project_id}"),
        json!({"kind":"bot","bot_id":"bot_main"}),
        json!([{"type":"project_card","project_id":project_id,"context":"created","note":null}]),
        format!("项目「{}」已创建", name.as_str().unwrap_or(&project_id)),
        None,
    );
    Ok(json!({"project":project,"chat":chat}))
}

fn assignment_create(state: &mut MockState, params: &Value, delegation: bool) -> RpcResult {
    let bot_id = params
        .get("bot_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "bot_id is required", None))?;
    let title = params
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(if delegation {
            "转交任务"
        } else {
            "新任务"
        });
    let instruction = params
        .get("instruction")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "instruction is required", None))?;
    if !state
        .bots
        .iter()
        .any(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
    {
        return Err(rpc_error("not_found", "Bot not found", None));
    }
    let project_id = params.get("project_id").and_then(Value::as_str);
    let project = project_id
        .map(|project_id| {
            state
                .projects
                .iter()
                .find(|project| project.get("id").and_then(Value::as_str) == Some(project_id))
                .ok_or_else(|| rpc_error("not_found", "project not found", None))
        })
        .transpose()?;
    let origin_chat_id = project
        .and_then(|project| project.get("chat_id").and_then(Value::as_str))
        .unwrap_or("chat_main")
        .to_owned();
    let created_at = now();
    let model = state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
        .and_then(|bot| bot.get("model"))
        .filter(|model| !model.is_null())
        .cloned()
        .unwrap_or(json!("prv_mock/mock-model"));
    let from = params
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("bot_main");
    let sender = if from == "user" {
        json!({"kind":"user"})
    } else {
        json!({"kind":"bot","bot_id":if from == "main" { "bot_main" } else { from }})
    };
    let assignment = json!({
        "id":id("asg"),"project_id":project_id,"origin_chat_id":origin_chat_id,"bot_id":bot_id,
        "title":title,"instruction":instruction,"from":sender,
        "trigger_message_id":params.get("trigger_message_id").cloned().unwrap_or(Value::Null),
        "parent_assignment_id":params.get("parent_assignment_id").cloned().unwrap_or(Value::Null),
        "status":"working","queue_reason":null,"wait":null,"created_at":created_at,
        "started_at":created_at,"finished_at":null,"recovered_at":null,"usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"requests":0,"cost":null},
        "subagents_active":0,"pending_messages":0,"steers":[],"result_message_id":null,"model":model,
        "priority":params.get("priority").cloned().unwrap_or(json!(1)),"root_message_id":params.get("root_message_id").cloned().unwrap_or(Value::Null),
        "loop_hops":params.get("loop_hops").cloned().unwrap_or(json!(0))
    });
    state.assignments.push(assignment.clone());
    state.emit(
        "assignment.created",
        json!({"assignment":assignment.clone()}),
    );
    let assignment_id = assignment["id"].as_str().unwrap_or_default();
    if !origin_chat_id.starts_with("dm_") {
        let message_id = if delegation {
            format!("msg_delegation_{assignment_id}")
        } else {
            format!("msg_task_card_{assignment_id}")
        };
        upsert_mock_message(
            state,
            &origin_chat_id,
            &message_id,
            json!({"kind":"system"}),
            json!([if delegation && project_id.is_none() {
                json!({"type":"delegation","bot_id":bot_id,"assignment_id":assignment_id})
            } else {
                json!({"type":"task_card","assignment_id":assignment_id})
            }]),
            format!("任务：{title}"),
            Some(assignment_id),
        );
    }
    Ok(assignment)
}
fn project_update(state: &mut MockState, params: &Value) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let project = state
        .projects
        .iter_mut()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
        .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
    merge_patch(project, params.get("patch"));
    project["updated_at"] = json!(now());
    let result = project.clone();
    state.emit("project.updated", json!({"project":result.clone()}));
    Ok(json!({"project":result}))
}
fn project_member(state: &mut MockState, params: &Value, add: bool) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let bot_id = params.get("bot_id").and_then(Value::as_str).unwrap_or("");
    let main_bot_id = mock_main_bot_id(state);
    let bot_is_main = is_mock_main_id(state, bot_id);
    if !add && bot_is_main {
        return Err(rpc_error(
            "forbidden",
            "forbidden: main Bot cannot be removed from a project",
            None,
        ));
    }
    let canonical_bot_id = if bot_is_main {
        main_bot_id.clone()
    } else {
        bot_id.to_owned()
    };
    if add
        && !is_mock_main_id(state, bot_id)
        && !state
            .bots
            .iter()
            .any(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
    {
        return Err(rpc_error("not_found", "bot not found", None));
    }
    if !add {
        let project = state
            .projects
            .iter()
            .find(|project| project.get("id").and_then(Value::as_str) == Some(project_id))
            .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
        let is_member = project
            .get("members")
            .and_then(Value::as_array)
            .is_some_and(|members| {
                members.iter().any(|member| {
                    member.get("bot_id").and_then(Value::as_str) == Some(canonical_bot_id.as_str())
                })
            });
        if !is_member {
            return Err(rpc_error(
                "conflict",
                "bot is not a project member",
                Some(json!({"project_id":project_id,"bot_id":bot_id})),
            ));
        }
    }
    let (result, chat_id) = {
        let project = state
            .projects
            .iter_mut()
            .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
            .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
        let members = project
            .get_mut("members")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| rpc_error("invalid_params", "project members must be an array", None))?;
        ensure_mock_main_member(members, &main_bot_id);
        if add {
            if !members.iter().any(|member| {
                member.get("bot_id").and_then(Value::as_str) == Some(canonical_bot_id.as_str())
            }) {
                let worker_count = members
                    .iter()
                    .filter(|member| !is_mock_main_id_value(member, &main_bot_id))
                    .count();
                if !bot_is_main && worker_count >= 6 {
                    return Err(rpc_error(
                        "invalid_params",
                        "invalid parameters: project has at most 6 workers plus main",
                        None,
                    ));
                }
                members.push(json!({"bot_id":canonical_bot_id,"role_note":params.get("role_note").and_then(Value::as_str).unwrap_or(""),"joined_at":now()}));
            }
        } else {
            members.retain(|member| {
                member.get("bot_id").and_then(Value::as_str) != Some(canonical_bot_id.as_str())
            });
        }
        project["updated_at"] = json!(now());
        (
            project.clone(),
            project
                .get("chat_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        )
    };
    let member_ids = result
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|member| member.get("bot_id").cloned())
        .collect::<Vec<_>>();
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id.as_str()))
    {
        chat["member_bot_ids"] = json!(member_ids);
        chat["updated_at"] = result["updated_at"].clone();
    }
    state.emit("project.updated", json!({"project":result.clone()}));
    let updated_chat = state
        .chats
        .iter()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some(chat_id.as_str()))
        .cloned();
    if let Some(chat) = updated_chat {
        state.emit("chat.updated", json!({"chat":chat.clone()}));
    }
    Ok(json!({"project":result}))
}

fn mock_main_bot_id(state: &MockState) -> String {
    state
        .bots
        .iter()
        .find(|bot| bot.get("is_main").and_then(Value::as_bool) == Some(true))
        .and_then(|bot| bot.get("id").and_then(Value::as_str))
        .unwrap_or("bot_main")
        .to_owned()
}

fn is_mock_main_id(state: &MockState, bot_id: &str) -> bool {
    bot_id == "main"
        || state.bots.iter().any(|bot| {
            bot.get("id").and_then(Value::as_str) == Some(bot_id)
                && bot.get("is_main").and_then(Value::as_bool) == Some(true)
        })
}

fn ensure_mock_main_member(members: &mut Vec<Value>, main_bot_id: &str) {
    let main = members.iter_mut().find(|member| {
        member
            .get("bot_id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == "main" || id == "bot_main" || id == main_bot_id)
    });
    if let Some(main) = main {
        main["bot_id"] = json!(main_bot_id);
    } else {
        members.insert(
            0,
            json!({"bot_id":main_bot_id,"role_note":"负责人","joined_at":now()}),
        );
    }
}

fn is_mock_main_id_value(member: &Value, main_bot_id: &str) -> bool {
    member
        .get("bot_id")
        .and_then(Value::as_str)
        .is_some_and(|id| id == "main" || id == "bot_main" || id == main_bot_id)
}
fn project_status(state: &mut MockState, params: &Value, status: &str) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let project = state
        .projects
        .iter_mut()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
        .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
    project["status"] = json!(status);
    project["updated_at"] = json!(now());
    let result = project.clone();
    state.emit("project.updated", json!({"project":result.clone()}));
    if status == "done" {
        set_review_card_mock(
            state,
            project_id,
            "confirmed",
            Value::Null,
            "项目已确认完成",
        )?;
    }
    Ok(json!({"project":result}))
}
fn project_request_review(state: &mut MockState, params: &Value) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "project_id is required", None))?;
    let summary = params
        .get("summary")
        .and_then(Value::as_str)
        .filter(|summary| !summary.trim().is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "summary is required", None))?;
    let project = state
        .projects
        .iter_mut()
        .find(|project| project.get("id").and_then(Value::as_str) == Some(project_id))
        .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
    if !matches!(
        project.get("status").and_then(Value::as_str),
        Some("active" | "review")
    ) {
        return Err(rpc_error(
            "conflict",
            "only active or review projects can be submitted for review",
            None,
        ));
    }
    project["status"] = json!("review");
    project["updated_at"] = json!(now());
    let result = project.clone();
    let artifacts = extra_get(state, "announcements")
        .into_iter()
        .find(|announcement| {
            announcement.get("project_id").and_then(Value::as_str) == Some(project_id)
        })
        .and_then(|announcement| {
            announcement
                .get("artifacts")
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|artifact| {
            json!({
                "artifact_id":artifact.get("id").cloned().unwrap_or(Value::Null),
                "title":artifact.get("title").cloned().unwrap_or_else(|| json!("")),
                "path_or_url":artifact.get("path_or_url").cloned().unwrap_or_else(|| json!("")),
                "bot_id":artifact.get("bot_id").cloned().unwrap_or_else(|| json!("bot_main")),
                "kind":artifact.get("kind").cloned().unwrap_or_else(|| json!("file"))
            })
        })
        .collect::<Vec<_>>();
    state.emit("project.updated", json!({"project":result.clone()}));
    set_review_card_mock(state, project_id, "pending", json!(artifacts), summary)?;
    Ok(json!({"project":result}))
}

fn set_review_card_mock(
    state: &mut MockState,
    project_id: &str,
    review_state: &str,
    artifacts: Value,
    summary: &str,
) -> Result<(), super::RpcError> {
    let message_id = format!("msg_project_review_{project_id}");
    let next_seq = next_chat_message_seq(state, "chat_main");
    let messages = state.messages.entry("chat_main".into()).or_default();
    let existing = messages
        .iter()
        .position(|message| message.get("id").and_then(Value::as_str) == Some(&message_id));
    let artifacts = if artifacts.is_null() {
        existing
            .and_then(|index| messages[index].pointer("/blocks/0/artifacts"))
            .filter(|value| value.is_array())
            .cloned()
            .unwrap_or_else(|| json!([]))
    } else {
        artifacts
    };
    let message = if let Some(index) = existing {
        let message = &mut messages[index];
        message["blocks"] = json!([{
            "type":"review_card",
            "project_id":project_id,
            "artifacts":artifacts,
            "state":review_state
        }]);
        message["fallback_text"] = json!(summary);
        message.clone()
    } else {
        let seq = next_seq;
        let message = json!({
            "id":message_id,
            "chat_id":"chat_main",
            "seq":seq,
            "sender":{"kind":"bot","bot_id":"bot_main"},
            "created_at":now(),
            "edited_at":null,
            "deleted":false,
            "reply_to":null,
            "thread_count":0,
            "mentions":[{"kind":"user"}],
            "blocks":[{"type":"review_card","project_id":project_id,"artifacts":artifacts,"state":review_state}],
            "fallback_text":summary,
            "intent":null,
            "assignment_id":null,
            "streaming":false,
            "delivery":[],
            "reactions":[]
        });
        messages.push(message.clone());
        message
    };
    let event = if existing.is_some() {
        "message.updated"
    } else {
        "message.created"
    };
    state.emit(event, json!({"message":message.clone()}));
    if let Some(chat) = state
        .chats
        .iter_mut()
        .find(|chat| chat.get("id").and_then(Value::as_str) == Some("chat_main"))
    {
        chat["last_message"] = json!({
            "message_id":message["id"],
            "sender":message["sender"],
            "text":message["fallback_text"],
            "created_at":message["created_at"]
        });
        chat["last_seq"] = message["seq"].clone();
        chat["updated_at"] = message["created_at"].clone();
        let updated = chat.clone();
        state.emit("chat.updated", json!({"chat":updated}));
    }
    Ok(())
}

fn project_request_changes(
    state: &mut MockState,
    params: &Value,
    home: &std::path::Path,
) -> RpcResult {
    let project_id = params
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let project_chat_id = state
        .projects
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
        .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
    let chat_id = project_chat_id
        .get("chat_id")
        .and_then(Value::as_str)
        .unwrap_or("chat_main")
        .to_owned();
    let updated_project = if let Some(project) = state
        .projects
        .iter_mut()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
    {
        project["status"] = json!("active");
        project["updated_at"] = json!(now());
        Some(project.clone())
    } else {
        None
    };
    if let Some(project) = updated_project {
        state.emit("project.updated", json!({"project":project}));
    }
    set_review_card_mock(
        state,
        project_id,
        "changes_requested",
        Value::Null,
        params
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("已请求修改"),
    )?;
    send_chat(
        state,
        &json!({"chat_id":chat_id,"text":params.get("text").cloned().unwrap_or(json!("")),"mentions":[{"kind":"main"}]}),
        home,
    )
}

fn assignment_list(state: &MockState, params: &Value) -> RpcResult {
    let mut items = state
        .assignments
        .iter()
        .filter(|x| {
            params
                .get("project_id")
                .and_then(Value::as_str)
                .is_none_or(|id| x.get("project_id").and_then(Value::as_str) == Some(id))
        })
        .filter(|x| {
            params
                .get("bot_id")
                .and_then(Value::as_str)
                .is_none_or(|id| x.get("bot_id").and_then(Value::as_str) == Some(id))
        })
        .filter(|x| {
            params
                .get("status")
                .and_then(Value::as_array)
                .is_none_or(|statuses| {
                    statuses
                        .iter()
                        .any(|status| status.as_str() == x.get("status").and_then(Value::as_str))
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    let offset = params
        .get("cursor")
        .and_then(Value::as_str)
        .and_then(|x| x.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 100) as usize;
    let end = (offset + limit).min(items.len());
    let next = (end < items.len()).then(|| end.to_string());
    items = items[offset.min(items.len())..end].to_vec();
    Ok(json!({"items":items,"next_cursor":next}))
}
fn assignment_stop(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("assignment_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let item = state
        .assignments
        .iter_mut()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "assignment not found", None))?;
    item["status"] = json!("cancelled");
    item["finished_at"] = json!(now());
    let result = item.clone();
    let mut questions = extra_get(state, "questions");
    for question in questions.iter_mut().filter(|question| {
        question.get("assignment_id").and_then(Value::as_str) == Some(idv)
            && question.get("state").and_then(Value::as_str) == Some("pending")
    }) {
        question["state"] = json!("expired");
    }
    extra_set(state, "questions", questions);
    state.emit("assignment.updated", json!({"assignment":result.clone()}));
    let origin_chat_id = result
        .get("origin_chat_id")
        .and_then(Value::as_str)
        .unwrap_or("chat_main");
    if !origin_chat_id.starts_with("dm_") {
        upsert_mock_message(
            state,
            origin_chat_id,
            &format!("msg_task_stopped_{idv}"),
            json!({"kind":"system"}),
            json!([{"type":"system","code":"task_stopped","text":"任务已停止"}]),
            "任务已停止".into(),
            Some(idv),
        );
    }
    Ok(json!({"assignment":result}))
}

fn assignment_retry(state: &mut MockState, params: &Value) -> RpcResult {
    let assignment_id = params
        .get("assignment_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| rpc_error("invalid_params", "assignment_id is required", None))?;
    let client_request_id = params
        .get("client_request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .ok_or_else(|| {
            rpc_error(
                "invalid_params",
                "client_request_id must contain 1 to 128 bytes",
                None,
            )
        })?;
    // mock_call caches mutations by client_request_id before dispatch, matching
    // the protocol's retry idempotency behavior.
    let source = state
        .assignments
        .iter()
        .find(|assignment| assignment.get("id").and_then(Value::as_str) == Some(assignment_id))
        .cloned()
        .ok_or_else(|| rpc_error("not_found", "assignment not found", None))?;
    if source.get("status").and_then(Value::as_str) != Some("failed") {
        return Err(rpc_error(
            "conflict",
            "only failed assignments can be retried",
            None,
        ));
    }

    let bot_id = source
        .get("bot_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bot = state
        .bots
        .iter()
        .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
        .ok_or_else(|| rpc_error("not_found", "assignment Bot not found", None))?;
    let bot_running = state
        .assignments
        .iter()
        .filter(|assignment| {
            assignment.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                && assignment.get("status").and_then(Value::as_str) == Some("working")
        })
        .count();
    let global_running = state
        .assignments
        .iter()
        .filter(|assignment| {
            assignment.get("bot_id").and_then(Value::as_str) != Some("bot_main")
                && assignment.get("status").and_then(Value::as_str) == Some("working")
        })
        .count();
    let same_project_running = state.assignments.iter().any(|assignment| {
        assignment.get("project_id") == source.get("project_id")
            && assignment.get("bot_id").and_then(Value::as_str) == Some(bot_id)
            && matches!(
                assignment.get("status").and_then(Value::as_str),
                Some("working" | "queued")
            )
    });
    let bot_limit = bot.get("max_parallel").and_then(Value::as_u64).unwrap_or(2) as usize;
    let global_limit = state
        .settings
        .pointer("/concurrency/global")
        .and_then(Value::as_u64)
        .unwrap_or(4) as usize;
    let (status, queue_reason) = if bot_id != "bot_main" && bot_running >= bot_limit {
        ("queued", Some("bot_parallel_limit"))
    } else if bot_id != "bot_main" && global_running >= global_limit {
        ("queued", Some("global_limit"))
    } else if same_project_running {
        ("queued", Some("serial_in_project"))
    } else {
        ("working", None)
    };
    let created_at = now();
    let mut retry = source.clone();
    retry["id"] = json!(id("asg"));
    retry["parent_assignment_id"] = json!(assignment_id);
    retry["status"] = json!(status);
    retry["queue_reason"] = queue_reason.map_or(Value::Null, |reason| json!(reason));
    retry["wait"] = Value::Null;
    retry["created_at"] = json!(created_at);
    retry["started_at"] = if status == "working" {
        json!(created_at)
    } else {
        Value::Null
    };
    retry["finished_at"] = Value::Null;
    retry["recovered_at"] = Value::Null;
    retry["usage"] = json!({
        "input_tokens":0,
        "output_tokens":0,
        "cache_read_tokens":0,
        "cache_write_tokens":0,
        "cost":null,
        "requests":0
    });
    retry["subagents_active"] = json!(0);
    retry["steers"] = json!([]);
    retry["result_message_id"] = Value::Null;
    state.assignments.push(retry.clone());
    state.emit("assignment.created", json!({"assignment":retry.clone()}));
    let _ = client_request_id;
    Ok(json!({"assignment":retry}))
}
fn trace_history(state: &MockState, params: &Value) -> RpcResult {
    let key = params
        .get("assignment_id")
        .or_else(|| params.get("chat_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let items = state.traces.get(key).cloned().unwrap_or_default();
    let (items, has_more) = if params.get("tail").and_then(Value::as_bool) == Some(true) {
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .clamp(1, 500) as usize;
        let has_more = items.len() > limit;
        let mut tail = items.into_iter().rev().take(limit).collect::<Vec<_>>();
        tail.reverse();
        (tail, has_more)
    } else {
        paged(&items, params, "aseq", 200, 500)
    };
    let first = items.first().and_then(|x| x.get("aseq")).cloned();
    let last = items.last().and_then(|x| x.get("aseq")).cloned();
    Ok(
        json!({"items":items,"first_aseq":first,"last_aseq":last,"has_more_before":has_more,"live":false}),
    )
}

fn filtered_list(
    state: &MockState,
    key: &str,
    params: &Value,
    field: &str,
    output: &str,
) -> RpcResult {
    let filter = params.get(field).and_then(Value::as_array);
    let values = extra_get(state, key)
        .into_iter()
        .filter(|x| {
            filter.is_none_or(|f| {
                f.iter()
                    .any(|v| v.as_str() == x.get(field).and_then(Value::as_str))
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({output:values}))
}
fn approval_decide(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("approval_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let item = extra_get(state, "approvals")
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "approval not found", None))?;
    let mut item = item;
    if params.get("decision").and_then(Value::as_str) == Some("always_allow")
        && item.get("always_allow_supported").and_then(Value::as_bool) == Some(false)
    {
        return Err(rpc_error(
            "conflict",
            "always allow is not supported for this built-in dangerous operation",
            None,
        ));
    }
    item["state"] = json!(match params
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("deny")
    {
        "allow_once" => "allowed_once",
        "always_allow" => "always_allowed",
        _ => "denied",
    });
    item["decided_at"] = json!(now());
    if item["state"] == "always_allowed" {
        let rules = state.settings["approvals"]["rules"]
            .as_array_mut()
            .ok_or_else(|| rpc_error("internal", "settings.approvals.rules is invalid", None))?;
        let approval_id = item["id"].clone();
        if !rules
            .iter()
            .any(|rule| rule.get("id") == Some(&approval_id))
        {
            rules.push(json!({
                "id":approval_id,
                "kind":"auto_allow",
                "text":item.get("summary").cloned().unwrap_or_else(|| json!("")),
                "created_at":item.get("decided_at").cloned().unwrap_or_else(|| json!(now())),
                "bot_id":item.get("bot_id").cloned().unwrap_or(Value::Null),
                "builtin":false
            }));
        }
    }
    replace_extra(state, "approvals", item.clone());
    state.emit("approval.resolved", json!({"approval":item.clone()}));
    Ok(json!({"approval":item}))
}
fn question_answer(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("question_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let item = extra_get(state, "questions")
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "question not found", None))?;
    let mut item = item;
    if item.get("state").and_then(Value::as_str) == Some("expired") {
        return Err(rpc_error("conflict", "question has expired", None));
    }
    item["state"] = json!("answered");
    item["answer"] = json!({
        "option_index":params.get("option_index").cloned().unwrap_or(Value::Null),
        "option_indexes":params.get("option_indexes").cloned().unwrap_or_else(|| params.get("option_index").and_then(Value::as_u64).map(|value| json!([value])).unwrap_or_else(|| json!([]))),
        "text":params.get("text").cloned().unwrap_or(Value::Null),
        "at":now()
    });
    replace_extra(state, "questions", item.clone());
    state.emit("question.answered", json!({"question":item.clone()}));
    Ok(json!({"question":item}))
}
fn question_ask(state: &mut MockState, params: &Value) -> RpcResult {
    let required = ["bot_id", "assignment_id", "chat_id", "text"];
    for field in required {
        if params.get(field).and_then(Value::as_str).is_none() {
            return Err(rpc_error(
                "invalid_params",
                &format!("{field} is required"),
                None,
            ));
        }
    }
    let question = json!({
        "id":id("q"),
        "bot_id":params["bot_id"],
        "assignment_id":params["assignment_id"],
        "chat_id":params["chat_id"],
        "text":params["text"],
        "options":params.get("options").cloned().unwrap_or_else(|| json!([])),
        "multi":params.get("multi").and_then(Value::as_bool).unwrap_or(false),
        "allow_free_text":params.get("allow_free_text").and_then(Value::as_bool).unwrap_or(false),
        "state":"pending",
        "answer":Value::Null
    });
    replace_extra(state, "questions", question.clone());
    state.emit("question.asked", json!({"question":question.clone()}));
    Ok(json!({"question":question}))
}
fn question_list(state: &MockState, params: &Value) -> RpcResult {
    if params.get("chat_id").and_then(Value::as_str).is_none()
        && params
            .get("assignment_id")
            .and_then(Value::as_str)
            .is_none()
    {
        return Err(rpc_error(
            "invalid_params",
            "question.list requires chat_id or assignment_id",
            None,
        ));
    }
    let states = params.get("state").and_then(Value::as_array);
    let questions = extra_get(state, "questions")
        .into_iter()
        .filter(|question| {
            params
                .get("chat_id")
                .and_then(Value::as_str)
                .is_none_or(|chat_id| {
                    question.get("chat_id").and_then(Value::as_str) == Some(chat_id)
                })
        })
        .filter(|question| {
            params
                .get("assignment_id")
                .and_then(Value::as_str)
                .is_none_or(|assignment_id| {
                    question.get("assignment_id").and_then(Value::as_str) == Some(assignment_id)
                })
        })
        .filter(|question| {
            states.is_none_or(|values| {
                values
                    .iter()
                    .any(|state| state.as_str() == question.get("state").and_then(Value::as_str))
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"questions":questions}))
}
fn approval_request(state: &mut MockState, params: &Value) -> RpcResult {
    let approval = json!({
        "id":params.get("id").cloned().unwrap_or_else(|| json!(id("apr"))),
        "bot_id":params.get("bot_id").cloned().unwrap_or_else(|| json!("main")),
        "assignment_id":params.get("assignment_id").cloned().unwrap_or(Value::Null),
        "chat_id":params.get("chat_id").cloned().unwrap_or_else(|| json!("chat_main")),
        "tool":params.get("tool").cloned().unwrap_or_else(|| json!("unknown")),
        "risk":params.get("risk").cloned().unwrap_or_else(|| json!("write")),
        "summary":params.get("summary").cloned().unwrap_or_else(|| json!("")),
        "detail":params.get("detail").cloned().unwrap_or_else(|| json!("")),
        "matched_rule_text":params.get("matched_rule_text").cloned().unwrap_or(Value::Null),
        "always_allow_supported":params.get("always_allow_supported").and_then(Value::as_bool).unwrap_or(false),
        "state":"pending",
        "created_at":now(),
        "decided_at":Value::Null
    });
    replace_extra(state, "approvals", approval.clone());
    state.emit("approval.requested", json!({"approval":approval.clone()}));
    Ok(json!({"approval":approval}))
}
fn workbench(state: &MockState) -> RpcResult {
    let active_status = |status: &str| {
        matches!(
            status,
            "queued" | "working" | "waiting_user" | "waiting_bot" | "blocked"
        )
    };
    let running = state
        .assignments
        .iter()
        .filter(|assignment| assignment.get("status").and_then(Value::as_str) == Some("working"))
        .count() as u64;
    let subagents_running = state
        .assignments
        .iter()
        .map(|assignment| {
            assignment
                .get("subagents_active")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
        .sum::<u64>();
    let mut waiting = Vec::new();
    for approval in extra_get(state, "approvals")
        .into_iter()
        .filter(|approval| approval.get("state").and_then(Value::as_str) == Some("pending"))
    {
        waiting.push(json!({"kind":"approval","approval":approval}));
    }
    for question in extra_get(state, "questions")
        .into_iter()
        .filter(|question| question.get("state").and_then(Value::as_str) == Some("pending"))
    {
        waiting.push(json!({"kind":"question","question":question}));
    }
    for project in state
        .projects
        .iter()
        .filter(|project| project.get("status").and_then(Value::as_str) == Some("review"))
    {
        waiting.push(json!({
            "kind":"review",
            "project_id":project.get("id").cloned().unwrap_or(Value::Null),
            "since":project.get("updated_at").cloned().unwrap_or_else(|| json!(now()))
        }));
    }
    for takeover in extra_get(state, "takeovers")
        .into_iter()
        .filter(|takeover| takeover.get("state").and_then(Value::as_str) == Some("active"))
    {
        waiting.push(json!({
            "kind":"takeover",
            "bot_id":takeover.get("bot_id").cloned().unwrap_or(Value::Null),
            "assignment_id":takeover.get("assignment_id").cloned().unwrap_or(Value::Null),
            "reason":takeover.get("reason").cloned().unwrap_or_else(|| json!("user takeover"))
        }));
    }
    let today = Utc::now().date_naive().to_string();
    let done_today = state
        .assignments
        .iter()
        .filter(|assignment| assignment.get("status").and_then(Value::as_str) == Some("done"))
        .filter(|assignment| {
            assignment
                .get("finished_at")
                .and_then(Value::as_str)
                .is_some_and(|finished| finished.starts_with(&today))
        })
        .cloned()
        .collect::<Vec<_>>();
    let bots = state
        .bots
        .iter()
        .map(|bot| {
            let bot_id = bot.get("id").and_then(Value::as_str).unwrap_or_default();
            let assignments = state
                .assignments
                .iter()
                .filter(|assignment| {
                    assignment.get("bot_id").and_then(Value::as_str) == Some(bot_id)
                        && assignment
                            .get("status")
                            .and_then(Value::as_str)
                            .is_some_and(active_status)
                })
                .cloned()
                .collect::<Vec<_>>();
            let active = assignments
                .iter()
                .filter(|assignment| {
                    assignment.get("status").and_then(Value::as_str) == Some("working")
                })
                .count() as u64;
            json!({
                "bot_id":bot_id,
                "active":active,
                "max_parallel":bot.get("max_parallel").and_then(Value::as_u64).unwrap_or(2),
                "assignments":assignments
            })
        })
        .collect::<Vec<_>>();
    let workbench = json!({
        "running":running,
        "global_limit":state.settings.pointer("/concurrency/global").and_then(Value::as_u64).unwrap_or(4),
        "subagents_running":subagents_running,
        "waiting":waiting,
        "bots":bots,
        "done_today":done_today
    });
    Ok(workbench)
}

fn skill_default(name: &str, description: &str, source: &str, content: Option<&str>) -> Value {
    json!({"name":name,"description":description,"source":source,"path":format!("builtin://{name}"),"files":[],"enabled":true,"disabled_bot_ids":[],"invocations_7d":{"total":0,"by_bot":[]},"last_invoked_at":null,"updated_at":now(),"content":content})
}
fn skill_get(state: &MockState, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let mut item = extra_get(state, "skills")
        .into_iter()
        .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| rpc_error("not_found", "skill not found", None))?;
    item.as_object_mut()
        .expect("skill records are objects")
        .entry("last_invoked_at")
        .or_insert(Value::Null);
    Ok(json!({"skill":item}))
}
fn skill_create(state: &mut MockState, params: &Value, source: &str) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() {
        return Err(rpc_error("invalid_params", "name is required", None));
    }
    if extra_get(state, "skills")
        .iter()
        .any(|x| x.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(rpc_error("conflict", "skill already exists", None));
    }
    let content = params.get("content").and_then(Value::as_str).unwrap_or("");
    let description = content
        .lines()
        .find_map(|line| line.strip_prefix("description:"))
        .map(str::trim)
        .unwrap_or("");
    let mut skill = skill_default(name, description, source, Some(content));
    skill["path"] = json!(format!("~/MacBot/skills/{name}"));
    extra_get(state, "skills")
        .into_iter()
        .chain([skill.clone()])
        .collect::<Vec<_>>()
        .pipe(|v| extra_set(state, "skills", v));
    state.emit("skill.updated", json!({"skill":skill.clone()}));
    Ok(json!({"skill":skill}))
}
fn skill_update(state: &mut MockState, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let mut skill = extra_get(state, "skills")
        .into_iter()
        .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| rpc_error("not_found", "skill not found", None))?;
    if skill.get("source").and_then(Value::as_str) == Some("builtin") {
        return Err(rpc_error(
            "forbidden",
            "builtin skill cannot be modified",
            None,
        ));
    }
    let content = params.get("content").and_then(Value::as_str).unwrap_or("");
    skill["content"] = json!(content);
    skill["updated_at"] = json!(now());
    replace_extra(state, "skills", skill.clone());
    state.emit("skill.updated", json!({"skill":skill.clone()}));
    Ok(json!({"skill":skill}))
}
fn skill_delete(state: &mut MockState, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let skills = extra_get(state, "skills");
    let skill = skills
        .iter()
        .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| rpc_error("not_found", "skill not found", None))?;
    if skill.get("source").and_then(Value::as_str) == Some("builtin") {
        return Err(rpc_error(
            "forbidden",
            "builtin skill cannot be modified",
            None,
        ));
    }
    extra_set(
        state,
        "skills",
        skills
            .into_iter()
            .filter(|x| x.get("name").and_then(Value::as_str) != Some(name))
            .collect(),
    );
    state.emit("skill.deleted", json!({"name":name}));
    Ok(json!({}))
}
fn skill_set_enabled(state: &mut MockState, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut skill = extra_get(state, "skills")
        .into_iter()
        .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| rpc_error("not_found", "skill not found", None))?;
    if let Some(bot_id) = params.get("bot_id").and_then(Value::as_str) {
        let ids = skill["disabled_bot_ids"].as_array_mut().unwrap();
        ids.retain(|x| x.as_str() != Some(bot_id));
        if !enabled {
            ids.push(json!(bot_id));
        }
    } else {
        skill["enabled"] = json!(enabled);
    }
    replace_extra(state, "skills", skill.clone());
    state.emit("skill.updated", json!({"skill":skill.clone()}));
    Ok(json!({"skill":skill}))
}
fn skill_publish(state: &mut MockState, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let mut skill = extra_get(state, "skills")
        .into_iter()
        .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| rpc_error("not_found", "skill not found", None))?;
    if skill.get("source").and_then(Value::as_str) != Some("draft") {
        return Err(rpc_error(
            "invalid_params",
            "only draft skills can be published",
            None,
        ));
    }
    skill["source"] = json!("user");
    skill["enabled"] = json!(true);
    replace_extra(state, "skills", skill.clone());
    state.emit("skill.updated", json!({"skill":skill.clone()}));
    Ok(json!({"skill":skill}))
}
fn skill_import(state: &mut MockState, params: &Value) -> RpcResult {
    let source = params.get("source").cloned().unwrap_or(Value::Null);
    let name = source
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| source.get("url").and_then(Value::as_str))
        .and_then(|x| x.rsplit('/').next())
        .unwrap_or("imported-skill")
        .trim_end_matches(".git");
    let content = "---\nname: imported-skill\ndescription: Imported mock skill\n---\n";
    let result = skill_create(
        state,
        &json!({"name":if name.is_empty(){"imported-skill"}else{name},"content":content}),
        "imported",
    );
    result.map(|value| json!({"skills":[value["skill"].clone()]}))
}

fn routine_list(state: &MockState, params: &Value) -> RpcResult {
    let bot_id = params.get("bot_id").and_then(Value::as_str);
    Ok(
        json!({"routines":extra_get(state,"routines").into_iter().filter(|x|bot_id.is_none_or(|id|x.get("bot_id").and_then(Value::as_str)==Some(id))).collect::<Vec<_>>() }),
    )
}
fn routine_create(state: &mut MockState, params: &Value) -> RpcResult {
    let routine = json!({"id":id("routine"),"bot_id":params["bot_id"],"project_id":params.get("project_id").cloned().unwrap_or(Value::Null),"name":params.get("name").cloned().unwrap_or(json!("Routine")),"instructions":params.get("instructions").cloned().unwrap_or(json!("")),"schedules":params.get("schedules").cloned().unwrap_or(json!([])),"timezone":params.get("timezone").cloned().unwrap_or(json!("Asia/Shanghai")),"enabled":true,"next_run_at":null,"last_run":null,"created_at":now(),"updated_at":now()});
    let mut rows = extra_get(state, "routines");
    rows.push(routine.clone());
    extra_set(state, "routines", rows);
    state.emit("routine.updated", json!({"routine":routine.clone()}));
    Ok(json!({"routine":routine}))
}
fn routine_update(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("routine_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut routine = extra_get(state, "routines")
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "routine not found", None))?;
    merge_patch(&mut routine, params.get("patch"));
    routine["updated_at"] = json!(now());
    replace_extra(state, "routines", routine.clone());
    state.emit("routine.updated", json!({"routine":routine.clone()}));
    Ok(json!({"routine":routine}))
}
fn routine_delete(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("routine_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rows = extra_get(state, "routines");
    if !rows
        .iter()
        .any(|x| x.get("id").and_then(Value::as_str) == Some(idv))
    {
        return Err(rpc_error("not_found", "routine not found", None));
    }
    extra_set(
        state,
        "routines",
        rows.into_iter()
            .filter(|x| x.get("id").and_then(Value::as_str) != Some(idv))
            .collect(),
    );
    state.emit("routine.deleted", json!({"routine_id":idv}));
    Ok(json!({}))
}
fn routine_set_enabled(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("routine_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut routine = extra_get(state, "routines")
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "routine not found", None))?;
    routine["enabled"] = json!(params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true));
    routine["updated_at"] = json!(now());
    replace_extra(state, "routines", routine.clone());
    state.emit("routine.updated", json!({"routine":routine.clone()}));
    Ok(json!({"routine":routine}))
}
fn routine_test_run(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("routine_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !extra_get(state, "routines")
        .iter()
        .any(|x| x.get("id").and_then(Value::as_str) == Some(idv))
    {
        return Err(rpc_error("not_found", "routine not found", None));
    }
    let run = json!({"id":id("routine_run"),"routine_id":idv,"assignment_id":null,"trigger":"test","status":"running","started_at":now(),"finished_at":null,"error":null});
    let mut rows = extra_get(state, "routine_runs");
    rows.push(run.clone());
    extra_set(state, "routine_runs", rows);
    state.emit("routine.run", json!({"run":run.clone()}));
    Ok(json!({"run":run}))
}
fn routine_runs(state: &MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("routine_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok(
        json!({"runs":extra_get(state,"routine_runs").into_iter().filter(|x|x.get("routine_id").and_then(Value::as_str)==Some(idv)).rev().take(20).collect::<Vec<_>>() }),
    )
}

fn provider_create(state: &mut MockState, params: &Value) -> RpcResult {
    let provider = json!({"id":id("provider"),"name":params.get("name").cloned().unwrap_or(json!("Mock")),"api_kind":params.get("api_kind").cloned().unwrap_or(json!("openai-completions")),"base_url":params.get("base_url").cloned().unwrap_or(json!("")),"has_key":params.get("api_key").and_then(Value::as_str).is_some_and(|key| !key.is_empty()),"headers":params.get("headers").cloned().unwrap_or(json!({})),"created_at":now(),"updated_at":now()});
    let mut rows = extra_get(state, "providers");
    rows.push(provider.clone());
    extra_set(state, "providers", rows);
    state.emit(
        "provider.updated",
        json!({"provider":provider.clone(),"models":extra_get(state,"models")}),
    );
    Ok(json!({"provider":provider}))
}
fn provider_update(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("provider_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut provider = extra_get(state, "providers")
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_str) == Some(idv))
        .ok_or_else(|| rpc_error("not_found", "provider not found", None))?;
    if let Some(patch) = params.get("patch").and_then(Value::as_object) {
        for (key, value) in patch {
            if key == "api_key" {
                provider["has_key"] = json!(value.as_str().is_some_and(|x| !x.is_empty()));
            } else {
                provider[key] = value.clone();
            }
        }
    }
    provider["updated_at"] = json!(now());
    replace_extra(state, "providers", provider.clone());
    state.emit(
        "provider.updated",
        json!({"provider":provider.clone(),"models":extra_get(state,"models")}),
    );
    Ok(json!({"provider":provider}))
}
fn provider_delete(state: &mut MockState, params: &Value) -> RpcResult {
    let idv = params
        .get("provider_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rows = extra_get(state, "providers");
    if !rows
        .iter()
        .any(|x| x.get("id").and_then(Value::as_str) == Some(idv))
    {
        return Err(rpc_error("not_found", "provider not found", None));
    }
    if extra_get(state, "models")
        .iter()
        .any(|x| x.get("provider_id").and_then(Value::as_str) == Some(idv))
    {
        return Err(rpc_error("conflict", "provider is used by a model", None));
    }
    extra_set(
        state,
        "providers",
        rows.into_iter()
            .filter(|x| x.get("id").and_then(Value::as_str) != Some(idv))
            .collect(),
    );
    state.emit("provider.deleted", json!({"provider_id":idv}));
    Ok(json!({}))
}
fn model_upsert(state: &mut MockState, params: &Value) -> RpcResult {
    let provider_id = params
        .get("provider_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !extra_get(state, "providers")
        .iter()
        .any(|x| x.get("id").and_then(Value::as_str) == Some(provider_id))
    {
        return Err(rpc_error("not_found", "provider not found", None));
    }
    let model = json!({"ref":params.get("ref").cloned().unwrap_or(json!(format!("{provider_id}:{}",params.get("model_id").and_then(Value::as_str).unwrap_or("model")))),"provider_id":provider_id,"model_id":params.get("model_id").cloned().unwrap_or(json!("model")),"display_name":params.get("display_name").cloned().unwrap_or(params.get("model_id").cloned().unwrap_or(json!("Model"))),"context_window":params.get("context_window").cloned().unwrap_or(json!(128000)),"max_output":params.get("max_output").cloned().unwrap_or(json!(4096)),"caps":params.get("caps").cloned().unwrap_or(json!({"vision":false,"tools":true,"reasoning":false})),"price":params.get("price").cloned().unwrap_or(Value::Null),"enabled":params.get("enabled").and_then(Value::as_bool).unwrap_or(true)});
    let mut rows = extra_get(state, "models");
    rows.retain(|x| x.get("ref") != model.get("ref"));
    rows.push(model.clone());
    extra_set(state, "models", rows);
    state.emit("provider.updated",json!({"provider":extra_get(state,"providers").into_iter().find(|x|x.get("id").and_then(Value::as_str)==Some(provider_id)).unwrap_or(Value::Null),"models":extra_get(state,"models")}));
    Ok(json!({"model":model}))
}
fn model_delete(state: &mut MockState, params: &Value) -> RpcResult {
    let reference = params.get("ref").cloned().unwrap_or(Value::Null);
    let rows = extra_get(state, "models");
    let provider_id = rows
        .iter()
        .find(|model| model.get("ref") == Some(&reference))
        .and_then(|model| model.get("provider_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    extra_set(
        state,
        "models",
        rows.into_iter()
            .filter(|x| x.get("ref") != Some(&reference))
            .collect(),
    );
    if let Some(provider_id) = provider_id {
        let provider = extra_get(state, "providers")
            .into_iter()
            .find(|provider| provider.get("id").and_then(Value::as_str) == Some(&provider_id))
            .unwrap_or(Value::Null);
        state.emit(
            "provider.updated",
            json!({"provider":provider,"models":extra_get(state,"models")}),
        );
    }
    Ok(json!({}))
}
fn settings_update(state: &mut MockState, params: &Value) -> RpcResult {
    crate::adapter::validate_settings_secret_params(params)?;
    if let Some(proposed) = params
        .pointer("/patch/approvals/rules")
        .and_then(Value::as_array)
    {
        let current_builtins = state
            .settings
            .pointer("/approvals/rules")
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .filter(|rule| rule.get("builtin").and_then(Value::as_bool) == Some(true))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if current_builtins
            .iter()
            .any(|builtin| !proposed.iter().any(|rule| rule == *builtin))
            || proposed
                .iter()
                .filter(|rule| rule.get("builtin").and_then(Value::as_bool) == Some(true))
                .any(|rule| !current_builtins.contains(&rule))
        {
            return Err(rpc_error(
                "forbidden",
                "built-in approval rules are read-only",
                None,
            ));
        }
    }
    merge_patch(&mut state.settings, params.get("patch"));
    if let Some(key) = params.get("web_search_key").and_then(Value::as_str) {
        state.settings["web_search"]["has_key"] = json!(!key.is_empty());
    }
    let settings = state.settings.clone();
    state.emit("settings.updated", json!({"settings":settings.clone()}));
    Ok(json!({"settings":settings}))
}

fn merge_patch(target: &mut Value, patch: Option<&Value>) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.and_then(Value::as_object))
    {
        for (key, value) in patch {
            if let Some(existing) = target.get_mut(key) {
                if existing.is_object() && value.is_object() {
                    merge_patch(existing, Some(value));
                    continue;
                }
            }
            target.insert(key.clone(), value.clone());
        }
    }
}
fn replace_extra(state: &mut MockState, key: &str, value: Value) {
    let mut rows = extra_get(state, key);
    if let Some(idv) = value.get("id").and_then(Value::as_str) {
        rows.retain(|x| x.get("id").and_then(Value::as_str) != Some(idv));
    } else if let Some(name) = value.get("name").and_then(Value::as_str) {
        rows.retain(|x| x.get("name").and_then(Value::as_str) != Some(name));
    }
    rows.push(value);
    extra_set(state, key, rows);
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

#[cfg(test)]
mod contract_tests {
    use super::super::{Gateway, GatewayConfig, MockState};
    use chrono::{Duration, Utc};
    use macbot_protocol::{
        Announcement, Assignment, Bot, Chat, Device, Message, Model, Project, Provider, Routine,
        RoutineRun, SearchHit, SearchResult, Settings, Skill, SkillDetail, UsageBreakdownResult,
        UsageSummaryResult, UsageTimeseriesResult, WorkbenchResult,
    };
    use serde_json::{json, Value};

    async fn gateway() -> Gateway {
        let home = tempfile::tempdir().unwrap();
        // Keep the directory alive so a gateway can read persisted mock data.
        let path = home.keep();
        gateway_at(path).await
    }

    async fn gateway_at(home: std::path::PathBuf) -> Gateway {
        Gateway::new(GatewayConfig {
            home,
            mock: true,
            password: Some("dev".into()),
            ..Default::default()
        })
    }

    async fn call(gateway: &Gateway, method: &str, params: Value) -> Value {
        gateway.rpc(method, params).await.unwrap()
    }

    #[tokio::test]
    async fn mock_project_create_requires_one_to_six_non_main_workers() {
        let gateway = gateway().await;
        for member_bot_ids in [json!([]), json!(vec!["worker"; 7])] {
            let error = gateway
                .rpc(
                    "project.create",
                    json!({"name":"invalid group","member_bot_ids":member_bot_ids}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "invalid_params");
            assert_eq!(
                error.message,
                "invalid parameters: a project needs 1-6 member bots"
            );
        }

        for main_id in ["main", "bot_main"] {
            let error = gateway
                .rpc(
                    "project.create",
                    json!({"name":"main duplication","member_bot_ids":[main_id]}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "invalid_params");
            assert_eq!(
                error.message,
                "invalid parameters: main Bot is added to every project automatically"
            );
        }

        let worker = call(&gateway, "bot.create", json!({"name":"worker"})).await;
        let worker_id = worker["bot"]["id"].as_str().unwrap();
        let duplicate = gateway
            .rpc(
                "project.create",
                json!({"name":"duplicate workers","member_bot_ids":[worker_id,worker_id]}),
            )
            .await
            .unwrap_err();
        assert_eq!(duplicate.code, "invalid_params");
        assert!(duplicate.message.contains("appears more than once"));

        let created = call(
            &gateway,
            "project.create",
            json!({"name":"one worker","member_bot_ids":[worker_id]}),
        )
        .await;
        let project_members = created["project"]["members"].as_array().unwrap();
        assert_eq!(project_members.len(), 2);
        assert_eq!(created["project"]["lead_bot_id"], "bot_main");
        assert_eq!(project_members[0]["bot_id"], "bot_main");
        assert_eq!(project_members[1]["bot_id"], worker_id);
        let chat_members = created["chat"]["member_bot_ids"].as_array().unwrap();
        assert_eq!(chat_members, &[json!("bot_main"), json!(worker_id)]);
    }

    #[tokio::test]
    async fn mock_project_members_keep_main_and_enforce_six_worker_limit() {
        let gateway = gateway().await;
        let mut workers = Vec::new();
        for index in 0..7 {
            let result = call(
                &gateway,
                "bot.create",
                json!({"name":format!("worker-{index}")}),
            )
            .await;
            workers.push(result["bot"]["id"].as_str().unwrap().to_owned());
        }
        let created = call(
            &gateway,
            "project.create",
            json!({"name":"member invariants","member_bot_ids":[workers[0]]}),
        )
        .await;
        let project_id = created["project"]["id"].as_str().unwrap();

        for main_id in ["main", "bot_main"] {
            let error = gateway
                .rpc(
                    "project.remove_member",
                    json!({"project_id":project_id,"bot_id":main_id}),
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "forbidden");
            assert_eq!(
                error.message,
                "forbidden: main Bot cannot be removed from a project"
            );
        }
        let unchanged = call(&gateway, "project.get", json!({"project_id":project_id})).await;
        assert!(unchanged["project"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["bot_id"] == "bot_main"));

        call(
            &gateway,
            "project.add_member",
            json!({"project_id":project_id,"bot_id":"main"}),
        )
        .await;
        for worker_id in &workers[1..6] {
            call(
                &gateway,
                "project.add_member",
                json!({"project_id":project_id,"bot_id":worker_id}),
            )
            .await;
        }
        let seventh = gateway
            .rpc(
                "project.add_member",
                json!({"project_id":project_id,"bot_id":workers[6]}),
            )
            .await
            .unwrap_err();
        assert_eq!(seventh.code, "invalid_params");
        assert_eq!(
            seventh.message,
            "invalid parameters: project has at most 6 workers plus main"
        );

        let removed = call(
            &gateway,
            "project.remove_member",
            json!({"project_id":project_id,"bot_id":workers[1]}),
        )
        .await;
        assert_eq!(removed["project"]["members"].as_array().unwrap().len(), 6);
        assert!(removed["project"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|member| member["bot_id"] == "bot_main"));
        let bootstrap = call(&gateway, "bootstrap", json!({})).await;
        let chat = bootstrap["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|chat| chat["project_id"] == project_id)
            .unwrap();
        assert_eq!(chat["member_bot_ids"].as_array().unwrap().len(), 6);
        assert!(chat["member_bot_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "bot_main"));

        for worker_id in workers.iter().take(6).filter(|id| *id != &workers[1]) {
            call(
                &gateway,
                "project.remove_member",
                json!({"project_id":project_id,"bot_id":worker_id}),
            )
            .await;
        }
        let main_only = call(&gateway, "project.get", json!({"project_id":project_id})).await;
        assert_eq!(main_only["project"]["members"].as_array().unwrap().len(), 1);
        assert_eq!(main_only["project"]["members"][0]["bot_id"], "bot_main");
    }

    #[test]
    fn login_announcement_members_ignores_assignments_from_other_projects() {
        let mut state = MockState::new("test", false, "node");
        state.assignments = vec![
            json!({
                "id":"asg_other_project","bot_id":"bot_code","project_id":"prj_other",
                "status":"working","started_at":"2026-10-14T00:00:00Z"
            }),
            json!({
                "id":"asg_login_project","bot_id":"bot_code","project_id":"prj_login",
                "status":"waiting_user","started_at":"2026-10-14T01:00:00Z"
            }),
        ];

        let members =
            super::login_announcement_members(&state, "prj_login", &["bot_code".to_owned()]);

        assert_eq!(members[0]["state"], "waiting_user");
        assert_eq!(members[0]["current_assignment_id"], "asg_login_project");
    }

    #[tokio::test]
    async fn mock_host_permissions_returns_conservative_states_and_settings_defaults_auto_allow() {
        let gateway = gateway().await;
        let permissions = call(&gateway, "host.permissions", json!({})).await;
        let _: macbot_protocol::HostPermissionsResult =
            serde_json::from_value(permissions.clone()).unwrap();
        assert_eq!(permissions["screen_recording"], "unknown");
        assert_eq!(permissions["full_disk_access"], "unknown");
        let settings = call(&gateway, "settings.get", json!({})).await["settings"].clone();
        let _: Settings = serde_json::from_value(settings.clone()).unwrap();
        assert_eq!(settings["approvals"]["mode"], "always_allow");
        assert!(settings["approvals"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule.get("builtin").and_then(Value::as_bool) == Some(true)));
    }

    #[tokio::test]
    async fn mock_pending_review_round_trips_through_card_bootstrap_and_workbench() {
        let gateway = gateway().await;
        let bootstrap = call(&gateway, "bootstrap", json!({})).await;
        let project_id = bootstrap["pending"]["reviews"][0]
            .as_str()
            .expect("mock seeds a project awaiting review")
            .to_owned();
        let main_history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        let card = main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["blocks"][0]["type"] == "review_card"
                    && message["blocks"][0]["project_id"] == project_id
            })
            .expect("pending review is represented by a main chat card");
        assert_eq!(card["blocks"][0]["state"], "pending");
        let workbench = call(&gateway, "workbench.get", json!({})).await;
        assert!(workbench["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["kind"] == "review" && item["project_id"] == project_id }));

        let requested_changes = call(
            &gateway,
            "project.request_changes",
            json!({"project_id":project_id,"text":"请补充验收说明"}),
        )
        .await;
        assert_eq!(
            requested_changes["message"]["fallback_text"],
            "请补充验收说明"
        );
        let after_changes = call(&gateway, "bootstrap", json!({})).await;
        assert!(!after_changes["pending"]["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &project_id));
        let main_history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        let card = main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| {
                message["blocks"][0]["type"] == "review_card"
                    && message["blocks"][0]["project_id"] == project_id
            })
            .unwrap();
        assert_eq!(card["blocks"][0]["state"], "changes_requested");

        let review_request = json!({
            "project_id":project_id,
            "summary":"已补充验收说明",
            "client_request_id":"mock-review-resubmit"
        });
        let first = call(&gateway, "project.request_review", review_request.clone()).await;
        let retry = call(&gateway, "project.request_review", review_request).await;
        assert_eq!(first, retry);
        assert_eq!(first["project"]["status"], "review");
        let confirmed = call(
            &gateway,
            "project.confirm_done",
            json!({"project_id":project_id}),
        )
        .await;
        assert_eq!(confirmed["project"]["status"], "done");
        let after_confirmation = call(&gateway, "bootstrap", json!({})).await;
        assert!(!after_confirmation["pending"]["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &project_id));
        let main_history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        let cards = main_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["blocks"][0]["type"] == "review_card"
                    && message["blocks"][0]["project_id"] == project_id
            })
            .collect::<Vec<_>>();
        assert_eq!(cards.len(), 1, "the review message is updated in place");
        assert_eq!(cards[0]["blocks"][0]["state"], "confirmed");
    }

    #[tokio::test]
    async fn web_redesign_trace_is_detailed_and_keeps_assignment_queue_state() {
        let gateway = gateway().await;
        let assignment = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_code_web"}),
        )
        .await["assignment"]
            .clone();
        assert_eq!(assignment["status"], "working");
        assert_eq!(assignment["origin_chat_id"], "chat_web");
        assert_eq!(assignment["project_id"], "prj_web");

        let history = call(
            &gateway,
            "trace.history",
            json!({"assignment_id":"asg_code_web","tail":true,"limit":200}),
        )
        .await;
        let items = history["items"].as_array().expect("trace items");
        assert_eq!(items.len(), 16);
        for (index, item) in items.iter().enumerate() {
            assert_eq!(item["aseq"], index as u64 + 1);
            assert_eq!(item["assignment_id"], "asg_code_web");
            assert_eq!(item["chat_id"], assignment["origin_chat_id"]);
            serde_json::from_value::<macbot_protocol::TraceItem>(item.clone())
                .expect("mock trace item matches the public protocol type");
        }
        assert_eq!(items[0]["type"], "run.start");
        assert!(items.iter().any(|item| item["type"] == "tool.end"));
        assert!(items
            .iter()
            .any(|item| { item["type"] == "run.end" && item["run_id"] == "run_code_web_sub1" }));
        assert_eq!(items.last().unwrap()["type"], "tool.start");
        assert!(!items
            .iter()
            .any(|item| { item["type"] == "run.end" && item["run_id"] == "run_code_web" }));

        let chat_history = call(&gateway, "trace.history", json!({"chat_id":"chat_web"})).await;
        assert_eq!(chat_history["items"].as_array().unwrap().len(), items.len());
        assert!(chat_history["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| {
                item["assignment_id"] == assignment["id"]
                    && item["chat_id"] == assignment["origin_chat_id"]
            }));

        let queued = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_queued"}),
        )
        .await;
        assert_eq!(queued["assignment"]["status"], "queued");
        assert_eq!(queued["assignment"]["queue_reason"], "serial_in_project");
    }

    #[tokio::test]
    async fn mock_message_block_scenarios_cover_clickable_project_task_and_delegation_cards() {
        let gateway = gateway().await;
        let login = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        let messages = login["messages"].as_array().unwrap();
        for block_type in ["completion", "progress", "task_card", "system"] {
            assert!(
                messages.iter().any(|message| {
                    message["blocks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|block| block["type"] == block_type)
                }),
                "chat_login contains {block_type} block"
            );
        }
        for (assignment_id, status) in [
            ("asg_mock_queued", "queued"),
            ("asg_mock_waiting", "waiting_user"),
            ("asg_mock_working", "working"),
            ("asg_mock_done", "done"),
            ("asg_mock_failed", "failed"),
        ] {
            let task = call(
                &gateway,
                "assignment.get",
                json!({"assignment_id":assignment_id}),
            )
            .await;
            assert_eq!(task["assignment"]["status"], status);
            assert!(messages.iter().any(|message| {
                message["blocks"][0]["type"] == "task_card"
                    && message["blocks"][0]["assignment_id"] == assignment_id
            }));
            let trace = call(
                &gateway,
                "trace.history",
                json!({"assignment_id":assignment_id}),
            )
            .await;
            assert!(trace.get("items").is_some());
        }
        let main = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(main["messages"].as_array().unwrap().iter().any(|message| {
            message["blocks"][0]["type"] == "delegation"
                && message["blocks"][0]["assignment_id"] == "asg_mock_delegation"
        }));
        let project_card = main["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["blocks"][0]["type"] == "project_card")
            .unwrap();
        let project_id = project_card["blocks"][0]["project_id"].clone();
        let project = call(&gateway, "project.get", json!({"project_id":project_id})).await;
        assert_eq!(project["project"]["chat_id"], "chat_login");

        let stopped = call(
            &gateway,
            "assignment.stop",
            json!({"assignment_id":"asg_mock_waiting"}),
        )
        .await;
        assert_eq!(stopped["assignment"]["status"], "cancelled");
        let after_stop = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_waiting"}),
        )
        .await;
        assert_eq!(after_stop["assignment"]["status"], "cancelled");
        let stopped_history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        assert!(stopped_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["blocks"][0]["type"] == "system"
                    && message["blocks"][0]["code"] == "task_stopped"
                    && message["assignment_id"] == "asg_mock_waiting"
            }));
    }

    #[tokio::test]
    async fn pending_review_mock_has_group_history_and_project_artifacts() {
        let gateway = gateway().await;
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_mock_review","limit":100}),
        )
        .await;
        let messages = history["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert!(messages.iter().any(|message| {
            message["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["type"] == "progress")
        }));
        assert!(messages.iter().any(|message| {
            message["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["type"] == "completion")
        }));

        let project = call(
            &gateway,
            "project.get",
            json!({"project_id":"prj_mock_review"}),
        )
        .await;
        assert_eq!(project["project"]["chat_id"], "chat_mock_review");
        let artifacts = project["announcement"]["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 3);
        assert_eq!(
            artifacts
                .iter()
                .filter_map(|artifact| artifact["id"].as_str())
                .collect::<Vec<_>>(),
            ["art_mock_brief", "art_mock_build", "art_mock_report"]
        );
    }

    #[tokio::test]
    async fn mock_project_without_announcement_returns_explicit_empty_text() {
        let gateway = gateway().await;
        let result = call(&gateway, "project.get", json!({"project_id":"prj_web"})).await;
        assert_eq!(result["project"]["chat_id"], "chat_web");

        let raw_announcement = &result["announcement"];
        for required in [
            "project_id",
            "text",
            "version",
            "text_updated_by",
            "text_updated_at",
            "members",
            "artifacts",
            "highlights",
            "updated_at",
        ] {
            assert!(
                raw_announcement.get(required).is_some(),
                "missing {required}"
            );
        }
        assert!(raw_announcement["text"].is_string());
        assert_eq!(raw_announcement["text"], "");
        let text_updated_at = raw_announcement["text_updated_at"]
            .as_str()
            .expect("text_updated_at should be a string");
        chrono::DateTime::parse_from_rfc3339(text_updated_at)
            .expect("text_updated_at should be RFC3339");
        assert_eq!(
            raw_announcement["text_updated_at"],
            raw_announcement["updated_at"]
        );

        let announcement: Announcement = serde_json::from_value(raw_announcement.clone()).unwrap();
        assert_eq!(announcement.project_id, "prj_web");
        assert_eq!(announcement.text, "");
        assert_eq!(announcement.members.len(), 0);
        assert_eq!(announcement.artifacts.len(), 0);
    }

    #[tokio::test]
    async fn mock_profile_language_matches_production_validation() {
        let gateway = gateway().await;
        let normalized = call(
            &gateway,
            "memory.profile.update",
            json!({"language":"简体中文","expected_version":1}),
        )
        .await;
        assert_eq!(normalized["profile"]["language"], "zh-CN");
        let invalid = gateway
            .rpc(
                "memory.profile.update",
                json!({"name":"Changed","language":"zh_CN","expected_version":2}),
            )
            .await
            .unwrap_err();
        assert_eq!(invalid.code, "invalid_params");
        assert!(invalid.message.contains("BCP 47"));
        assert_eq!(
            call(&gateway, "memory.profile.get", json!({})).await,
            normalized
        );
        let cleared = call(
            &gateway,
            "memory.profile.update",
            json!({"language":" ","expected_version":2}),
        )
        .await;
        assert!(cleared["profile"]["language"].is_null());
    }

    #[tokio::test]
    async fn mock_user_memory_and_profile_enforce_versions_and_emit_user_events() {
        let gateway = gateway().await;
        let initial = call(&gateway, "memory.user.list", json!({"owner_id":"ignored"})).await;
        assert!(initial["entries"].as_array().unwrap().len() >= 6);
        call(
            &gateway,
            "mock.set_memory_scenario",
            json!({"quota_full":false}),
        )
        .await;
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["id"] == "msg_main_welcome"
                    && message["chat_id"] == "chat_main"
                    && message["sender"]["kind"] == "user"
            }));
        assert!(initial["entries"].as_array().unwrap().iter().any(|entry| {
            entry["source"]["chat_id"] == "chat_main"
                && history["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["id"] == entry["source"]["message_id"])
        }));
        let created = call(
            &gateway,
            "memory.user.create",
            json!({"content":"请使用简短列表","owner_id":"attacker"}),
        )
        .await;
        assert_eq!(created["entry"]["source"]["by"]["kind"], "user");
        let entry_id = created["entry"]["id"].as_str().unwrap();
        let spaced = call(
            &gateway,
            "memory.user.create",
            json!({"content":"  preserve surrounding spaces  "}),
        )
        .await;
        assert_eq!(
            spaced["entry"]["content"],
            "  preserve surrounding spaces  "
        );
        let spaced_id = spaced["entry"]["id"].as_str().unwrap();
        let conflict = gateway
            .rpc(
                "memory.user.update",
                json!({"entry_id":entry_id,"content":"stale","expected_version":0}),
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, "conflict");
        assert_eq!(conflict.details.unwrap()["current"]["version"], 1);
        let updated = call(
            &gateway,
            "memory.user.update",
            json!({"entry_id":entry_id,"content":"请使用简短编号列表","expected_version":1}),
        )
        .await;
        assert_eq!(updated["entry"]["version"], 2);
        let spaced_updated = call(
            &gateway,
            "memory.user.update",
            json!({
                "entry_id":spaced_id,
                "content":"  updated with spaces  ",
                "expected_version":1
            }),
        )
        .await;
        assert_eq!(
            spaced_updated["entry"]["content"],
            "  updated with spaces  "
        );

        let profile = call(&gateway, "memory.profile.get", json!({})).await;
        assert_eq!(profile["profile"]["timezone"], "Asia/Shanghai");
        let profile = call(
            &gateway,
            "memory.profile.update",
            json!({"name":"Ari","expected_version":1}),
        )
        .await;
        assert_eq!(profile["profile"]["name"], "Ari");
        assert_eq!(profile["profile"]["version"], 2);
        let profile_conflict = gateway
            .rpc(
                "memory.profile.update",
                json!({"name":"Other","expected_version":1}),
            )
            .await
            .unwrap_err();
        assert_eq!(profile_conflict.code, "conflict");
        assert_eq!(profile_conflict.details.unwrap()["current"]["version"], 2);

        let deleted = call(
            &gateway,
            "memory.user.delete",
            json!({"entry_id":entry_id,"expected_version":2}),
        )
        .await;
        assert_eq!(
            deleted["quota"]["used_chars"].as_u64().unwrap(),
            initial["quota"]["used_chars"].as_u64().unwrap()
                + "  updated with spaces  ".chars().count() as u64
        );
        let events = gateway
            .state
            .inner
            .read()
            .await
            .events
            .iter()
            .filter_map(|event| event.get("event").and_then(Value::as_str))
            .filter(|name| {
                matches!(
                    *name,
                    "memory.user.updated" | "memory.user.deleted" | "memory.profile.updated"
                )
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            [
                "memory.user.updated",
                "memory.user.updated",
                "memory.user.updated",
                "memory.user.updated",
                "memory.profile.updated",
                "memory.user.deleted"
            ]
        );
    }

    #[tokio::test]
    async fn mock_memory_scenarios_expose_quota_conflict_and_complete_profile() {
        let gateway = gateway().await;
        let quota = gateway
            .rpc("memory.user.create", json!({"content":"new preference"}))
            .await
            .unwrap_err();
        assert_eq!(quota.code, "quota_full");
        let quota_entries = quota
            .details
            .and_then(|details| details.get("entries").and_then(Value::as_array).cloned())
            .unwrap();
        assert!(quota_entries.len() >= 6);
        assert!(quota_entries.iter().all(|entry| {
            entry["scope"] == "user"
                && entry["kind"] == "user_preference"
                && entry["version"].as_u64().is_some()
        }));

        let conflict = gateway
            .rpc(
                "memory.user.update",
                json!({"entry_id":"mem_mock_conflict","content":"edited","expected_version":1}),
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, "conflict");
        assert_eq!(conflict.details.unwrap()["current"]["version"], 2);

        call(
            &gateway,
            "mock.set_memory_scenario",
            json!({"quota_full":false,"conflict_once":false}),
        )
        .await;
        let updated = call(
            &gateway,
            "memory.user.update",
            json!({"entry_id":"mem_mock_conflict","content":"edited","expected_version":2}),
        )
        .await;
        assert_eq!(updated["entry"]["version"], 3);

        let empty_profile = call(&gateway, "memory.profile.get", json!({})).await;
        assert_eq!(empty_profile["profile"]["onboarded"], false);
        let complete_profile = call(&gateway, "mock.use_complete_profile", json!({})).await;
        assert_eq!(complete_profile["profile"]["name"], "小明");
        assert_eq!(complete_profile["profile"]["language"], "zh-CN");
        assert_eq!(complete_profile["profile"]["identity"], "产品经理");
        assert_eq!(complete_profile["profile"]["onboarded"], true);
        assert!(complete_profile["profile"]["evidence"].is_object());
    }

    #[tokio::test]
    async fn mock_chat_locale_is_scoped_to_each_message_and_not_inherited() {
        let gateway_instance = gateway().await;
        let first = call(
            &gateway_instance,
            "chat.send",
            json!({"chat_id":"chat_main","text":"bonjour","client_locale":"fr-FR","client_request_id":"locale-fr"}),
        )
        .await;
        let second = call(
            &gateway_instance,
            "chat.send",
            json!({"chat_id":"chat_main","text":"hello","client_request_id":"locale-none"}),
        )
        .await;
        let first_id = first["message"]["id"].as_str().unwrap();
        let second_id = second["message"]["id"].as_str().unwrap();
        let state = gateway_instance.state.inner.read().await;
        assert_eq!(
            state.client_locales.get(first_id).map(String::as_str),
            Some("fr-FR")
        );
        assert!(!state.client_locales.contains_key(second_id));

        let separate_gateway = gateway().await;
        let separate = call(
            &separate_gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"hello"}),
        )
        .await;
        assert!(!separate_gateway
            .state
            .inner
            .read()
            .await
            .client_locales
            .contains_key(separate["message"]["id"].as_str().unwrap()));
    }

    #[tokio::test]
    async fn mock_bot_dm_trigger_emits_once_persists_and_is_host_scoped() {
        let gateway_instance = gateway().await;
        // Initialize fixtures before listening so this subscription observes
        // only the deliberately triggered acceptance message.
        call(&gateway_instance, "bootstrap", json!({})).await;
        let created = call(
            &gateway_instance,
            "bot.create",
            json!({"name":"Notification bot"}),
        )
        .await;
        let bot_id = created["bot"]["id"].as_str().unwrap();
        let chat_id = created["dm_chat"]["id"].as_str().unwrap();
        assert_eq!(created["dm_chat"]["kind"], "direct");
        let mut events = gateway_instance.state.events.subscribe();
        let params = json!({
            "bot_id":bot_id,
            "chat_id":chat_id,
            "text":"Notification acceptance message",
            "client_request_id":"notify-bot-dm-1"
        });
        let first = call(&gateway_instance, "mock.trigger_bot_dm", params.clone()).await;
        let message = first["message"].clone();
        assert_eq!(message["sender"]["kind"], "bot");
        assert_eq!(message["sender"]["bot_id"], bot_id);
        assert_eq!(message["chat_id"], chat_id);
        assert_eq!(message["fallback_text"], "Notification acceptance message");

        let mut observed = false;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Ok(event) = events.recv().await {
                if event["event"] == "message.created"
                    && event["data"]["message"]["id"] == message["id"]
                {
                    observed = true;
                    break;
                }
            }
        })
        .await
        .expect("triggered Bot DM event is broadcast");
        assert!(observed);

        let replay = call(&gateway_instance, "mock.trigger_bot_dm", params).await;
        assert_eq!(replay, first, "retry returns the original message");
        assert!(events.try_recv().is_err(), "retry does not broadcast again");
        let history = call(
            &gateway_instance,
            "chat.history",
            json!({"chat_id":chat_id,"limit":100}),
        )
        .await;
        assert_eq!(
            history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["id"] == message["id"])
                .count(),
            1
        );
        let chat =
            call(&gateway_instance, "chat.get", json!({"chat_id":chat_id})).await["chat"].clone();
        assert_eq!(chat["last_message"]["message_id"], message["id"]);
        assert_eq!(chat["last_seq"], message["seq"]);

        let other_host = gateway().await;
        call(&other_host, "bootstrap", json!({})).await;
        let other_bot = call(
            &other_host,
            "bot.create",
            json!({"name":"Notification bot"}),
        )
        .await;
        let other_bot_message = call(
            &other_host,
            "mock.trigger_bot_dm",
            json!({
                "bot_id":other_bot["bot"]["id"],
                "chat_id":other_bot["dm_chat"]["id"],
                "text":"Other Host message",
                "client_request_id":"notify-bot-dm-1"
            }),
        )
        .await;
        assert_ne!(other_bot_message["message"]["id"], message["id"]);
        let other_history = call(
            &other_host,
            "chat.history",
            json!({"chat_id":other_bot["dm_chat"]["id"],"limit":100}),
        )
        .await;
        assert!(other_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == other_bot_message["message"]["id"]));
        assert!(!other_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == message["id"]));
    }

    #[tokio::test]
    async fn mock_bot_dm_trigger_requires_valid_bot_chat_and_request_id() {
        let gateway = gateway().await;
        call(&gateway, "bootstrap", json!({})).await;
        let base_seq = gateway.state.inner.read().await.seq;
        let cases = [
            json!({"bot_id":"missing","chat_id":"chat_main","text":"hello","client_request_id":"invalid-bot"}),
            json!({"bot_id":"bot_main","chat_id":"missing","text":"hello","client_request_id":"invalid-chat"}),
            json!({"bot_id":"bot_main","chat_id":"chat_login","text":"hello","client_request_id":"wrong-chat"}),
            json!({"bot_id":"bot_main","chat_id":"chat_main","text":"hello"}),
        ];
        let expected = ["not_found", "not_found", "invalid_params", "invalid_params"];
        for (params, code) in cases.into_iter().zip(expected) {
            let error = gateway
                .rpc("mock.trigger_bot_dm", params)
                .await
                .unwrap_err();
            assert_eq!(error.code, code);
        }
        let state = gateway.state.inner.read().await;
        assert_eq!(state.seq, base_seq, "invalid calls emit no events");
        assert!(!state.messages.values().flatten().any(|message| {
            message["fallback_text"] == "hello"
                && message["sender"]["kind"] == "bot"
                && message["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("msg_"))
        }));
    }

    #[tokio::test]
    async fn mock_bot_group_trigger_is_member_scoped_idempotent_and_broadcast() {
        let gateway_instance = gateway().await;
        call(&gateway_instance, "bootstrap", json!({})).await;
        let project = call(
            &gateway_instance,
            "project.create",
            json!({"name":"Group notification test","member_bot_ids":["bot_code"]}),
        )
        .await;
        let chat_id = project["chat"]["id"].as_str().unwrap().to_owned();
        let mut events = gateway_instance.state.events.subscribe();
        let params = json!({
            "bot_id":"bot_code",
            "chat_id":chat_id,
            "text":"Group notification acceptance message",
            "client_request_id":"notify-bot-group-1"
        });
        let first = call(
            &gateway_instance,
            "mock.trigger_bot_group_message",
            params.clone(),
        )
        .await;
        let message = first["message"].clone();
        assert_eq!(message["sender"]["kind"], "bot");
        assert_eq!(message["sender"]["bot_id"], "bot_code");
        assert_eq!(message["chat_id"], chat_id);
        assert_eq!(
            message["fallback_text"],
            "Group notification acceptance message"
        );
        assert_eq!(message["seq"], 1);

        let observed = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let event = events.recv().await.unwrap();
                if event["event"] == "message.created"
                    && event["data"]["message"]["id"] == message["id"]
                {
                    break event;
                }
            }
        })
        .await
        .expect("triggered Bot group message should be broadcast");
        assert_eq!(observed["data"]["message"], message);

        let replay = call(&gateway_instance, "mock.trigger_bot_group_message", params).await;
        assert_eq!(replay, first, "retry returns the original message");
        assert!(events.try_recv().is_err(), "retry does not broadcast twice");
        let history = call(
            &gateway_instance,
            "chat.history",
            json!({"chat_id":chat_id,"limit":100}),
        )
        .await;
        assert_eq!(
            history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["id"] == message["id"])
                .count(),
            1
        );
        let chat =
            call(&gateway_instance, "chat.get", json!({"chat_id":chat_id})).await["chat"].clone();
        assert_eq!(chat["last_message"]["message_id"], message["id"]);
        assert_eq!(chat["last_seq"], message["seq"]);

        let other_host = gateway().await;
        call(&other_host, "bootstrap", json!({})).await;
        let other_project = call(
            &other_host,
            "project.create",
            json!({"name":"Other host group","member_bot_ids":["bot_code"]}),
        )
        .await;
        let other_chat_id = other_project["chat"]["id"].as_str().unwrap();
        let other_message = call(
            &other_host,
            "mock.trigger_bot_group_message",
            json!({
                "bot_id":"bot_code",
                "chat_id":other_chat_id,
                "text":"Other Host message",
                "client_request_id":"notify-bot-group-1"
            }),
        )
        .await;
        assert_ne!(other_message["message"]["id"], message["id"]);
        let other_history = call(
            &other_host,
            "chat.history",
            json!({"chat_id":other_chat_id,"limit":100}),
        )
        .await;
        assert!(other_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == other_message["message"]["id"]));
        assert!(!other_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == message["id"]));
    }

    #[tokio::test]
    async fn mock_bot_group_trigger_rejects_non_members_and_non_project_chats() {
        let gateway = gateway().await;
        call(&gateway, "bootstrap", json!({})).await;
        let project = call(
            &gateway,
            "project.create",
            json!({"name":"Group permission test","member_bot_ids":["bot_product"]}),
        )
        .await;
        let chat_id = project["chat"]["id"].as_str().unwrap();
        let base_seq = gateway.state.inner.read().await.seq;
        let cases = [
            (
                json!({"bot_id":"bot_code","chat_id":chat_id,"text":"not a member","client_request_id":"group-not-member"}),
                "invalid_params",
            ),
            (
                json!({"bot_id":"bot_main","chat_id":"chat_main","text":"not a group","client_request_id":"group-not-group"}),
                "invalid_params",
            ),
            (
                json!({"bot_id":"missing","chat_id":chat_id,"text":"unknown bot","client_request_id":"group-missing-bot"}),
                "not_found",
            ),
            (
                json!({"bot_id":"bot_product","chat_id":"missing","text":"unknown chat","client_request_id":"group-missing-chat"}),
                "not_found",
            ),
            (
                json!({"bot_id":"bot_product","chat_id":chat_id,"text":"missing request id"}),
                "invalid_params",
            ),
        ];
        for (params, expected_code) in cases {
            let error = gateway
                .rpc("mock.trigger_bot_group_message", params)
                .await
                .unwrap_err();
            assert_eq!(error.code, expected_code);
        }
        let state = gateway.state.inner.read().await;
        assert_eq!(state.seq, base_seq, "invalid requests emit no events");
        assert!(!state
            .messages
            .get(chat_id)
            .into_iter()
            .flatten()
            .any(|message| {
                message["fallback_text"] == "not a member"
                    || message["fallback_text"] == "unknown bot"
            }));
    }

    #[tokio::test]
    async fn mock_clear_main_chat_is_host_scoped_idempotent_and_stable_on_reconnect() {
        let gateway_instance = gateway().await;
        call(&gateway_instance, "bootstrap", json!({})).await;
        let main_before = call(
            &gateway_instance,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(!main_before["messages"].as_array().unwrap().is_empty());
        let prior_chat = call(
            &gateway_instance,
            "chat.get",
            json!({"chat_id":"chat_main"}),
        )
        .await["chat"]
            .clone();
        let prior_high_water = prior_chat["last_seq"].as_u64().unwrap();
        assert!(
            main_before["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|message| message["seq"].as_u64())
                .max()
                .unwrap_or(0)
                <= prior_high_water
        );
        let login_before = call(
            &gateway_instance,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        let login_count = login_before["messages"].as_array().unwrap().len();

        let mut events = gateway_instance.state.events.subscribe();
        let params = json!({
            "chat_id":"chat_main",
            "client_request_id":"clear-main-for-empty-state"
        });
        let cleared = call(&gateway_instance, "mock.clear_main_chat", params.clone()).await;
        assert_eq!(cleared["cleared"], true);
        assert_eq!(cleared["chat"]["id"], "chat_main");
        assert!(cleared["chat"]["last_message"].is_null());
        assert_eq!(cleared["chat"]["last_seq"], prior_high_water);

        let mut deleted_ids = Vec::new();
        let mut chat_updated = false;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Ok(event) = events.recv().await {
                match event["event"].as_str() {
                    Some("message.deleted") if event["data"]["chat_id"] == "chat_main" => {
                        deleted_ids.push(event["data"]["message_id"].as_str().unwrap().to_owned());
                    }
                    Some("chat.updated") if event["data"]["chat"]["id"] == "chat_main" => {
                        chat_updated = true;
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("main-chat clearing events should be broadcast");
        assert_eq!(
            deleted_ids.len(),
            main_before["messages"].as_array().unwrap().len()
        );
        assert!(chat_updated);

        let retry = call(&gateway_instance, "mock.clear_main_chat", params).await;
        assert_eq!(retry, cleared, "retry returns the original clear result");
        assert!(events.try_recv().is_err(), "retry does not broadcast twice");

        // A fresh client bootstrap and history read must not reseed the empty
        // main conversation for this mock Host.
        call(&gateway_instance, "bootstrap", json!({})).await;
        let main_after = call(
            &gateway_instance,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(main_after["messages"].as_array().unwrap().is_empty());
        let next_message = call(
            &gateway_instance,
            "chat.send",
            json!({"chat_id":"chat_main","text":"after clear","client_request_id":"send-after-clear"}),
        )
        .await;
        assert!(next_message["message"]["seq"].as_u64().unwrap() > prior_high_water);
        let login_after = call(
            &gateway_instance,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        assert_eq!(
            login_after["messages"].as_array().unwrap().len(),
            login_count
        );

        let other_host = gateway().await;
        call(&other_host, "bootstrap", json!({})).await;
        let other_main = call(
            &other_host,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(!other_main["messages"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn mock_clear_main_chat_rejects_other_chats_and_missing_request_id() {
        let gateway_instance = gateway().await;
        call(&gateway_instance, "bootstrap", json!({})).await;
        let base_seq = gateway_instance.state.inner.read().await.seq;
        let wrong_chat = gateway_instance
            .rpc(
                "mock.clear_main_chat",
                json!({"chat_id":"chat_login","client_request_id":"clear-not-main"}),
            )
            .await
            .unwrap_err();
        assert_eq!(wrong_chat.code, "invalid_params");
        let missing_request = gateway_instance
            .rpc("mock.clear_main_chat", json!({"chat_id":"chat_main"}))
            .await
            .unwrap_err();
        assert_eq!(missing_request.code, "invalid_params");
        let state = gateway_instance.state.inner.read().await;
        assert_eq!(state.seq, base_seq, "invalid calls emit no events");
        assert!(state
            .messages
            .get("chat_main")
            .is_some_and(|messages| !messages.is_empty()));
    }

    #[tokio::test]
    async fn mock_explicit_skill_loads_are_durable_per_bot_and_replay_with_history() {
        let gateway = gateway().await;
        call(
            &gateway,
            "skill.create",
            json!({"name":"slash-proof","content":"---\nname: slash-proof\ndescription: Proof skill\n---\nUse this skill."}),
        )
        .await;
        let bots = call(&gateway, "bot.list", json!({"include_hidden":true})).await;
        let bot_ids = bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|bot| bot["id"].as_str())
            .take(2)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(bot_ids.len(), 2, "fixture must include two Bots");

        let mut live_events = gateway.state.events.subscribe();
        let sent = call(
            &gateway,
            "chat.send",
            json!({
                "chat_id":"chat_login",
                "text":"/slash-proof use the shared checklist",
                "mentions":[
                    {"kind":"bot","bot_id":bot_ids[0]},
                    {"kind":"bot","bot_id":bot_ids[1]}
                ],
                "client_request_id":"mock-loaded-skill-two-bots"
            }),
        )
        .await;
        let message = sent["message"].clone();
        assert_eq!(message["loaded_skills"].as_array().unwrap().len(), 2);
        assert!(message["loaded_skills"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| {
                record["name"] == "slash-proof"
                    && record["assignment_id"].is_null()
                    && record["loaded_at"].is_string()
            }));
        let recorded_bots = message["loaded_skills"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|record| record["bot_id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(recorded_bots.len(), 2);

        let mut saw_updated = false;
        for _ in 0..3 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), live_events.recv())
                .await
                .expect("send events should be broadcast")
                .unwrap();
            if event["event"] == "message.updated"
                && event["data"]["message"]["id"] == message["id"]
            {
                saw_updated = true;
                assert_eq!(
                    event["data"]["message"]["loaded_skills"],
                    message["loaded_skills"]
                );
            }
        }
        assert!(
            saw_updated,
            "successful skill admission emits message.updated"
        );

        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        let restored = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == message["id"])
            .expect("history restores the updated source message");
        assert_eq!(restored["loaded_skills"], message["loaded_skills"]);
        let replay_events = gateway
            .state
            .inner
            .read()
            .await
            .events
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assert!(replay_events.iter().any(|event| {
            event["event"] == "message.updated"
                && event["data"]["message"]["id"] == message["id"]
                && event["data"]["message"]["loaded_skills"] == message["loaded_skills"]
        }));
        let resume = call(&gateway, "session.resume", json!({"last_seq":0})).await;
        assert_eq!(resume["mode"], "replay");
    }

    #[tokio::test]
    async fn mock_unknown_or_bot_disabled_explicit_skill_is_not_recorded() {
        let gateway = gateway().await;
        call(
            &gateway,
            "skill.create",
            json!({"name":"limited-skill","content":"---\nname: limited-skill\ndescription: Limited\n---\nBody"}),
        )
        .await;
        let bots = call(&gateway, "bot.list", json!({"include_hidden":true})).await;
        let bot_id = bots["bots"][0]["id"].as_str().unwrap();
        call(
            &gateway,
            "skill.set_enabled",
            json!({"name":"limited-skill","enabled":false,"bot_id":bot_id}),
        )
        .await;
        let mut events = gateway.state.events.subscribe();
        let missing = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"/missing-skill continue","mentions":[],"client_request_id":"mock-missing-skill"}),
        )
        .await["message"]
            .clone();
        let disabled = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"/limited-skill continue","mentions":[],"client_request_id":"mock-disabled-skill"}),
        )
        .await["message"]
            .clone();
        assert_eq!(missing["loaded_skills"], json!([]));
        assert_eq!(disabled["loaded_skills"], json!([]));
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        for id in [
            missing["id"].as_str().unwrap(),
            disabled["id"].as_str().unwrap(),
        ] {
            let stored = history["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"].as_str() == Some(id))
                .unwrap();
            assert_eq!(stored["loaded_skills"], json!([]));
        }
        while let Ok(event) = events.try_recv() {
            assert!(
                !(event["event"] == "message.updated"
                    && [
                        missing["id"].as_str().unwrap(),
                        disabled["id"].as_str().unwrap()
                    ]
                    .contains(&event["data"]["message"]["id"].as_str().unwrap_or("")))
            );
        }
    }

    #[tokio::test]
    async fn mock_chat_send_resolves_quotes_and_reply_mentions() {
        let gateway = gateway().await;
        call(&gateway, "bootstrap", json!({})).await;
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        let bot_message = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["sender"]["kind"] == "bot")
            .expect("bootstrap seeds the main bot onboarding greeting");
        let bot_message_id = bot_message["id"].as_str().unwrap();
        let bot_excerpt = bot_message["fallback_text"].as_str().unwrap();

        let reply = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"好的","quote":bot_message_id}),
        )
        .await;
        assert_eq!(reply["message"]["reply_to"], Value::Null);
        assert_eq!(reply["message"]["quote"]["message_id"], bot_message_id);
        assert_eq!(reply["message"]["quote"]["excerpt"], bot_excerpt);
        assert!(reply["message"]["mentions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mention| mention["bot_id"] == "bot_main"));

        let reference = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"另作备注","quote":bot_message_id,"quote_mode":"reference"}),
        )
        .await;
        assert_eq!(reference["message"]["quote"]["message_id"], bot_message_id);
        assert!(!reference["message"]["mentions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mention| mention["bot_id"] == "bot_main"));

        let cross_chat = gateway
            .rpc(
                "chat.send",
                json!({"chat_id":"chat_login","text":"跨群引用","quote":bot_message_id}),
            )
            .await
            .unwrap_err();
        assert_eq!(cross_chat.code, "not_found");
        let invalid_mode = gateway
            .rpc(
                "chat.send",
                json!({"chat_id":"chat_main","text":"错误模式","quote":bot_message_id,"quote_mode":"thread"}),
            )
            .await
            .unwrap_err();
        assert_eq!(invalid_mode.code, "invalid_argument");
    }

    #[tokio::test]
    async fn mock_chat_send_persists_uploaded_file_blocks_including_attachment_only() {
        let gateway = gateway().await;
        let uploads = gateway.state.home.join("data/uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        std::fs::write(
            uploads.join("upl_mock-note.json"),
            serde_json::to_vec(&json!({
                "root":"upload",
                "root_id":"upl_mock-note",
                "path":"",
                "name":"notes.txt",
                "size":42,
                "mime":"text/plain"
            }))
            .unwrap(),
        )
        .unwrap();

        let with_text = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"请看附件","attachments":["upl_mock-note"]}),
        )
        .await;
        assert_eq!(with_text["message"]["blocks"][0]["markdown"], "请看附件");
        assert_eq!(with_text["message"]["blocks"][1]["type"], "file");
        assert_eq!(
            with_text["message"]["blocks"][1]["file"]["root_id"],
            "upl_mock-note"
        );
        assert_eq!(
            with_text["message"]["blocks"][1]["file"]["name"],
            "notes.txt"
        );

        let attachment_only = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"","attachments":["upl_mock-note"]}),
        )
        .await;
        assert_eq!(attachment_only["message"]["fallback_text"], "notes.txt");
        assert_eq!(
            attachment_only["message"]["blocks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(attachment_only["message"]["blocks"][0]["type"], "file");

        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        let persisted = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == attachment_only["message"]["id"])
            .unwrap();
        assert_eq!(persisted["fallback_text"], "notes.txt");
        assert_eq!(persisted["blocks"][0]["file"]["mime"], "text/plain");
    }

    #[tokio::test]
    async fn mock_project_and_assignment_creation_emit_cards_for_rpc_navigation() {
        let gateway = gateway().await;
        let created_project = call(
            &gateway,
            "project.create",
            json!({"name":"卡片 RPC 示例","goal":"验证 project_card","member_bot_ids":["bot_code"]}),
        )
        .await;
        let project_id = created_project["project"]["id"].clone();
        let main = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(main["messages"].as_array().unwrap().iter().any(|message| {
            message["blocks"][0]["type"] == "project_card"
                && message["blocks"][0]["project_id"] == project_id
        }));
        let target_project_chat = created_project["chat"]["id"].clone();

        let task = call(
            &gateway,
            "assignment.create",
            json!({
                "bot_id":"bot_code","project_id":project_id,"title":"验证任务卡",
                "instruction":"点任务卡打开详情"
            }),
        )
        .await;
        let _: macbot_protocol::Assignment = serde_json::from_value(task.clone()).unwrap();
        let task_id = task["id"].clone();
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":target_project_chat,"limit":100}),
        )
        .await;
        assert!(history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["blocks"][0]["type"] == "task_card"
                    && message["blocks"][0]["assignment_id"] == task_id
            }));
        assert_eq!(
            call(&gateway, "assignment.get", json!({"assignment_id":task_id}),).await["assignment"]
                ["id"],
            task_id
        );

        let delegation = call(
            &gateway,
            "delegate",
            json!({"bot_id":"bot_product","title":"单 Bot 调研","instruction":"调研登录模式"}),
        )
        .await;
        let main_after = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","limit":100}),
        )
        .await;
        assert!(main_after["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["blocks"][0]["type"] == "delegation"
                    && message["blocks"][0]["assignment_id"] == delegation["id"]
            }));
    }

    #[tokio::test]
    async fn mock_assignment_create_and_retry_include_null_recovered_at_in_events() {
        let gateway = gateway().await;
        let mut events = gateway.state.events.subscribe();
        let created = call(
            &gateway,
            "assignment.create",
            json!({
                "bot_id":"bot_code",
                "title":"recovery timestamp contract",
                "instruction":"check null default"
            }),
        )
        .await;
        let assignment_id = created["id"].as_str().unwrap().to_owned();
        assert!(created["recovered_at"].is_null());
        let created_event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let event = events.recv().await.unwrap();
                if event["event"] == "assignment.created"
                    && event["data"]["assignment"]["id"] == assignment_id
                {
                    break event;
                }
            }
        })
        .await
        .expect("assignment.created should be broadcast");
        assert!(created_event["data"]["assignment"]["recovered_at"].is_null());

        {
            let mut state = gateway.state.inner.write().await;
            let assignment = state
                .assignments
                .iter_mut()
                .find(|assignment| assignment["id"] == assignment_id)
                .unwrap();
            assignment["status"] = json!("failed");
        }
        let mut events = gateway.state.events.subscribe();
        let retried = call(
            &gateway,
            "assignment.retry",
            json!({"assignment_id":assignment_id,"client_request_id":"retry-recovered-at"}),
        )
        .await;
        let retry = retried["assignment"].clone();
        let retry_id = retry["id"].as_str().unwrap().to_owned();
        assert!(retry["recovered_at"].is_null());
        let retry_event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let event = events.recv().await.unwrap();
                if event["event"] == "assignment.created"
                    && event["data"]["assignment"]["id"] == retry_id
                {
                    break event;
                }
            }
        })
        .await
        .expect("retried assignment.created should be broadcast");
        assert!(retry_event["data"]["assignment"]["recovered_at"].is_null());
    }

    #[tokio::test]
    async fn mock_artifact_history_supports_version_list_paging_and_content_reads() {
        let gateway = gateway().await;
        let latest = call(
            &gateway,
            "artifact.versions.list",
            json!({"artifact_id":"art_prd"}),
        )
        .await;
        let _: macbot_protocol::ArtifactVersionsResult =
            serde_json::from_value(latest.clone()).unwrap();
        assert_eq!(latest["has_more"], false);
        assert_eq!(latest["versions"][0]["version"], 2);
        assert_eq!(latest["versions"][1]["version"], 1);

        let page = call(
            &gateway,
            "artifact.versions.list",
            json!({"artifact_id":"art_prd","limit":1}),
        )
        .await;
        assert_eq!(page["versions"].as_array().unwrap().len(), 1);
        assert_eq!(page["versions"][0]["version"], 2);
        assert_eq!(page["has_more"], true);
        let earlier = call(
            &gateway,
            "artifact.versions.list",
            json!({"artifact_id":"art_prd","before_version":2}),
        )
        .await;
        assert_eq!(earlier["versions"][0]["version"], 1);

        let content = call(
            &gateway,
            "artifact.version.get",
            json!({"artifact_id":"art_prd","version":1}),
        )
        .await;
        let _: macbot_protocol::ArtifactVersionGetResult =
            serde_json::from_value(content.clone()).unwrap();
        assert_eq!(content["content_encoding"], "utf8");
        assert!(content["content"].as_str().unwrap().contains("版本 1"));

        for (artifact_id, relative_path, title) in [
            ("art_mock_brief", "docs/brief.md", "项目说明"),
            ("art_mock_report", "reports/acceptance.md", "验收报告"),
        ] {
            let history = call(
                &gateway,
                "artifact.versions.list",
                json!({"artifact_id":artifact_id,"limit":20}),
            )
            .await;
            let history: macbot_protocol::ArtifactVersionsResult =
                serde_json::from_value(history).unwrap();
            assert_eq!(history.versions.len(), 1);
            assert_eq!(history.versions[0].artifact_id, artifact_id);
            assert_eq!(history.versions[0].version, 1);
            assert_eq!(history.versions[0].title, title);
            assert_eq!(
                history.versions[0].kind,
                macbot_protocol::ArtifactKind::File
            );

            let current = call(
                &gateway,
                "artifact.version.get",
                json!({"artifact_id":artifact_id,"version":1}),
            )
            .await;
            let current: macbot_protocol::ArtifactVersionGetResult =
                serde_json::from_value(current).unwrap();
            let expected = std::fs::read_to_string(
                gateway
                    .state
                    .home
                    .join("projects/mock-review")
                    .join(relative_path),
            )
            .unwrap();
            assert_eq!(current.content, expected);
            assert_eq!(current.version.byte_size as usize, expected.len());

            let repeated = call(
                &gateway,
                "artifact.versions.list",
                json!({"artifact_id":artifact_id,"limit":20}),
            )
            .await;
            assert_eq!(repeated["versions"].as_array().unwrap().len(), 1);
        }

        let no_history = call(
            &gateway,
            "artifact.versions.list",
            json!({"artifact_id":"art_code"}),
        )
        .await;
        assert_eq!(no_history["versions"].as_array().unwrap().len(), 0);
        assert_eq!(no_history["has_more"], false);
        let missing = gateway
            .rpc("artifact.versions.list", json!({"artifact_id":"missing"}))
            .await
            .unwrap_err();
        assert_eq!(missing.code, "not_found");
    }

    #[tokio::test]
    async fn mock_question_history_lists_multiple_answered_and_expired_questions() {
        let gateway = gateway().await;
        let initial = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_test","assignment_id":"asg_mock_questions"}),
        )
        .await;
        let seeded = initial["questions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|question| question["state"] == "pending")
            .expect("mock includes one pending question")
            .clone();
        let scope = json!({
            "bot_id":seeded["bot_id"],
            "assignment_id":seeded["assignment_id"],
            "chat_id":seeded["chat_id"]
        });
        let first = call(&gateway, "question.ask", json!({
            "bot_id":scope["bot_id"],"assignment_id":scope["assignment_id"],"chat_id":scope["chat_id"],
            "text":"选择部署区","options":["测试","生产"],"multi":false,"allow_free_text":true
        })).await["question"].clone();
        let second = call(&gateway, "question.ask", json!({
            "bot_id":scope["bot_id"],"assignment_id":scope["assignment_id"],"chat_id":scope["chat_id"],
            "text":"选择通知方式","options":["站内","邮件"],"multi":true,"allow_free_text":false
        })).await["question"].clone();
        call(
            &gateway,
            "question.answer",
            json!({
                "question_id":first["id"],"option_index":1,"option_indexes":[1],"text":null
            }),
        )
        .await;
        call(
            &gateway,
            "assignment.stop",
            json!({"assignment_id":scope["assignment_id"]}),
        )
        .await;

        let all = call(
            &gateway,
            "question.list",
            json!({"chat_id":scope["chat_id"],"assignment_id":scope["assignment_id"]}),
        )
        .await;
        let questions = all["questions"].as_array().unwrap();
        assert_eq!(questions.len(), 5);
        assert!(questions
            .iter()
            .any(|question| question["id"] == first["id"] && question["state"] == "answered"));
        assert!(questions
            .iter()
            .any(|question| question["id"] == second["id"] && question["state"] == "expired"));
        assert!(questions
            .iter()
            .any(|question| question["id"] == seeded["id"] && question["state"] == "expired"));
        let answered = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_test","state":["answered"]}),
        )
        .await;
        assert_eq!(answered["questions"].as_array().unwrap().len(), 2);
        let expired = call(
            &gateway,
            "question.list",
            json!({"assignment_id":"asg_mock_questions","state":["expired"]}),
        )
        .await;
        assert_eq!(expired["questions"].as_array().unwrap().len(), 3);
        let _: Vec<macbot_protocol::Question> =
            serde_json::from_value(all["questions"].clone()).unwrap();
    }

    #[tokio::test]
    async fn mock_question_list_requires_a_scope_and_history_cards_are_hydratable() {
        let gateway = gateway().await;
        let error = gateway.rpc("question.list", json!({})).await.unwrap_err();
        assert_eq!(error.code, "invalid_params");

        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_test","limit":100}),
        )
        .await;
        let message = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == "msg_mock_question_history")
            .expect("mock chat history contains a question deck");
        let ids = message["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block.get("question_id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "q_mock_history_answered",
                "q_mock_history_expired",
                "q_mock_history_pending"
            ]
        );
        let questions = call(&gateway, "question.list", json!({"chat_id":"chat_test"})).await;
        for id in ids {
            assert!(questions["questions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|question| question["id"] == id));
        }

        let dm = "chat_test";
        let approval_history =
            call(&gateway, "chat.history", json!({"chat_id":dm,"limit":100})).await;
        assert!(approval_history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["id"] == "msg_mock_approval_history"
                    && message["blocks"][0]["approval_id"] == "apr_mock_history"
            }));
        let approvals = call(&gateway, "approval.list", json!({})).await;
        assert!(approvals["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|approval| {
                approval["id"] == "apr_mock_history"
                    && approval["matched_rule_text"] == "git push 先问我"
                    && approval["always_allow_supported"].as_bool() == Some(false)
            }));
    }

    #[tokio::test]
    async fn mock_login_history_has_hydratable_questions_in_all_protocol_states() {
        let gateway = gateway().await;
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        let message = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["id"] == "msg_mock_login_questions")
            .expect("login group has a question-card history message");
        let ids = message["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block.get("question_id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "q_mock_login_pending",
                "q_mock_login_answered",
                "q_mock_login_expired"
            ]
        );

        let listed = call(&gateway, "question.list", json!({"chat_id":"chat_login"})).await;
        let questions = listed["questions"].as_array().unwrap();
        assert_eq!(questions.len(), 8);
        for (id, expected_state) in [
            ("q_mock_login_pending", "pending"),
            ("q_mock_login_answered", "answered"),
            ("q_mock_login_expired", "expired"),
        ] {
            let question = questions
                .iter()
                .find(|question| question["id"] == id)
                .expect("every history question id is hydrated by question.list");
            assert_eq!(question["state"], expected_state);
            let _: macbot_protocol::Question = serde_json::from_value(question.clone()).unwrap();
        }

        let expired = gateway
            .rpc(
                "question.answer",
                json!({"question_id":"q_mock_login_expired","option_index":0}),
            )
            .await
            .unwrap_err();
        assert_eq!(expired.code, "conflict");
        let still_expired = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_login","state":["expired"]}),
        )
        .await;
        assert_eq!(still_expired["questions"].as_array().unwrap().len(), 1);

        let answered = call(
            &gateway,
            "question.answer",
            json!({"question_id":"q_mock_login_pending","option_index":1}),
        )
        .await;
        assert_eq!(answered["question"]["state"], "answered");
        let answered_list = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_login","state":["answered"]}),
        )
        .await;
        assert_eq!(answered_list["questions"].as_array().unwrap().len(), 4);

        let projects = call(&gateway, "project.list", json!({})).await;
        assert!(projects["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|project| project["id"] == "prj_login" && project["status"] == "done"));
    }

    #[tokio::test]
    async fn mock_login_question_flow_answers_independently_and_preserves_other_state() {
        let gateway = gateway().await;
        let flow_ids = [
            "q_mock_login_flow_1",
            "q_mock_login_flow_2",
            "q_mock_login_flow_3",
        ];
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        let messages = history["messages"].as_array().unwrap();
        let flow = messages
            .iter()
            .find(|message| message["id"] == "msg_mock_login_question_flow")
            .expect("login group includes an independent three-question operation card");
        let ids = flow["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block.get("question_id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(ids, flow_ids);
        for (message_id, question_id) in [
            (
                "msg_mock_login_single_browsers",
                "q_mock_login_single_browsers",
            ),
            (
                "msg_mock_login_single_remember",
                "q_mock_login_single_remember",
            ),
        ] {
            let message = messages
                .iter()
                .find(|message| message["id"] == message_id)
                .expect("prototype single-question history card is present");
            assert_eq!(message["blocks"].as_array().unwrap().len(), 1);
            assert_eq!(message["blocks"][0]["question_id"], question_id);
        }

        let initial = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_login","assignment_id":"asg_mock_waiting"}),
        )
        .await;
        let questions = initial["questions"].as_array().unwrap();
        for id in flow_ids {
            assert_eq!(
                questions
                    .iter()
                    .find(|question| question["id"] == id)
                    .expect("flow question is available through question.list")["state"],
                "pending"
            );
        }

        let before_assignment = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_waiting"}),
        )
        .await;
        let before_project = call(&gateway, "project.get", json!({"project_id":"prj_login"})).await;
        let before_approvals = call(&gateway, "approval.list", json!({})).await;

        let first = call(
            &gateway,
            "question.answer",
            json!({"question_id":flow_ids[0],"option_index":1}),
        )
        .await;
        assert_eq!(first["question"]["state"], "answered");
        let partial = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_login","assignment_id":"asg_mock_waiting"}),
        )
        .await;
        let partial = partial["questions"].as_array().unwrap();
        assert_eq!(
            partial
                .iter()
                .filter(|question| flow_ids.contains(&question["id"].as_str().unwrap_or("")))
                .filter(|question| question["state"] == "answered")
                .count(),
            1
        );
        assert!(flow_ids[1..].iter().all(|id| partial
            .iter()
            .any(|question| question["id"] == *id && question["state"] == "pending")));

        call(
            &gateway,
            "question.answer",
            json!({"question_id":flow_ids[1],"text":"用 hi@app.com，已经配好 SPF"}),
        )
        .await;
        call(
            &gateway,
            "question.answer",
            json!({"question_id":flow_ids[2],"option_indexes":[0,1]}),
        )
        .await;
        let completed = call(
            &gateway,
            "question.list",
            json!({"chat_id":"chat_login","assignment_id":"asg_mock_waiting"}),
        )
        .await;
        let completed = completed["questions"].as_array().unwrap();
        for id in flow_ids {
            assert!(completed
                .iter()
                .any(|question| question["id"] == id && question["state"] == "answered"));
        }
        let history_after = call(&gateway, "question.list", json!({"chat_id":"chat_login"})).await;
        for (id, state) in [
            ("q_mock_login_pending", "pending"),
            ("q_mock_login_answered", "answered"),
            ("q_mock_login_expired", "expired"),
            ("q_mock_login_single_browsers", "answered"),
            ("q_mock_login_single_remember", "answered"),
        ] {
            assert!(history_after["questions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|question| question["id"] == id && question["state"] == state));
        }

        assert_eq!(
            call(
                &gateway,
                "assignment.get",
                json!({"assignment_id":"asg_mock_waiting"}),
            )
            .await,
            before_assignment,
            "answering the independent deck leaves existing queue assignment state untouched"
        );
        assert_eq!(
            call(&gateway, "project.get", json!({"project_id":"prj_login"}),).await,
            before_project,
            "question answers do not change project state"
        );
        assert_eq!(
            call(&gateway, "approval.list", json!({})).await,
            before_approvals,
            "question answers do not change approval state"
        );
    }

    #[tokio::test]
    async fn mock_code_bot_private_chat_has_conversation_project_and_approval_cards() {
        let gateway = gateway().await;
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_code","limit":100}),
        )
        .await;
        let messages = history["messages"].as_array().unwrap();
        assert!(messages.iter().any(|message| {
            message["id"] == "msg_mock_code_dm_user" && message["sender"]["kind"] == "user"
        }));
        assert!(messages.iter().any(|message| {
            message["id"] == "msg_mock_code_dm_reply"
                && message["blocks"].as_array().unwrap().iter().any(|block| {
                    block["type"] == "project_card" && block["project_id"] == "prj_login"
                })
        }));
        assert!(messages.iter().any(|message| {
            message["id"] == "msg_mock_code_dm_approval"
                && message["blocks"].as_array().unwrap().iter().any(|block| {
                    block["type"] == "approval" && block["approval_id"] == "apr_mock_pending"
                })
        }));
        let _: Vec<macbot_protocol::Message> =
            serde_json::from_value(history["messages"].clone()).unwrap();
        let chat = call(&gateway, "chat.get", json!({"chat_id":"chat_code"})).await;
        assert_eq!(chat["chat"]["kind"], "direct");
        assert_eq!(
            chat["chat"]["last_message"]["message_id"],
            "msg_mock_code_dm_approval"
        );
    }

    #[tokio::test]
    async fn mock_approval_keeps_matched_rule_text_snapshot() {
        let gateway = gateway().await;
        let created = call(&gateway, "approval.request", json!({
            "bot_id":"main","assignment_id":null,"chat_id":"chat_main","tool":"bash","risk":"exec",
            "summary":"执行发布命令","detail":"git push","matched_rule_text":"git push 先问我"
        })).await["approval"].clone();
        let approvals = call(&gateway, "approval.list", json!({})).await;
        let stored = approvals["approvals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == created["id"])
            .unwrap();
        assert_eq!(stored["matched_rule_text"], "git push 先问我");
        let _: macbot_protocol::Approval = serde_json::from_value(stored.clone()).unwrap();
    }

    #[tokio::test]
    async fn mock_mutations_return_protocol_objects() {
        let gateway = gateway().await;

        let bootstrap = call(&gateway, "bootstrap", json!({})).await;
        for bot in bootstrap["bots"].as_array().unwrap() {
            let _: Bot = serde_json::from_value(bot.clone()).unwrap();
        }
        for chat in bootstrap["chats"].as_array().unwrap() {
            let _: Chat = serde_json::from_value(chat.clone()).unwrap();
        }
        for project in bootstrap["projects"].as_array().unwrap() {
            let _: Project = serde_json::from_value(project.clone()).unwrap();
        }
        let _: Settings = serde_json::from_value(bootstrap["settings"].clone()).unwrap();

        let providers = call(&gateway, "provider.list", json!({})).await;
        for provider in providers["providers"].as_array().unwrap() {
            let _: Provider = serde_json::from_value(provider.clone()).unwrap();
        }
        for model in providers["models"].as_array().unwrap() {
            let _: Model = serde_json::from_value(model.clone()).unwrap();
        }
        let skills = call(&gateway, "skill.list", json!({})).await;
        for skill in skills["skills"].as_array().unwrap() {
            assert!(skill.get("last_invoked_at").is_some());
            let _: Skill = serde_json::from_value(skill.clone()).unwrap();
        }

        let device = call(
            &gateway,
            "device.register",
            json!({"device_id":"dev-1","platform":"macos","app_version":"1","device_name":"test","push_token":null}),
        )
        .await;
        let _: Device = serde_json::from_value(device["device"].clone()).unwrap();

        let bot = call(&gateway, "bot.create", json!({"name":"Worker"})).await;
        let _: Bot = serde_json::from_value(bot["bot"].clone()).unwrap();
        let _: Chat = serde_json::from_value(bot["dm_chat"].clone()).unwrap();

        let message = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"hello","mentions":[]}),
        )
        .await;
        let message: Message = serde_json::from_value(message["message"].clone()).unwrap();
        let reacted = call(
            &gateway,
            "chat.react",
            json!({"message_id":message.id,"emoji":"👍","on":true}),
        )
        .await;
        let _: Message = serde_json::from_value(reacted["message"].clone()).unwrap();

        let project = call(
            &gateway,
            "project.create",
            json!({"name":"Demo","goal":"Ship","member_bot_ids":["bot_code"]}),
        )
        .await;
        let project_value: Project = serde_json::from_value(project["project"].clone()).unwrap();
        let _: Chat = serde_json::from_value(project["chat"].clone()).unwrap();
        let detail = call(
            &gateway,
            "project.get",
            json!({"project_id":project_value.id}),
        )
        .await;
        let _: Project = serde_json::from_value(detail["project"].clone()).unwrap();
        let _: Announcement = serde_json::from_value(detail["announcement"].clone()).unwrap();

        let skill = call(
            &gateway,
            "skill.create",
            json!({"name":"demo-skill","content":"---\nname: demo-skill\ndescription: Demo\n---\nUse it."}),
        )
        .await;
        let _: Skill = serde_json::from_value(skill["skill"].clone()).unwrap();
        let detail = call(&gateway, "skill.get", json!({"name":"demo-skill"})).await;
        let _: SkillDetail = serde_json::from_value(detail["skill"].clone()).unwrap();

        let routine = call(
            &gateway,
            "routine.create",
            json!({"bot_id":"bot_main","name":"Daily","instructions":"check","schedules":[{"cron":"0 9 * * *","label":"morning"}]}),
        )
        .await;
        let routine_value: Routine = serde_json::from_value(routine["routine"].clone()).unwrap();
        let run = call(
            &gateway,
            "routine.test_run",
            json!({"routine_id":routine_value.id}),
        )
        .await;
        let _: RoutineRun = serde_json::from_value(run["run"].clone()).unwrap();

        let provider = call(
            &gateway,
            "provider.create",
            json!({"name":"Mock","api_kind":"openai-completions","base_url":"http://mock"}),
        )
        .await;
        let provider_value: Provider =
            serde_json::from_value(provider["provider"].clone()).unwrap();
        let model = call(
            &gateway,
            "model.upsert",
            json!({"provider_id":provider_value.id,"model_id":"mock-model"}),
        )
        .await;
        let _: Model = serde_json::from_value(model["model"].clone()).unwrap();
    }

    #[tokio::test]
    async fn mock_skill_examples_support_design_screens_and_stay_deleted() {
        let gateway = gateway().await;
        let listed = call(&gateway, "skill.list", json!({})).await;
        let skills = listed["skills"].as_array().unwrap();
        for skill in skills {
            let _: Skill = serde_json::from_value(skill.clone()).unwrap();
        }

        let web_auth = skills
            .iter()
            .find(|skill| skill["name"] == "web-auth-patterns")
            .unwrap();
        assert_eq!(web_auth["source"], "user");
        assert_eq!(web_auth["invocations_7d"]["total"], 14);
        assert_eq!(
            web_auth["invocations_7d"]["by_bot"][0]["bot_id"],
            "bot_code"
        );
        assert_eq!(
            web_auth["invocations_7d"]["by_bot"][1]["bot_id"],
            "bot_test"
        );
        let last_invoked_at = web_auth["last_invoked_at"].as_str().unwrap();
        let last_invoked_at = last_invoked_at.parse::<chrono::DateTime<Utc>>().unwrap();
        assert!(last_invoked_at > Utc::now() - Duration::days(7));

        let draft = skills
            .iter()
            .find(|skill| skill["name"] == "prd-template")
            .unwrap();
        let _: Skill = serde_json::from_value(draft.clone()).unwrap();
        assert_eq!(draft["source"], "draft");
        assert_eq!(draft["enabled"], false);
        assert_eq!(draft["draft_from"]["bot_id"], "bot_product");
        assert_eq!(draft["draft_from"]["project_id"], "prj_login");

        let detail = call(&gateway, "skill.get", json!({"name":"web-auth-patterns"})).await;
        let detail: SkillDetail = serde_json::from_value(detail["skill"].clone()).unwrap();
        assert!(detail.content.contains("bcrypt"));
        assert_eq!(
            detail.skill.files,
            ["scripts/gen-test-users.sh", "references/owasp.md"]
        );

        let published = call(&gateway, "skill.publish", json!({"name":"prd-template"})).await;
        let published: Skill = serde_json::from_value(published["skill"].clone()).unwrap();
        assert_eq!(published.source, macbot_protocol::SkillSource::User);
        assert!(published.enabled);
        assert_eq!(
            published.draft_from.unwrap().project_id.as_deref(),
            Some("prj_login")
        );

        let created = call(
            &gateway,
            "skill.create",
            json!({"name":"screen-created-skill","content":"---\nname: screen-created-skill\ndescription: Created in the UI\n---\nBody"}),
        )
        .await;
        assert_eq!(created["skill"]["source"], "user");
        call(
            &gateway,
            "skill.delete",
            json!({"name":"screen-created-skill"}),
        )
        .await;
        call(&gateway, "skill.delete", json!({"name":"prd-template"})).await;
        call(
            &gateway,
            "skill.delete",
            json!({"name":"web-auth-patterns"}),
        )
        .await;
        let listed_again = call(&gateway, "skill.list", json!({})).await;
        let remaining = listed_again["skills"].as_array().unwrap();
        assert!(!remaining.iter().any(|skill| {
            matches!(
                skill["name"].as_str(),
                Some("screen-created-skill" | "prd-template" | "web-auth-patterns")
            )
        }));
    }

    #[tokio::test]
    async fn mock_skill_attachment_files_are_seeded_and_listed_in_skill_detail() {
        let home = tempfile::tempdir().unwrap().keep();
        let data = home.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(
            data.join("mock-skills.json"),
            serde_json::to_vec(&json!([{
                "name":"web-auth-patterns",
                "description":"登录、注册和找回密码的实现规范",
                "source":"user",
                "path":"~/MacBot/skills/web-auth-patterns",
                "files":[],
                "enabled":true,
                "disabled_bot_ids":[],
                "invocations_7d":{"total":14,"by_bot":[]},
                "last_invoked_at":null,
                "updated_at":"2026-10-12T00:00:00Z",
                "content":"---\nname: web-auth-patterns\ndescription: Login security\n---\n# 登录实现规范\n使用 bcrypt。"
            }]))
            .unwrap(),
        )
        .unwrap();
        let gateway = gateway_at(home.clone()).await;

        let listed = call(&gateway, "skill.list", json!({})).await;
        let skill = listed["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|skill| skill["name"] == "web-auth-patterns")
            .unwrap();
        let detail = call(&gateway, "skill.get", json!({"name":"web-auth-patterns"})).await;
        let detail: SkillDetail = serde_json::from_value(detail["skill"].clone()).unwrap();

        assert_eq!(
            skill["files"],
            json!(["scripts/gen-test-users.sh", "references/owasp.md"])
        );
        assert_eq!(
            detail.skill.files,
            ["scripts/gen-test-users.sh", "references/owasp.md"]
        );
        assert!(detail.content.contains("# 登录实现规范"));

        let script = std::fs::read_to_string(
            home.join("skills/web-auth-patterns/scripts/gen-test-users.sh"),
        )
        .unwrap();
        assert!(script.starts_with("#!/usr/bin/env bash\nset -euo pipefail"));
        assert!(script.contains("alice@example.test"));

        let reference =
            std::fs::read_to_string(home.join("skills/web-auth-patterns/references/owasp.md"))
                .unwrap();
        assert!(reference.contains("HttpOnly, Secure, and SameSite"));
        assert!(reference.contains("Rate-limit login"));
    }

    #[tokio::test]
    async fn mock_skill_deletions_survive_gateway_restart_while_fresh_home_gets_examples() {
        let home = tempfile::tempdir().unwrap().keep();
        let gateway = gateway_at(home.clone()).await;
        let initial = call(&gateway, "skill.list", json!({})).await;
        assert!(initial["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "web-auth-patterns"));
        assert!(initial["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "prd-template"));

        call(
            &gateway,
            "skill.delete",
            json!({"name":"web-auth-patterns"}),
        )
        .await;
        call(&gateway, "skill.delete", json!({"name":"prd-template"})).await;
        drop(gateway);

        let restarted = gateway_at(home).await;
        let after_restart = call(&restarted, "skill.list", json!({})).await;
        let names = after_restart["skills"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|skill| skill["name"].as_str())
            .collect::<Vec<_>>();
        assert!(!names.contains(&"web-auth-patterns"));
        assert!(!names.contains(&"prd-template"));
        assert!(names.contains(&"agent-browser"));

        let fresh_home = tempfile::tempdir().unwrap().keep();
        let fresh = gateway_at(fresh_home).await;
        let fresh_skills = call(&fresh, "skill.list", json!({})).await["skills"].clone();
        assert!(fresh_skills
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "web-auth-patterns"));
        assert!(fresh_skills
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "prd-template"));
    }

    #[tokio::test]
    async fn mock_routines_seed_three_design_examples_and_run_history_once() {
        let gateway = gateway().await;
        let _ = call(&gateway, "bootstrap", json!({})).await;

        let listed = call(&gateway, "routine.list", json!({})).await;
        let routines = listed["routines"].as_array().unwrap();
        assert_eq!(routines.len(), 3);
        for routine in routines {
            let _: Routine = serde_json::from_value(routine.clone()).unwrap();
        }
        assert_eq!(routines[0]["name"], "每日 X 科技侦察");
        assert_eq!(routines[0]["schedules"][0]["label"], "工作日 09:00");
        assert_eq!(routines[0]["last_run"]["status"], "done");
        assert_eq!(routines[2]["name"], "周报汇总");
        assert_eq!(routines[2]["enabled"], false);

        let runs = call(
            &gateway,
            "routine.runs",
            json!({"routine_id":"rtn_mock_x_scout"}),
        )
        .await;
        let run_rows = runs["runs"].as_array().unwrap();
        assert_eq!(run_rows.len(), 3);
        for run in run_rows {
            let _: RoutineRun = serde_json::from_value(run.clone()).unwrap();
        }

        // The mock default is seeded once. Repeated reads keep it stable and
        // do not duplicate rows or recreate routines after a user deletes them.
        let _ = call(
            &gateway,
            "routine.delete",
            json!({"routine_id":"rtn_mock_weekly_summary"}),
        )
        .await;
        let listed_again = call(&gateway, "routine.list", json!({})).await;
        assert_eq!(listed_again["routines"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn mock_filters_pages_and_detects_request_conflicts() {
        let gateway = gateway().await;
        let first = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"one","mentions":[],"client_request_id":"r-1"}),
        )
        .await;
        let replay = call(
            &gateway,
            "chat.send",
            json!({"chat_id":"chat_main","text":"changed","mentions":[],"client_request_id":"r-1"}),
        )
        .await;
        assert_eq!(first, replay);
        let conflict = gateway
            .rpc(
                "bot.create",
                json!({"name":"wrong","client_request_id":"r-1"}),
            )
            .await
            .unwrap_err();
        assert_eq!(conflict.code, "conflict");

        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_main","after_seq":0,"limit":1}),
        )
        .await;
        assert_eq!(history["messages"].as_array().unwrap().len(), 1);
        assert!(history["has_more"].as_bool().unwrap());

        let projects = call(&gateway, "project.list", json!({"status":["active"]})).await;
        assert!(projects["projects"].is_array());
    }

    #[tokio::test]
    async fn mock_waiting_queue_counts_per_assignment_and_promotes_only_selected_delivery() {
        let gateway = gateway().await;
        let other_code_task = call(
            &gateway,
            "assignment.create",
            json!({
                "bot_id":"bot_code",
                "project_id":"prj_login",
                "origin_chat_id":"chat_login",
                "title":"编码并行测试任务",
                "instruction":"保持执行",
                "from":"main"
            }),
        )
        .await;
        let other_bot_task = call(
            &gateway,
            "assignment.create",
            json!({
                "bot_id":"bot_test",
                "project_id":"prj_login",
                "origin_chat_id":"chat_login",
                "title":"测试并行任务",
                "instruction":"保持执行",
                "from":"main"
            }),
        )
        .await;
        let send_params = json!({
            "chat_id":"chat_login",
            "text":"请把交付补充移动端说明",
            "mentions":[],
            "client_request_id":"mock-waiting-queue-send"
        });
        let mut send_events = gateway.state.events.subscribe();
        let sent = call(&gateway, "chat.send", send_params.clone()).await;
        let retry = call(&gateway, "chat.send", send_params).await;
        assert_eq!(retry, sent, "a retry replays the canonical queued message");
        let message = sent["message"].clone();
        let second_message = call(
            &gateway,
            "chat.send",
            json!({
                "chat_id":"chat_login",
                "text":"再补一条待看消息",
                "mentions":[],
                "client_request_id":"mock-waiting-queue-send-second"
            }),
        )
        .await["message"]
            .clone();
        let deliveries = message["delivery"].as_array().unwrap();
        let mut counted_assignments = Vec::new();
        while counted_assignments.len() < 3 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), send_events.recv())
                .await
                .unwrap()
                .unwrap();
            if event["event"] == "assignment.updated"
                && event["data"]["assignment"]["pending_messages"] == 2
            {
                counted_assignments.push(event["data"]["assignment"]["id"].clone());
            }
        }
        for assignment_id in [
            "asg_mock_working",
            other_code_task["id"].as_str().unwrap(),
            other_bot_task["id"].as_str().unwrap(),
        ] {
            assert!(
                deliveries.iter().any(|delivery| {
                    delivery["assignment_id"] == assignment_id && delivery["state"] == "queued"
                }),
                "missing queued delivery for {assignment_id}: {deliveries:?}"
            );
            let assignment = call(
                &gateway,
                "assignment.get",
                json!({"assignment_id":assignment_id}),
            )
            .await;
            assert_eq!(assignment["assignment"]["pending_messages"], 2);
        }

        // Opening the chat advances the human read cursor, but the Bot still
        // has a queued delivery and must be allowed to promote it.
        call(
            &gateway,
            "chat.mark_read",
            json!({"chat_id":"chat_login","seq":message["seq"]}),
        )
        .await;

        let mut steer_events = gateway.state.events.subscribe();
        let steer_params = json!({
            "message_id":message["id"],
            "bot_id":"bot_code",
            "assignment_id":"asg_mock_working",
            "client_request_id":"mock-waiting-queue-steer"
        });
        let promoted = call(&gateway, "message.steer", steer_params.clone()).await;
        let retry = call(&gateway, "message.steer", steer_params).await;
        assert_eq!(retry, promoted, "a retry does not duplicate the steer");
        let duplicate_without_request_id = call(
            &gateway,
            "message.steer",
            json!({
                "message_id":message["id"],
                "bot_id":"bot_code",
                "assignment_id":"asg_mock_working"
            }),
        )
        .await;
        assert_eq!(duplicate_without_request_id, promoted);
        let assignment_event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let event = steer_events.recv().await.unwrap();
                if event["event"] == "assignment.updated"
                    && event["data"]["assignment"]["id"] == "asg_mock_working"
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            assignment_event["data"]["assignment"]["pending_messages"],
            2
        );
        assert_eq!(
            assignment_event["data"]["assignment"]["steers"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let promoted_delivery = promoted["message"]["delivery"]
            .as_array()
            .unwrap()
            .iter()
            .find(|delivery| delivery["assignment_id"] == "asg_mock_working")
            .unwrap();
        assert_eq!(promoted_delivery["state"], "queued");
        assert_eq!(promoted["assignment"]["pending_messages"], 2);
        assert_eq!(
            promoted["assignment"]["steers"].as_array().unwrap().len(),
            1
        );

        for assignment_id in [
            other_code_task["id"].as_str().unwrap(),
            other_bot_task["id"].as_str().unwrap(),
        ] {
            let assignment = call(
                &gateway,
                "assignment.get",
                json!({"assignment_id":assignment_id}),
            )
            .await;
            assert_eq!(assignment["assignment"]["pending_messages"], 2);
        }
        let history = call(
            &gateway,
            "chat.history",
            json!({"chat_id":"chat_login","limit":100}),
        )
        .await;
        let stored = history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["id"] == message["id"])
            .unwrap();
        assert_eq!(stored["delivery"], promoted["message"]["delivery"]);
        assert!(history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["id"] == second_message["id"]));
        let _ = call(&gateway, "bootstrap", json!({})).await;
        let after_reconnect = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_working"}),
        )
        .await;
        assert_eq!(after_reconnect["assignment"]["pending_messages"], 2);

        // An explicitly promoted steer remains idempotent after the Bot has
        // consumed its delivery, and the retry returns the current delivery.
        {
            let mut state = gateway.state.inner.write().await;
            let message = state
                .messages
                .get_mut("chat_login")
                .unwrap()
                .iter_mut()
                .find(|row| row["id"] == message["id"])
                .unwrap();
            message["delivery"][0]["state"] = json!("read");
            state
                .assignments
                .iter_mut()
                .find(|assignment| assignment["id"] == "asg_mock_working")
                .unwrap()["pending_messages"] = json!(1);
        }
        let replay = call(
            &gateway,
            "message.steer",
            json!({
                "message_id":message["id"],
                "bot_id":"bot_code",
                "assignment_id":"asg_mock_working"
            }),
        )
        .await;
        assert_eq!(
            replay["message"]["delivery"][0]["state"], "read",
            "retries return the current state"
        );
        assert_eq!(replay["assignment"]["steers"].as_array().unwrap().len(), 1);
        gateway
            .state
            .inner
            .write()
            .await
            .assignments
            .iter_mut()
            .find(|assignment| assignment["id"] == "asg_mock_working")
            .unwrap()["status"] = json!("done");
        let completed_replay = call(
            &gateway,
            "message.steer",
            json!({
                "message_id":message["id"],
                "bot_id":"bot_code",
                "assignment_id":"asg_mock_working"
            }),
        )
        .await;
        assert_eq!(completed_replay["assignment"]["status"], "done");
        assert_eq!(completed_replay["message"]["delivery"][0]["state"], "read");
        gateway
            .state
            .inner
            .write()
            .await
            .assignments
            .iter_mut()
            .find(|assignment| assignment["id"] == "asg_mock_working")
            .unwrap()["status"] = json!("cancelled");
        let cancelled_replay = call(
            &gateway,
            "message.steer",
            json!({
                "message_id":message["id"],
                "bot_id":"bot_code",
                "assignment_id":"asg_mock_working"
            }),
        )
        .await;
        assert_eq!(cancelled_replay["assignment"]["status"], "cancelled");
        assert_eq!(cancelled_replay["message"]["delivery"][0]["state"], "read");
    }

    #[tokio::test]
    async fn mock_steer_rejects_a_delivery_already_consumed_by_the_bot() {
        let gateway = gateway().await;
        let sent = call(
            &gateway,
            "chat.send",
            json!({
                "chat_id":"chat_login",
                "text":"这条待看消息随后被 Bot 消费",
                "mentions":[],
                "client_request_id":"mock-consumed-queued-message"
            }),
        )
        .await;
        let message_id = sent["message"]["id"].as_str().unwrap().to_owned();
        let seq = sent["message"]["seq"].as_u64().unwrap();
        call(
            &gateway,
            "chat.mark_read",
            json!({"chat_id":"chat_login","seq":seq}),
        )
        .await;

        // Simulate the Bot's delivery transition while leaving the task active.
        {
            let mut state = gateway.state.inner.write().await;
            let message = state
                .messages
                .get_mut("chat_login")
                .unwrap()
                .iter_mut()
                .find(|message| message["id"] == message_id)
                .unwrap();
            message["delivery"][0]["state"] = json!("read");
            state
                .assignments
                .iter_mut()
                .find(|assignment| assignment["id"] == "asg_mock_working")
                .unwrap()["pending_messages"] = json!(0);
        }

        let error = gateway
            .rpc(
                "message.steer",
                json!({
                    "message_id":message_id,
                    "bot_id":"bot_code",
                    "assignment_id":"asg_mock_working",
                    "client_request_id":"mock-consumed-steer"
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert_eq!(error.message, "message is not waiting for this Bot");
        let assignment = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_working"}),
        )
        .await["assignment"]
            .clone();
        assert_eq!(assignment["pending_messages"], 0);
        assert!(assignment["steers"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn mock_deleted_queued_message_cannot_be_promoted_and_does_not_change_assignment() {
        let gateway = gateway().await;
        let sent = call(
            &gateway,
            "chat.send",
            json!({
                "chat_id":"chat_login",
                "text":"待删除的待看消息",
                "mentions":[],
                "client_request_id":"mock-delete-queued-message"
            }),
        )
        .await;
        let message_id = sent["message"]["id"].as_str().unwrap().to_owned();
        assert_eq!(sent["message"]["delivery"][0]["state"], "queued");
        assert_eq!(
            call(
                &gateway,
                "assignment.get",
                json!({"assignment_id":"asg_mock_working"})
            )
            .await["assignment"]["pending_messages"],
            1
        );

        call(
            &gateway,
            "message.delete",
            json!({"message_id":message_id,"client_request_id":"mock-delete-queued"}),
        )
        .await;
        let before_steer = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_working"}),
        )
        .await["assignment"]
            .clone();
        assert_eq!(before_steer["pending_messages"], 0);

        let error = gateway
            .rpc(
                "message.steer",
                json!({
                    "message_id":message_id,
                    "bot_id":"bot_code",
                    "assignment_id":"asg_mock_working",
                    "client_request_id":"mock-steer-deleted-queued"
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
        let after_steer = call(
            &gateway,
            "assignment.get",
            json!({"assignment_id":"asg_mock_working"}),
        )
        .await["assignment"]
            .clone();
        assert_eq!(after_steer, before_steer);
        assert!(after_steer["steers"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_indexes_fixture_corpus_and_honors_filters() {
        let gateway = gateway().await;
        let artifacts = call(
            &gateway,
            "search",
            json!({"query":"PRD","kinds":["artifact"],"limit":10}),
        )
        .await;
        let typed: SearchResult = serde_json::from_value(artifacts.clone()).unwrap();
        assert!(
            !typed.results.is_empty(),
            "fixture artifact corpus is empty"
        );
        assert!(typed
            .results
            .iter()
            .all(|hit| hit.kind == macbot_protocol::SearchKind::Artifact));
        for hit in artifacts["results"].as_array().unwrap() {
            let _: SearchHit = serde_json::from_value(hit.clone()).unwrap();
            assert_eq!(hit.as_object().unwrap().len(), 6);
        }

        let messages = call(
            &gateway,
            "search",
            json!({"query":"邮箱","kinds":["message"],"limit":1}),
        )
        .await;
        let typed: SearchResult = serde_json::from_value(messages).unwrap();
        assert_eq!(typed.results.len(), 1);
        assert_eq!(typed.results[0].kind, macbot_protocol::SearchKind::Message);

        let routine = call(
            &gateway,
            "routine.create",
            json!({
                "bot_id":"bot_main",
                "name":"夜间 PRD 巡检",
                "instructions":"检查 PRD 更新",
                "schedules":[{"cron":"0 * * * *","label":"hourly"}],
                "timezone":"Asia/Shanghai",
                "client_request_id":"search-routine"
            }),
        )
        .await;
        let routine_id = routine["routine"]["id"].as_str().unwrap();
        let routines = call(
            &gateway,
            "search",
            json!({"query":"巡检","kinds":["routine"],"limit":10}),
        )
        .await;
        let typed: SearchResult = serde_json::from_value(routines).unwrap();
        assert!(typed.results.iter().any(|hit| hit.id == routine_id));
    }

    #[tokio::test]
    async fn assignment_fixture_is_typed() {
        let gateway = gateway().await;
        let assignments = call(&gateway, "assignment.list", json!({})).await;
        for item in assignments["items"].as_array().unwrap() {
            let _: Assignment = serde_json::from_value(item.clone()).unwrap();
        }
    }

    #[tokio::test]
    async fn workbench_aggregates_assignments_waiting_and_takeover_typed() {
        let gateway = gateway().await;
        let assignments = call(&gateway, "assignment.list", json!({})).await;
        let items = assignments["items"].as_array().unwrap();
        let expected_running = items
            .iter()
            .filter(|item| item["status"] == "working")
            .count();
        let workbench = call(&gateway, "workbench.get", json!({})).await;
        let typed: WorkbenchResult = serde_json::from_value(workbench).unwrap();
        assert_eq!(typed.workbench.running as usize, expected_running);
        assert!(typed
            .workbench
            .waiting
            .iter()
            .any(|item| { matches!(item, macbot_protocol::WorkbenchWaiting::Approval { .. }) }));
        assert!(typed
            .workbench
            .waiting
            .iter()
            .any(|item| { matches!(item, macbot_protocol::WorkbenchWaiting::Question { .. }) }));
        assert!(typed
            .workbench
            .bots
            .iter()
            .any(|bot| !bot.assignments.is_empty()));
        assert!(typed
            .workbench
            .done_today
            .iter()
            .all(|assignment| assignment.status == macbot_protocol::AssignmentStatus::Done));

        call(
            &gateway,
            "takeover.start",
            json!({"bot_id":"bot_main","assignment_id":"asg_code_web"}),
        )
        .await;
        let active = call(&gateway, "workbench.get", json!({})).await;
        let active: WorkbenchResult = serde_json::from_value(active).unwrap();
        assert!(active.workbench.waiting.iter().any(|item| {
            matches!(item, macbot_protocol::WorkbenchWaiting::Takeover { bot_id, .. } if bot_id == "bot_main")
        }));
        call(
            &gateway,
            "takeover.release",
            json!({"bot_id":"bot_main","note":"done"}),
        )
        .await;
        let released = call(&gateway, "workbench.get", json!({})).await;
        let released: WorkbenchResult = serde_json::from_value(released).unwrap();
        assert!(!released
            .workbench
            .waiting
            .iter()
            .any(|item| { matches!(item, macbot_protocol::WorkbenchWaiting::Takeover { .. }) }));
    }

    #[tokio::test]
    async fn mock_members_and_waiting_items_are_referential() {
        let gateway = gateway().await;
        let bootstrap = call(&gateway, "bootstrap", json!({})).await;
        let assignments = call(&gateway, "assignment.list", json!({})).await;
        let bots = call(&gateway, "bot.list", json!({})).await;
        let assignment_ids = assignments["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        let bot_dm_chats = bots["bots"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|bot| Some((bot["id"].as_str()?, bot["dm_chat_id"].as_str()?)))
            .collect::<std::collections::HashMap<_, _>>();
        for waiting in bootstrap["pending"]["approvals"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let assignment_id = waiting["assignment_id"].as_str().unwrap();
            let bot_id = waiting["bot_id"].as_str().unwrap();
            assert!(assignment_ids.contains(assignment_id));
            assert_eq!(
                waiting["chat_id"].as_str(),
                bot_dm_chats.get(bot_id).copied()
            );
        }
        for question in bootstrap["pending"]["questions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let assignment_id = question["assignment_id"].as_str().unwrap();
            let assignment = assignments["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == assignment_id)
                .unwrap();
            assert_eq!(question["bot_id"], assignment["bot_id"]);
            let chat_id = question["chat_id"].as_str().unwrap();
            assert!(
                bot_dm_chats
                    .get(question["bot_id"].as_str().unwrap())
                    .copied()
                    == Some(chat_id)
                    || assignment["origin_chat_id"].as_str() == Some(chat_id),
                "question must be visible in the Bot DM or the assignment origin chat"
            );
        }

        let created = call(
            &gateway,
            "project.create",
            json!({"name":"Selected members","member_bot_ids":["bot_code"]}),
        )
        .await;
        let members = created["project"]["members"].as_array().unwrap();
        assert!(members.iter().any(|member| member["bot_id"] == "bot_main"));
        assert!(members.iter().any(|member| member["bot_id"] == "bot_code"));
        let chat_members = created["chat"]["member_bot_ids"].as_array().unwrap();
        assert!(chat_members.iter().any(|member| member == "bot_main"));
        assert!(chat_members.iter().any(|member| member == "bot_code"));
    }

    #[test]
    fn usage_auto_granularity_matches_production_ranges() {
        let from = Utc::now();
        for (days, expected) in [
            (1, "hour"),
            (7, "hour"),
            (8, "day"),
            (90, "day"),
            (91, "week"),
        ] {
            let result = super::usage_timeseries(&json!({"from":from.to_rfc3339(),"to":(from+Duration::days(days)).to_rfc3339(),"granularity":"auto"})).unwrap();
            assert_eq!(result["granularity"], expected);
            if days == 1 {
                assert_eq!(result["buckets"].as_array().unwrap().len(), 25);
            }
        }
    }

    #[tokio::test]
    async fn usage_mock_is_seeded_filtered_and_typed() {
        let gateway = gateway().await;
        let now = Utc::now();
        let from = (now - Duration::days(7)).to_rfc3339();
        let to = (now + Duration::hours(1)).to_rfc3339();

        let summary = call(&gateway, "usage.summary", json!({"from":from,"to":to})).await;
        let summary: UsageSummaryResult = serde_json::from_value(summary).unwrap();
        assert!(summary.current.usage.requests >= 5);
        assert!(summary.current.usage.input_tokens > 0);
        assert!(summary.current.usage.cache_read_tokens > 0);
        assert!(summary.current.usage.cache_write_tokens > 0);
        assert!(summary.current.usage.cost.is_some());
        assert!(summary.previous.usage.requests > 0);
        assert!(summary.current.tasks_done > 0);

        let outside = call(
            &gateway,
            "usage.summary",
            json!({
                "from":(now + Duration::days(1)).to_rfc3339(),
                "to":(now + Duration::days(2)).to_rfc3339()
            }),
        )
        .await;
        let outside: UsageSummaryResult = serde_json::from_value(outside).unwrap();
        assert_eq!(outside.current.usage.requests, 0);

        let calendar = call(
            &gateway,
            "usage.heatmap",
            json!({"mode":"calendar","from":from,"to":to,"metric":"tokens"}),
        )
        .await;
        let calendar: macbot_protocol::HeatmapResult = serde_json::from_value(calendar).unwrap();
        let macbot_protocol::HeatmapResult::Calendar(calendar) = calendar else {
            panic!("calendar mode returned weekhour shape");
        };
        assert!(calendar.days.iter().any(|day| day.requests > 0));
        assert!(
            calendar.days.iter().map(|day| day.requests).sum::<u64>()
                >= summary.current.usage.requests
        );

        let weekhour = call(
            &gateway,
            "usage.heatmap",
            json!({"mode":"weekhour","from":from,"to":to,"metric":"requests"}),
        )
        .await;
        let weekhour: macbot_protocol::HeatmapResult = serde_json::from_value(weekhour).unwrap();
        let macbot_protocol::HeatmapResult::Weekhour(weekhour) = weekhour else {
            panic!("weekhour mode returned calendar shape");
        };
        let matrix = weekhour.matrix;
        assert_eq!(matrix.len(), 7);
        assert!(matrix.iter().all(|row| row.len() == 24));
        assert!(matrix.iter().flatten().any(|value| *value > 0.0));

        let timeseries = call(
            &gateway,
            "usage.timeseries",
            json!({"from":from,"to":to,"granularity":"day","dimension":"model","metric":"tokens","split_io":true,"top":2}),
        )
        .await;
        let timeseries: UsageTimeseriesResult = serde_json::from_value(timeseries).unwrap();
        assert!(!timeseries.buckets.is_empty());
        assert!(timeseries.series.iter().any(|series| series.key == "other"));
        assert!(timeseries
            .series
            .iter()
            .all(|series| series.values.len() == timeseries.buckets.len()));

        let breakdown = call(
            &gateway,
            "usage.breakdown",
            json!({"from":from,"to":to,"dimension":"project"}),
        )
        .await;
        let breakdown: UsageBreakdownResult = serde_json::from_value(breakdown).unwrap();
        assert!(breakdown.rows.iter().any(|row| row.key == "prj_login"));
        assert!(breakdown.rows.iter().any(|row| row.key == "prj_web"));
        assert_eq!(
            breakdown
                .rows
                .iter()
                .map(|row| row.usage.requests)
                .sum::<u64>(),
            summary.current.usage.requests
        );
        assert!(breakdown
            .rows
            .iter()
            .any(|row| row.phases.contains_key("compact")));

        let model_breakdown = call(
            &gateway,
            "usage.breakdown",
            json!({"from":from,"to":to,"dimension":"model"}),
        )
        .await;
        let model_breakdown: UsageBreakdownResult =
            serde_json::from_value(model_breakdown).unwrap();
        assert!(model_breakdown
            .rows
            .iter()
            .any(|row| row.key == "free-model" && row.usage.cost.is_none()));
    }

    #[tokio::test]
    async fn dashboard_mock_dimensions_resolve_and_support_drilldown() {
        let gateway = gateway().await;
        let now = Utc::now();
        let from = (now - Duration::days(30)).to_rfc3339();
        let to = (now + Duration::hours(1)).to_rfc3339();

        let bootstrap = call(&gateway, "bootstrap", json!({})).await;
        let bot_ids = bootstrap["bots"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|bot| bot["id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        let project_ids = bootstrap["projects"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|project| project["id"].as_str())
            .collect::<std::collections::HashSet<_>>();
        let providers = call(&gateway, "provider.list", json!({})).await;
        let model_ids = providers["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["model_id"].as_str())
            .collect::<std::collections::HashSet<_>>();

        let summary = call(&gateway, "usage.summary", json!({"from":from,"to":to})).await;
        let summary: UsageSummaryResult = serde_json::from_value(summary).unwrap();
        assert!(summary.previous.usage.requests >= 3);
        assert!(summary.previous.usage.input_tokens > 0);
        assert!(summary.current.usage.requests > summary.previous.usage.requests);
        assert!(
            summary.current.usage.input_tokens + summary.current.usage.output_tokens
                > summary.previous.usage.input_tokens + summary.previous.usage.output_tokens
        );

        for dimension in ["model", "bot", "project"] {
            let result = call(
                &gateway,
                "usage.timeseries",
                json!({
                    "from":from,
                    "to":to,
                    "granularity":"day",
                    "dimension":dimension,
                    "metric":"tokens"
                }),
            )
            .await;
            let series = result["series"].as_array().unwrap();
            assert!(!series.is_empty(), "{dimension} dimension must have usage");
            assert!(series.iter().all(|item| {
                item["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value.as_f64().unwrap_or_default() > 0.0)
            }));
            for item in series {
                let key = item["key"].as_str().unwrap();
                match dimension {
                    "model" => assert!(model_ids.contains(key), "unresolvable model {key}"),
                    "bot" => assert!(bot_ids.contains(key), "unresolvable bot {key}"),
                    "project" => assert!(project_ids.contains(key), "unresolvable project {key}"),
                    _ => unreachable!(),
                }
            }
        }

        let heatmap = call(
            &gateway,
            "usage.heatmap",
            json!({"mode":"weekhour","from":from,"to":to,"metric":"tokens"}),
        )
        .await;
        let matrix = heatmap["matrix"].as_array().unwrap();
        assert_eq!(matrix.len(), 7);
        assert!(matrix.iter().any(|day| {
            day.as_array()
                .unwrap()
                .iter()
                .any(|hour| hour.as_f64().unwrap_or_default() > 0.0)
        }));

        let drilled = call(
            &gateway,
            "usage.breakdown",
            json!({"from":from,"to":to,"dimension":"project","drill":{"bot_id":"bot_code"}}),
        )
        .await;
        let rows = drilled["rows"].as_array().unwrap();
        assert!(rows.iter().any(|row| row["key"] == "prj_login"));
        assert!(rows.iter().any(|row| row["key"] == "prj_web"));
    }

    #[tokio::test]
    async fn mock_search_has_cross_entity_hits_and_empty_result() {
        let gateway = gateway().await;
        let hits = call(&gateway, "search", json!({"query":"登录","limit":100})).await;
        let results = hits["results"].as_array().unwrap();
        assert!(results.iter().any(|result| result["kind"] == "chat"));
        assert!(results.iter().any(|result| result["kind"] == "message"));

        let bot_hits = call(&gateway, "search", json!({"query":"编码","kinds":["bot"]})).await;
        assert!(bot_hits["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["id"] == "bot_code"));

        let no_hits = call(
            &gateway,
            "search",
            json!({"query":"query-with-no-mock-match-4519","limit":100}),
        )
        .await;
        assert!(no_hits["results"].as_array().unwrap().is_empty());
    }
}
