use std::sync::{Arc, Mutex};

use macbot_client_core::{AppState, Client, ClientConfig, ClientEvent};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};

struct BridgeData {
    connection: &'static str,
    error: Option<String>,
    state: AppState,
    client: Option<Client>,
}

impl Default for BridgeData {
    fn default() -> Self {
        Self {
            connection: "disconnected",
            error: None,
            state: AppState::default(),
            client: None,
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
    settings: Option<Value>,
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
        settings: data.state.settings.clone(),
    }
}

fn emit_snapshot(app: &AppHandle, bridge: &SharedBridge) {
    if let Ok(data) = bridge.0.lock() {
        let _ = app.emit("bridge-state", snapshot(&data));
    }
}

#[tauri::command]
async fn connect_host(
    app: AppHandle,
    bridge: State<'_, SharedBridge>,
    endpoint: String,
    password: String,
    _name: String,
) -> Result<(), String> {
    eprintln!(
        "tauri-bridge: connect requested endpoint={}",
        endpoint.trim()
    );
    if endpoint.trim().is_empty() {
        return Err("请输入 Bot 主机地址".into());
    }
    {
        let mut data = bridge
            .0
            .lock()
            .map_err(|_| "客户端状态不可用".to_string())?;
        data.connection = "connecting";
        data.error = None;
    }
    emit_snapshot(&app, &bridge);

    let mut config = ClientConfig::new(endpoint.trim(), password);
    config.app_version = env!("CARGO_PKG_VERSION").into();
    let handle = Client::spawn(config);
    eprintln!("tauri-bridge: client worker spawned");
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
            eprintln!(
                "tauri-bridge: received client event: {}",
                match &event {
                    ClientEvent::Connected { .. } => "connected",
                    ClientEvent::Bootstrap(_) => "bootstrap",
                    ClientEvent::Protocol(_) => "protocol",
                    ClientEvent::Disconnected { .. } => "disconnected",
                    ClientEvent::TransportError(_) => "transport-error",
                }
            );
            if let Ok(mut data) = bridge_state.0.lock() {
                match event {
                    ClientEvent::Connected { hello, .. } => {
                        data.connection = "connected";
                        data.error = None;
                        data.state.hello = Some(hello);
                    }
                    ClientEvent::Bootstrap(bootstrap) => data.state.apply_bootstrap(bootstrap),
                    ClientEvent::Protocol(protocol_event) => data.state.apply_event(protocol_event),
                    ClientEvent::Disconnected { error } => {
                        data.connection = "disconnected";
                        data.error = error;
                    }
                    ClientEvent::TransportError(error) => {
                        data.connection = "disconnected";
                        data.error = Some(error);
                    }
                }
            }
            emit_snapshot(&app_handle, &bridge_state);
        }
        eprintln!("tauri-bridge: client event channel closed");
    });
    Ok(())
}

#[tauri::command]
async fn send_message(
    bridge: State<'_, SharedBridge>,
    chat_id: String,
    text: String,
) -> Result<(), String> {
    let client = bridge
        .0
        .lock()
        .map_err(|_| "客户端状态不可用".to_string())?
        .client
        .clone()
        .ok_or_else(|| "尚未连接 Bot 主机".to_string())?;
    client
        .request(
            "chat.send",
            json!({"chat_id": chat_id, "text": text, "mentions": []}),
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(SharedBridge::default())
        .invoke_handler(tauri::generate_handler![
            connect_host,
            send_message,
            protocol_request,
            get_bridge_state,
            set_window_material
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Mac Bot desktop application");
}
