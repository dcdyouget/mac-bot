//! HTTP/WebSocket gateway for macbotd.
//!
//! The gateway deliberately speaks JSON at its boundary.  The orchestrator and
//! store crates can be attached through [`RpcBackend`] without coupling the
//! network process to their internal data model.

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{
    body::Body,
    extract::{ConnectInfo, Multipart, Query, State, WebSocketUpgrade},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use macbot_browser::{BrowserManager, ProcessRunner};
use password_hash::SaltString;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::{Component, Path as FsPath, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs,
    net::TcpListener,
    sync::{broadcast, RwLock},
};
use tracing::{info, warn};
use uuid::Uuid;

#[allow(dead_code)]
mod execution;
mod mock;

mod adapter;
pub use adapter::ProductionBackend;

const VERSION: &str = "0.1.0";
const PROTOCOL: u64 = 1;
const MAX_UPLOAD: usize = 100 * 1024 * 1024;

/// A backend can replace the mock dispatcher without changing the wire layer.
#[async_trait::async_trait]
pub trait RpcBackend: Send + Sync + 'static {
    async fn call(&self, method: &str, params: Value, state: &GatewayState) -> RpcResult;
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub bind_addr: SocketAddr,
    pub home: PathBuf,
    pub password: Option<String>,
    pub mock: bool,
    pub host_name: String,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], 7788)),
            home: default_home(),
            password: None,
            mock: false,
            host_name: "Mac Bot".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct Gateway {
    pub state: GatewayState,
    auth: Arc<AuthStore>,
    backend: Option<Arc<dyn RpcBackend>>,
    bind_addr: SocketAddr,
    mock: bool,
}

#[derive(Clone)]
pub struct GatewayState {
    pub(crate) inner: Arc<RwLock<MockState>>,
    pub home: PathBuf,
    pub host_name: Arc<RwLock<String>>,
    pub node_id: Arc<RwLock<String>>,
    /// Live event fan-out for every connected control socket.  The durable
    /// queue in `MockState` remains the source for resume; this channel only
    /// carries events produced after a socket has connected.
    pub events: broadcast::Sender<Value>,
    pub(crate) browser: Arc<tokio::sync::Mutex<BrowserManager<ProcessRunner>>>,
}

