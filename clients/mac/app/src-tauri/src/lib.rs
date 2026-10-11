use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use macbot_client_core::{AppState, Client, ClientConfig, ClientEvent};
use macbot_desktop_services::{
    host_storage::{HostRecord, HostStore},
    local_settings,
    outbox::{self, PendingMessages},
    state_cache,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{
    ipc::{InvokeBody, Request},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

struct BridgeData {
    connection: &'static str,
    error: Option<String>,
    state: AppState,
    client: Option<Client>,
    active_host: Option<HostRecord>,
    pending_messages: PendingMessages,
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
        }
    }
}

#[derive(Clone, Default)]
struct SharedBridge(Arc<Mutex<BridgeData>>);

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
    {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
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
            if let Ok(mut data) = bridge_state.0.lock() {
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
                        data.state.apply_event(protocol_event);
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
                    }
                    ClientEvent::TransportError(error) => {
                        data.connection = "disconnected";
                        data.error = Some(error);
                        for message in data.pending_messages.values_mut() {
                            message["in_flight"] = json!(false);
                        }
                        persist_outbox(&data);
                    }
                }
            }
            emit_snapshot(&app_handle, &bridge_state);
            if retry_after_event {
                retry_pending_messages(&app_handle, &bridge_state).await;
            }
        }
    });
    Ok(())
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

async fn retry_pending_messages(app: &AppHandle, bridge: &SharedBridge) {
    loop {
        let next = {
            let Ok(mut data) = bridge.0.lock() else {
                return;
            };
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
            if result.is_ok() {
                data.pending_messages.remove(&next.1);
            } else if let Some(message) = data.pending_messages.get_mut(&next.1) {
                message["in_flight"] = json!(false);
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
    use super::{skill_upload_payload, uploaded_skill_id, InvokeBody};
    use serde_json::json;

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
}
