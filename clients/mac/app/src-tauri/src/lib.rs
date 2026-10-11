use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use macbot_client_core::{
    AppState, Client, ClientConfig, ClientEvent, ScreenClient, ScreenEvent, ScreenFrame,
    ScreenHandle,
};
use macbot_desktop_services::{
    host_storage::{HostRecord, HostStore},
    local_settings,
    outbox::{self, PendingMessages},
    state_cache,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{
    ipc::{Channel, InvokeBody, Request, Response},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;
// IPC is capped at 8 MiB of JPEG data (plus the small wire header). The core
// parser has already allocated the incoming JPEG before this native limit runs.
const MAX_SCREEN_JPEG_BYTES: usize = 8 * 1024 * 1024;

struct BridgeData {
    connection: &'static str,
    error: Option<String>,
    state: AppState,
    client: Option<Client>,
    active_host: Option<HostRecord>,
    pending_messages: PendingMessages,
    trace_subscription: Option<TraceSubscription>,
    trace_subscribing: Option<TraceScope>,
    buffered_trace_events: Vec<(String, Value)>,
    buffered_trace_bytes: usize,
    trace_buffer_overflow: bool,
    connection_generation: u64,
    trace_cleanup: Option<PendingTraceCleanup>,
    screen_sessions: BTreeMap<u64, ScreenSession>,
    screen_generation: u64,
}

impl Default for BridgeData {
    fn default() -> Self {
        Self {
            connection: "disconnected",
            error: None,
            state: AppState::default(),
            client: None,
            active_host: None,
            pending_messages: BTreeMap::new(),
            trace_subscription: None,
            trace_subscribing: None,
            buffered_trace_events: Vec::new(),
            buffered_trace_bytes: 0,
            trace_buffer_overflow: false,
            connection_generation: 0,
            trace_cleanup: None,
            screen_sessions: BTreeMap::new(),
            screen_generation: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TraceScope {
    assignment_id: Option<String>,
    chat_id: Option<String>,
}

#[derive(Clone, Debug)]
struct TraceSubscription {
    scope: TraceScope,
    stream: String,
    in_flight: Vec<Value>,
    since_aseq: u64,
    seen_aseq: BTreeSet<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingTraceCleanup {
    generation: u64,
    stream: String,
}

#[derive(Clone)]
struct ScreenSession {
    bot_id: String,
    generation: u64,
    host_generation: u64,
    client: ScreenClient,
}

struct ScreenPump {
    bot_id: String,
    generation: u64,
    host_generation: u64,
    client: ScreenClient,
    frame_channel: Channel<Response>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
#[allow(dead_code)] // These fields are intentionally read by serde during validation only.
enum ScreenInput {
    Mouse {
        action: MouseAction,
        x: f64,
        y: f64,
        button: MouseButton,
        click_count: u32,
    },
    Wheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Key {
        action: KeyAction,
        key: String,
        code: String,
        text: Option<String>,
        modifiers: Vec<String>,
    },
    Touch {
        action: TouchAction,
        points: Vec<ScreenPoint>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum MouseAction {
    Move,
    Down,
    Up,
    Click,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum KeyAction {
    Down,
    Up,
    Press,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum TouchAction {
    Start,
    Move,
    End,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // Coordinates are validated here and forwarded unchanged to client-core.
struct ScreenPoint {
    x: f64,
    y: f64,
}

#[derive(Clone)]
struct SharedBridge(
    Arc<Mutex<BridgeData>>,
    Arc<tokio::sync::Mutex<()>>,
    Arc<tokio::sync::Mutex<()>>,
);

impl Default for SharedBridge {
    fn default() -> Self {
        Self(
            Arc::new(Mutex::new(BridgeData::default())),
            Arc::new(tokio::sync::Mutex::new(())),
            Arc::new(tokio::sync::Mutex::new(())),
        )
    }
}

const MAX_BUFFERED_TRACE_EVENTS: usize = 512;
const MAX_BUFFERED_TRACE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Serialize)]
struct BridgeSnapshot {
    connection: &'static str,
    error: Option<String>,
    node_id: Option<String>,
    bots: Map<String, Value>,
    chats: Map<String, Value>,
    projects: Map<String, Value>,
    messages: Map<String, Value>,
    assignments: Map<String, Value>,
    approvals: Map<String, Value>,
    questions: Map<String, Value>,
    settings: Option<Value>,
    active_host: Option<HostRecord>,
    pending_messages: BTreeMap<String, Value>,
}

fn map_values(values: &std::collections::BTreeMap<String, Value>) -> Map<String, Value> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn snapshot(data: &BridgeData) -> BridgeSnapshot {
    BridgeSnapshot {
        connection: data.connection,
        error: data.error.clone(),
        node_id: data
            .state
            .hello
            .as_ref()
            .and_then(|hello| hello.get("node_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        bots: map_values(&data.state.bots),
        chats: map_values(&data.state.chats),
        projects: map_values(&data.state.projects),
        messages: map_values(&data.state.messages),
        assignments: map_values(&data.state.assignments),
        approvals: map_values(&data.state.approvals),
        questions: map_values(&data.state.questions),
        settings: data.state.settings.clone(),
        active_host: data.active_host.clone(),
        pending_messages: data.pending_messages.clone(),
    }
}

fn emit_snapshot(app: &AppHandle, bridge: &SharedBridge) {
    if let Ok(data) = bridge.0.lock() {
        let _ = app.emit("bridge-state", snapshot(&data));
    }
}

fn connection_is_current(data: &BridgeData, generation: u64) -> bool {
    data.connection_generation == generation
}

fn trace_tauri_event(event: &str) -> Option<&'static str> {
    match event {
        "trace.item" => Some("trace-item"),
        "trace.delta" => Some("trace-delta"),
        "trace.tool_output" => Some("trace-tool-output"),
        _ => None,
    }
}

fn route_trace_event(
    data: &mut BridgeData,
    event: &str,
    payload: Value,
) -> Option<(&'static str, Value)> {
    let tauri_event = trace_tauri_event(event)?;
    let stream = payload.get("stream").and_then(Value::as_str)?;
    if let Some(subscription) = data.trace_subscription.as_mut() {
        if subscription.stream != stream {
            return None;
        }
        if event == "trace.item" {
            let aseq = payload
                .get("item")
                .and_then(|item| item.get("aseq"))
                .and_then(Value::as_u64)?;
            if aseq <= subscription.since_aseq || !subscription.seen_aseq.insert(aseq) {
                return None;
            }
        }
        return Some((tauri_event, payload));
    }
    if data.trace_subscribing.is_some() {
        let size = serde_json::to_vec(&payload)
            .map(|bytes| bytes.len().saturating_add(event.len()))
            .unwrap_or(usize::MAX);
        if data.buffered_trace_events.len() < MAX_BUFFERED_TRACE_EVENTS
            && data.buffered_trace_bytes.saturating_add(size) <= MAX_BUFFERED_TRACE_BYTES
        {
            data.buffered_trace_bytes = data.buffered_trace_bytes.saturating_add(size);
            data.buffered_trace_events.push((event.to_owned(), payload));
        } else {
            data.trace_buffer_overflow = true;
        }
    }
    None
}

fn activate_trace_subscription(
    data: &mut BridgeData,
    subscription: TraceSubscription,
) -> (Vec<(&'static str, Value)>, bool) {
    let overflow = std::mem::replace(&mut data.trace_buffer_overflow, false);
    let buffered = std::mem::take(&mut data.buffered_trace_events);
    data.buffered_trace_bytes = 0;
    data.trace_subscribing = None;
    if overflow {
        data.trace_subscription = None;
        return (Vec::new(), true);
    }

    let stream = subscription.stream.clone();
    data.trace_subscription = Some(subscription);
    let mut events = Vec::new();
    for (event, payload) in buffered {
        if payload.get("stream").and_then(Value::as_str) != Some(stream.as_str()) {
            continue;
        }
        if let Some(event) = route_trace_event(data, &event, payload) {
            events.push(event);
        }
    }
    (events, false)
}

fn finish_trace_unsubscribe(data: &mut BridgeData, stream: &str) -> bool {
    if data
        .trace_subscription
        .as_ref()
        .is_none_or(|active| active.stream != stream)
    {
        return false;
    }
    data.trace_subscription = None;
    data.trace_subscribing = None;
    data.buffered_trace_events.clear();
    data.buffered_trace_bytes = 0;
    data.trace_buffer_overflow = false;
    true
}

fn retain_trace_cleanup(data: &mut BridgeData, generation: u64, stream: String) {
    data.trace_cleanup = Some(PendingTraceCleanup { generation, stream });
}

fn apply_retry_result(
    data: &mut BridgeData,
    generation: u64,
    request_id: &str,
    succeeded: bool,
) -> bool {
    if !connection_is_current(data, generation) {
        return false;
    }
    if succeeded {
        data.pending_messages.remove(request_id);
    } else if let Some(message) = data.pending_messages.get_mut(request_id) {
        message["in_flight"] = json!(false);
    }
    true
}

fn screen_is_current(data: &BridgeData, screen_generation: u64, host_generation: u64) -> bool {
    screen_generation_matches(
        data.connection,
        data.connection_generation,
        data.screen_sessions
            .get(&screen_generation)
            .map(|session| (session.generation, session.host_generation)),
        screen_generation,
        host_generation,
    )
}

fn screen_generation_matches(
    connection: &str,
    current_host_generation: u64,
    session_generations: Option<(u64, u64)>,
    screen_generation: u64,
    host_generation: u64,
) -> bool {
    connection == "connected"
        && current_host_generation == host_generation
        && session_generations == Some((screen_generation, host_generation))
}

fn screen_client_for_generation(data: &BridgeData, generation: u64) -> Option<ScreenClient> {
    data.screen_sessions
        .get(&generation)
        .filter(|session| {
            session.generation == generation
                && session.host_generation == data.connection_generation
                && data.connection == "connected"
        })
        .map(|session| session.client.clone())
}

fn screen_connection_payload(
    status: &str,
    bot_id: &str,
    generation: u64,
    error: Option<&str>,
) -> Value {
    json!({
        "status": status,
        "bot_id": bot_id,
        "generation": generation,
        "error": error,
    })
}

fn encode_screen_frame(frame: &ScreenFrame) -> Result<Vec<u8>, String> {
    if frame.jpeg.len() > MAX_SCREEN_JPEG_BYTES {
        return Err(format!(
            "屏幕 JPEG 数据超过 {} MiB IPC 上限",
            MAX_SCREEN_JPEG_BYTES / (1024 * 1024)
        ));
    }
    let header = serde_json::to_vec(&frame.header).map_err(|error| error.to_string())?;
    let header_length = u32::try_from(header.len()).map_err(|_| "屏幕帧头过大".to_string())?;
    let mut packet = Vec::with_capacity(4 + header.len() + frame.jpeg.len());
    packet.extend_from_slice(&header_length.to_be_bytes());
    packet.extend_from_slice(&header);
    packet.extend_from_slice(&frame.jpeg);
    Ok(packet)
}

fn validate_screen_input(input: &Value) -> Result<(), String> {
    if input.get("type").and_then(Value::as_str) == Some("key") && input.get("text").is_none() {
        return Err("电脑键盘输入缺少 text 字段".into());
    }
    serde_json::from_value::<ScreenInput>(input.clone())
        .map(|_| ())
        .map_err(|error| format!("电脑输入格式无效：{error}"))
}

fn trace_subscription_state(
    status: &str,
    subscription: Option<&TraceSubscription>,
    scope: Option<&TraceScope>,
    error: Option<&str>,
) -> Value {
    let scope = subscription.map(|active| &active.scope).or(scope);
    json!({
        "status": status,
        "stream": subscription.map(|active| active.stream.as_str()),
        "assignment_id": scope.and_then(|scope| scope.assignment_id.as_deref()),
        "chat_id": scope.and_then(|scope| scope.chat_id.as_deref()),
        "in_flight": subscription.map(|active| active.in_flight.clone()).unwrap_or_default(),
        "error": error,
    })
}

fn saved_host_for_endpoint(endpoint: &str) -> Option<HostRecord> {
    HostStore::load()
        .ok()?
        .hosts()
        .iter()
        .find(|host| host.addresses.iter().any(|address| address == endpoint))
        .cloned()
}

fn restore_host_state(data: &mut BridgeData, host: HostRecord) {
    let cached_state = host
        .node_id
        .as_deref()
        .zip(state_cache::default_root().ok())
        .and_then(|(node_id, root)| state_cache::load(root, &host.id, node_id).ok().flatten());
    data.state = cached_state.map(|cache| cache.state).unwrap_or_default();
    data.pending_messages = host
        .node_id
        .as_deref()
        .and_then(|node_id| outbox::load(&host.id, node_id).ok())
        .unwrap_or_default();
    for message in data.pending_messages.values_mut() {
        message["in_flight"] = json!(false);
    }
    data.active_host = Some(host);
}

fn persist_cache(data: &BridgeData) {
    let Some(host) = &data.active_host else {
        return;
    };
    let Some(node_id) = host.node_id.as_deref() else {
        return;
    };
    let Ok(root) = state_cache::default_root() else {
        return;
    };
    let Ok(stamp) = state_cache::cache_stamp() else {
        return;
    };
    let _ = state_cache::save_ordered(root, &host.id, node_id, &data.state, stamp);
}

fn persist_outbox(data: &BridgeData) {
    let Some(host) = &data.active_host else {
        return;
    };
    let Some(node_id) = host.node_id.as_deref() else {
        return;
    };
    let _ = outbox::save(&host.id, node_id, &data.pending_messages);
}

fn pending_message(params: &Value) -> Value {
    json!({
        "id": params["client_request_id"],
        "chat_id": params["chat_id"],
        "seq": u64::MAX,
        "sender": {"kind": "user"},
        "created_at": chrono::Utc::now().to_rfc3339(),
        "reply_to": params.get("reply_to").cloned().unwrap_or(Value::Null),
        "quote": params.get("quote").cloned().unwrap_or(Value::Null),
        "quote_mode": params.get("quote_mode").cloned().unwrap_or(Value::Null),
        "deleted": false,
        "blocks": [{"type": "text", "markdown": params["text"]}],
        "fallback_text": params["text"],
        "send_status": "queued",
        "retry_params": params,
        "in_flight": true
    })
}

fn persist_local_preferences(preferences: &Value) -> Result<Value, String> {
    let mut current = local_settings::load().unwrap_or_default();
    let original_launch_at_login = current.launch_at_login;
    if let Some(theme) = preferences.get("theme").and_then(Value::as_str) {
        if !matches!(theme, "system" | "light" | "dark") {
            return Err("主题必须是 system、light 或 dark".into());
        }
        current.theme = theme.to_owned();
    }
    if let Some(notifications) = preferences.get("notifications").and_then(Value::as_bool) {
        current.notifications = notifications;
    }
    if let Some(launch_at_login) = preferences.get("launch_at_login").and_then(Value::as_bool) {
        current.launch_at_login = launch_at_login;
    }
    local_settings::save(&current).map_err(|error| error.to_string())?;
    if current.launch_at_login != original_launch_at_login {
        local_settings::set_launch_at_login(current.launch_at_login)
            .map_err(|error| error.to_string())?;
    }

    if let Some(enabled) = preferences.get("translucent").and_then(Value::as_bool) {
        local_settings::set_translucent(enabled).map_err(|error| error.to_string())?;
    }
    if let Some(enabled) = preferences.get("sound_enabled").and_then(Value::as_bool) {
        local_settings::set_sound_enabled(enabled).map_err(|error| error.to_string())?;
    }
    if let Some(enabled) = preferences.get("keep_in_menubar").and_then(Value::as_bool) {
        local_settings::set_keep_in_menubar(enabled).map_err(|error| error.to_string())?;
    }
    if let Some(size) = preferences.get("message_font_size").and_then(Value::as_str) {
        local_settings::set_message_font_size(size).map_err(|error| error.to_string())?;
    }
    if let Some(preferences_by_kind) = preferences.get("notification_preferences") {
        if let Some(object) = preferences_by_kind.as_object() {
            for (key, value) in object {
                let enabled = value
                    .as_bool()
                    .ok_or_else(|| format!("通知偏好 {key} 必须是布尔值"))?;
                local_settings::set_notification_preference(key, enabled)
                    .map_err(|error| error.to_string())?;
            }
        } else {
            return Err("notification_preferences 必须是对象".into());
        }
    }
    read_local_preferences()
}

fn read_local_preferences() -> Result<Value, String> {
    let settings = local_settings::load().unwrap_or_default();
    let notification_preferences =
        local_settings::notification_preferences().map_err(|error| error.to_string())?;
    Ok(json!({
        "theme": settings.theme,
        "notifications": settings.notifications,
        "launch_at_login": settings.launch_at_login,
        "translucent": local_settings::translucent(),
        "sound_enabled": local_settings::sound_enabled(),
        "keep_in_menubar": local_settings::keep_in_menubar(),
        "message_font_size": local_settings::message_font_size().as_str(),
        "notification_preferences": notification_preferences
    }))
}

#[tauri::command]
async fn connect_host(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    endpoint: String,
    password: String,
    name: String,
) -> Result<(), String> {
    if endpoint.trim().is_empty() {
        return Err("请输入 Bot 主机地址".into());
    }
    let endpoint = endpoint.trim().to_owned();
    let known_host = saved_host_for_endpoint(&endpoint);
    connect_to_host(app, bridge, endpoint, password, name, known_host).await
}

async fn connect_to_host(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    endpoint: String,
    password: String,
    name: String,
    known_host: Option<HostRecord>,
) -> Result<(), String> {
    let (generation, previous_generation) = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        let previous_generation = data.connection_generation;
        data.connection_generation = data.connection_generation.wrapping_add(1);
        data.connection = "connecting";
        data.error = None;
        (data.connection_generation, previous_generation)
    };
    clear_screen_for_host_switch(&app, bridge.inner(), previous_generation).await;
    clear_trace_for_host_switch(&app, bridge.inner(), previous_generation).await;
    {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection_generation != generation {
            return Err("连接请求已被更新".into());
        }
        data.connection = "connecting";
        data.error = None;
        data.state = AppState::default();
        data.active_host = known_host.clone();
        data.pending_messages.clear();
        if let Some(host) = known_host.clone() {
            restore_host_state(&mut data, host);
        }
    }
    emit_snapshot(&app, &bridge);

    let mut config = ClientConfig::new(&endpoint, &password);
    config.app_version = env!("CARGO_PKG_VERSION").into();
    if let Ok(store) = HostStore::load() {
        config.device_id = store.device_id().to_owned();
    }
    if let Some(host) = &known_host {
        config.addresses = host
            .addresses
            .iter()
            .filter(|address| *address != &endpoint)
            .cloned()
            .collect();
        config.node_id = host.node_id.clone();
        if let Ok(data) = bridge.0.lock() {
            config.last_seq = data.state.last_seq;
            config.has_cached_state = data.state.hello.as_ref().is_some_and(|hello| {
                hello.get("node_id").and_then(Value::as_str) == host.node_id.as_deref()
            });
        }
    }
    let handle = Client::spawn(config);
    {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection_generation != generation {
            return Err("连接请求已被更新".into());
        }
        data.client = Some(handle.client.clone());
    }

    let app_handle = app.clone();
    let bridge_state = bridge.inner().clone();
    tauri::async_runtime::spawn(async move {
        let mut events = handle.events;
        while let Some(event) = events.recv().await {
            let retry_after_event = match &event {
                ClientEvent::Connected { resumed, .. } => *resumed,
                ClientEvent::Bootstrap(_) => true,
                _ => false,
            };
            let mut trace_events_to_emit = Vec::new();
            let mut trace_disconnected = None;
            let mut screen_disconnected = Vec::new();
            if let Ok(mut data) = bridge_state.0.lock() {
                if !connection_is_current(&data, generation) {
                    break;
                }
                match event {
                    ClientEvent::Connected {
                        hello,
                        endpoint: connected_endpoint,
                        ..
                    } => {
                        data.connection = "connected";
                        data.error = None;
                        let node_id = hello
                            .get("node_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        data.state.hello = Some(hello);
                        if !node_id.is_empty() {
                            let mut addresses = vec![connected_endpoint];
                            if let Some(host) = &known_host {
                                for address in &host.addresses {
                                    if !addresses.contains(address) {
                                        addresses.push(address.clone());
                                    }
                                }
                            }
                            let host_name = if name.trim().is_empty() {
                                endpoint.clone()
                            } else {
                                name.trim().to_owned()
                            };
                            match HostStore::with_store(|store| {
                                store.remember(
                                    host_name,
                                    addresses,
                                    &password,
                                    Some(node_id),
                                    data.state.last_seq,
                                )
                            }) {
                                Ok(host) => {
                                    data.active_host = Some(host.clone());
                                    data.pending_messages = host
                                        .node_id
                                        .as_deref()
                                        .and_then(|node_id| outbox::load(&host.id, node_id).ok())
                                        .unwrap_or_default();
                                    for message in data.pending_messages.values_mut() {
                                        message["in_flight"] = json!(false);
                                    }
                                }
                                Err(error) => {
                                    data.error =
                                        Some(format!("连接成功，但保存 Host 失败：{error}"));
                                }
                            }
                        }
                    }
                    ClientEvent::Bootstrap(bootstrap) => {
                        data.state.apply_bootstrap(bootstrap);
                        persist_cache(&data);
                    }
                    ClientEvent::Protocol(protocol_event) => {
                        let old_seq = data.state.last_seq;
                        let event_name = protocol_event.event.clone();
                        let event_data = protocol_event.data.clone();
                        data.state.apply_event(protocol_event);
                        if let Some(routed) = route_trace_event(&mut data, &event_name, event_data)
                        {
                            trace_events_to_emit.push(routed);
                        }
                        if data.state.last_seq != old_seq {
                            persist_cache(&data);
                        }
                    }
                    ClientEvent::Disconnected { error } => {
                        data.connection = "disconnected";
                        data.error = error;
                        for message in data.pending_messages.values_mut() {
                            message["in_flight"] = json!(false);
                        }
                        persist_outbox(&data);
                        trace_disconnected = data.trace_subscription.take();
                        data.trace_subscribing = None;
                        data.buffered_trace_events.clear();
                        data.buffered_trace_bytes = 0;
                        data.trace_buffer_overflow = false;
                        screen_disconnected = std::mem::take(&mut data.screen_sessions)
                            .into_values()
                            .collect();
                        if !screen_disconnected.is_empty() {
                            data.screen_generation = data.screen_generation.wrapping_add(1);
                        }
                    }
                    ClientEvent::TransportError(error) => {
                        data.connection = "disconnected";
                        data.error = Some(error);
                        for message in data.pending_messages.values_mut() {
                            message["in_flight"] = json!(false);
                        }
                        persist_outbox(&data);
                        trace_disconnected = data.trace_subscription.take();
                        data.trace_subscribing = None;
                        data.buffered_trace_events.clear();
                        data.buffered_trace_bytes = 0;
                        data.trace_buffer_overflow = false;
                        screen_disconnected = std::mem::take(&mut data.screen_sessions)
                            .into_values()
                            .collect();
                        if !screen_disconnected.is_empty() {
                            data.screen_generation = data.screen_generation.wrapping_add(1);
                        }
                    }
                }
                for (event_name, payload) in trace_events_to_emit {
                    let _ = app_handle.emit(event_name, payload);
                }
                if let Some(subscription) = trace_disconnected {
                    emit_trace_subscription_state(
                        &app_handle,
                        "disconnected",
                        Some(&subscription),
                        None,
                        None,
                    );
                }
                for session in &screen_disconnected {
                    let _ = app_handle.emit(
                        "computer-connection",
                        screen_connection_payload(
                            "disconnected",
                            &session.bot_id,
                            session.generation,
                            None,
                        ),
                    );
                }
            }
            for session in screen_disconnected {
                let _ = session.client.close().await;
            }
            emit_snapshot(&app_handle, &bridge_state);
            if retry_after_event {
                retry_pending_messages(&app_handle, &bridge_state, generation).await;
            }
        }
    });
    Ok(())
}

async fn clear_trace_for_host_switch(
    app: &AppHandle,
    bridge: &SharedBridge,
    previous_generation: u64,
) {
    let _trace_guard = bridge.1.lock().await;
    let (client, subscription, cleanup) = match bridge.0.lock() {
        Ok(mut data) => {
            let subscription = data.trace_subscription.take();
            let cleanup = if data
                .trace_cleanup
                .as_ref()
                .is_some_and(|pending| pending.generation == previous_generation)
            {
                data.trace_cleanup.take()
            } else {
                None
            };
            data.trace_subscribing = None;
            data.buffered_trace_events.clear();
            data.buffered_trace_bytes = 0;
            data.trace_buffer_overflow = false;
            (data.client.clone(), subscription, cleanup)
        }
        Err(_) => (None, None, None),
    };
    if let Some(client) = client {
        if let Some(subscription) = subscription.as_ref() {
            let _ = client
                .request("trace.unsubscribe", json!({"stream": subscription.stream}))
                .await;
        }
        if let Some(cleanup) = cleanup {
            let _ = client
                .request("trace.unsubscribe", json!({"stream": cleanup.stream}))
                .await;
        }
    }
    if let Some(subscription) = subscription {
        let _ = app.emit(
            "trace-subscription-state",
            trace_subscription_state("unsubscribed", Some(&subscription), None, None),
        );
    }
}

async fn clear_screen_for_host_switch(
    app: &AppHandle,
    bridge: &SharedBridge,
    previous_host_generation: u64,
) {
    let _screen_guard = bridge.2.lock().await;
    let sessions = match bridge.0.lock() {
        Ok(mut data) => {
            let generations: Vec<_> = data
                .screen_sessions
                .iter()
                .filter(|(_, session)| session.host_generation == previous_host_generation)
                .map(|(generation, _)| *generation)
                .collect();
            let sessions: Vec<_> = generations
                .into_iter()
                .filter_map(|generation| data.screen_sessions.remove(&generation))
                .collect();
            if !sessions.is_empty() {
                data.screen_generation = data.screen_generation.wrapping_add(1);
            }
            sessions
        }
        Err(_) => Vec::new(),
    };
    for session in sessions {
        let _ = session.client.close().await;
        let _ = app.emit(
            "computer-connection",
            screen_connection_payload("disconnected", &session.bot_id, session.generation, None),
        );
    }
}

fn emit_trace_subscription_state(
    app: &AppHandle,
    status: &str,
    subscription: Option<&TraceSubscription>,
    scope: Option<&TraceScope>,
    error: Option<&str>,
) {
    let _ = app.emit(
        "trace-subscription-state",
        trace_subscription_state(status, subscription, scope, error),
    );
}

#[tauri::command]
async fn trace_subscribe(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    assignment_id: Option<String>,
    chat_id: Option<String>,
    since_aseq: u64,
) -> Result<Value, String> {
    if assignment_id.is_some() == chat_id.is_some() {
        return Err("trace.subscribe 需要且只能指定 assignment_id 或 chat_id".into());
    }
    let scope = TraceScope {
        assignment_id,
        chat_id,
    };
    let _trace_guard = bridge.1.lock().await;

    let (client, previous, generation, cleanup) = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        let client = data
            .client
            .clone()
            .ok_or_else(|| "尚未连接 Bot 主机".to_string())?;
        if data.connection != "connected" {
            return Err("尚未连接 Bot 主机".into());
        }
        let generation = data.connection_generation;
        if data
            .trace_cleanup
            .as_ref()
            .is_some_and(|pending| pending.generation != generation)
        {
            data.trace_cleanup = None;
        }
        let cleanup = data.trace_cleanup.take();
        let previous = data.trace_subscription.clone();
        (client, previous, generation, cleanup)
    };

    if let Some(cleanup) = cleanup {
        if let Err(error) = client
            .request("trace.unsubscribe", json!({"stream": cleanup.stream}))
            .await
        {
            if let Ok(mut data) = bridge.0.lock() {
                data.trace_cleanup = Some(cleanup);
            }
            let message = error.to_string();
            emit_trace_subscription_state(&app, "error", None, Some(&scope), Some(&message));
            return Err(message);
        }
    }

    if let Some(active) = previous.as_ref().filter(|active| active.scope == scope) {
        return Ok(trace_subscription_state(
            "subscribed",
            Some(active),
            None,
            None,
        ));
    }

    if let Some(previous) = previous {
        if let Err(error) = client
            .request("trace.unsubscribe", json!({"stream": previous.stream}))
            .await
        {
            let message = error.to_string();
            emit_trace_subscription_state(&app, "error", Some(&previous), None, Some(&message));
            return Err(message);
        }
        if let Ok(mut data) = bridge.0.lock() {
            if data
                .trace_subscription
                .as_ref()
                .is_some_and(|active| active.stream == previous.stream)
            {
                data.trace_subscription = None;
            }
        }
        emit_trace_subscription_state(&app, "unsubscribed", Some(&previous), None, None);
    }

    let can_subscribe = bridge
        .0
        .lock()
        .map(|data| data.connection_generation == generation && data.connection == "connected")
        .unwrap_or(false);
    if !can_subscribe {
        let message = "连接已断开";
        emit_trace_subscription_state(&app, "disconnected", None, Some(&scope), Some(message));
        return Err(message.into());
    }

    if let Ok(mut data) = bridge.0.lock() {
        data.trace_subscribing = None;
        data.buffered_trace_events.clear();
        data.buffered_trace_bytes = 0;
        data.trace_buffer_overflow = false;
    }

    {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        data.trace_subscribing = Some(scope.clone());
    }

    let mut params = json!({"since_aseq": since_aseq});
    if let Some(assignment_id) = &scope.assignment_id {
        params["assignment_id"] = json!(assignment_id);
    }
    if let Some(chat_id) = &scope.chat_id {
        params["chat_id"] = json!(chat_id);
    }
    let response = match client.request("trace.subscribe", params).await {
        Ok(response) => response,
        Err(error) => {
            if let Ok(mut data) = bridge.0.lock() {
                data.trace_subscribing = None;
                data.buffered_trace_events.clear();
                data.buffered_trace_bytes = 0;
                data.trace_buffer_overflow = false;
            }
            let message = error.to_string();
            emit_trace_subscription_state(&app, "error", None, Some(&scope), Some(&message));
            return Err(message);
        }
    };
    let Some(stream) = response
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| !stream.is_empty())
        .map(str::to_owned)
    else {
        if let Ok(mut data) = bridge.0.lock() {
            data.trace_subscribing = None;
            data.buffered_trace_events.clear();
            data.buffered_trace_bytes = 0;
            data.trace_buffer_overflow = false;
        }
        let message = "trace.subscribe 响应缺少 stream";
        emit_trace_subscription_state(&app, "error", None, Some(&scope), Some(message));
        return Err(message.into());
    };
    let in_flight = response
        .get("in_flight")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let subscription = TraceSubscription {
        scope: scope.clone(),
        stream: stream.clone(),
        in_flight,
        since_aseq,
        seen_aseq: BTreeSet::new(),
    };
    let (attached, overflow) = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection != "connected" {
            data.trace_subscribing = None;
            data.buffered_trace_events.clear();
            data.buffered_trace_bytes = 0;
            data.trace_buffer_overflow = false;
            (false, false)
        } else {
            let (buffered, overflow) = activate_trace_subscription(&mut data, subscription.clone());
            if overflow {
                emit_trace_subscription_state(
                    &app,
                    "error",
                    Some(&subscription),
                    None,
                    Some("trace.subscribe 实时事件缓冲已满，请重新加载轨迹"),
                );
            } else {
                emit_trace_subscription_state(&app, "subscribed", Some(&subscription), None, None);
                for (event_name, event_payload) in buffered {
                    let _ = app.emit(event_name, event_payload);
                }
            }
            (true, overflow)
        }
    };
    if !attached {
        let cleanup_error = client
            .request("trace.unsubscribe", json!({"stream": subscription.stream}))
            .await
            .err();
        if let Some(error) = cleanup_error {
            if let Ok(mut data) = bridge.0.lock() {
                retain_trace_cleanup(&mut data, generation, subscription.stream.clone());
            }
            let message = format!("连接已断开；取消服务端轨迹订阅失败：{error}");
            emit_trace_subscription_state(&app, "disconnected", None, Some(&scope), Some(&message));
        } else {
            emit_trace_subscription_state(
                &app,
                "disconnected",
                None,
                Some(&scope),
                Some("连接已断开"),
            );
        }
        return Err("连接已断开".into());
    }
    if overflow {
        let message = "trace.subscribe 实时事件缓冲已满，请重新加载轨迹";
        let cleanup_error = client
            .request("trace.unsubscribe", json!({"stream": subscription.stream}))
            .await
            .err();
        if let Some(cleanup_error) = cleanup_error {
            if let Ok(mut data) = bridge.0.lock() {
                retain_trace_cleanup(&mut data, generation, subscription.stream.clone());
            }
            let combined = format!("{message}；取消服务端订阅失败：{cleanup_error}");
            emit_trace_subscription_state(
                &app,
                "error",
                Some(&subscription),
                None,
                Some(&combined),
            );
            return Err(combined);
        }
        return Err(message.into());
    }
    Ok(trace_subscription_state(
        "subscribed",
        Some(&subscription),
        None,
        None,
    ))
}

#[tauri::command]
async fn trace_unsubscribe(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    stream: String,
) -> Result<(), String> {
    let _trace_guard = bridge.1.lock().await;
    let (client, subscription, cleanup) = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        let subscription = data
            .trace_subscription
            .as_ref()
            .filter(|active| active.stream == stream)
            .cloned();
        let cleanup = if subscription.is_none()
            && data.trace_cleanup.as_ref().is_some_and(|pending| {
                pending.generation == data.connection_generation && pending.stream == stream
            }) {
            data.trace_cleanup.take()
        } else {
            None
        };
        if subscription.is_none() && cleanup.is_none() {
            return Ok(());
        }
        (data.client.clone(), subscription, cleanup)
    };
    let active_subscription = subscription;
    let request_stream = active_subscription
        .as_ref()
        .map(|active| active.stream.as_str())
        .or_else(|| cleanup.as_ref().map(|pending| pending.stream.as_str()))
        .expect("one trace stream was selected")
        .to_owned();
    let Some(client) = client else {
        let message = "尚未连接 Bot 主机";
        if let Some(cleanup) = cleanup {
            if let Ok(mut data) = bridge.0.lock() {
                data.trace_cleanup = Some(cleanup);
            }
        }
        if let Some(subscription) = active_subscription.as_ref() {
            emit_trace_subscription_state(&app, "error", Some(subscription), None, Some(message));
        }
        return Err(message.into());
    };
    if let Err(error) = client
        .request("trace.unsubscribe", json!({"stream": request_stream}))
        .await
    {
        let message = error.to_string();
        if let Some(cleanup) = cleanup {
            if let Ok(mut data) = bridge.0.lock() {
                data.trace_cleanup = Some(cleanup);
            }
        }
        if let Some(subscription) = active_subscription.as_ref() {
            emit_trace_subscription_state(&app, "error", Some(subscription), None, Some(&message));
        }
        return Err(message);
    }
    let removed = if let Ok(mut data) = bridge.0.lock() {
        if let Some(subscription) = active_subscription.as_ref() {
            finish_trace_unsubscribe(&mut data, &subscription.stream)
        } else {
            true
        }
    } else {
        false
    };
    if !removed {
        return Ok(());
    }
    if let Some(subscription) = active_subscription.as_ref() {
        emit_trace_subscription_state(&app, "unsubscribed", Some(subscription), None, None);
    }
    Ok(())
}

fn take_screen_session(
    data: &mut BridgeData,
    screen_generation: u64,
    host_generation: u64,
) -> Option<ScreenSession> {
    if !screen_is_current(data, screen_generation, host_generation) {
        return None;
    }
    let session = data.screen_sessions.remove(&screen_generation);
    if session.is_some() {
        data.screen_generation = data.screen_generation.wrapping_add(1);
    }
    session
}

fn spawn_screen_event_pump(
    app: AppHandle,
    bridge: SharedBridge,
    pump: ScreenPump,
    mut events: tokio::sync::mpsc::Receiver<ScreenEvent>,
) {
    tauri::async_runtime::spawn(async move {
        let ScreenPump {
            bot_id,
            generation: screen_generation,
            host_generation,
            client,
            frame_channel,
        } = pump;
        while let Some(event) = events.recv().await {
            match event {
                ScreenEvent::State(state) => {
                    let Ok(data) = bridge.0.lock() else {
                        break;
                    };
                    if !screen_is_current(&data, screen_generation, host_generation) {
                        break;
                    }
                    let _ = app.emit(
                        "computer-connection",
                        screen_connection_payload("connected", &bot_id, screen_generation, None),
                    );
                    let _ = app.emit(
                        "computer-state",
                        json!({"generation": screen_generation, "state": state}),
                    );
                }
                ScreenEvent::Frame(frame) => {
                    let packet = match encode_screen_frame(&frame) {
                        Ok(packet) => packet,
                        Err(error) => {
                            let session = bridge.0.lock().ok().and_then(|mut data| {
                                take_screen_session(&mut data, screen_generation, host_generation)
                            });
                            if let Some(session) = session {
                                let _ = app.emit(
                                    "computer-connection",
                                    screen_connection_payload(
                                        "error",
                                        &bot_id,
                                        screen_generation,
                                        Some(&error),
                                    ),
                                );
                                let _ = session.client.close().await;
                            }
                            break;
                        }
                    };
                    let mut disconnect = None;
                    let send_result = if let Ok(mut data) = bridge.0.lock() {
                        if !screen_is_current(&data, screen_generation, host_generation) {
                            break;
                        }
                        match frame_channel.send(Response::new(packet)) {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                disconnect = take_screen_session(
                                    &mut data,
                                    screen_generation,
                                    host_generation,
                                );
                                let message = format!("电脑画面 IPC 发送失败：{error}");
                                let _ = app.emit(
                                    "computer-connection",
                                    screen_connection_payload(
                                        "error",
                                        &bot_id,
                                        screen_generation,
                                        Some(&message),
                                    ),
                                );
                                Err(message)
                            }
                        }
                    } else {
                        break;
                    };
                    if send_result.is_err() {
                        if let Some(session) = disconnect {
                            let _ = session.client.close().await;
                        } else {
                            let _ = client.close().await;
                        }
                        break;
                    }
                }
                ScreenEvent::Closed => {
                    let session = bridge.0.lock().ok().and_then(|mut data| {
                        take_screen_session(&mut data, screen_generation, host_generation)
                    });
                    if let Some(session) = session {
                        let _ = app.emit(
                            "computer-connection",
                            screen_connection_payload(
                                "disconnected",
                                &bot_id,
                                screen_generation,
                                None,
                            ),
                        );
                        let _ = session.client.close().await;
                    }
                    break;
                }
                ScreenEvent::Error(error) => {
                    let session = bridge.0.lock().ok().and_then(|mut data| {
                        take_screen_session(&mut data, screen_generation, host_generation)
                    });
                    if let Some(session) = session {
                        let _ = app.emit(
                            "computer-connection",
                            screen_connection_payload(
                                "error",
                                &bot_id,
                                screen_generation,
                                Some(&error),
                            ),
                        );
                        let _ = session.client.close().await;
                    }
                    break;
                }
            }
        }
    });
}

#[tauri::command]
async fn screen_connect(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    bot_id: String,
    quality: String,
    tab_id: Option<String>,
    frame_channel: Channel<Response>,
) -> Result<u64, String> {
    if bot_id.trim().is_empty() {
        return Err("Bot id 不能为空".into());
    }
    if !matches!(quality.as_str(), "auto" | "high" | "low") {
        return Err("不支持的电脑画面画质".into());
    }
    let _screen_guard = bridge.2.lock().await;
    let (host, host_generation) = {
        let data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection != "connected" {
            return Err("请先连接 Bot 主机".into());
        }
        let host = data
            .active_host
            .clone()
            .ok_or_else(|| "当前连接没有已保存的 Host".to_string())?;
        (host, data.connection_generation)
    };
    let endpoint = host
        .addresses
        .first()
        .cloned()
        .ok_or_else(|| "当前 Host 没有可用地址".to_string())?;
    let password = HostStore::load()
        .and_then(|store| store.password(&host))
        .map_err(|error| format!("无法读取主机凭证：{error}"))?;
    let config = ClientConfig::new(&endpoint, &password);

    let screen_generation = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection != "connected" || data.connection_generation != host_generation {
            return Err("Host 连接已变化，请重试".into());
        }
        data.screen_generation = data.screen_generation.wrapping_add(1);
        data.screen_generation
    };

    let handle = ScreenHandle::spawn(config, bot_id.clone(), quality, tab_id);
    let client = handle.client.clone();
    let accepted = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection != "connected" || data.connection_generation != host_generation {
            false
        } else {
            data.screen_sessions.insert(
                screen_generation,
                ScreenSession {
                    bot_id: bot_id.clone(),
                    generation: screen_generation,
                    host_generation,
                    client: client.clone(),
                },
            );
            true
        }
    };
    if !accepted {
        let _ = client.close().await;
        return Err("Host 连接已变化，请重试".into());
    }
    let _ = app.emit(
        "computer-connection",
        screen_connection_payload("connecting", &bot_id, screen_generation, None),
    );
    spawn_screen_event_pump(
        app,
        bridge.inner().clone(),
        ScreenPump {
            bot_id,
            generation: screen_generation,
            host_generation,
            client,
            frame_channel,
        },
        handle.events,
    );
    Ok(screen_generation)
}

#[tauri::command]
async fn screen_disconnect(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    generation: u64,
) -> Result<(), String> {
    let _screen_guard = bridge.2.lock().await;
    let session = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if !data.screen_sessions.contains_key(&generation) {
            return Ok(());
        }
        let session = data.screen_sessions.remove(&generation);
        data.screen_generation = data.screen_generation.wrapping_add(1);
        session
    };
    if let Some(session) = session {
        let _ = session.client.close().await;
        let _ = app.emit(
            "computer-connection",
            screen_connection_payload("disconnected", &session.bot_id, generation, None),
        );
    }
    Ok(())
}

#[tauri::command]
async fn screen_ack(
    bridge: State<'_, SharedBridge>,
    generation: u64,
    seq: u64,
) -> Result<(), String> {
    let client = {
        let data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        screen_client_for_generation(&data, generation)
    };
    if let Some(client) = client {
        client.ack(seq).await.map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn screen_switch_tab(
    bridge: State<'_, SharedBridge>,
    generation: u64,
    tab_id: String,
) -> Result<(), String> {
    let client = {
        let data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        screen_client_for_generation(&data, generation)
    }
    .ok_or_else(|| "电脑画面连接已过期".to_string())?;
    client
        .switch_tab(tab_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn screen_input(
    bridge: State<'_, SharedBridge>,
    generation: u64,
    event: Value,
) -> Result<(), String> {
    validate_screen_input(&event)?;
    let client = {
        let data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        screen_client_for_generation(&data, generation)
    }
    .ok_or_else(|| "电脑画面连接已过期".to_string())?;
    client.input(event).await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    chat_id: String,
    text: String,
) -> Result<(), String> {
    let shared_bridge = bridge.inner().clone();
    let (client, request_id, params) = {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        let client = data
            .client
            .clone()
            .ok_or_else(|| "尚未连接 Bot 主机".to_string())?;
        let request_id = Uuid::new_v4().to_string();
        let params = json!({
            "chat_id": chat_id,
            "text": text,
            "mentions": [],
            "client_request_id": request_id,
        });
        data.pending_messages
            .insert(request_id.clone(), pending_message(&params));
        persist_outbox(&data);
        (client, request_id, params)
    };
    let result = client.request("chat.send", params).await;
    if let Ok(mut data) = shared_bridge.0.lock() {
        if result.is_ok() {
            data.pending_messages.remove(&request_id);
        } else if let Some(message) = data.pending_messages.get_mut(&request_id) {
            message["in_flight"] = json!(false);
        }
        persist_outbox(&data);
        let _ = app.emit("bridge-state", snapshot(&data));
    }
    result.map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
fn list_hosts() -> Result<Vec<HostRecord>, String> {
    HostStore::load()
        .map(|store| store.hosts().to_vec())
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn connect_saved_host(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    host_id: String,
) -> Result<(), String> {
    let store = HostStore::load().map_err(|error| error.to_string())?;
    let host = store
        .get(&host_id)
        .cloned()
        .ok_or_else(|| "找不到已保存的 Host".to_string())?;
    let password = store.password(&host).map_err(|error| error.to_string())?;
    let endpoint = host
        .addresses
        .first()
        .cloned()
        .ok_or_else(|| "Host 没有可用地址".to_string())?;
    let name = host.name.clone();
    connect_to_host(app, bridge, endpoint, password, name, Some(host)).await
}

async fn retry_pending_messages(app: &AppHandle, bridge: &SharedBridge, generation: u64) {
    loop {
        let next = {
            let Ok(mut data) = bridge.0.lock() else {
                return;
            };
            if !connection_is_current(&data, generation) {
                return;
            }
            let Some((id, message)) = data.pending_messages.iter_mut().find(|(_, message)| {
                message["send_status"] == "queued" && message["in_flight"] != true
            }) else {
                return;
            };
            message["in_flight"] = json!(true);
            let request = message["retry_params"].clone();
            let id = id.clone();
            persist_outbox(&data);
            (data.client.clone(), id, request)
        };
        let Some(client) = next.0 else {
            return;
        };
        let result = client.request("chat.send", next.2).await;
        if let Ok(mut data) = bridge.0.lock() {
            if !apply_retry_result(&mut data, generation, &next.1, result.is_ok()) {
                return;
            }
            persist_outbox(&data);
            let _ = app.emit("bridge-state", snapshot(&data));
        }
        if result.is_err() {
            return;
        }
    }
}

#[tauri::command]
fn remove_host(host_id: String, bridge: State<'_, SharedBridge>) -> Result<(), String> {
    let mut store = HostStore::load().map_err(|error| error.to_string())?;
    store.remove(&host_id).map_err(|error| error.to_string())?;
    if let Ok(mut data) = bridge.0.lock() {
        if data
            .active_host
            .as_ref()
            .is_some_and(|host| host.id == host_id)
        {
            data.active_host = None;
            data.pending_messages.clear();
        }
    }
    Ok(())
}

#[tauri::command]
fn get_local_preferences() -> Result<Value, String> {
    read_local_preferences()
}

#[tauri::command]
fn save_local_preferences(preferences: Value) -> Result<Value, String> {
    persist_local_preferences(&preferences)
}

#[tauri::command]
fn get_pending_messages(
    bridge: State<'_, SharedBridge>,
) -> Result<BTreeMap<String, Value>, String> {
    bridge
        .0
        .lock()
        .map(|data| data.pending_messages.clone())
        .map_err(|_| "客户端状态不可用".to_string())
}

#[tauri::command]
fn get_bridge_state(bridge: State<'_, SharedBridge>) -> Result<BridgeSnapshot, String> {
    let data = bridge
        .0
        .lock()
        .map_err(|_| "客户端状态不可用".to_string())?;
    Ok(snapshot(&data))
}

#[tauri::command]
async fn set_window_material(app: AppHandle, translucent: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不可用".to_string())?;
    #[cfg(target_os = "macos")]
    {
        let translucent = translucent && !local_settings::reduce_transparency_enabled();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let task_window = window.clone();
        window
            .run_on_main_thread(move || {
                let result = window_vibrancy::clear_vibrancy(&task_window)
                    .map_err(|error| format!("无法清除旧材质：{error}"))
                    .and_then(|_| {
                        if translucent {
                            window_vibrancy::apply_vibrancy(
                                &task_window,
                                window_vibrancy::NSVisualEffectMaterial::Sidebar,
                                Some(
                                    window_vibrancy::NSVisualEffectState::FollowsWindowActiveState,
                                ),
                                Some(40.0),
                            )
                            .map_err(|error| format!("无法启用半透明材质：{error}"))
                        } else {
                            Ok(())
                        }
                    });
                let _ = sender.send(result);
            })
            .map_err(|error| format!("无法更新窗口材质：{error}"))?;
        receiver
            .await
            .map_err(|error| format!("窗口材质更新未完成：{error}"))??;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, translucent);
    Ok(())
}

#[tauri::command]
async fn protocol_request(
    bridge: State<'_, SharedBridge>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    let client = bridge
        .0
        .lock()
        .map_err(|_| "客户端状态不可用".to_string())?
        .client
        .clone()
        .ok_or_else(|| "尚未连接 Bot 主机".to_string())?;
    client
        .request(&method, params)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_usage_csv(app: AppHandle, file_name: String, csv: String) -> Result<bool, String> {
    let Some(file_path) = app
        .dialog()
        .file()
        .set_file_name(&file_name)
        .add_filter("CSV", &["csv"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = file_path
        .into_path()
        .map_err(|error| format!("无法读取所选文件路径：{error}"))?;
    std::fs::write(path, csv.as_bytes()).map_err(|error| format!("无法保存 CSV：{error}"))?;
    Ok(true)
}

fn skill_upload_payload(
    body: &InvokeBody,
    encoded_file_name: Option<&str>,
) -> Result<(String, Vec<u8>), String> {
    let file_name = encoded_file_name
        .map(percent_encoding::percent_decode_str)
        .ok_or_else(|| "技能压缩包文件名缺失".to_string())?
        .decode_utf8()
        .map_err(|error| format!("技能压缩包文件名无效：{error}"))?
        .to_string();
    let file_name = file_name
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "技能压缩包文件名不能为空".to_string())?
        .to_owned();
    if !file_name.to_ascii_lowercase().ends_with(".zip") {
        return Err("请选择 ZIP 格式的技能包".into());
    }
    let bytes = match body {
        InvokeBody::Raw(bytes) => bytes.clone(),
        InvokeBody::Json(_) => return Err("技能压缩包内容格式无效".into()),
    };
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err("技能压缩包不能超过 100 MB".into());
    }
    Ok((file_name, bytes))
}

fn uploaded_skill_id(body: &Value) -> Result<String, String> {
    body.get("upload_id")
        .and_then(Value::as_str)
        .filter(|upload_id| !upload_id.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "技能包上传响应缺少 upload_id".to_string())
}

#[tauri::command]
async fn upload_skill_zip(
    bridge: State<'_, SharedBridge>,
    request: Request<'_>,
) -> Result<String, String> {
    let encoded_file_name = request
        .headers()
        .get("x-file-name")
        .and_then(|value| value.to_str().ok());
    let (file_name, bytes) = skill_upload_payload(request.body(), encoded_file_name)?;

    let (host, endpoint) = {
        let data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        if data.connection != "connected" {
            return Err("请先连接 Bot 主机".into());
        }
        let host = data
            .active_host
            .clone()
            .ok_or_else(|| "当前连接没有已保存的主机凭证".to_string())?;
        let endpoint = host
            .addresses
            .first()
            .cloned()
            .ok_or_else(|| "当前主机没有可用地址".to_string())?;
        (host, endpoint)
    };
    let password = HostStore::load()
        .and_then(|store| store.password(&host))
        .map_err(|error| format!("无法读取主机凭证：{error}"))?;
    let url = macbot_client_core::http_url(&endpoint, "/api/v1/uploads")
        .map_err(|error| error.to_string())?;
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(file_name)
        .mime_str("application/zip")
        .map_err(|error| error.to_string())?;
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(password)
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .map_err(|error| format!("技能包上传失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("技能包上传失败：{error}"))?;
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("无法读取技能包上传响应：{error}"))?;
    uploaded_skill_id(&body)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(SharedBridge::default())
        .invoke_handler(tauri::generate_handler![
            connect_host,
            connect_saved_host,
            list_hosts,
            remove_host,
            send_message,
            protocol_request,
            save_usage_csv,
            upload_skill_zip,
            trace_subscribe,
            trace_unsubscribe,
            screen_connect,
            screen_disconnect,
            screen_ack,
            screen_input,
            screen_switch_tab,
            get_bridge_state,
            get_local_preferences,
            save_local_preferences,
            get_pending_messages,
            set_window_material
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Mac Bot desktop application");
}

#[cfg(test)]
mod tests {
    use super::{
        activate_trace_subscription, apply_retry_result, connection_is_current,
        encode_screen_frame, finish_trace_unsubscribe, retain_trace_cleanup, route_trace_event,
        screen_connection_payload, screen_generation_matches, skill_upload_payload,
        trace_subscription_state, trace_tauri_event, uploaded_skill_id, validate_screen_input,
        BridgeData, InvokeBody, PendingTraceCleanup, ScreenFrame, ScreenPoint, TraceScope,
        TraceSubscription, MAX_BUFFERED_TRACE_BYTES, MAX_SCREEN_JPEG_BYTES,
    };
    use macbot_client_core::ScreenFrameHeader;
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn raw_upload_bytes_keep_the_decoded_file_name() {
        let (file_name, bytes) = skill_upload_payload(
            &InvokeBody::Raw(vec![80, 75, 3, 4]),
            Some("skill%20bundle.zip"),
        )
        .unwrap();
        assert_eq!(file_name, "skill bundle.zip");
        assert_eq!(bytes, [80, 75, 3, 4]);
    }

    #[test]
    fn upload_payload_rejects_json_or_missing_names() {
        assert!(skill_upload_payload(&InvokeBody::Json(json!([1, 2])), Some("x.zip")).is_err());
        assert!(skill_upload_payload(&InvokeBody::Raw(vec![]), None).is_err());
        assert!(skill_upload_payload(&InvokeBody::Raw(vec![]), Some("x.txt")).is_err());
    }

    #[test]
    fn upload_response_requires_a_non_empty_id() {
        assert_eq!(
            uploaded_skill_id(&json!({"upload_id":"upl_1"})).unwrap(),
            "upl_1"
        );
        assert!(uploaded_skill_id(&json!({"upload_id":"  "})).is_err());
        assert!(uploaded_skill_id(&json!({})).is_err());
    }

    #[test]
    fn trace_event_routing_filters_stream_and_duplicate_items() {
        let mut data = BridgeData {
            trace_subscription: Some(TraceSubscription {
                scope: TraceScope {
                    assignment_id: Some("a1".into()),
                    chat_id: None,
                },
                stream: "s1".into(),
                in_flight: vec![],
                since_aseq: 4,
                seen_aseq: BTreeSet::new(),
            }),
            ..BridgeData::default()
        };

        assert!(route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"other", "item":{"aseq":5}}),
        )
        .is_none());
        assert!(route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":4}}),
        )
        .is_none());
        let (event, payload) = route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":5}}),
        )
        .unwrap();
        assert_eq!(event, "trace-item");
        assert_eq!(payload["item"]["aseq"], 5);
        assert!(route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":5}}),
        )
        .is_none());
        let (event, payload) = route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":7}}),
        )
        .unwrap();
        assert_eq!(event, "trace-item");
        assert_eq!(payload["item"]["aseq"], 7);
        let (event, payload) = route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":6}}),
        )
        .unwrap();
        assert_eq!(event, "trace-item");
        assert_eq!(payload["item"]["aseq"], 6);
        assert!(route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":7}}),
        )
        .is_none());
    }

    #[test]
    fn trace_events_are_buffered_until_subscribe_returns() {
        let mut data = BridgeData {
            trace_subscribing: Some(TraceScope {
                assignment_id: Some("a1".into()),
                chat_id: None,
            }),
            ..BridgeData::default()
        };
        assert!(route_trace_event(
            &mut data,
            "trace.delta",
            json!({"stream":"s1", "request_id":"r1", "text":"hello"}),
        )
        .is_none());
        assert_eq!(data.buffered_trace_events.len(), 1);
        assert!(route_trace_event(&mut data, "trace.unknown", json!({"stream":"s1"}),).is_none());
        assert_eq!(data.buffered_trace_events.len(), 1);
    }

    #[test]
    fn buffered_items_are_committed_before_live_items_can_advance_cursor() {
        let mut data = BridgeData {
            trace_subscribing: Some(TraceScope {
                assignment_id: Some("a1".into()),
                chat_id: None,
            }),
            buffered_trace_events: vec![(
                "trace.item".into(),
                json!({"stream":"s1", "item":{"aseq":5}}),
            )],
            buffered_trace_bytes: 64,
            ..BridgeData::default()
        };
        let subscription = TraceSubscription {
            scope: TraceScope {
                assignment_id: Some("a1".into()),
                chat_id: None,
            },
            stream: "s1".into(),
            in_flight: vec![],
            since_aseq: 4,
            seen_aseq: BTreeSet::new(),
        };
        let (buffered, overflow) = activate_trace_subscription(&mut data, subscription);
        assert!(!overflow);
        assert_eq!(buffered.len(), 1);
        assert_eq!(buffered[0].1["item"]["aseq"], 5);
        let (live_event, live_payload) = route_trace_event(
            &mut data,
            "trace.item",
            json!({"stream":"s1", "item":{"aseq":6}}),
        )
        .unwrap();
        assert_eq!(live_event, "trace-item");
        assert_eq!(live_payload["item"]["aseq"], 6);
    }

    #[test]
    fn trace_buffer_has_an_aggregate_byte_limit() {
        let mut data = BridgeData {
            trace_subscribing: Some(TraceScope {
                assignment_id: Some("a1".into()),
                chat_id: None,
            }),
            ..BridgeData::default()
        };
        let oversized = "x".repeat(MAX_BUFFERED_TRACE_BYTES);
        assert!(route_trace_event(
            &mut data,
            "trace.delta",
            json!({"stream":"s1", "text":oversized}),
        )
        .is_none());
        assert!(data.buffered_trace_events.is_empty());
        assert_eq!(data.buffered_trace_bytes, 0);
        assert!(data.trace_buffer_overflow);
    }

    #[test]
    fn stale_connection_generations_are_rejected() {
        let data = BridgeData {
            connection_generation: 8,
            ..BridgeData::default()
        };
        assert!(connection_is_current(&data, 8));
        assert!(!connection_is_current(&data, 7));
    }

    #[test]
    fn unsubscribe_failure_leaves_stream_available_for_retry() {
        let mut data = BridgeData {
            trace_subscription: Some(TraceSubscription {
                scope: TraceScope {
                    assignment_id: Some("a1".into()),
                    chat_id: None,
                },
                stream: "s1".into(),
                in_flight: vec![],
                since_aseq: 2,
                seen_aseq: BTreeSet::new(),
            }),
            ..BridgeData::default()
        };
        // Failed requests do not call finish_trace_unsubscribe and retain the stream.
        assert!(data.trace_subscription.is_some());
        assert!(!finish_trace_unsubscribe(&mut data, "different-stream"));
        assert_eq!(data.trace_subscription.as_ref().unwrap().stream, "s1");
        assert!(finish_trace_unsubscribe(&mut data, "s1"));
        assert!(data.trace_subscription.is_none());
    }

    #[test]
    fn failed_overflow_cleanup_retains_stream_for_retry() {
        let mut data = BridgeData {
            connection_generation: 9,
            ..BridgeData::default()
        };
        retain_trace_cleanup(&mut data, 9, "s-orphan".into());
        assert_eq!(
            data.trace_cleanup,
            Some(PendingTraceCleanup {
                generation: 9,
                stream: "s-orphan".into(),
            })
        );
    }

    #[test]
    fn stale_retry_results_cannot_mutate_new_host_outbox() {
        let mut data = BridgeData {
            connection_generation: 4,
            ..BridgeData::default()
        };
        data.pending_messages
            .insert("same-id".into(), json!({"in_flight":true}));
        assert!(!apply_retry_result(&mut data, 3, "same-id", true));
        assert_eq!(data.pending_messages["same-id"]["in_flight"], true);
        assert!(apply_retry_result(&mut data, 4, "same-id", true));
        assert!(!data.pending_messages.contains_key("same-id"));
    }

    #[test]
    fn screen_generation_rejects_old_host_and_screen_sessions() {
        assert!(screen_generation_matches(
            "connected",
            4,
            Some((7, 4)),
            7,
            4
        ));
        assert!(!screen_generation_matches(
            "connected",
            5,
            Some((7, 4)),
            7,
            4
        ));
        assert!(!screen_generation_matches(
            "disconnected",
            4,
            Some((7, 4)),
            7,
            4
        ));
        assert!(!screen_generation_matches(
            "connected",
            4,
            Some((6, 4)),
            7,
            4
        ));
    }

    #[test]
    fn screen_frame_channel_packet_preserves_wire_header_and_jpeg() {
        let frame = ScreenFrame {
            header: ScreenFrameHeader {
                seq: 42,
                tab_id: "tab-1".into(),
                w: 1280,
                h: 720,
                ts: 1234,
                url: "https://example.test".into(),
            },
            jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
        };
        let packet = encode_screen_frame(&frame).unwrap();
        let parsed = macbot_client_core::parse_screen_frame(&packet).unwrap();
        assert_eq!(parsed.header, frame.header);
        assert_eq!(parsed.jpeg, frame.jpeg);
    }

    #[test]
    fn oversized_screen_frame_is_rejected_before_ipc() {
        let frame = ScreenFrame {
            header: ScreenFrameHeader {
                seq: 1,
                tab_id: "tab-1".into(),
                w: 1600,
                h: 900,
                ts: 0,
                url: String::new(),
            },
            jpeg: vec![0; MAX_SCREEN_JPEG_BYTES + 1],
        };
        assert!(encode_screen_frame(&frame).is_err());
    }

    #[test]
    fn screen_input_accepts_protocol_variants_and_rejects_unknown_types() {
        for event in [
            json!({"type":"mouse", "action":"click", "x":1, "y":2, "button":"left", "click_count":1}),
            json!({"type":"wheel", "x":1, "y":2, "dx":0, "dy":-4}),
            json!({"type":"key", "action":"press", "key":"a", "code":"KeyA", "text":null, "modifiers":[]}),
            json!({"type":"touch", "action":"start", "points":[{"x":1.5,"y":2}]}),
        ] {
            assert!(validate_screen_input(&event).is_ok());
        }
        assert!(validate_screen_input(&json!({"type":"exec"})).is_err());
        assert!(validate_screen_input(&json!({})).is_err());
        assert!(validate_screen_input(&json!({"type":"mouse", "action":"hover", "x":1, "y":2, "button":"left", "click_count":0})).is_err());
        assert!(validate_screen_input(&json!({"type":"mouse", "action":"click", "x":1, "y":2, "button":"left", "click_count":1, "extra":true})).is_err());
        assert!(validate_screen_input(
            &json!({"type":"key", "action":"press", "key":"a", "code":"KeyA", "modifiers":[]})
        )
        .is_err());
    }

    #[test]
    fn screen_point_requires_both_protocol_coordinates() {
        assert!(serde_json::from_value::<ScreenPoint>(json!({"x":1,"y":2})).is_ok());
        assert!(serde_json::from_value::<ScreenPoint>(json!({"x":1})).is_err());
        assert!(serde_json::from_value::<ScreenPoint>(json!({"x":1,"y":2,"z":3})).is_err());
    }

    #[test]
    fn screen_connection_event_has_stable_generation_fields() {
        let payload = screen_connection_payload("connected", "bot-1", 3, None);
        assert_eq!(payload["status"], "connected");
        assert_eq!(payload["bot_id"], "bot-1");
        assert_eq!(payload["generation"], 3);
        assert!(payload["error"].is_null());
    }

    #[test]
    fn trace_protocol_events_have_stable_tauri_names_and_payloads() {
        assert_eq!(trace_tauri_event("trace.item"), Some("trace-item"));
        assert_eq!(trace_tauri_event("trace.delta"), Some("trace-delta"));
        assert_eq!(
            trace_tauri_event("trace.tool_output"),
            Some("trace-tool-output")
        );
        assert_eq!(trace_tauri_event("trace.unknown"), None);

        let mut data = BridgeData {
            trace_subscription: Some(TraceSubscription {
                scope: TraceScope {
                    assignment_id: Some("a1".into()),
                    chat_id: None,
                },
                stream: "s1".into(),
                in_flight: vec![],
                since_aseq: 0,
                seen_aseq: BTreeSet::new(),
            }),
            ..BridgeData::default()
        };
        let (event, payload) = route_trace_event(
            &mut data,
            "trace.delta",
            json!({"stream":"s1", "request_id":"r1", "channel":"analysis", "text":"x"}),
        )
        .unwrap();
        assert_eq!(event, "trace-delta");
        assert_eq!(payload["request_id"], "r1");
        let (event, payload) = route_trace_event(
            &mut data,
            "trace.tool_output",
            json!({"stream":"s1", "call_id":"call1", "chunk":"out"}),
        )
        .unwrap();
        assert_eq!(event, "trace-tool-output");
        assert_eq!(payload["chunk"], "out");
    }

    #[test]
    fn trace_subscription_state_has_stable_shape() {
        let subscription = TraceSubscription {
            scope: TraceScope {
                assignment_id: None,
                chat_id: Some("c1".into()),
            },
            stream: "s1".into(),
            in_flight: vec![json!({"request_id":"r1", "text":"prompt", "thinking":""})],
            since_aseq: 0,
            seen_aseq: BTreeSet::new(),
        };
        let state = trace_subscription_state("subscribed", Some(&subscription), None, None);
        assert_eq!(state["status"], "subscribed");
        assert_eq!(state["stream"], "s1");
        assert_eq!(state["chat_id"], "c1");
        assert_eq!(state["in_flight"].as_array().unwrap().len(), 1);
        assert!(state["error"].is_null());
    }
}