impl GatewayState {
    /// Publish an already durable event to the in-memory fan-out queue.
    /// Callers must append to Store first; this method only handles live clients
    /// and the bounded replay buffer.
    pub(crate) async fn publish_event(&self, seq: u64, event: &str, data: Value) {
        let value = self.inner.write().await.emit_persisted(seq, event, data);
        let _ = self.events.send(value);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

pub type RpcResult = Result<Value, RpcError>;

#[derive(Clone)]
struct AuthStore {
    path: PathBuf,
    hash: Arc<RwLock<Option<String>>>,
}

impl AuthStore {
    fn load(home: &FsPath, initial_password: Option<&str>) -> Self {
        let path = home.join("data/auth.json");
        let existing_file = path.exists();
        let loaded = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                v.get("password_hash")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let hash = loaded.or_else(|| {
            if existing_file {
                // A corrupt existing auth file must fail closed.  Treating it
                // as first-run would let --password silently replace it.
                return Some("$argon2id$v=19$m=19456,t=2,p=1$invalid$invalid".into());
            }
            initial_password.and_then(|password| {
                let _ = std::fs::create_dir_all(home.join("data"));
                let salt = SaltString::generate(&mut OsRng);
                Argon2::default()
                    .hash_password(password.as_bytes(), &salt)
                    .ok()
                    .map(|h| h.to_string())
            })
        });
        let store = Self {
            path,
            hash: Arc::new(RwLock::new(hash.clone())),
        };
        if let Some(hash) = hash {
            if let Err(error) = store.write_hash_sync(&hash) {
                warn!(%error, "cannot persist password hash");
            }
        }
        store
    }

    fn write_hash_sync(&self, hash: &str) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&json!({"password_hash": hash})).unwrap(),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, &self.path)
    }

    async fn set_password(&self, password: &str) -> Result<(), String> {
        if password.is_empty() {
            return Err("password must not be empty".into());
        }
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| e.to_string())?
            .to_string();
        self.write_hash_sync(&hash).map_err(|e| e.to_string())?;
        *self.hash.write().await = Some(hash);
        Ok(())
    }

    async fn verify(&self, password: &str) -> AuthState {
        let hash = self.hash.read().await.clone();
        match hash {
            None => AuthState::SetupRequired,
            Some(hash) => tokio::task::spawn_blocking({
                let password = password.to_owned();
                move || match PasswordHash::new(&hash) {
                    Ok(parsed)
                        if Argon2::default()
                            .verify_password(password.as_bytes(), &parsed)
                            .is_ok() =>
                    {
                        AuthState::Authorized
                    }
                    _ => AuthState::Unauthorized,
                }
            })
            .await
            .unwrap_or(AuthState::Unauthorized),
        }
    }

    async fn setup_required(&self) -> bool {
        self.hash.read().await.is_none()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AuthState {
    Authorized,
    Unauthorized,
    SetupRequired,
}

#[derive(Clone)]
pub(crate) struct MockState {
    seq: u64,
    events: VecDeque<Value>,
    bots: Vec<Value>,
    chats: Vec<Value>,
    messages: HashMap<String, Vec<Value>>,
    projects: Vec<Value>,
    assignments: Vec<Value>,
    traces: HashMap<String, Vec<Value>>,
    idempotency: HashMap<String, Value>,
    devices: HashMap<String, Value>,
    /// State owned by the mock dispatcher (skills, routines, providers,
    /// approvals, questions and models). Kept generic so the wire layer does
    /// not need to know every runtime implementation detail.
    pub(crate) extra: HashMap<String, Vec<Value>>,
    settings: Value,
}

impl MockState {
    fn new(host_name: &str, load_fixture: bool, node_id: &str) -> Self {
        let now = now();
        let main_id = "bot_main";
        let chat_id = "chat_main";
        let bot = json!({
            "id": main_id, "name": "总管", "label": "协调负责人", "description": "负责拆解和跟进任务",
            "avatar": {"kind":"bean", "color": 4}, "is_main": true, "model": null, "max_parallel": 8,
            "tools": {"files":true,"bash":true,"browser":true,"subagent":true,"web":true,"mcp":true},
            "browser_mode":"headless", "pinned":true, "hidden":false, "notifications":true,
            "dm_chat_id": chat_id, "created_at":now, "updated_at":now,
            "status":{"summary":"idle","active":0,"queued":0,"waiting":0}
        });
        let chat = json!({
            "id":chat_id,"kind":"main","title":"总管","bot_id":main_id,"project_id":null,
            "member_bot_ids":[],"last_message":null,"last_seq":0,"last_read_seq":0,"unread":0,
            "attention":"none","pinned":true,"muted":false,"updated_at":now
        });
        let hello = json!({"v":1,"kind":"evt","event":"hello","data":{
            "protocol":1,"server_version":VERSION,"node_id":node_id,"host_name":host_name,
            "server_time":now,"last_seq":0,"timezone":"Asia/Shanghai","currency":"CNY","features":["mock","browser"]
        }});
        let settings = json!({
            "host_name":host_name,"timezone":"Asia/Shanghai","currency":"CNY",
            "concurrency":{"global":4,"bot_default":2,"subagent_per_run":2,"subagent_global":4,"loop_hops":8},
            "models":{"bot_default":null,"main":null,"subagent":"inherit","maintenance":null},
            "main_bot":{"auto_create_project":true},"approvals":{"mode":"require","rules":[]},
            "browser":{"default_mode":"headless","chrome_profile":"","stream":{"desktop":{"max_width":1280,"quality":70,"max_fps":15},"mobile":{"max_width":720,"quality":50,"max_fps":10}}},
            "skills":{"extra_dirs":[]},"trace":{"save_full_requests":false},"web_search":{"provider":null,"endpoint":null,"has_key":false},"push":{"apns_configured":false}
        });
        let mut state = Self {
            seq: 0,
            events: VecDeque::from([hello]),
            bots: vec![bot],
            chats: vec![chat],
            messages: HashMap::new(),
            projects: vec![],
            assignments: vec![],
            traces: HashMap::new(),
            idempotency: HashMap::new(),
            devices: HashMap::new(),
            extra: HashMap::new(),
            settings,
        };
        if load_fixture {
            state.load_scenario();
        }
        state
    }

    /// The checked-in scenario is the canonical S0 mock script.  Loading it
    /// here keeps the mock useful to both clients without duplicating their
    /// expected bootstrap data in gateway code.
    fn load_scenario(&mut self) {
        for line in
            include_str!("../../../../protocol/fixtures/scenarios/login-feature.jsonl").lines()
        {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
            let data = event.get("data").cloned().unwrap_or(Value::Null);
            if event_name == "hello" {
                if let Some(first) = self.events.front_mut() {
                    *first = event;
                }
                continue;
            }
            if event_name == "trace.item" && event.get("seq").and_then(Value::as_u64).is_none() {
                self.index_trace_item(&data);
                continue;
            }
            if let Some(seq) = event.get("seq").and_then(Value::as_u64) {
                self.restore_event(seq, event_name, data);
            }
        }
    }

    fn restore_event(&mut self, seq: u64, event_name: &str, data: Value) {
        self.emit_persisted(seq, event_name, data.clone());
        match event_name {
            "bot.created" | "bot.updated" => {
                if let Some(bot) = data.get("bot") {
                    self.bots.retain(|item| item.get("id") != bot.get("id"));
                    self.bots.push(bot.clone());
                }
            }
            "chat.created" | "chat.updated" => {
                if let Some(chat) = data.get("chat") {
                    self.chats.retain(|item| item.get("id") != chat.get("id"));
                    self.chats.push(chat.clone());
                }
            }
            "project.created" | "project.updated" => {
                if let Some(project) = data.get("project") {
                    self.projects.retain(|p| p.get("id") != project.get("id"));
                    self.projects.push(project.clone());
                }
            }
            "message.created" | "message.updated" => {
                if let Some(message) = data.get("message") {
                    let chat = message
                        .get("chat_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let messages = self.messages.entry(chat).or_default();
                    messages.retain(|item| item.get("id") != message.get("id"));
                    messages.push(message.clone());
                }
            }
            "assignment.created" | "assignment.updated" => {
                if let Some(assignment) = data.get("assignment") {
                    self.assignments
                        .retain(|a| a.get("id") != assignment.get("id"));
                    self.assignments.push(assignment.clone());
                }
            }
            "trace.item" => {
                self.index_trace_item(&data);
            }
            "announcement.updated" => {
                if let Some(announcement) = data.get("announcement") {
                    let announcements = self.extra.entry("announcements".into()).or_default();
                    announcements
                        .retain(|item| item.get("project_id") != announcement.get("project_id"));
                    announcements.push(announcement.clone());
                }
            }
            _ => {}
        }
    }

    fn index_trace_item(&mut self, data: &Value) {
        let Some(item) = data.get("item") else {
            return;
        };
        let mut keys = Vec::new();
        if let Some(assignment) = item.get("assignment_id").and_then(Value::as_str) {
            keys.push(assignment.to_string());
        }
        if let Some(chat) = item.get("chat_id").and_then(Value::as_str) {
            if !keys.iter().any(|key| key == chat) {
                keys.push(chat.to_string());
            }
        }
        if let Some(stream) = data.get("stream").and_then(Value::as_str) {
            if !keys.iter().any(|key| key == stream) {
                keys.push(stream.to_string());
            }
        }
        for key in keys {
            self.traces.entry(key).or_default().push(item.clone());
        }
    }

    fn emit(&mut self, event: &str, data: Value) -> Value {
        self.seq += 1;
        self.emit_persisted(self.seq, event, data)
    }

    fn emit_persisted(&mut self, seq: u64, event: &str, data: Value) -> Value {
        self.seq = self.seq.max(seq);
        let evt = json!({"v":1,"kind":"evt","seq":seq,"event":event,"data":data});
        self.events.push_back(evt.clone());
        while self.events.len() > 100_000 {
            self.events.pop_front();
        }
        evt
    }
}

impl Gateway {
    pub fn new(config: GatewayConfig) -> Self {
        let persisted_name = std::fs::read_to_string(config.home.join("data/settings.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| {
                value
                    .get("host_name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let host_name = persisted_name.unwrap_or_else(|| config.host_name.clone());
        let (events, _) = broadcast::channel(512);
        let node_id = if config.mock {
            "node_1".to_string()
        } else {
            let node_path = config.home.join("data/node_id");
            std::fs::create_dir_all(config.home.join("data")).ok();
            std::fs::read_to_string(&node_path)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    let value = Uuid::now_v7().to_string();
                    let _ = std::fs::write(&node_path, &value);
                    value
                })
        };
        let state = GatewayState {
            inner: Arc::new(RwLock::new(MockState::new(
                &host_name,
                config.mock,
                &node_id,
            ))),
            home: config.home.clone(),
            host_name: Arc::new(RwLock::new(host_name)),
            node_id: Arc::new(RwLock::new(node_id)),
            events,
            browser: Arc::new(tokio::sync::Mutex::new(BrowserManager::new(
                macbot_browser::SessionConfig::default(),
                Arc::new(ProcessRunner),
            ))),
        };
        Self {
            state,
            auth: Arc::new(AuthStore::load(&config.home, config.password.as_deref())),
            backend: None,
            bind_addr: config.bind_addr,
            mock: config.mock,
        }
    }

    pub fn with_backend(mut self, backend: Arc<dyn RpcBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Set the local password from the companion CLI.
    pub async fn set_password(&self, password: &str) -> Result<(), String> {
        self.auth.set_password(password).await
    }

    pub async fn setup_required(&self) -> bool {
        self.auth.setup_required().await
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/ws", get(ws_handler))
            .route("/ws/screen", get(screen_handler))
            .route("/api/v1/health", get(health_handler))
            .route("/api/v1/rpc", post(rpc_handler))
            .route("/api/v1/files", get(file_handler))
            .route("/api/v1/files/list", get(file_list_handler))
            .route("/api/v1/uploads", post(upload_handler))
            .route("/api/v1/trace/output", get(trace_output_handler))
            .route("/api/v1/usage/export.csv", get(usage_csv_handler))
            .route("/admin", get(admin_handler))
            .route("/admin/setup", post(admin_setup_handler))
            .route("/admin/settings", post(admin_settings_handler))
            .route("/admin/status", get(admin_status_handler))
            .route("/admin/logs", get(admin_logs_handler))
            .route("/admin/restart", post(admin_restart_handler))
            .with_state(self.clone())
    }

    pub async fn serve(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        fs::create_dir_all(self.state.home.join("data")).await?;
        fs::create_dir_all(self.state.home.join("uploads")).await?;
        let listener = TcpListener::bind(self.config_addr()).await?;
        info!(addr = %listener.local_addr()?, "macbot gateway listening");
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
        Ok(())
    }

    fn config_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    async fn rpc(&self, method: &str, params: Value) -> RpcResult {
        if let Some(backend) = &self.backend {
            return backend.call(method, params, &self.state).await;
        }
        if self.mock {
            mock::mock_call(method, params, &self.state).await
        } else {
            Err(rpc_error(
                "unavailable",
                "runtime backend is not configured",
                None,
            ))
        }
    }
}

fn default_home() -> PathBuf {
    std::env::var_os("MACBOT_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_path().join("MacBot"))
}
fn dirs_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::now_v7())
}

fn auth_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| query.map(str::to_string))
}

async fn authorize(gw: &Gateway, headers: &HeaderMap, query: Option<&str>) -> Result<(), Response> {
    match gw
        .auth
        .verify(auth_token(headers, query).as_deref().unwrap_or(""))
        .await
    {
        AuthState::Authorized => Ok(()),
        AuthState::SetupRequired => Err(error_response(
            StatusCode::FORBIDDEN,
            "setup_required",
            "set a password at /admin",
        )),
        AuthState::Unauthorized => Err(error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid access password",
        )),
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"error":code,"message":message}))).into_response()
}

