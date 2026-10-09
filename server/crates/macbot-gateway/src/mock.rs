//! Deterministic protocol mock.
//!
//! This module is deliberately stateful and typed at the JSON boundary. It is
//! used by both clients before a real runtime backend is configured. Every
//! mutating method returns the object it changed and appends the corresponding
//! protocol event to the gateway event log.

use super::{id, now, rpc_error, GatewayState, MockState, RpcResult};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde_json::{json, Value};

pub(crate) async fn mock_call(method: &str, params: Value, state: &GatewayState) -> RpcResult {
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

    let result = dispatch(method, params, &mut data, state).await?;
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
) -> RpcResult {
    ensure_mock_defaults(state);
    match method {
        "ping" => Ok(json!({"server_time":now()})),
        "session.resume" => Ok(json!({"mode":"replay"})),
        "bootstrap" => {
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
        "chat.send" => send_chat(state, &params),
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
        "project.archive" => project_status(state, &params, "archived"),
        "project.reopen" => project_status(state, &params, "active"),
        "project.request_changes" => project_request_changes(state, &params),
        "assignment.list" => assignment_list(state, &params),
        "assignment.get" => find_wrapped(
            &state.assignments,
            params.get("assignment_id"),
            "assignment",
        ),
        "assignment.stop" => assignment_stop(state, &params),
        "trace.history" => trace_history(state, &params),
        "trace.subscribe" => Ok(json!({"stream":id("stream"),"in_flight":[]})),
        "trace.unsubscribe" => Ok(json!({})),
        "approval.list" => filtered_list(state, "approvals", &params, "state", "approvals"),
        "approval.decide" => approval_decide(state, &params),
        "question.answer" => question_answer(state, &params),
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
        "settings.update" => settings_update(state, &params),
        "usage.summary" => usage_summary(&params),
        "usage.heatmap" => usage_heatmap(&params),
        "usage.timeseries" => usage_timeseries(&params),
        "usage.breakdown" => usage_breakdown(&params),
        "search" => Ok(json!({"results":[]})),
        _ => Err(rpc_error(
            "invalid_params",
            &format!("unknown method: {method}"),
            None,
        )),
    }
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
        .unwrap_or_else(|| "asgn_mock_1".to_owned());
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

fn extra_get(state: &MockState, key: &str) -> Vec<Value> {
    state.extra.get(key).cloned().unwrap_or_default()
}
fn extra_set(state: &mut MockState, key: &str, values: Vec<Value>) {
    state.extra.insert(key.to_string(), values);
}

fn ensure_mock_defaults(state: &mut MockState) {
    if extra_get(state, "skills").is_empty() {
        let skills = vec![
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
        ];
        extra_set(state, "skills", skills);
    }
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
    if extra_get(state, "approvals").is_empty() {
        extra_set(
            state,
            "approvals",
            vec![json!({
                "id":"apr_mock_pending",
                "bot_id":"bot_main",
                "assignment_id":"asgn_mock_1",
                "chat_id":"chat_main",
                "tool":"bash",
                "risk":"exec",
                "summary":"Run the mock verification command",
                "detail":"The deterministic mock keeps one pending approval for the workbench.",
                "state":"pending",
                "created_at":now(),
                "decided_at":null
            })],
        );
    }
    if extra_get(state, "questions").is_empty() {
        extra_set(
            state,
            "questions",
            vec![json!({
                "id":"q_mock_pending",
                "bot_id":"bot_main",
                "assignment_id":"asgn_mock_1",
                "chat_id":"chat_main",
                "text":"Which deterministic mock path should continue?",
                "options":["safe","fast"],
                "allow_free_text":true,
                "state":"pending",
                "answer":null
            })],
        );
    }
}
fn pending(state: &MockState) -> Value {
    json!({"approvals":extra_get(state,"approvals").iter().filter(|x| x.get("state").and_then(Value::as_str)==Some("pending")).cloned().collect::<Vec<_>>(),"questions":extra_get(state,"questions"),"reviews":[]})
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
            project_id: Some("project_alpha"),
            model_id: "mock-model",
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
            bot_id: "bot_worker",
            project_id: Some("project_alpha"),
            model_id: "fast-model",
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
            project_id: Some("project_beta"),
            model_id: "mock-model",
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
            bot_id: "bot_worker",
            project_id: Some("project_beta"),
            model_id: "vision-model",
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
            project_id: Some("project_alpha"),
            model_id: "fast-model",
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
            bot_id: "bot_worker",
            project_id: Some("project_beta"),
            model_id: "mock-model",
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
            bot_id: "bot_worker",
            project_id: Some("project_alpha"),
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
            project_id: Some("project_alpha"),
            model_id: "mock-model",
            phase: "work",
            input_tokens: 500,
            output_tokens: 160,
            cache_read_tokens: 20,
            cache_write_tokens: 0,
            cost: Some(0.05),
            task_done: true,
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
        .unwrap_or("day");
    let granularity = if requested == "hour" && (to - from) <= Duration::days(14) {
        "hour"
    } else if requested == "week" {
        "week"
    } else {
        "day"
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
    let messages = state.messages.get(chat_id).cloned().unwrap_or_default();
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

fn send_chat(state: &mut MockState, params: &Value) -> RpcResult {
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
    let seq = state
        .messages
        .get(&chat_id)
        .and_then(|messages| {
            messages
                .iter()
                .filter_map(|message| message.get("seq").and_then(Value::as_u64))
                .max()
        })
        .unwrap_or(0)
        + 1;
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let message = json!({"id":id("msg"),"chat_id":chat_id,"seq":seq,"sender":{"kind":"user"},"created_at":now(),"edited_at":null,"deleted":false,"reply_to":params.get("reply_to").cloned().unwrap_or(Value::Null),"thread_count":0,"mentions":params.get("mentions").cloned().unwrap_or(json!([])),"blocks":[{"type":"text","markdown":text}],"fallback_text":text,"intent":null,"assignment_id":null,"streaming":false,"delivery":[],"reactions":[]});
    state
        .messages
        .entry(chat_id.clone())
        .or_default()
        .push(message.clone());
    state.emit("message.created", json!({"message":message.clone()}));
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
    Ok(json!({"message":message}))
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
    let bot = json!({"id":bot_id,"name":name,"label":params.get("label").and_then(Value::as_str).unwrap_or(""),"description":params.get("description").and_then(Value::as_str).unwrap_or(""),"avatar":params.get("avatar").cloned().unwrap_or(json!({"kind":"bean","color":0})),"is_main":false,"model":params.get("model").cloned().unwrap_or(Value::Null),"max_parallel":params.get("max_parallel").and_then(Value::as_u64).unwrap_or(2),"tools":params.get("tools").cloned().unwrap_or(json!({"files":true,"bash":true,"browser":false,"subagent":false,"web":false,"mcp":false})),"browser_mode":params.get("browser_mode").cloned().unwrap_or(json!("headless")),"pinned":false,"hidden":false,"notifications":true,"dm_chat_id":chat_id,"created_at":now,"updated_at":now,"status":{"summary":"idle","active":0,"queued":0,"waiting":0}});
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
            json!({"project_id":project_id,"members":[],"artifacts":[],"highlights":highlights,"updated_at":now()})
        });
    Ok(json!({"project":project,"announcement":announcement}))
}
fn create_project(state: &mut MockState, params: &Value) -> RpcResult {
    let now = now();
    let project_id = id("prj");
    let chat_id = id("chat");
    let name = params.get("name").cloned().unwrap_or(json!("新项目"));
    let slug = name
        .as_str()
        .unwrap_or("project")
        .to_lowercase()
        .replace(' ', "-");
    let members = params.get("member_bot_ids").cloned().unwrap_or(json!([]));
    let project = json!({"id":project_id,"chat_id":chat_id,"name":name,"slug":slug,"goal":params.get("goal").cloned().unwrap_or(json!("")),"flow":params.get("flow").cloned().unwrap_or(json!([])),"deadline":params.get("deadline").cloned().unwrap_or(Value::Null),"home_path":format!("~/MacBot/projects/{slug}/"),"status":"active","lead_bot_id":"bot_main","members":[{"bot_id":"bot_main","role_note":"负责人","joined_at":now}],"created_by":{"kind":"user"},"created_at":now,"updated_at":now,"done_at":null,"highlights":[]});
    let chat = json!({"id":chat_id,"kind":"project","title":project["name"],"bot_id":null,"project_id":project_id,"member_bot_ids":members,"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,"attention":"none","pinned":false,"muted":false,"updated_at":now});
    state.projects.push(project.clone());
    state.chats.push(chat.clone());
    state.emit("project.created", json!({"project":project.clone()}));
    state.emit("chat.created", json!({"chat":chat.clone()}));
    Ok(json!({"project":project,"chat":chat}))
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
    let project = state
        .projects
        .iter_mut()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(project_id))
        .ok_or_else(|| rpc_error("not_found", "project not found", None))?;
    let members = project["members"].as_array_mut().unwrap();
    if add {
        if !members
            .iter()
            .any(|m| m.get("bot_id").and_then(Value::as_str) == Some(bot_id))
        {
            members.push(json!({"bot_id":bot_id,"role_note":params.get("role_note").and_then(Value::as_str).unwrap_or(""),"joined_at":now()}));
        }
    } else {
        members.retain(|m| m.get("bot_id").and_then(Value::as_str) != Some(bot_id));
    }
    project["updated_at"] = json!(now());
    let result = project.clone();
    state.emit("project.updated", json!({"project":result.clone()}));
    Ok(json!({"project":result}))
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
    Ok(json!({"project":result}))
}
fn project_request_changes(state: &mut MockState, params: &Value) -> RpcResult {
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
    send_chat(
        state,
        &json!({"chat_id":chat_id,"text":params.get("text").cloned().unwrap_or(json!("")),"mentions":[{"kind":"main"}]}),
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
    item["status"] = json!("done");
    let result = item.clone();
    state.emit("assignment.updated", json!({"assignment":result.clone()}));
    Ok(json!({"assignment":result}))
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
    item["state"] = json!("answered");
    item["answer"] = json!({"option_index":params.get("option_index").cloned().unwrap_or(Value::Null),"text":params.get("text").cloned().unwrap_or(Value::Null),"at":now()});
    replace_extra(state, "questions", item.clone());
    state.emit("question.answered", json!({"question":item.clone()}));
    Ok(json!({"question":item}))
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
    Ok(json!({"workbench":workbench}))
}

fn skill_default(name: &str, description: &str, source: &str, content: Option<&str>) -> Value {
    json!({"name":name,"description":description,"source":source,"path":format!("builtin://{name}"),"files":[],"enabled":true,"disabled_bot_ids":[],"invocations_7d":{"total":0,"by_bot":[]},"updated_at":now(),"content":content})
}
fn skill_get(state: &MockState, params: &Value) -> RpcResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let item = extra_get(state, "skills")
        .into_iter()
        .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| rpc_error("not_found", "skill not found", None))?;
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
    merge_patch(&mut state.settings, params.get("patch"));
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
    use super::super::{Gateway, GatewayConfig};
    use chrono::{Duration, Utc};
    use macbot_protocol::{
        Announcement, Assignment, Bot, Chat, Device, Message, Model, Project, Provider, Routine,
        RoutineRun, Settings, Skill, SkillDetail, UsageBreakdownResult, UsageSummaryResult,
        UsageTimeseriesResult, WorkbenchResult,
    };
    use serde_json::{json, Value};

    async fn gateway() -> Gateway {
        let home = tempfile::tempdir().unwrap();
        // Keep the directory alive for the duration of the gateway. The mock
        // itself is in-memory, but Gateway::new reads settings from this path.
        let path = home.keep();
        Gateway::new(GatewayConfig {
            home: path,
            mock: true,
            password: Some("dev".into()),
            ..Default::default()
        })
    }

    async fn call(gateway: &Gateway, method: &str, params: Value) -> Value {
        gateway.rpc(method, params).await.unwrap()
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
            json!({"name":"Demo","goal":"Ship","member_bot_ids":[]}),
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
        assert!(breakdown.rows.iter().any(|row| row.key == "project_alpha"));
        assert!(breakdown.rows.iter().any(|row| row.key == "project_beta"));
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
}