async fn health_handler(State(gw): State<Gateway>) -> impl IntoResponse {
    Json(
        json!({"ok":true,"protocol":PROTOCOL,"version":VERSION,"setup_required":gw.auth.setup_required().await}),
    )
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if let Err(response) = authorize(&gw, &headers, query.token.as_deref()).await {
        return response;
    }
    ws.on_upgrade(move |socket| ws_session(socket, gw))
        .into_response()
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsReq {
    v: Option<u64>,
    kind: String,
    id: String,
    method: String,
    #[serde(default)]
    params: Value,
}

async fn ws_session(socket: axum::extract::ws::WebSocket, gw: Gateway) {
    let (mut sink, mut stream) = socket.split();
    let mut live = gw.state.events.subscribe();
    let mut trace_streams: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut last_received = std::time::Instant::now();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(60));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let host_name = gw.state.host_name.read().await.clone();
    let node_id = gw.state.node_id.read().await.clone();
    let hello = {
        let state = gw.state.inner.read().await;
        let mut hello = state
            .events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("hello"))
            .cloned()
            .unwrap_or_else(|| json!({"v":1,"kind":"evt","event":"hello","data":hello_value(&state, &host_name, &node_id)}));
        hello["data"]["server_time"] = json!(now());
        hello["data"]["last_seq"] = json!(state.seq);
        hello["data"]["host_name"] = json!(host_name);
        hello
    };
    if sink.send(text_frame(&hello)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(frame)) = incoming else { break };
                last_received = std::time::Instant::now();
                let text = match frame {
                    axum::extract::ws::Message::Text(t) => t,
                    axum::extract::ws::Message::Ping(p) => {
                        let _ = sink.send(axum::extract::ws::Message::Pong(p)).await;
                        continue;
                    }
                    axum::extract::ws::Message::Close(_) => break,
                    _ => continue,
                };
                let request: WsReq = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = sink.send(text_frame(&json!({"v":1,"kind":"res","id":"","ok":false,"error":rpc_error("invalid_params","invalid request",None)}))).await;
                        continue;
                    }
                };
                if request.method == "trace.subscribe" && trace_streams.len() >= 8 {
                    let _ = sink.send(text_frame(&json!({"v":1,"kind":"res","id":request.id,"ok":false,"error":rpc_error("conflict","maximum of 8 trace streams per connection",None)}))).await;
                    continue;
                }
                let result = handle_ws_request(&gw, &request).await;
                match result {
                    Ok(res) => {
                        let replay = res.get("replay_events").and_then(Value::as_array).cloned().unwrap_or_default();
                        if request.method == "trace.subscribe" {
                            if let Some(stream_id) = res.get("stream").and_then(Value::as_str) {
                                trace_streams.insert(stream_id.to_owned(), (request.params.get("assignment_id").and_then(Value::as_str).map(str::to_owned), request.params.get("chat_id").and_then(Value::as_str).map(str::to_owned)));
                            }
                        } else if request.method == "trace.unsubscribe" {
                            if let Some(stream_id) = request.params.get("stream").and_then(Value::as_str) { trace_streams.remove(stream_id); }
                        }
                        let mut wire_result = res;
                        if let Some(object) = wire_result.as_object_mut() { object.remove("replay_events"); }
                        if sink.send(text_frame(&json!({"v":1,"kind":"res","id":request.id,"ok":true,"result":wire_result}))).await.is_err() { break; }
                        for event in replay { if sink.send(text_frame(&event)).await.is_err() { break; } }
                        if request.method == "session.resume" {
                            let last = request.params.get("last_seq").and_then(Value::as_u64).unwrap_or(0);
                            let mode = wire_result.get("mode").and_then(Value::as_str).unwrap_or("reset");
                            if mode == "replay" {
                                let events = {
                                    let state = gw.state.inner.read().await;
                                    state.events.iter().filter(|event| event.get("seq").and_then(Value::as_u64).unwrap_or(0) > last).cloned().collect::<Vec<_>>()
                                };
                                for event in events { if sink.send(text_frame(&event)).await.is_err() { return; } }
                            }
                            let seq = gw.state.inner.read().await.seq;
                            if sink.send(text_frame(&json!({"v":1,"kind":"evt","event":"sync.done","data":{"seq":seq}}))).await.is_err() { break; }
                        }
                    }
                    Err(err) => {
                        if sink.send(text_frame(&json!({"v":1,"kind":"res","id":request.id,"ok":false,"error":err}))).await.is_err() { break; }
                    }
                }
            }
            event = live.recv() => {
                match event {
                    Ok(event) => {
                        if event.get("event").and_then(Value::as_str) == Some("trace.item") {
                            let item = event.get("data").and_then(|data| data.get("item")).cloned().or_else(|| event.get("data").cloned()).unwrap_or(Value::Null);
                            for (stream_id, (assignment_id, chat_id)) in &trace_streams {
                                let matches_assignment = assignment_id.as_deref().is_none_or(|id| item.get("assignment_id").and_then(Value::as_str) == Some(id));
                                let matches_chat = chat_id.as_deref().is_none_or(|id| item.get("chat_id").and_then(Value::as_str) == Some(id));
                                if matches_assignment && matches_chat {
                                    let frame = json!({"v":1,"kind":"evt","event":"trace.item","data":{"stream":stream_id,"item":item}});
                                    if sink.send(text_frame(&frame)).await.is_err() { break; }
                                }
                            }
                        } else if sink.send(text_frame(&event)).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Do not silently drop persisted events.  Reconnect and
                        // use the durable cursor to resume from the last seq.
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = heartbeat.tick() => {
                if last_received.elapsed() > std::time::Duration::from_secs(90) { break; }
                if sink.send(axum::extract::ws::Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
        }
    }
}

fn text_frame(value: &Value) -> axum::extract::ws::Message {
    axum::extract::ws::Message::Text(value.to_string().into())
}

async fn handle_ws_request(gw: &Gateway, request: &WsReq) -> RpcResult {
    if request.kind != "req" || request.v.unwrap_or(1) != 1 {
        return Err(rpc_error("version_unsupported", "unsupported frame", None));
    }
    if request.method == "session.resume" {
        let last = request
            .params
            .get("last_seq")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let state = gw.state.inner.read().await;
        let oldest = state
            .events
            .iter()
            .filter_map(|event| event.get("seq").and_then(Value::as_u64))
            .min()
            .unwrap_or(state.seq.saturating_add(1));
        return Ok(json!({"mode": if last > 0 && last + 1 >= oldest {"replay"} else {"reset"}}));
    }
    if request.method == "trace.subscribe" {
        let assignment = request
            .params
            .get("assignment_id")
            .and_then(Value::as_str)
            .or_else(|| request.params.get("chat_id").and_then(Value::as_str))
            .unwrap_or("");
        let since = request
            .params
            .get("since_aseq")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let state = gw.state.inner.read().await;
        let items = state.traces.get(assignment).cloned().unwrap_or_default();
        drop(state);
        let stream = id("stream");
        let mut out = vec![];
        for item in items {
            if item.get("aseq").and_then(Value::as_u64).unwrap_or(0) > since {
                out.push(json!({"v":1,"kind":"evt","event":"trace.item","data":{"stream":stream,"item":item}}));
            }
        }
        return Ok(json!({"stream":stream,"in_flight":[],"replay_events":out}));
    }
    gw.rpc(&request.method, request.params.clone()).await
}

async fn rpc_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Json(req): Json<RpcHttpRequest>,
) -> Response {
    if let Err(response) = authorize(&gw, &headers, None).await {
        return response;
    }
    let result = gw.rpc(&req.method, req.params).await;
    match result {
        Ok(value) => Json(json!({"ok":true,"result":value})).into_response(),
        Err(error) => Json(json!({"ok":false,"error":error})).into_response(),
    }
}
#[derive(Debug, Deserialize)]
struct RpcHttpRequest {
    method: String,
    #[serde(default)]
    params: Value,
}

async fn screen_handler(
    ws: WebSocketUpgrade,
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<ScreenQuery>,
) -> Response {
    if let Err(response) = authorize(&gw, &headers, query.token.as_deref()).await {
        return response;
    }
    ws.on_upgrade(move |socket| screen_session(socket, gw, query))
        .into_response()
}
#[derive(Debug, Deserialize, Clone)]
struct ScreenQuery {
    bot_id: Option<String>,
    #[serde(rename = "quality")]
    _quality: Option<String>,
    tab_id: Option<String>,
    token: Option<String>,
}

const MOCK_SCREEN_FRAMES: [&[u8]; 3] = [
    include_bytes!("../assets/mock-screen-1.jpg"),
    include_bytes!("../assets/mock-screen-2.jpg"),
    include_bytes!("../assets/mock-screen-3.jpg"),
];

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0..2] != [0xff, 0xd8] {
        return None;
    }
    let mut offset = 2;
    while offset + 3 < bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset >= bytes.len() {
            return None;
        }
        let marker = bytes[offset];
        offset += 1;
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if offset + 2 > bytes.len() {
            return None;
        }
        let segment_len = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if segment_len < 2 || offset + segment_len > bytes.len() {
            return None;
        }
        let is_sof = matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf);
        if is_sof && segment_len >= 7 {
            let height = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]) as u32;
            return Some((width, height));
        }
        offset += segment_len;
    }
    None
}

fn mock_screen_view(state: &MockState, bot_id: &str) -> (String, Vec<Value>) {
    let tabs = state
        .extra
        .get(&format!("screen_tabs:{bot_id}"))
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let driver = state
        .extra
        .get(&format!("screen_driver:{bot_id}"))
        .into_iter()
        .flatten()
        .find_map(|value| value.get("driver").and_then(Value::as_str))
        .unwrap_or("idle")
        .to_owned();
    (driver, tabs)
}

fn mock_screen_state(bot_id: &str, driver: &str, tabs: &[Value], selected: &str) -> Value {
    let tabs = tabs
        .iter()
        .map(|tab| {
            let mut tab = tab.clone();
            let is_active = tab.get("tab_id").and_then(Value::as_str) == Some(selected);
            if let Some(object) = tab.as_object_mut() {
                object.insert("active".into(), json!(is_active));
            }
            tab
        })
        .collect::<Vec<_>>();
    json!({"type":"state","state":{"bot_id":bot_id,"driver":driver,
        "tabs":tabs,"width":320,"height":180}})
}

fn mock_screen_frame(seq: u64, tab_id: &str, tabs: &[Value]) -> Vec<u8> {
    let url = tabs
        .iter()
        .find(|tab| tab.get("tab_id").and_then(Value::as_str) == Some(tab_id))
        .and_then(|tab| tab.get("url").and_then(Value::as_str))
        .unwrap_or("about:blank");
    let jpeg = MOCK_SCREEN_FRAMES[(seq as usize) % MOCK_SCREEN_FRAMES.len()];
    let header =
        json!({"seq":seq,"tab_id":tab_id,"w":320,"h":180,"ts":unix_ms(),"url":url}).to_string();
    let mut bytes = Vec::with_capacity(4 + header.len() + jpeg.len());
    bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(jpeg);
    bytes
}

async fn mock_screen_session(
    socket: axum::extract::ws::WebSocket,
    gw: Gateway,
    query: ScreenQuery,
) {
    let (mut sink, mut stream) = socket.split();
    let bot_id = query.bot_id.unwrap_or_else(|| "bot_main".into());
    let mut selected = query.tab_id.unwrap_or_else(|| "tab_mock_1".into());
    let mut seq = 1u64;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    let view = gw.state.inner.read().await;
    let (mut driver, mut tabs) = mock_screen_view(&view, &bot_id);
    drop(view);
    let mut last_state = mock_screen_state(&bot_id, &driver, &tabs, &selected);
    if sink.send(text_frame(&last_state)).await.is_err() {
        return;
    }
    if sink
        .send(axum::extract::ws::Message::Binary(
            mock_screen_frame(seq, &selected, &tabs).into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let view = gw.state.inner.read().await;
                let (next_driver, next_tabs) = mock_screen_view(&view, &bot_id);
                drop(view);
                driver = next_driver;
                tabs = next_tabs;
                if !tabs.iter().any(|tab| tab.get("tab_id").and_then(Value::as_str) == Some(&selected)) {
                    selected = tabs.first().and_then(|tab| tab.get("tab_id").and_then(Value::as_str)).unwrap_or("tab_mock_1").to_owned();
                }
                let state = mock_screen_state(&bot_id, &driver, &tabs, &selected);
                if state != last_state {
                    last_state = state.clone();
                    if sink.send(text_frame(&state)).await.is_err() { break; }
                }
            }
            message = stream.next() => {
                let Some(Ok(message)) = message else { break; };
                match message {
                    axum::extract::ws::Message::Text(text) => {
                        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
                        match parsed.get("type").and_then(Value::as_str) {
                            Some("ack") if parsed.get("seq").and_then(Value::as_u64) == Some(seq) => {
                                seq += 1;
                                if sink.send(axum::extract::ws::Message::Binary(mock_screen_frame(seq, &selected, &tabs).into())).await.is_err() { break; }
                            }
                            Some("switch_tab") => {
                                if driver != "user" {
                                    if sink.send(text_frame(&json!({"type":"error","error":{"code":"permission_denied","message":"takeover is not active"}}))).await.is_err() { break; }
                                    continue;
                                }
                                let Some(requested) = parsed.get("tab_id").and_then(Value::as_str) else {
                                    let _ = sink.send(text_frame(&json!({"type":"error","error":{"code":"invalid_request","message":"tab_id is required"}}))).await;
                                    continue;
                                };
                                if !tabs.iter().any(|tab| tab.get("tab_id").and_then(Value::as_str) == Some(requested)) {
                                    let _ = sink.send(text_frame(&json!({"type":"error","error":{"code":"forbidden","message":"tab is outside the screen assignment"}}))).await;
                                    continue;
                                }
                                selected = requested.to_owned();
                                let state = mock_screen_state(&bot_id, &driver, &tabs, &selected);
                                last_state = state.clone();
                                if sink.send(text_frame(&state)).await.is_err() { break; }
                            }
                            Some("input") => {
                                if driver != "user" {
                                    if sink.send(text_frame(&json!({"type":"error","error":{"code":"permission_denied","message":"takeover is not active"}}))).await.is_err() { break; }
                                } else if let Some(event) = parsed.get("event") {
                                    if serde_json::from_value::<macbot_protocol::ScreenInput>(event.clone()).is_err()
                                        && sink.send(text_frame(&json!({"type":"error","error":{"code":"invalid_request","message":"invalid screen input"}}))).await.is_err() { break; }
                                } else if sink.send(text_frame(&json!({"type":"error","error":{"code":"invalid_request","message":"event is required"}}))).await.is_err() { break; }
                            }
                            _ => {}
                        }
                    }
                    axum::extract::ws::Message::Ping(payload) => {
                        if sink.send(axum::extract::ws::Message::Pong(payload)).await.is_err() { break; }
                    }
                    axum::extract::ws::Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn screen_session(socket: axum::extract::ws::WebSocket, gw: Gateway, query: ScreenQuery) {
    if gw.mock {
        mock_screen_session(socket, gw, query).await;
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let bot_id = query.bot_id.unwrap_or_else(|| "bot_main".into());
    let selected_tab = query.tab_id.unwrap_or_default();
    let (screen_state, tab_id, tab_url) = {
        let mut browser = gw.state.browser.lock().await;
        let state = browser
            .state(&bot_id)
            .unwrap_or_else(|_| browser.session(&bot_id).clone());
        let tab_id = if selected_tab.is_empty() {
            state
                .tabs
                .first()
                .map(|tab| tab.tab_id.clone())
                .unwrap_or_else(|| "tab_mock".into())
        } else {
            selected_tab
        };
        let url = state
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.url.clone())
            .unwrap_or_else(|| "about:blank".into());
        let driver = if state.takeover {
            "user"
        } else if state.tabs.is_empty() {
            "idle"
        } else {
            "bot"
        };
        let tabs = state.tabs.into_iter().map(|tab| json!({"tab_id":tab.tab_id,"title":tab.title,"url":tab.url,"assignment_id":tab.assignment_id,"active":tab.active})).collect::<Vec<_>>();
        (
            json!({"type":"state","state":{"bot_id":bot_id,"driver":driver,"tabs":tabs,"width":if gw.mock {320} else {1280},"height":if gw.mock {180} else {720}}}),
            tab_id,
            url,
        )
    };
    let state = screen_state;
    if sink.send(text_frame(&state)).await.is_err() {
        return;
    }
    let mut seq = 0u64;
    // A frame is sent only after the previous one has been ACKed.  This is the
    // same back-pressure rule used by the browser sidecar, and prevents a slow
    // mobile decoder from accumulating an unbounded queue.
    let mock = gw.mock;
    let capture = |seq: u64| {
        let browser = gw.state.browser.clone();
        let bot_id = bot_id.clone();
        let tab_id = tab_id.clone();
        let tab_url = tab_url.clone();
        async move {
            let jpeg = {
                let mut manager = browser.lock().await;
                manager.screenshot(&bot_id, &tab_id).ok()
            };
            let (jpeg, width, height) = match jpeg {
                Some(jpeg) => {
                    let (width, height) = jpeg_dimensions(&jpeg).unwrap_or((1280, 720));
                    (jpeg, width, height)
                }
                None if mock => (
                    MOCK_SCREEN_FRAMES[(seq as usize) % MOCK_SCREEN_FRAMES.len()].to_vec(),
                    320,
                    180,
                ),
                None => return None,
            };
            let header = json!({"seq":seq,"tab_id":tab_id,"w":width,"h":height,"ts":unix_ms(),"url":tab_url}).to_string();
            let mut bytes = Vec::with_capacity(4 + header.len() + jpeg.len());
            bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
            bytes.extend_from_slice(header.as_bytes());
            bytes.extend_from_slice(&jpeg);
            Some(bytes)
        }
    };
    seq += 1;
    let Some(frame) = capture(seq).await else {
        let _ = sink
            .send(text_frame(&json!({"type":"state","state":{"bot_id":bot_id,"tab_id":tab_id,"driver":"idle","availability":"unavailable","reason":"screenshot_unavailable"}})))
            .await;
        return;
    };
    if sink
        .send(axum::extract::ws::Message::Binary(frame.into()))
        .await
        .is_err()
    {
        return;
    }
    while let Some(Ok(message)) = stream.next().await {
        match message {
            axum::extract::ws::Message::Text(text) => {
                let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
                match parsed.get("type").and_then(Value::as_str) {
                    Some("ack") => {
                        if parsed.get("seq").and_then(Value::as_u64) == Some(seq) {
                            seq += 1;
                            let Some(frame) = capture(seq).await else {
                                let _ = sink.send(text_frame(&json!({"type":"state","state":{"bot_id":bot_id,"tab_id":tab_id,"driver":"idle","availability":"unavailable","reason":"screenshot_unavailable"}}))).await;
                                break;
                            };
                            if sink
                                .send(axum::extract::ws::Message::Binary(frame.into()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    Some("switch_tab") => {
                        if let Some(requested) = parsed.get("tab_id").and_then(Value::as_str) {
                            let mut browser = gw.state.browser.lock().await;
                            let assignment = browser.state(&bot_id).ok().and_then(|state| {
                                state
                                    .tabs
                                    .into_iter()
                                    .find(|tab| tab.tab_id == requested)
                                    .map(|tab| tab.assignment_id)
                            });
                            if let Some(assignment) = assignment {
                                let _ = browser.switch_tab(&bot_id, &assignment);
                            }
                        }
                    }
                    Some("input") => {
                        if let Some(event) = parsed.get("event") {
                            if let Ok(event) =
                                serde_json::from_value::<macbot_browser::ScreenInput>(event.clone())
                            {
                                let mut browser = gw.state.browser.lock().await;
                                let assignment = browser.state(&bot_id).ok().and_then(|state| {
                                    state
                                        .tabs
                                        .into_iter()
                                        .find(|tab| tab.tab_id == tab_id)
                                        .map(|tab| tab.assignment_id)
                                });
                                if let Some(assignment) = assignment {
                                    let _ = browser.input(&bot_id, &assignment, event);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            axum::extract::ws::Message::Ping(p) => {
                let _ = sink.send(axum::extract::ws::Message::Pong(p)).await;
            }
            axum::extract::ws::Message::Close(_) => break,
            _ => {}
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hello_value(state: &MockState, host_name: &str, node_id: &str) -> Value {
    json!({"protocol":1,"server_version":VERSION,"node_id":node_id,"host_name":host_name,"server_time":now(),"last_seq":state.seq,"timezone":"Asia/Shanghai","currency":"CNY","features":["mock","browser"]})
}
pub(crate) fn rpc_error(code: &str, message: &str, details: Option<Value>) -> RpcError {
    RpcError {
        code: code.into(),
        message: message.into(),
        details,
    }
}
async fn file_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    let path = match resolve_file(&gw.state.home, &query) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let bytes = match fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "not_found", "file not found"),
    };
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    ranged_response(
        &headers,
        &bytes,
        HeaderValue::from_str(&mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    )
}
#[derive(Debug, Deserialize)]
struct FileQuery {
    root: String,
    root_id: Option<String>,
    path: String,
}
#[allow(clippy::result_large_err)]
fn resolve_file(home: &FsPath, q: &FileQuery) -> Result<PathBuf, Response> {
    let root = match q.root.as_str() {
        "project" => home
            .join("projects")
            .join(q.root_id.clone().unwrap_or_default()),
        "bot" => home
            .join("bots")
            .join(q.root_id.clone().unwrap_or_default()),
        "upload" => home
            .join("uploads")
            .join(q.root_id.clone().unwrap_or_default()),
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_params",
                "invalid file root",
            ))
        }
    };
    let rel = FsPath::new(&q.path);
    if rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_params",
            "path escapes root",
        ));
    }
    let candidate = root.join(rel);
    if let (Ok(root_real), Ok(candidate_real)) = (
        std::fs::canonicalize(&root),
        std::fs::canonicalize(&candidate),
    ) {
        if !candidate_real.starts_with(root_real) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_params",
                "path escapes root",
            ));
        }
    }
    Ok(candidate)
}
fn ranged_response(headers: &HeaderMap, bytes: &[u8], mime: HeaderValue) -> Response {
    let total = bytes.len();
    if total == 0 {
        let mut response = Response::new(Body::empty());
        response.headers_mut().insert(header::CONTENT_TYPE, mime);
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        return response;
    }
    let mut start = 0;
    let mut end = total.saturating_sub(1);
    let mut status = StatusCode::OK;
    if let Some(raw) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if let Some(spec) = raw.strip_prefix("bytes=") {
            let mut it = spec.splitn(2, '-');
            let first = it.next().unwrap_or("");
            let second = it.next().unwrap_or("");
            if first.is_empty() {
                let suffix = second.parse::<usize>().unwrap_or(0);
                if suffix == 0 {
                    return error_response(
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        "invalid_params",
                        "invalid range",
                    );
                }
                start = total.saturating_sub(suffix);
            } else {
                start = first.parse().unwrap_or(total);
                end = second.parse::<usize>().unwrap_or(end).min(end);
            }
            if start <= end && start < total {
                status = StatusCode::PARTIAL_CONTENT
            } else {
                return error_response(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "invalid_params",
                    "invalid range",
                );
            }
        }
    }
    let body = Body::from(bytes[start..=end].to_vec());
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, mime);
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&(end - start + 1).to_string()).unwrap(),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap(),
        );
    }
    response
}
async fn file_list_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<FileQuery>,
) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    let dir = match resolve_file(&gw.state.home, &query) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut entries = vec![];
    let mut rd = match fs::read_dir(dir).await {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "not_found", "directory not found"),
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            entries.push(json!({"name":entry.file_name(),"path":entry.path().file_name(),"is_dir":meta.is_dir(),"size":meta.len(),"modified_at":meta.modified().ok().and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_secs())}));
        }
    }
    Json(json!({"entries":entries})).into_response()
}

async fn upload_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    let upload_id = id("upl");
    let dir = gw.state.home.join("uploads");
    if let Err(e) = fs::create_dir_all(&dir).await {
        warn!(%e,"cannot create upload directory");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "cannot create upload directory",
        );
    }
    let mut name = "upload.bin".to_string();
    let mut size = 0usize;
    let target = dir.join(&upload_id);
    let mut file = match fs::File::create(&target).await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "cannot create upload",
            )
        }
    };
    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(n) = field.file_name() {
            name = n.to_string();
        }
        let bytes = match field.bytes().await {
            Ok(v) => v,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_params",
                    "invalid multipart body",
                )
            }
        };
        size += bytes.len();
        if size > MAX_UPLOAD {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_params",
                "upload exceeds 100 MB",
            );
        }
        if tokio::io::AsyncWriteExt::write_all(&mut file, &bytes)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "cannot write upload",
            );
        }
    }
    let mime = mime_guess::from_path(&name)
        .first_or_octet_stream()
        .to_string();
    Json(json!({"upload_id":upload_id,"file":{"root":"upload","root_id":upload_id,"path":"","name":name,"size":size,"mime":mime}})).into_response()
}
async fn trace_output_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<OutputQuery>,
) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    if !is_safe_component(&query.run_id) || !is_safe_component(&query.call_id) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_params",
            "invalid trace identifier",
        );
    }
    let path = gw
        .state
        .home
        .join("runs")
        .join(&query.run_id)
        .join(format!("{}.txt", query.call_id));
    match fs::read(path).await {
        Ok(b) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], b).into_response(),
        Err(_) => error_response(StatusCode::NOT_FOUND, "not_found", "trace output not found"),
    }
}

fn is_safe_component(value: &str) -> bool {
    let mut components = FsPath::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}
#[derive(Debug, Deserialize)]
struct OutputQuery {
    run_id: String,
    call_id: String,
}
async fn usage_csv_handler(State(gw): State<Gateway>, headers: HeaderMap) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    let body = "date,bot_id,project_id,input_tokens,output_tokens,cost\n";
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=usage.csv",
            ),
        ],
        body,
    )
        .into_response()
}

async fn admin_handler(
    State(gw): State<Gateway>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !gw.auth.setup_required().await && !is_local_or_basic(remote, &headers, &gw).await {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "admin authentication required",
        );
    }
    Html("<!doctype html><meta charset=utf-8><title>Mac Bot</title><h1>Mac Bot</h1><p id=status>正在加载</p><form method=post action=/admin/setup><input name=password type=password placeholder=访问密码><button>设置密码</button></form><script>fetch('/api/v1/health').then(r=>r.json()).then(x=>status.textContent=JSON.stringify(x))</script>").into_response()
}
async fn is_local_or_basic(remote: SocketAddr, headers: &HeaderMap, gw: &Gateway) -> bool {
    if remote.ip().is_loopback() {
        return true;
    }
    if let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(encoded) = v.strip_prefix("Basic ") {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) {
                if let Ok(s) = String::from_utf8(bytes) {
                    return gw
                        .auth
                        .verify(s.split_once(':').map(|(_, p)| p).unwrap_or(""))
                        .await
                        == AuthState::Authorized;
                }
            }
        }
    }
    false
}
async fn admin_setup_handler(
    State(gw): State<Gateway>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    form: axum::extract::Form<HashMap<String, String>>,
) -> Response {
    if !gw.auth.setup_required().await {
        return error_response(
            StatusCode::CONFLICT,
            "conflict",
            "password already configured",
        );
    }
    if !remote.ip().is_loopback() || !is_local_or_basic(remote, &headers, &gw).await {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "setup is local only",
        );
    }
    match gw
        .auth
        .set_password(form.get("password").map(String::as_str).unwrap_or(""))
        .await
    {
        Ok(()) => Html("密码已设置，请返回客户端连接。").into_response(),
        Err(e) => error_response(StatusCode::BAD_REQUEST, "invalid_params", &e),
    }
}
async fn admin_settings_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Json(patch): Json<Value>,
) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    if let Some(name) = patch.get("host_name").and_then(Value::as_str) {
        *gw.state.host_name.write().await = name.to_string();
    }
    let settings_path = gw.state.home.join("data/settings.json");
    if let Some(parent) = settings_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let persisted = json!({"host_name":gw.state.host_name.read().await.clone(),"port":patch.get("port").and_then(Value::as_u64).unwrap_or(7788)});
    let tmp = settings_path.with_extension("json.tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(&persisted) {
        let _ = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &settings_path));
    }
    Json(json!({"ok":true,"host_name":gw.state.host_name.read().await.clone()})).into_response()
}

async fn admin_status_handler(
    State(gw): State<Gateway>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !is_local_or_basic(remote, &headers, &gw).await {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "admin authentication required",
        );
    }
    let state = gw.state.inner.read().await;
    Json(json!({"running":true,"port":gw.bind_addr.port(),"host_name":gw.state.host_name.read().await.clone(),"seq":state.seq,"mock":gw.mock})).into_response()
}

async fn admin_logs_handler(
    State(gw): State<Gateway>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !is_local_or_basic(remote, &headers, &gw).await {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "admin authentication required",
        );
    }
    let path = gw.state.home.join("data/macbot.log");
    match fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes).into_response(),
        Err(_) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            Vec::<u8>::new(),
        )
            .into_response(),
    }
}

async fn admin_restart_handler(
    State(gw): State<Gateway>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !is_local_or_basic(remote, &headers, &gw).await {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "admin authentication required",
        );
    }
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
    let requested = uid.as_deref().is_some_and(|uid| {
        std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &format!("gui/{uid}/com.macbot.server")])
            .status()
            .is_ok_and(|status| status.success())
    });
    Json(json!({"ok":requested,"restarting":requested,"message":"restart requested; LaunchAgent will restart the process"})).into_response()
}

pub async fn run(config: GatewayConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let gateway = Gateway::new(config.clone());
    let gateway = if config.mock {
        gateway
    } else {
        let backend = Arc::new(ProductionBackend::open(&config.home)?);
        // Rebuild the in-memory event window used by websocket resume and
        // trace replay from the durable global event log before accepting
        // connections.  The store has already repaired a truncated tail.
        let persisted = backend.store.events_since(0)?;
        {
            let mut state = gateway.state.inner.write().await;
            for event in persisted {
                state.restore_event(event.seq, &event.event, event.data);
            }
        }
        let tick_backend = backend.clone();
        let tick_state = gateway.state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(error) = tick_backend.tick_routines(&tick_state, Utc::now()).await {
                    warn!(?error, "routine scheduler tick failed");
                }
            }
        });
        gateway.with_backend(backend)
    };
    fs::create_dir_all(&config.home).await?;
    let listener = TcpListener::bind(config.bind_addr).await?;
    info!(addr=%config.bind_addr,"macbot gateway listening");
    axum::serve(
        listener,
        gateway
            .router()
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;
    #[tokio::test]
    async fn mock_bootstrap_and_idempotency() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let first=gw.rpc("chat.send",json!({"chat_id":"chat_main","text":"hello","mentions":[],"client_request_id":"req-1"})).await.unwrap();
        let second=gw.rpc("chat.send",json!({"chat_id":"chat_main","text":"changed","mentions":[],"client_request_id":"req-1"})).await.unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn mock_bootstrap_and_hello_share_node_id() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let bootstrap = gw.rpc("bootstrap", json!({})).await.unwrap();
        assert_eq!(bootstrap["hello"]["node_id"], "node_1");
        assert_eq!(gw.state.node_id.read().await.as_str(), "node_1");
    }

    #[tokio::test]
    async fn formal_node_id_is_uuidv7_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let first = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            ..Default::default()
        });
        let first_id = first.state.node_id.read().await.clone();
        assert_eq!(Uuid::parse_str(&first_id).unwrap().get_version_num(), 7);
        let second = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            ..Default::default()
        });
        assert_eq!(second.state.node_id.read().await.as_str(), first_id);
    }

    #[tokio::test]
    async fn mock_fixture_indexes_trace_chat_updates_and_team_objects() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let state = gw.state.inner.read().await;
        assert!(!state.traces.get("asg_product").unwrap().is_empty());
        assert!(!state.traces.get("chat_login").unwrap().is_empty());
        assert!(state.bots.iter().any(|bot| bot["id"] == "bot_product"));
        assert!(state.chats.iter().any(|chat| chat["id"] == "chat_login"));
        assert_eq!(
            state
                .messages
                .get("chat_login")
                .unwrap()
                .iter()
                .filter(|message| message["id"] == "msg_steer")
                .count(),
            1
        );
        drop(state);
        let project = gw
            .rpc("project.get", json!({"project_id":"prj_login"}))
            .await
            .unwrap();
        assert!(!project["announcement"]["artifacts"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn range() {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, HeaderValue::from_static("bytes=1-2"));
        let r = ranged_response(&h, b"abcd", HeaderValue::from_static("text/plain"));
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        h.insert(header::RANGE, HeaderValue::from_static("bytes=-2"));
        let r = ranged_response(&h, b"abcd", HeaderValue::from_static("text/plain"));
        assert_eq!(
            r.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 2-3/4"
        );
    }

    #[test]
    fn mock_screen_frames_have_protocol_dimensions() {
        for frame in MOCK_SCREEN_FRAMES {
            assert_eq!(jpeg_dimensions(frame), Some((320, 180)));
        }
    }

    #[tokio::test]
    async fn network_auth_and_health() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let health = gw
            .router()
            .clone()
            .oneshot(
                http::Request::builder()
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let unauthorized = gw
            .router()
            .oneshot(
                http::Request::builder()
                    .uri("/api/v1/rpc")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"method":"ping","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    }
}
