//! HTTP/WebSocket gateway for macbotd.
//!
//! The gateway deliberately speaks JSON at its boundary.  The orchestrator and
//! store crates can be attached through [`RpcBackend`] without coupling the
//! network process to their internal data model.

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Multipart, Query, State, WebSocketUpgrade},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use futures_util::{SinkExt, StreamExt};
use image::GenericImageView;
use macbot_browser::{BrowserError, BrowserManager, CliRunner, ProcessRunner};
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
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UnixListener},
    sync::{broadcast, mpsc, Mutex, RwLock},
};

use tracing::{info, warn};
use uuid::Uuid;

#[allow(dead_code)]
mod collaboration_tools;
#[allow(dead_code)]
mod execution;
#[allow(dead_code)]
mod features;
mod housekeeping;
#[allow(dead_code)]
mod memory_tools;
mod mock;
mod rate_limit;

mod adapter;
mod backend;
pub use adapter::ProductionBackend;
pub use backend::{ComposedBackend, RuntimeError, RuntimeExecution};

const VERSION: &str = "0.1.0";
const PROTOCOL: u64 = 1;
const MAX_UPLOAD: usize = 100 * 1024 * 1024;

/// A backend can replace the mock dispatcher without changing the wire layer.
#[async_trait::async_trait]
pub trait RpcBackend: Send + Sync + 'static {
    async fn call(&self, method: &str, params: Value, state: &GatewayState) -> RpcResult;

    /// Export usage through the ledger owned by this backend.  HTTP handlers
    /// must use this hook instead of opening the Store a second time: the
    /// production Store is protected by a process-wide single-writer lock.
    async fn export_usage_csv(&self, _params: &Value, _timezone: &str) -> Result<String, RpcError> {
        Err(rpc_error(
            "unsupported",
            "usage CSV export is unavailable",
            None,
        ))
    }
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
    trace_runtime: Arc<Mutex<TraceRuntime>>,
    event_lock: Arc<Mutex<()>>,
    pub(crate) browser: Arc<tokio::sync::Mutex<BrowserManager<ProcessRunner>>>,
    /// Number of live gateway screen connections sharing each Bot sidecar.
    /// The sidecar is disabled only when the last connection closes.
    pub(crate) screen_streams: Arc<tokio::sync::Mutex<HashMap<String, usize>>>,
}

#[derive(Debug, Clone)]
struct TraceInFlight {
    assignment_id: Option<String>,
    chat_id: String,
    text: String,
    thinking: String,
    tool_args: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct TraceRuntime {
    in_flight: HashMap<String, TraceInFlight>,
}

impl TraceRuntime {
    fn from_events(events: impl IntoIterator<Item = Value>) -> Self {
        let mut runtime = Self::default();
        for event in events {
            if event.get("event").and_then(Value::as_str) == Some("trace.item") {
                let item = event
                    .get("data")
                    .and_then(|data| data.get("item"))
                    .cloned()
                    .unwrap_or(Value::Null);
                runtime.update_persistent("trace.item", &json!({"item": item}));
            }
        }
        runtime
    }

    fn update_persistent(&mut self, event: &str, data: &Value) {
        if event != "trace.item" {
            return;
        }
        let Some(item) = data.get("item") else {
            return;
        };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(item_data) = item.get("data") else {
            return;
        };
        let Some(request_id) = item_data.get("request_id").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "llm.request" => {
                let Some(chat_id) = item.get("chat_id").and_then(Value::as_str) else {
                    return;
                };
                self.in_flight.insert(
                    request_id.to_owned(),
                    TraceInFlight {
                        assignment_id: item
                            .get("assignment_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        chat_id: chat_id.to_owned(),
                        text: String::new(),
                        thinking: String::new(),
                        tool_args: HashMap::new(),
                    },
                );
            }
            "llm.response" => {
                self.in_flight.remove(request_id);
            }
            _ => {}
        }
    }

    fn update_temporary(&mut self, event: &str, data: &Value) {
        let Some(request_id) = data.get("request_id").and_then(Value::as_str) else {
            return;
        };
        let Some(request) = self.in_flight.get_mut(request_id) else {
            return;
        };
        if event != "trace.delta" {
            return;
        }
        let text = data.get("text").and_then(Value::as_str).unwrap_or("");
        match data
            .get("channel")
            .and_then(Value::as_str)
            .unwrap_or("text")
        {
            "thinking" => request.thinking.push_str(text),
            "tool_args" => {
                if let Some(call_id) = data.get("call_id").and_then(Value::as_str) {
                    request
                        .tool_args
                        .entry(call_id.to_owned())
                        .or_default()
                        .push_str(text);
                }
            }
            _ => request.text.push_str(text),
        }
    }

    fn matches(&self, assignment_id: Option<&str>, chat_id: Option<&str>, data: &Value) -> bool {
        let request = data
            .get("request_id")
            .and_then(Value::as_str)
            .and_then(|id| self.in_flight.get(id));
        let event_assignment = data
            .get("assignment_id")
            .and_then(Value::as_str)
            .or_else(|| request.and_then(|item| item.assignment_id.as_deref()));
        let event_chat = data
            .get("chat_id")
            .and_then(Value::as_str)
            .or_else(|| request.map(|item| item.chat_id.as_str()));
        assignment_id.is_none_or(|expected| event_assignment == Some(expected))
            && chat_id.is_none_or(|expected| event_chat == Some(expected))
    }

    fn in_flight_for(&self, assignment_id: Option<&str>, chat_id: Option<&str>) -> Vec<Value> {
        self.in_flight
            .iter()
            .filter(|(_, request)| {
                assignment_id
                    .is_none_or(|expected| request.assignment_id.as_deref() == Some(expected))
                    && chat_id.is_none_or(|expected| request.chat_id == expected)
            })
            .map(|(request_id, request)| {
                json!({
                    "request_id": request_id,
                    "text": request.text,
                    "thinking": request.thinking,
                })
            })
            .collect()
    }
}

impl GatewayState {
    /// Publish an already durable event to the in-memory fan-out queue.
    /// Callers must append to Store first; this method only handles live clients
    /// and the bounded replay buffer.
    pub(crate) async fn publish_event(&self, seq: u64, event: &str, data: Value) {
        let _event_lock = self.event_lock.lock().await;
        let value = {
            let mut state = self.inner.write().await;
            let value = state.emit_persisted(seq, event, data);
            if event == "trace.item" {
                let trace_data = value.get("data").cloned().unwrap_or(Value::Null);
                state.index_trace_item(&trace_data);
            }
            value
        };
        if event == "trace.item" {
            let item = value
                .get("data")
                .and_then(|data| data.get("item"))
                .cloned()
                .unwrap_or(Value::Null);
            self.trace_runtime
                .lock()
                .await
                .update_persistent(event, &json!({"item": item}));
        }
        let _ = self.events.send(value);
    }

    pub(crate) async fn publish_temporary(&self, event: &str, data: Value) {
        let _event_lock = self.event_lock.lock().await;
        self.trace_runtime
            .lock()
            .await
            .update_temporary(event, &data);
        let _ = self.events.send(json!({
            "v": 1,
            "kind": "evt",
            "event": event,
            "data": data,
        }));
    }

    async fn rebuild_trace_runtime(&self) {
        let events = self
            .inner
            .read()
            .await
            .events
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        *self.trace_runtime.lock().await = TraceRuntime::from_events(events);
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
            "server_time":now,"last_seq":0,"timezone":"Asia/Shanghai","currency":"CNY","features":if load_fixture {json!(["mock","browser"])} else {json!(["browser"])}
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
        let value = self.emit_persisted(self.seq, event, data);
        if event == "trace.item" {
            let trace_data = value.get("data").cloned().unwrap_or(Value::Null);
            self.index_trace_item(&trace_data);
        }
        value
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
        let initial_state = MockState::new(&host_name, config.mock, &node_id);
        let mut trace_runtime = TraceRuntime::from_events(initial_state.events.iter().cloned());
        for items in initial_state.traces.values() {
            for item in items {
                trace_runtime.update_persistent("trace.item", &json!({"item": item}));
            }
        }
        let state = GatewayState {
            inner: Arc::new(RwLock::new(initial_state)),
            home: config.home.clone(),
            host_name: Arc::new(RwLock::new(host_name)),
            node_id: Arc::new(RwLock::new(node_id)),
            events,
            trace_runtime: Arc::new(Mutex::new(trace_runtime)),
            event_lock: Arc::new(Mutex::new(())),
            browser: Arc::new(tokio::sync::Mutex::new(BrowserManager::new(
                macbot_browser::SessionConfig::default(),
                Arc::new(ProcessRunner),
            ))),
            screen_streams: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
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
            .route(
                "/api/v1/uploads",
                post(upload_handler).layer(DefaultBodyLimit::max(MAX_UPLOAD + 1024 * 1024)),
            )
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

    /// Router for the per-user control socket. It is served only on the Unix
    /// socket and therefore does not expose password-management operations on
    /// the TCP listener.
    pub fn local_router(&self) -> Router {
        Router::new()
            .route("/__local/status", get(local_status_handler))
            .route("/__local/passwd", post(local_passwd_handler))
            .route("/__local/settings", post(local_settings_handler))
            .route("/__local/logs", get(local_logs_handler))
            .route("/__local/restart", post(local_restart_handler))
            .route("/__local/update", post(local_update_handler))
            .with_state(self.clone())
    }

    pub async fn serve(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        fs::create_dir_all(self.state.home.join("data")).await?;
        fs::create_dir_all(self.state.home.join("uploads")).await?;
        let socket_path = self.state.home.join("data/macbotd.sock");
        let _ = fs::remove_file(&socket_path).await;
        let unix = UnixListener::bind(&socket_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
        }
        let listener = TcpListener::bind(self.config_addr()).await?;
        info!(addr = %listener.local_addr()?, "macbot gateway listening");
        let tcp = axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        let local = axum::serve(unix, self.local_router().into_make_service());
        tokio::try_join!(tcp, local)?;
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

#[derive(Debug, Clone)]
struct TraceSubscription {
    assignment_id: Option<String>,
    chat_id: Option<String>,
    last_aseq: u64,
}

async fn ws_session(socket: axum::extract::ws::WebSocket, gw: Gateway) {
    let (mut sink, mut stream) = socket.split();
    let mut live = gw.state.events.subscribe();
    let mut trace_streams: HashMap<String, TraceSubscription> = HashMap::new();
    let mut last_received = std::time::Instant::now();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(60));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let host_name = gw.state.host_name.read().await.clone();
    let node_id = gw.state.node_id.read().await.clone();
    let production_hello = if !gw.mock {
        gw.rpc("bootstrap", json!({}))
            .await
            .ok()
            .and_then(|value| value.get("hello").cloned())
    } else {
        None
    };
    let hello = {
        let state = gw.state.inner.read().await;
        let mut hello = production_hello.map(|hello| json!({"v":1,"kind":"evt","event":"hello","data":hello})).unwrap_or_else(|| state
            .events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("hello"))
            .cloned()
            .unwrap_or_else(|| json!({"v":1,"kind":"evt","event":"hello","data":hello_value(&state, &host_name, &node_id)})));
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
                                let last_aseq = replay
                                    .iter()
                                    .filter_map(|event| {
                                        event
                                            .pointer("/data/item/aseq")
                                            .and_then(Value::as_u64)
                                    })
                                    .max()
                                    .unwrap_or_else(|| {
                                        request
                                            .params
                                            .get("since_aseq")
                                            .and_then(Value::as_u64)
                                            .unwrap_or(0)
                                    });
                                trace_streams.insert(
                                    stream_id.to_owned(),
                                    TraceSubscription {
                                        assignment_id: request
                                            .params
                                            .get("assignment_id")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned),
                                        chat_id: request
                                            .params
                                            .get("chat_id")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned),
                                        last_aseq,
                                    },
                                );
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
                        let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
                        if event_name == "trace.item" {
                            let item = event.get("data").and_then(|data| data.get("item")).cloned().or_else(|| event.get("data").cloned()).unwrap_or(Value::Null);
                            let aseq = item.get("aseq").and_then(Value::as_u64).unwrap_or(0);
                            let mut frames = Vec::new();
                            for (stream_id, subscription) in &mut trace_streams {
                                let matches_assignment = subscription.assignment_id.as_deref().is_none_or(|id| item.get("assignment_id").and_then(Value::as_str) == Some(id));
                                let matches_chat = subscription.chat_id.as_deref().is_none_or(|id| item.get("chat_id").and_then(Value::as_str) == Some(id));
                                if matches_assignment && matches_chat && aseq > subscription.last_aseq {
                                    subscription.last_aseq = aseq;
                                    frames.push(json!({"v":1,"kind":"evt","event":"trace.item","data":{"stream":stream_id,"item":item.clone()}}));
                                }
                            }
                            for frame in frames {
                                if sink.send(text_frame(&frame)).await.is_err() { break; }
                            }
                        } else if matches!(event_name, "trace.delta" | "trace.tool_output") {
                            let data = event.get("data").cloned().unwrap_or(Value::Null);
                            let runtime = gw.state.trace_runtime.lock().await.clone();
                            let mut frames = Vec::new();
                            for (stream_id, subscription) in &trace_streams {
                                if runtime.matches(
                                    subscription.assignment_id.as_deref(),
                                    subscription.chat_id.as_deref(),
                                    &data,
                                ) {
                                    let mut frame = event.clone();
                                    if let Some(object) = frame
                                        .get_mut("data")
                                        .and_then(Value::as_object_mut)
                                    {
                                        object.insert("stream".into(), json!(stream_id));
                                    }
                                    frames.push(frame);
                                }
                            }
                            for frame in frames {
                                if sink.send(text_frame(&frame)).await.is_err() { break; }
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
            .map(str::to_owned);
        let chat = request
            .params
            .get("chat_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if assignment.is_none() && chat.is_none() {
            return Err(rpc_error(
                "invalid_params",
                "assignment_id or chat_id is required",
                None,
            ));
        }
        let since = request
            .params
            .get("since_aseq")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let state = gw.state.inner.read().await;
        let mut items = Vec::new();
        if let Some(assignment) = assignment.as_deref() {
            items.extend(state.traces.get(assignment).cloned().unwrap_or_default());
        } else if let Some(chat) = chat.as_deref() {
            items.extend(state.traces.get(chat).cloned().unwrap_or_default());
        }
        drop(state);
        items.sort_by_key(|item| item.get("aseq").and_then(Value::as_u64).unwrap_or(0));
        items.dedup_by(|left, right| left.get("aseq") == right.get("aseq"));
        let stream = id("stream");
        let mut out = vec![];
        for item in items {
            let matches_assignment = assignment.as_deref().is_none_or(|expected| {
                item.get("assignment_id").and_then(Value::as_str) == Some(expected)
            });
            let matches_chat = chat.as_deref().is_none_or(|expected| {
                item.get("chat_id").and_then(Value::as_str) == Some(expected)
            });
            if !matches_assignment || !matches_chat {
                continue;
            }
            if item.get("aseq").and_then(Value::as_u64).unwrap_or(0) > since {
                out.push(json!({"v":1,"kind":"evt","event":"trace.item","data":{"stream":stream,"item":item}}));
            }
        }
        let in_flight = gw
            .state
            .trace_runtime
            .lock()
            .await
            .in_flight_for(assignment.as_deref(), chat.as_deref());
        return Ok(json!({"stream":stream,"in_flight":in_flight,"replay_events":out}));
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
    let mobile = screen_user_agent_is_mobile(&headers);
    let quality = configured_screen_quality(&gw, mobile, query.quality.as_deref()).await;
    ws.on_upgrade(move |socket| screen_session(socket, gw, query, quality))
        .into_response()
}
#[derive(Debug, Deserialize, Clone)]
struct ScreenQuery {
    bot_id: Option<String>,
    quality: Option<String>,
    tab_id: Option<String>,
    /// Optional assignment scope for tab selection and input isolation.
    assignment_id: Option<String>,
    token: Option<String>,
}

fn screen_user_agent_is_mobile(headers: &HeaderMap) -> bool {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        .is_some_and(|value| {
            value.contains("android")
                || value.contains("mobile")
                || value.contains("iphone")
                || value.contains("ipad")
                || value.contains("ipod")
        })
}

fn validate_screen_profile(value: Option<&Value>, fallback: ScreenQuality) -> ScreenQuality {
    let Some(profile) = value.and_then(Value::as_object) else {
        return fallback;
    };
    let Some(max_width) = profile
        .get("max_width")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (1..=8192).contains(value))
    else {
        return fallback;
    };
    let Some(jpeg_quality) = profile
        .get("quality")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| (1..=100).contains(value))
    else {
        return fallback;
    };
    let Some(max_fps) = profile
        .get("max_fps")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (1..=120).contains(value))
    else {
        return fallback;
    };
    ScreenQuality {
        max_width,
        jpeg_quality,
        max_fps,
    }
}

fn configured_screen_profile(settings: &Value, mobile: bool) -> Option<Value> {
    let profile = if mobile { "mobile" } else { "desktop" };
    settings
        .pointer(&format!("/browser/stream/{profile}"))
        .cloned()
}

async fn configured_screen_quality(
    gw: &Gateway,
    mobile: bool,
    requested: Option<&str>,
) -> ScreenQuality {
    let fallback = if mobile {
        ScreenQuality {
            max_width: 720,
            jpeg_quality: 50,
            max_fps: 10,
        }
    } else {
        ScreenQuality {
            max_width: 1280,
            jpeg_quality: 70,
            max_fps: 15,
        }
    };
    if matches!(requested, Some("high" | "low")) {
        return screen_quality(requested);
    }
    let persisted = fs::read(gw.state.home.join("data/settings.json"))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let in_memory = gw.state.inner.read().await.settings.clone();
    let profile = persisted
        .as_ref()
        .and_then(|settings| configured_screen_profile(settings, mobile))
        .or_else(|| configured_screen_profile(&in_memory, mobile));
    validate_screen_profile(profile.as_ref(), fallback)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenQuality {
    max_width: u32,
    jpeg_quality: u8,
    max_fps: u32,
}

fn screen_quality(quality: Option<&str>) -> ScreenQuality {
    match quality {
        Some("high") => ScreenQuality {
            max_width: 1600,
            jpeg_quality: 85,
            max_fps: 20,
        },
        Some("low") => ScreenQuality {
            max_width: 640,
            jpeg_quality: 30,
            max_fps: 8,
        },
        // The protocol's auto desktop profile is the server default. A
        // mobile-specific profile is kept in the browser configuration and
        // can be selected by a mobile gateway when that signal is available.
        _ => ScreenQuality {
            max_width: 1280,
            jpeg_quality: 70,
            max_fps: 15,
        },
    }
}

fn encode_screen_jpeg(jpeg: &[u8], quality: ScreenQuality) -> Option<(Vec<u8>, u32, u32)> {
    let decoded = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let (width, height) = decoded.dimensions();
    let (target_width, target_height) = if width > quality.max_width {
        let target_width = quality.max_width;
        let target_height = ((height as u64 * target_width as u64) / width as u64)
            .max(1)
            .try_into()
            .ok()?;
        (target_width, target_height)
    } else {
        (width, height)
    };
    let image = if (target_width, target_height) == (width, height) {
        decoded
    } else {
        decoded.resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
    };
    let mut output = Vec::new();
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality.jpeg_quality);
    encoder.encode_image(&image).ok()?;
    Some((output, target_width, target_height))
}

fn screen_state<R: CliRunner>(
    manager: &mut BrowserManager<R>,
    bot_id: &str,
    assignment_id: Option<&str>,
    requested_tab: Option<&str>,
    mock: bool,
) -> Result<(Value, String, Option<String>), BrowserError> {
    let state = manager
        .state(bot_id)
        .unwrap_or_else(|_| manager.session(bot_id).clone());
    let scope = assignment_id.map(str::to_owned);
    let tabs = scope
        .as_deref()
        .map(|assignment| {
            state
                .tabs
                .iter()
                .filter(|tab| tab.assignment_id == assignment)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| state.tabs.clone());
    if let Some(tab_id) = requested_tab.filter(|tab_id| {
        !(tab_id.is_empty()
            || tabs.iter().any(|tab| tab.tab_id == *tab_id)
            || mock && *tab_id == "tab_mock")
    }) {
        return Err(BrowserError::Invalid(format!(
            "tab {tab_id} is outside the screen assignment"
        )));
    }
    let tab_id = requested_tab
        .filter(|tab_id| !tab_id.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            tabs.iter()
                .find(|tab| tab.active)
                .or_else(|| tabs.first())
                .map(|tab| tab.tab_id.clone())
        })
        .unwrap_or_else(|| {
            if mock {
                "tab_mock".into()
            } else {
                String::new()
            }
        });
    let driver = if state.takeover {
        "user"
    } else if !tabs.is_empty() {
        "bot"
    } else {
        "idle"
    };
    let tabs = tabs
        .into_iter()
        .map(|tab| {
            // The selected tab is authoritative for this connection. Browser
            // sidecars may lag their tab event, so expose a deterministic
            // active marker to clients immediately after switch_tab.
            let active = tab.tab_id == tab_id;
            json!({"tab_id":tab.tab_id,"title":tab.title,"url":tab.url,
                "assignment_id":tab.assignment_id,"active":active})
        })
        .collect::<Vec<_>>();
    Ok((
        json!({"type":"state","state":{"bot_id":bot_id,
            "driver":driver,"tabs":tabs,"width":if mock {320} else {1280},
            "height":if mock {180} else {720}}}),
        tab_id,
        scope,
    ))
}

fn screen_state_url(state: &Value, tab_id: &str) -> String {
    state
        .get("state")
        .and_then(|value| value.get("tabs"))
        .and_then(Value::as_array)
        .and_then(|tabs| {
            tabs.iter()
                .find(|tab| tab.get("tab_id").and_then(Value::as_str) == Some(tab_id))
        })
        .and_then(|tab| tab.get("url").and_then(Value::as_str))
        .filter(|url| !url.is_empty())
        .unwrap_or("about:blank")
        .to_owned()
}

fn screen_error(error: &str, message: impl Into<String>) -> axum::extract::ws::Message {
    text_frame(&json!({"type":"error","error":{"code":error,"message":message.into()}}))
}

enum SidecarEvent {
    Frame {
        seq: u64,
        jpeg: Vec<u8>,
        width: u32,
        height: u32,
        viewport_width: u32,
        viewport_height: u32,
        timestamp: u64,
    },
    Url(String),
    Tabs {
        active_tab_id: Option<String>,
    },
    Ping(Vec<u8>),
    InvalidFrame {
        seq: u64,
    },
    Closed,
}

async fn sidecar_connect(
    port: u16,
    quality: ScreenQuality,
) -> Result<
    (
        tokio::io::ReadHalf<TcpStream>,
        tokio::io::WriteHalf<TcpStream>,
    ),
    String,
> {
    let mut socket = TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|error| error.to_string())?;
    let key = base64::engine::general_purpose::STANDARD.encode(Uuid::now_v7().as_bytes());
    let request = format!(
        "GET /?pacing=ack&maxFps={} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nOrigin: http://127.0.0.1\r\n\r\n",
        quality.max_fps
    );
    socket
        .write_all(request.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        socket
            .read_exact(&mut byte)
            .await
            .map_err(|error| error.to_string())?;
        response.push(byte[0]);
        if response.len() > 16 * 1024 {
            return Err("sidecar handshake headers too large".into());
        }
    }
    if !response.starts_with(b"HTTP/1.1 101") {
        return Err("sidecar stream handshake failed".into());
    }
    Ok(tokio::io::split(socket))
}

async fn sidecar_write_frame(
    writer: &mut tokio::io::WriteHalf<TcpStream>,
    opcode: u8,
    payload: &[u8],
) -> Result<(), String> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | (opcode & 0x0f));
    let length = payload.len();
    if length < 126 {
        frame.push(0x80 | length as u8);
    } else if length <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(length as u64).to_be_bytes());
    }
    let mask_uuid = Uuid::now_v7();
    let mask = &mask_uuid.as_bytes()[..4];
    frame.extend_from_slice(mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    writer
        .write_all(&frame)
        .await
        .map_err(|error| error.to_string())
}

async fn sidecar_read_frame(
    reader: &mut tokio::io::ReadHalf<TcpStream>,
) -> Result<(u8, Vec<u8>), String> {
    let mut head = [0u8; 2];
    reader
        .read_exact(&mut head)
        .await
        .map_err(|error| error.to_string())?;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let mut length = (head[1] & 0x7f) as u64;
    if length == 126 {
        let mut bytes = [0u8; 2];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        length = u16::from_be_bytes(bytes) as u64;
    } else if length == 127 {
        let mut bytes = [0u8; 8];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        length = u64::from_be_bytes(bytes);
    }
    if length > 32 * 1024 * 1024 {
        return Err("sidecar frame too large".into());
    }
    let mut mask = [0u8; 4];
    if masked {
        reader
            .read_exact(&mut mask)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut payload = vec![0u8; length as usize];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|e| e.to_string())?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok((opcode, payload))
}

async fn sidecar_reader(
    mut reader: tokio::io::ReadHalf<TcpStream>,
    events: mpsc::Sender<SidecarEvent>,
) {
    loop {
        let Ok((opcode, payload)) = sidecar_read_frame(&mut reader).await else {
            let _ = events.send(SidecarEvent::Closed).await;
            return;
        };
        match opcode {
            0x1 => {
                let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
                    continue;
                };
                match value.get("type").and_then(Value::as_str) {
                    Some("frame") => {
                        let Some(data) = value.get("data").and_then(Value::as_str) else {
                            continue;
                        };
                        let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data)
                        else {
                            continue;
                        };
                        let metadata = value.get("metadata").cloned().unwrap_or_default();
                        let Some((width, height)) = jpeg_dimensions(&jpeg) else {
                            // A bad JPEG must not become a fake frame. The
                            // sidecar stream is ack-paced, so the session loop
                            // acknowledges and discards it.
                            let _ = events
                                .send(SidecarEvent::InvalidFrame {
                                    seq: value.get("seq").and_then(Value::as_u64).unwrap_or(0),
                                })
                                .await;
                            continue;
                        };
                        // The JPEG may be downscaled for the client. Keep the
                        // browser's CDP viewport separately for input mapping.
                        let viewport_width = metadata
                            .get("deviceWidth")
                            .and_then(Value::as_u64)
                            .and_then(|value| u32::try_from(value).ok())
                            .filter(|value| *value > 0)
                            .unwrap_or(width.max(1));
                        let viewport_height = metadata
                            .get("deviceHeight")
                            .and_then(Value::as_u64)
                            .and_then(|value| u32::try_from(value).ok())
                            .filter(|value| *value > 0)
                            .unwrap_or(height.max(1));
                        let frame = SidecarEvent::Frame {
                            seq: value.get("seq").and_then(Value::as_u64).unwrap_or(0),
                            jpeg,
                            width,
                            height,
                            viewport_width,
                            viewport_height,
                            timestamp: metadata
                                .get("timestamp")
                                .and_then(Value::as_u64)
                                .unwrap_or_else(unix_ms),
                        };
                        if events.send(frame).await.is_err() {
                            return;
                        }
                    }
                    Some("url") => {
                        if let Some(url) = value.get("url").and_then(Value::as_str) {
                            if events
                                .send(SidecarEvent::Url(url.to_owned()))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Some("tabs") => {
                        let active_tab_id = value
                            .get("tabs")
                            .and_then(Value::as_array)
                            .and_then(|tabs| {
                                tabs.iter().find(|tab| {
                                    tab.get("active").and_then(Value::as_bool) == Some(true)
                                })
                            })
                            .and_then(|tab| tab.get("tabId").and_then(Value::as_str))
                            .map(str::to_owned);
                        if events
                            .send(SidecarEvent::Tabs { active_tab_id })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    _ => {}
                }
            }
            0x9 => {
                if events.send(SidecarEvent::Ping(payload)).await.is_err() {
                    return;
                }
            }
            0x8 => {
                let _ = events.send(SidecarEvent::Closed).await;
                return;
            }
            _ => {}
        }
    }
}

fn sidecar_stream_port(value: &Value) -> Option<u16> {
    value
        .get("port")
        .or_else(|| value.get("data").and_then(|data| data.get("port")))
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
}

fn sidecar_gateway_frame(
    seq: u64,
    frame: &SidecarEvent,
    tab_id: &str,
    url: &str,
    quality: ScreenQuality,
) -> Option<(Vec<u8>, u32, u32)> {
    let SidecarEvent::Frame {
        jpeg,
        width,
        height,
        timestamp,
        ..
    } = frame
    else {
        return None;
    };
    let (jpeg, actual_width, actual_height) =
        encode_screen_jpeg(jpeg, quality).unwrap_or_else(|| (jpeg.clone(), *width, *height));
    let header = json!({"seq":seq,"tab_id":tab_id,"w":actual_width,"h":actual_height,"ts":timestamp,"url":url})
        .to_string();
    let mut bytes = Vec::with_capacity(4 + header.len() + jpeg.len());
    bytes.extend_from_slice(&(header.len() as u32).to_be_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&jpeg);
    Some((bytes, actual_width, actual_height))
}

fn scale_screen_coordinate(value: f64, frame: u32, viewport: u32) -> f64 {
    if frame == 0 || viewport == 0 {
        value
    } else {
        value * f64::from(viewport) / f64::from(frame)
    }
}

fn sidecar_input_point(point: &Value, dimensions: (u32, u32, u32, u32)) -> Value {
    let (frame_width, frame_height, viewport_width, viewport_height) = dimensions;
    let mut point = point.clone();
    if let Some(object) = point.as_object_mut() {
        let x = object.get("x").and_then(Value::as_f64).unwrap_or(0.0);
        let y = object.get("y").and_then(Value::as_f64).unwrap_or(0.0);
        object.insert(
            "x".into(),
            json!(scale_screen_coordinate(x, frame_width, viewport_width)),
        );
        object.insert(
            "y".into(),
            json!(scale_screen_coordinate(y, frame_height, viewport_height)),
        );
    }
    point
}

fn sidecar_input(event: &Value, frame: Option<(u32, u32, u32, u32)>) -> Option<Vec<Value>> {
    let kind = event.get("type").and_then(Value::as_str)?;
    let (frame_width, frame_height, viewport_width, viewport_height) =
        frame.unwrap_or((0, 0, 0, 0));
    let x = scale_screen_coordinate(
        event.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        frame_width,
        viewport_width,
    );
    let y = scale_screen_coordinate(
        event.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        frame_height,
        viewport_height,
    );
    match kind {
        "mouse" => {
            let button = event
                .get("button")
                .and_then(Value::as_str)
                .unwrap_or("left");
            let click_count = event
                .get("click_count")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let message = |event_type: &str| {
                json!({
                    "type":"input_mouse", "eventType":event_type,
                    "x":x, "y":y, "button":button, "clickCount":click_count
                })
            };
            match event
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("move")
            {
                "click" => Some(vec![message("mousePressed"), message("mouseReleased")]),
                "down" => Some(vec![message("mousePressed")]),
                "up" => Some(vec![message("mouseReleased")]),
                _ => Some(vec![message("mouseMoved")]),
            }
        }
        "wheel" => Some(vec![json!({
            "type":"input_mouse", "eventType":"mouseWheel", "x":x, "y":y,
            "deltaX":scale_screen_coordinate(
                event.get("dx").and_then(Value::as_f64).unwrap_or(0.0),
                frame_width,
                viewport_width,
            ),
            "deltaY":scale_screen_coordinate(
                event.get("dy").and_then(Value::as_f64).unwrap_or(0.0),
                frame_height,
                viewport_height,
            )
        })]),
        "key" => {
            let key = event.get("key").and_then(Value::as_str).unwrap_or("");
            let text = event.get("text").cloned().unwrap_or(Value::Null);
            let message = |event_type: &str, text: Value| json!({"type":"input_keyboard","eventType":event_type,"key":key,"text":text});
            match event
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("press")
            {
                "down" => Some(vec![message("keyDown", text)]),
                "up" => Some(vec![message("keyUp", Value::Null)]),
                _ => Some(vec![
                    message("keyDown", text),
                    message("keyUp", Value::Null),
                ]),
            }
        }
        "touch" => Some(vec![json!({
            "type":"input_touch",
            "eventType": match event.get("action").and_then(Value::as_str).unwrap_or("move") {
                "start" => "touchStart", "end" => "touchEnd", _ => "touchMove"
            },
            "touchPoints":event.get("points").and_then(Value::as_array)
                .map(|points| points.iter().map(|point| sidecar_input_point(point, (frame_width, frame_height, viewport_width, viewport_height))).collect::<Vec<_>>())
                .unwrap_or_default()
        })]),
        _ => None,
    }
}

async fn release_screen_stream(gw: &Gateway, bot_id: &str, assignment_id: &str) {
    let mut streams = gw.state.screen_streams.lock().await;
    let should_disable = match streams.get_mut(bot_id) {
        Some(count) if *count > 1 => {
            *count -= 1;
            false
        }
        Some(_) => {
            streams.remove(bot_id);
            true
        }
        None => false,
    };
    if should_disable {
        let mut browser = gw.state.browser.lock().await;
        let _ = browser.set_screen_active(bot_id, false);
        let _ = browser.stream_disable_for_assignment(bot_id, assignment_id);
    }
}

async fn real_sidecar_screen_session(
    socket: axum::extract::ws::WebSocket,
    gw: Gateway,
    query: ScreenQuery,
    quality: ScreenQuality,
) {
    let (mut sink, mut client) = socket.split();
    let bot_id = query.bot_id.unwrap_or_else(|| "bot_main".into());
    let access_scope = query.assignment_id;
    let mut tab_id = query.tab_id.unwrap_or_default();
    let setup = {
        let mut browser = gw.state.browser.lock().await;
        if let Err(error) = browser.ensure_session_for_screen(&bot_id) {
            drop(browser);
            let _ = sink
                .send(screen_error("unavailable", error.to_string()))
                .await;
            return;
        }
        let state = screen_state(
            &mut browser,
            &bot_id,
            access_scope.as_deref(),
            (!tab_id.is_empty()).then_some(tab_id.as_str()),
            false,
        );
        let Ok((screen, selected, _)) = state else {
            let _ = sink
                .send(screen_error(
                    "forbidden",
                    "no browser tab is available for this Bot or assignment",
                ))
                .await;
            return;
        };
        tab_id = selected;
        let current_url = screen_state_url(&screen, &tab_id);
        // The sidecar stream is Bot-wide. An explicit assignment scopes the
        // visible tabs; an omitted assignment lets an authenticated screen
        // client browse every tab owned by the Bot. We still use the selected
        // tab's assignment to address the stream lifecycle API.
        let stream_scope = browser.state(&bot_id).ok().and_then(|state| {
            state
                .tabs
                .iter()
                .find(|tab| tab.tab_id == tab_id)
                .map(|tab| tab.assignment_id.clone())
        });
        let Some(stream_scope) = stream_scope else {
            let _ = sink
                .send(screen_error(
                    "unavailable",
                    "browser screencast is unavailable",
                ))
                .await;
            return;
        };
        let status = browser
            .stream_enable_for_assignment(&bot_id, &stream_scope, None)
            .ok();
        let port = status.as_ref().and_then(sidecar_stream_port).or_else(|| {
            browser
                .stream_status_for_assignment(&bot_id, &stream_scope)
                .ok()
                .and_then(|value| sidecar_stream_port(&value))
        });
        (screen, stream_scope, port, current_url)
    };
    let (state, stream_scope, port, initial_url) = setup;
    let Some(port) = port else {
        let mut browser = gw.state.browser.lock().await;
        let _ = browser.stream_disable_for_assignment(&bot_id, &stream_scope);
        let _ = sink
            .send(screen_error(
                "unavailable",
                "browser screencast is unavailable",
            ))
            .await;
        return;
    };
    // Serialize stream count and BrowserManager keepalive. Release takes the
    // same streams -> browser lock order, so an old connection cannot clear a
    // newly attached screen's keepalive state.
    let mut streams = gw.state.screen_streams.lock().await;
    let screen_active = {
        let mut browser = gw.state.browser.lock().await;
        browser.set_screen_active(&bot_id, true)
    };
    if let Err(error) = screen_active {
        let mut browser = gw.state.browser.lock().await;
        let _ = browser.stream_disable_for_assignment(&bot_id, &stream_scope);
        drop(browser);
        drop(streams);
        let _ = sink
            .send(screen_error("unavailable", error.to_string()))
            .await;
        return;
    }
    *streams.entry(bot_id.clone()).or_insert(0) += 1;
    drop(streams);
    if sink.send(text_frame(&state)).await.is_err() {
        release_screen_stream(&gw, &bot_id, &stream_scope).await;
        return;
    }
    let mut last_published_state = state.clone();
    let (reader_half, sidecar_writer) = match sidecar_connect(port, quality).await {
        Ok(parts) => parts,
        Err(error) => {
            let _ = sink.send(screen_error("unavailable", error)).await;
            release_screen_stream(&gw, &bot_id, &stream_scope).await;
            return;
        }
    };
    let (out_tx, mut out_rx) = mpsc::channel::<(u8, Vec<u8>)>(8);
    let writer_task = tokio::spawn(async move {
        let mut sidecar_writer = sidecar_writer;
        while let Some((opcode, payload)) = out_rx.recv().await {
            if sidecar_write_frame(&mut sidecar_writer, opcode, &payload)
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let reader_task = tokio::spawn(sidecar_reader(reader_half, events_tx));
    let mut gateway_seq = 0u64;
    let mut in_flight: Option<(u64, u64)> = None;
    let mut latest: Option<SidecarEvent> = None;
    let mut current_url = initial_url;
    // (actual JPEG width/height, CDP viewport width/height) for translating
    // client frame-pixel input into browser coordinates.
    let mut last_frame_dimensions: Option<(u32, u32, u32, u32)> = None;
    let mut sidecar_tab_id: Option<String> = None;
    let mut awaiting_tab_confirmation = false;
    let mut state_interval = tokio::time::interval(std::time::Duration::from_millis(100));
    'session: loop {
        tokio::select! {
            _ = state_interval.tick() => {
                // takeover.start/release is an RPC on the shared browser
                // manager; it does not produce a sidecar frame. Poll the
                // authoritative state so an already-open screen connection
                // promptly changes bot -> user -> bot/idle.
                let update = {
                    let mut browser = gw.state.browser.lock().await;
                    screen_state(
                        &mut browser,
                        &bot_id,
                        access_scope.as_deref(),
                        Some(&tab_id),
                        false,
                    ).ok()
                };
                if let Some((next_state, _, _)) = update {
                    if next_state != last_published_state {
                        last_published_state = next_state.clone();
                        if sink.send(text_frame(&next_state)).await.is_err() { break 'session; }
                    }
                }
            }
            incoming = events_rx.recv() => {
                let Some(incoming) = incoming else { break; };
                match incoming {
                    SidecarEvent::Frame { seq, jpeg, width, height, viewport_width, viewport_height, timestamp } => {
                        // A tab switch can leave one old frame in the sidecar
                        // queue. ACK and discard it until the active-tab event
                        // confirms that the pixels belong to the selected tab.
                        if awaiting_tab_confirmation
                            || sidecar_tab_id
                                .as_deref()
                                .is_some_and(|active| active != tab_id)
                        {
                            let _ = out_tx
                                .send((
                                    1,
                                    serde_json::to_vec(&json!({"type":"ack","seq":seq}))
                                        .unwrap_or_default(),
                                ))
                                .await;
                            continue;
                        }
                        let frame = SidecarEvent::Frame { seq, jpeg, width, height, viewport_width, viewport_height, timestamp };
                        if in_flight.is_some() { latest = Some(frame); continue; }
                        gateway_seq += 1;
                        if let Some((frame, actual_width, actual_height)) = sidecar_gateway_frame(gateway_seq, &frame, &tab_id, &current_url, quality) {
                            last_frame_dimensions = Some((actual_width, actual_height, viewport_width, viewport_height));
                            if sink.send(axum::extract::ws::Message::Binary(frame.into())).await.is_err() { break 'session; }
                            in_flight = Some((gateway_seq, seq));
                        }
                    }
                    SidecarEvent::Tabs { active_tab_id } => {
                        let initial_tabs = sidecar_tab_id.is_none();
                        sidecar_tab_id = active_tab_id.clone();
                        awaiting_tab_confirmation = false;
                        // BrowserManager restores ownership metadata, while
                        // the native browser is authoritative for the active
                        // page. A Bot-level screen must adopt that initial
                        // active tab or it can discard every first JPEG when
                        // another assignment opened the last tab.
                        if initial_tabs && access_scope.is_none() {
                            if let Some(active) = active_tab_id {
                                if active != tab_id {
                                    let known = {
                                        let browser = gw.state.browser.lock().await;
                                        browser
                                            .state(&bot_id)
                                            .ok()
                                            .is_some_and(|state| {
                                                state.tabs.iter().any(|tab| tab.tab_id == active)
                                            })
                                    };
                                    if known {
                                        tab_id = active;
                                        let update = {
                                            let mut browser = gw.state.browser.lock().await;
                                            screen_state(
                                                &mut browser,
                                                &bot_id,
                                                access_scope.as_deref(),
                                                Some(&tab_id),
                                                false,
                                            )
                                            .ok()
                                        };
                                        if let Some((state, _, _)) = update {
                                            last_published_state = state.clone();
                                            current_url = screen_state_url(&state, &tab_id);
                                            if sink.send(text_frame(&state)).await.is_err() {
                                                break 'session;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    SidecarEvent::Url(url) => {
                        current_url = url.clone();
                        // URL/title changes arrive from the sidecar before the
                        // next JPEG. Publish a state update so clients do not
                        // have to wait for (or decode) a frame to refresh tabs.
                        let update = {
                            let mut browser = gw.state.browser.lock().await;
                            let _ = browser.update_tab_url(&bot_id, &tab_id, &url);
                            screen_state(
                                &mut browser,
                                &bot_id,
                                access_scope.as_deref(),
                                Some(&tab_id),
                                false,
                            )
                            .ok()
                        };
                        if let Some((state, _, _)) = update {
                            last_published_state = state.clone();
                            if sink.send(text_frame(&state)).await.is_err() {
                                break 'session;
                            }
                        }
                    },
                    SidecarEvent::Ping(payload) => { let _ = out_tx.send((0xA, payload)).await; }
                    SidecarEvent::InvalidFrame { seq } => {
                        let _ = out_tx
                            .send((1, serde_json::to_vec(&json!({"type":"ack","seq":seq})).unwrap_or_default()))
                            .await;
                    }
                    SidecarEvent::Closed => break,
                }
            }
            incoming = client.next() => {
                let Some(Ok(message)) = incoming else { break; };
                match message {
                    axum::extract::ws::Message::Text(text) => {
                        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
                        match parsed.get("type").and_then(Value::as_str) {
                            Some("ack") => {
                                if let Some((gateway, sidecar)) = in_flight {
                                    if parsed.get("seq").and_then(Value::as_u64) == Some(gateway) {
                                        let _ = out_tx.send((1, serde_json::to_vec(&json!({"type":"ack","seq":sidecar})).unwrap_or_default())).await;
                                        in_flight = None;
                                        if let Some(frame) = latest.take() {
                                            gateway_seq += 1;
                                            if let Some((bytes, actual_width, actual_height)) = sidecar_gateway_frame(gateway_seq, &frame, &tab_id, &current_url, quality) {
                                                if let SidecarEvent::Frame { viewport_width, viewport_height, .. } = &frame {
                                                    last_frame_dimensions = Some((actual_width, actual_height, *viewport_width, *viewport_height));
                                                }
                                                if sink.send(axum::extract::ws::Message::Binary(bytes.into())).await.is_err() { break 'session; }
                                                if let SidecarEvent::Frame { seq, .. } = frame { in_flight = Some((gateway_seq, seq)); }
                                            }
                                        }
                                    }
                                }
                            }
                            Some("switch_tab") => {
                                let Some(requested) = parsed.get("tab_id").and_then(Value::as_str) else { continue; };
                                let mut browser = gw.state.browser.lock().await;
                                if !browser.state(&bot_id).map(|state| state.takeover).unwrap_or(false) {
                                    let _ = sink.send(screen_error("permission_denied", "takeover is not active")).await;
                                    continue;
                                }
                                let requested_scope = browser.state(&bot_id).ok().and_then(|state| {
                                    state.tabs.iter().find(|tab| tab.tab_id == requested).map(|tab| tab.assignment_id.clone())
                                });
                                let Some(requested_scope) = requested_scope else {
                                    let _ = sink.send(screen_error("forbidden", "tab is outside this Bot")).await;
                                    continue;
                                };
                                if access_scope
                                    .as_deref()
                                    .is_some_and(|scope| scope != requested_scope)
                                {
                                    let _ = sink.send(screen_error("forbidden", "tab is outside the screen assignment")).await;
                                    continue;
                                }
                                if browser
                                    .tab_for_assignment(&bot_id, &requested_scope, Some(requested))
                                    .is_err()
                                {
                                    let _ = sink
                                        .send(screen_error("forbidden", "tab is outside the screen assignment"))
                                        .await;
                                    continue;
                                }
                                if let Err(error) = browser.switch_tab(&bot_id, &requested_scope) {
                                    let _ = sink
                                        .send(screen_error("unavailable", error.to_string()))
                                        .await;
                                    continue;
                                }
                                let requested_url = browser
                                    .state(&bot_id)
                                    .ok()
                                    .and_then(|state| {
                                        state
                                            .tabs
                                            .iter()
                                            .find(|tab| tab.tab_id == requested)
                                            .map(|tab| tab.url.clone())
                                    })
                                    .filter(|url| !url.is_empty());
                                drop(browser);
                                tab_id = requested.to_owned();
                                current_url = requested_url.unwrap_or_else(|| "about:blank".into());
                                sidecar_tab_id = None;
                                awaiting_tab_confirmation = true;
                                let update = {
                                    let mut browser = gw.state.browser.lock().await;
                                    screen_state(
                                        &mut browser,
                                        &bot_id,
                                        access_scope.as_deref(),
                                        Some(&tab_id),
                                        false,
                                    )
                                    .ok()
                                };
                                if let Some((state, _, _)) = update {
                                    last_published_state = state.clone();
                                    if sink.send(text_frame(&state)).await.is_err() {
                                        break 'session;
                                    }
                                }
                            }
                            Some("input") => {
                                let Some(event) = parsed.get("event") else { let _ = sink.send(screen_error("invalid_request", "event is required")).await; continue; };
                                if serde_json::from_value::<macbot_protocol::ScreenInput>(event.clone()).is_err() {
                                    let _ = sink.send(screen_error("invalid_request", "invalid screen event")).await; continue;
                                }
                                let allowed = gw.state.browser.lock().await.state(&bot_id).map(|state| state.takeover).unwrap_or(false);
                                if !allowed { let _ = sink.send(screen_error("permission_denied", "takeover is not active")).await; continue; }
                                let Some(sidecar_events) = sidecar_input(event, last_frame_dimensions) else { continue; };
                                for sidecar_event in sidecar_events {
                                let _ = out_tx.send((1, serde_json::to_vec(&sidecar_event).unwrap_or_default())).await;
                            }
                            }
                            _ => {}
                        }
                    }
                    axum::extract::ws::Message::Ping(payload) => { let _ = sink.send(axum::extract::ws::Message::Pong(payload)).await; }
                    axum::extract::ws::Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    reader_task.abort();
    writer_task.abort();
    release_screen_stream(&gw, &bot_id, &stream_scope).await;
}

fn mock_screen_view(
    state: &MockState,
    bot_id: &str,
    assignment_id: Option<&str>,
) -> (String, Vec<Value>) {
    let tabs = state
        .extra
        .get(&format!("screen_tabs:{bot_id}"))
        .into_iter()
        .flatten()
        .filter(|tab| {
            assignment_id.is_none_or(|assignment| {
                tab.get("assignment_id").and_then(Value::as_str) == Some(assignment)
            })
        })
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
    let (width, height) = jpeg_dimensions(jpeg).unwrap_or((320, 180));
    let header = json!({"seq":seq,"tab_id":tab_id,"w":width,"h":height,"ts":unix_ms(),"url":url})
        .to_string();
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
    let requested_tab = query.tab_id;
    let mut selected = requested_tab.unwrap_or_default();
    let assignment_id = query.assignment_id;
    let mut seq = 1u64;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
    let view = gw.state.inner.read().await;
    let (mut driver, mut tabs) = mock_screen_view(&view, &bot_id, assignment_id.as_deref());
    drop(view);
    if selected.is_empty() {
        selected = tabs
            .iter()
            .find(|tab| tab.get("active").and_then(Value::as_bool) == Some(true))
            .or_else(|| tabs.first())
            .and_then(|tab| tab.get("tab_id").and_then(Value::as_str))
            .unwrap_or("tab_mock_1")
            .to_owned();
    }
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
                let (next_driver, next_tabs) = mock_screen_view(&view, &bot_id, assignment_id.as_deref());
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

async fn screen_session(
    socket: axum::extract::ws::WebSocket,
    gw: Gateway,
    query: ScreenQuery,
    quality: ScreenQuality,
) {
    if gw.mock {
        mock_screen_session(socket, gw, query).await;
        return;
    }
    real_sidecar_screen_session(socket, gw, query, quality).await;
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
    let root_id = q
        .root_id
        .as_deref()
        .filter(|value| is_safe_component(value));
    let Some(root_id) = root_id else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_params",
            "root_id must be one normal path component",
        ));
    };
    let root = match q.root.as_str() {
        "project" => home
            .join("projects")
            .join(project_slug(home, root_id).unwrap_or_else(|| root_id.to_owned())),
        "bot" => home.join("bots").join(root_id),
        "upload" => home.join("uploads").join(root_id),
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
    let candidate = if q.path.is_empty() {
        root.clone()
    } else {
        root.join(rel)
    };
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

fn project_slug(home: &FsPath, project_id: &str) -> Option<String> {
    let snapshot = home.join("data/orchestrator/state.json");
    let value = serde_json::from_str::<Value>(&std::fs::read_to_string(snapshot).ok()?).ok()?;
    let slug = value
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| projects.get(project_id))
        .and_then(|project| project.get("slug"))
        .and_then(Value::as_str)
        .filter(|slug| is_safe_component(slug))?;
    Some(slug.to_owned())
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
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_params",
                    "invalid multipart body",
                );
            }
        };
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
    if file.sync_all().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "cannot persist upload",
        );
    }
    let mime = mime_guess::from_path(&name)
        .first_or_octet_stream()
        .to_string();
    let reference =
        json!({"root":"upload","root_id":upload_id,"path":"","name":name,"size":size,"mime":mime});
    let metadata_dir = gw.state.home.join("data/uploads");
    let metadata_path = metadata_dir.join(format!("{upload_id}.json"));
    let temporary_path = metadata_dir.join(format!("{upload_id}.json.tmp"));
    let persist_metadata = async {
        fs::create_dir_all(&metadata_dir).await?;
        let mut metadata = fs::File::create(&temporary_path).await?;
        metadata.write_all(&serde_json::to_vec(&reference)?).await?;
        metadata.sync_all().await?;
        fs::rename(&temporary_path, &metadata_path).await?;
        fs::File::open(&metadata_dir).await?.sync_all().await?;
        Ok::<_, std::io::Error>(())
    };
    if persist_metadata.await.is_err() {
        let _ = fs::remove_file(&target).await;
        let _ = fs::remove_file(&temporary_path).await;
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "cannot persist upload metadata",
        );
    }
    Json(json!({"upload_id":upload_id,"file":reference})).into_response()
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
#[derive(Debug, Deserialize, Default)]
struct UsageCsvQuery {
    from: Option<String>,
    to: Option<String>,
    dimension: Option<String>,
    bot_id: Option<String>,
    project_id: Option<String>,
    timezone: Option<String>,
}
async fn usage_csv_handler(
    State(gw): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<UsageCsvQuery>,
) -> Response {
    if let Err(r) = authorize(&gw, &headers, None).await {
        return r;
    }
    let to = query.to.unwrap_or_else(now);
    let from = query.from.unwrap_or_else(|| {
        DateTime::parse_from_rfc3339(&to)
            .map(|value| (value.with_timezone(&Utc) - ChronoDuration::days(30)).to_rfc3339())
            .unwrap_or_else(|_| to.clone())
    });
    let dimension = query.dimension.unwrap_or_else(|| "bot".into());
    let mut params = json!({
        "from": from,
        "to": to,
        "dimension": dimension,
    });
    if let Some(bot_id) = query.bot_id {
        params["drill"] = json!({"bot_id": bot_id});
    } else if let Some(project_id) = query.project_id {
        params["drill"] = json!({"project_id": project_id});
    }
    let timezone = if let Some(timezone) = query.timezone {
        timezone
    } else {
        match gw.rpc("settings.get", json!({})).await {
            Ok(settings) => settings
                .get("settings")
                .and_then(|value| value.get("timezone"))
                .and_then(Value::as_str)
                .unwrap_or("Asia/Shanghai")
                .to_owned(),
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &error.code,
                    &error.message,
                )
            }
        }
    };
    let body = if let Some(backend) = gw.backend.as_ref() {
        match backend.export_usage_csv(&params, &timezone).await {
            Ok(body) => body,
            Err(error) => {
                let status = if error.code == "invalid_params" {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                return error_response(status, &error.code, &error.message);
            }
        }
    } else {
        match gw.rpc("usage.breakdown", params).await {
            Ok(value) => usage_breakdown_csv(&value),
            Err(error) => {
                return error_response(StatusCode::BAD_REQUEST, &error.code, &error.message)
            }
        }
    };
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

fn usage_breakdown_csv(value: &Value) -> String {
    let mut csv =
        "key,label,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,requests,cost\n"
            .to_owned();
    for row in value
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let quote = |value: &Value| {
            let text = value.as_str().unwrap_or("");
            format!("\"{}\"", text.replace('"', "\"\""))
        };
        let usage = row.get("usage").unwrap_or(&Value::Null);
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            quote(row.get("key").unwrap_or(&Value::Null)),
            quote(row.get("label").unwrap_or(&Value::Null)),
            usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("cache_read_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("cache_write_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage.get("requests").and_then(Value::as_u64).unwrap_or(0),
            usage
                .get("cost")
                .filter(|cost| !cost.is_null())
                .map(ToString::to_string)
                .unwrap_or_default()
        ));
    }
    csv
}

async fn admin_handler(
    State(gw): State<Gateway>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !is_local_or_basic(remote, &headers, &gw).await {
        let mut response = error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "admin authentication required",
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Mac Bot\""),
        );
        return response;
    }
    Html(ADMIN_HTML).into_response()
}

const ADMIN_HTML: &str = r#"<!doctype html>
<meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Mac Bot Server</title><h1>Mac Bot Server</h1>
<p id="status">加载中…</p>
<section id="setup"><h2>首次设置密码</h2>
<form method="post" action="/admin/setup"><input name="password" type="password" required placeholder="访问密码"><button>设置密码</button></form></section>
<section id="settings"><h2>设置</h2>
<form id="settings-form"><label>名称 <input id="host_name"></label>
<label>端口 <input id="port" type="number" min="1" max="65535"></label>
<button>保存</button></form><p id="settings-result"></p></section>
<section><h2>日志</h2><button id="reload-logs">刷新</button><pre id="logs"></pre></section>
<button id="restart">重启服务</button>
<script>
const $ = (id) => document.getElementById(id);
async function load() {
  const r = await fetch('/admin/status');
  if (!r.ok) { $('status').textContent = '需要 HTTP Basic Auth'; return; }
  const x = await r.json();
  if (x.setup_required) { $('status').textContent = '请先设置访问密码'; $('settings').style.display='none'; return; }
  $('status').textContent = `运行中 · ${x.host_name} · 端口 ${x.port} · seq ${x.seq}`;
  $('host_name').value = x.host_name || '';
  $('port').value = x.port || '';
  $('setup').style.display = 'none';
  await logs();
}
async function logs() { $('logs').textContent = await (await fetch('/admin/logs')).text(); }
$('settings-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const r = await fetch('/admin/settings', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({host_name:$('host_name').value, port:Number($('port').value)})});
  $('settings-result').textContent = r.ok ? '已保存，重启后端口生效' : await r.text();
});
$('reload-logs').onclick = logs;
$('restart').onclick = async () => { $('settings-result').textContent = await (await fetch('/admin/restart', {method:'POST'})).text(); };
load().catch((e) => $('status').textContent = e.toString());
</script>"#;
async fn is_local_or_basic(remote: SocketAddr, headers: &HeaderMap, gw: &Gateway) -> bool {
    if gw.auth.setup_required().await {
        return remote.ip().is_loopback();
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
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(patch): Json<Value>,
) -> Response {
    if !is_local_or_basic(remote, &headers, &gw).await {
        let mut response = error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "admin authentication required",
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Mac Bot\""),
        );
        return response;
    }
    local_settings_handler(State(gw), Json(patch)).await
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
    Json(json!({"running":true,"setup_required":gw.setup_required().await,"port":gw.bind_addr.port(),"host_name":gw.state.host_name.read().await.clone(),"seq":state.seq,"mock":gw.mock})).into_response()
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

#[derive(Debug, Deserialize)]
struct LocalPassword {
    password: String,
}

async fn local_settings_handler(State(gw): State<Gateway>, Json(patch): Json<Value>) -> Response {
    let settings_path = gw.state.home.join("data/settings.json");
    let current = fs::read_to_string(&settings_path)
        .await
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    let mut settings = current.as_object().cloned().unwrap_or_default();
    if let Some(name) = patch.get("host_name") {
        settings.insert("host_name".into(), name.clone());
    }
    if let Some(port) = patch.get("port").and_then(Value::as_u64) {
        if !(1..=u16::MAX as u64).contains(&port) {
            return error_response(StatusCode::BAD_REQUEST, "invalid_port", "port out of range");
        }
        settings.insert("port".into(), json!(port));
    }
    let tmp = settings_path.with_extension("json.tmp");
    let result = serde_json::to_vec_pretty(&Value::Object(settings.clone()))
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            use std::io::Write;
            std::fs::create_dir_all(settings_path.parent().unwrap_or(&gw.state.home))
                .map_err(|error| error.to_string())?;
            let mut file = std::fs::File::create(&tmp).map_err(|error| error.to_string())?;
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            std::fs::rename(&tmp, &settings_path).map_err(|error| error.to_string())?;
            std::fs::File::open(settings_path.parent().unwrap_or(&gw.state.home))
                .and_then(|directory| directory.sync_all())
                .map_err(|error| error.to_string())
        });
    match result {
        Ok(()) => {
            if let Some(name) = patch.get("host_name").and_then(Value::as_str) {
                *gw.state.host_name.write().await = name.to_owned();
            }
            Json(json!({"ok":true,"settings":settings,"restart_required":patch.get("port").is_some()})).into_response()
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "settings", &error),
    }
}

async fn local_status_handler(State(gw): State<Gateway>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "setup_required": gw.setup_required().await,
        "address": gw.bind_addr.to_string(),
        "home": gw.state.home,
    }))
}

async fn local_passwd_handler(
    State(gw): State<Gateway>,
    Json(payload): Json<LocalPassword>,
) -> Response {
    match gw.set_password(&payload.password).await {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(error) => error_response(StatusCode::BAD_REQUEST, "invalid_password", &error),
    }
}

async fn local_logs_handler(State(gw): State<Gateway>) -> Response {
    match fs::read_to_string(gw.state.home.join("data/macbot.log")).await {
        Ok(log) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], log).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            String::new(),
        )
            .into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "logs",
            &error.to_string(),
        ),
    }
}

async fn local_restart_handler(State(_gw): State<Gateway>) -> Json<Value> {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|uid| !uid.is_empty());
    let requested = uid.as_deref().is_some_and(|uid| {
        std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &format!("gui/{uid}/com.macbot.server")])
            .status()
            .is_ok_and(|status| status.success())
    });
    Json(json!({"ok":requested,"restarting":requested}))
}

async fn local_update_handler(State(_gw): State<Gateway>) -> Response {
    let configured = std::env::var_os("MACBOT_UPDATE_SCRIPT").map(PathBuf::from);
    let (script, bundled) = if let Some(path) = configured.filter(|path| path.is_file()) {
        (path, false)
    } else {
        let resources = std::env::current_exe()
            .ok()
            .and_then(|executable| executable.parent().map(FsPath::to_path_buf))
            .and_then(|bin| bin.parent().map(FsPath::to_path_buf))
            .map(|contents| contents.join("Resources/update-installed.sh"));
        if let Some(path) = resources.filter(|path| path.is_file()) {
            (path, true)
        } else {
            let source = std::env::current_exe()
                .ok()
                .and_then(|executable| executable.parent().map(FsPath::to_path_buf))
                .and_then(|bin| bin.parent().map(FsPath::to_path_buf))
                .and_then(|root| root.parent().map(FsPath::to_path_buf))
                .map(|server| server.join("macbotd/packaging/update.sh"));
            let Some(path) = source.filter(|path| path.is_file()) else {
                return error_response(
                    StatusCode::NOT_IMPLEMENTED,
                    "update_unavailable",
                    "no update script or packaged manifest",
                );
            };
            (path, false)
        }
    };
    if bundled {
        let manifest = script.with_file_name("update-manifest.json");
        let url = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| value.get("url").and_then(Value::as_str).map(str::to_owned));
        if url.as_deref().is_none_or(str::is_empty) {
            return error_response(
                StatusCode::NOT_IMPLEMENTED,
                "update_unavailable",
                "packaged update manifest has no URL",
            );
        }
    }
    match std::process::Command::new("sh").arg(&script).spawn() {
        Ok(_) => Json(json!({"ok":true,"started":true,"script":script})).into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update",
            &error.to_string(),
        ),
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
        let production = Arc::new(ProductionBackend::open(&config.home)?);
        let housekeeping_store = production.store.clone();
        // Rebuild the in-memory event window used by websocket resume and
        // trace replay from the durable global event log before accepting
        // connections.  The store has already repaired a truncated tail.
        let persisted = production.store.events_since(0)?;
        {
            let mut state = gateway.state.inner.write().await;
            for event in persisted {
                state.restore_event(event.seq, &event.event, event.data);
            }
        }
        gateway.state.rebuild_trace_runtime().await;
        let composed = Arc::new(ComposedBackend::open(
            production,
            gateway.state.clone(),
            config.home.clone(),
        )?);
        let tick_backend = composed.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            let mut last_upload_cleanup = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(60 * 60))
                .unwrap_or_else(std::time::Instant::now);
            loop {
                interval.tick().await;
                if let Err(error) = tick_backend.tick_routines(Utc::now()).await {
                    warn!(?error, "routine scheduler tick failed");
                }
                if let Err(error) = tick_backend.tick_features().await {
                    warn!(?error, "feature maintenance tick failed");
                }
                if last_upload_cleanup.elapsed() >= std::time::Duration::from_secs(60 * 60) {
                    if let Err(error) = housekeeping::cleanup_uploads(
                        &housekeeping_store,
                        std::time::SystemTime::now(),
                    ) {
                        warn!(?error, "upload housekeeping failed");
                    }
                    last_upload_cleanup = std::time::Instant::now();
                }
            }
        });
        gateway.with_backend(composed)
    };
    fs::create_dir_all(&config.home).await?;
    fs::create_dir_all(config.home.join("data")).await?;
    fs::create_dir_all(config.home.join("uploads")).await?;
    let socket_path = config.home.join("data/macbotd.sock");
    let _ = fs::remove_file(&socket_path).await;
    let unix = UnixListener::bind(&socket_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    }
    let listener = TcpListener::bind(config.bind_addr).await?;
    info!(addr=%config.bind_addr, socket=%socket_path.display(), "macbot gateway listening");
    let tcp = axum::serve(
        listener,
        gateway
            .router()
            .into_make_service_with_connect_info::<SocketAddr>(),
    );
    let local = axum::serve(unix, gateway.local_router().into_make_service());
    tokio::try_join!(tcp, local)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;
    #[tokio::test]
    async fn configured_admin_requires_basic_even_on_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            ..Default::default()
        });
        let remote = "127.0.0.1:1234".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert!(!is_local_or_basic(remote, &headers, &gw).await);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic YWRtaW46ZGV2"),
        );
        assert!(is_local_or_basic(remote, &headers, &gw).await);
        let fresh = Gateway::new(GatewayConfig {
            home: dir.path().join("fresh"),
            ..Default::default()
        });
        assert!(is_local_or_basic(remote, &HeaderMap::new(), &fresh).await);
        assert!(
            !is_local_or_basic("192.0.2.1:1234".parse().unwrap(), &HeaderMap::new(), &fresh).await
        );
    }

    #[tokio::test]
    async fn admin_settings_preserves_models_and_rejects_invalid_port() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            ..Default::default()
        });
        let path = dir.path().join("data/settings.json");
        let prior = json!({"host_name":"before","models":{"work":{"provider_id":"fake","model_id":"fake"}},"concurrency":{"global":2},"web_search":{"provider":"fake"}});
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, serde_json::to_vec(&prior).unwrap())
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic YWRtaW46ZGV2"),
        );
        let response = admin_settings_handler(
            State(gw.clone()),
            ConnectInfo("127.0.0.1:1234".parse().unwrap()),
            headers,
            Json(json!({"host_name":"after","port":7801})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let saved: Value = serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
        assert_eq!(saved["models"], prior["models"]);
        assert_eq!(saved["concurrency"], prior["concurrency"]);
        assert_eq!(saved["web_search"], prior["web_search"]);
        assert_eq!(saved["host_name"], "after");
        let response = local_settings_handler(
            State(gw.clone()),
            Json(json!({"host_name":"invalid","port":0})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(gw.state.host_name.read().await.as_str(), "after");
    }
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
        assert!(gw
            .state
            .trace_runtime
            .lock()
            .await
            .in_flight
            .contains_key("req_product_1"));
        let project = gw
            .rpc("project.get", json!({"project_id":"prj_login"}))
            .await
            .unwrap();
        assert!(!project["announcement"]["artifacts"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn trace_subscribe_returns_inflight_and_applies_both_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let item = json!({
            "assignment_id":"assignment_trace",
            "chat_id":"chat_trace",
            "run_id":"run_trace",
            "aseq":1,
            "type":"llm.request",
            "data":{"request_id":"run_trace:llm:0"}
        });
        gw.state
            .publish_event(1, "trace.item", json!({"stream":"chat_trace","item":item}))
            .await;
        gw.state
            .publish_temporary(
                "trace.delta",
                json!({"stream":"chat_trace","request_id":"run_trace:llm:0","channel":"text","text":"partial"}),
            )
            .await;

        let request = WsReq {
            v: Some(1),
            kind: "req".into(),
            id: "subscribe".into(),
            method: "trace.subscribe".into(),
            params: json!({"assignment_id":"assignment_trace","chat_id":"chat_trace","since_aseq":0}),
        };
        let result = handle_ws_request(&gw, &request).await.unwrap();
        assert_eq!(result["replay_events"].as_array().unwrap().len(), 1);
        assert_eq!(result["in_flight"][0]["request_id"], "run_trace:llm:0");
        assert_eq!(result["in_flight"][0]["text"], "partial");

        let wrong_chat = WsReq {
            params: json!({"assignment_id":"assignment_trace","chat_id":"other","since_aseq":0}),
            ..request
        };
        let result = handle_ws_request(&gw, &wrong_chat).await.unwrap();
        assert!(result["replay_events"].as_array().unwrap().is_empty());
        assert!(result["in_flight"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn trace_runtime_handles_concurrent_stream_deltas_and_subscribes() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let item = json!({
            "assignment_id":"assignment_concurrent",
            "chat_id":"chat_concurrent",
            "run_id":"run_concurrent",
            "aseq":1,
            "type":"llm.request",
            "data":{"request_id":"run_concurrent:llm:0"}
        });
        gw.state
            .publish_event(
                1,
                "trace.item",
                json!({"stream":"chat_concurrent","item":item}),
            )
            .await;
        let publisher = {
            let state = gw.state.clone();
            tokio::spawn(async move {
                let mut tasks = Vec::new();
                for _ in 0..32 {
                    let state = state.clone();
                    tasks.push(tokio::spawn(async move {
                        state
                            .publish_temporary(
                                "trace.delta",
                                json!({"request_id":"run_concurrent:llm:0","channel":"text","text":"x"}),
                            )
                            .await;
                    }));
                }
                for task in tasks {
                    task.await.unwrap();
                }
            })
        };
        let subscriber = {
            let state = gw.clone();
            tokio::spawn(async move {
                for _ in 0..16 {
                    let request = WsReq {
                        v: Some(1),
                        kind: "req".into(),
                        id: "subscribe".into(),
                        method: "trace.subscribe".into(),
                        params: json!({"assignment_id":"assignment_concurrent","since_aseq":0}),
                    };
                    handle_ws_request(&state, &request).await.unwrap();
                }
            })
        };
        publisher.await.unwrap();
        subscriber.await.unwrap();
        let runtime = gw.state.trace_runtime.lock().await;
        assert_eq!(runtime.in_flight["run_concurrent:llm:0"].text.len(), 32);
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
    fn upload_file_ref_with_empty_path_resolves_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("uploads")).unwrap();
        std::fs::write(dir.path().join("uploads/upload-1"), b"uploaded bytes").unwrap();
        let query = FileQuery {
            root: "upload".into(),
            root_id: Some("upload-1".into()),
            path: String::new(),
        };
        let path = resolve_file(dir.path(), &query).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"uploaded bytes");
    }

    #[test]
    fn file_resolution_uses_project_slug_and_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data/orchestrator")).unwrap();
        std::fs::write(
            dir.path().join("data/orchestrator/state.json"),
            json!({"projects":{"project-1":{"slug":"login-feature"}}}).to_string(),
        )
        .unwrap();
        let project = FileQuery {
            root: "project".into(),
            root_id: Some("project-1".into()),
            path: "artifact.md".into(),
        };
        assert_eq!(
            resolve_file(dir.path(), &project).unwrap(),
            dir.path().join("projects/login-feature/artifact.md")
        );
        let root_escape = FileQuery {
            root: "bot".into(),
            root_id: Some("../escape".into()),
            path: "file".into(),
        };
        assert!(resolve_file(dir.path(), &root_escape).is_err());
        let path_escape = FileQuery {
            root: "bot".into(),
            root_id: Some("bot-1".into()),
            path: "../escape".into(),
        };
        assert!(resolve_file(dir.path(), &path_escape).is_err());
    }

    #[test]
    fn mock_screen_frames_have_protocol_dimensions() {
        for frame in MOCK_SCREEN_FRAMES {
            assert_eq!(jpeg_dimensions(frame), Some((320, 180)));
        }
    }

    #[test]
    fn screen_quality_matches_protocol_profiles() {
        assert_eq!(
            screen_quality(Some("low")),
            ScreenQuality {
                max_width: 640,
                jpeg_quality: 30,
                max_fps: 8
            }
        );
        assert_eq!(
            screen_quality(Some("high")),
            ScreenQuality {
                max_width: 1600,
                jpeg_quality: 85,
                max_fps: 20
            }
        );
        assert_eq!(
            screen_quality(Some("auto")),
            ScreenQuality {
                max_width: 1280,
                jpeg_quality: 70,
                max_fps: 15
            }
        );
    }

    #[test]
    fn screen_auto_selects_mobile_from_user_agent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (Linux; Android 16; Mobile)"),
        );
        assert!(screen_user_agent_is_mobile(&headers));
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0)"),
        );
        assert!(!screen_user_agent_is_mobile(&headers));
    }

    #[tokio::test]
    async fn screen_auto_uses_configured_desktop_and_mobile_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            mock: true,
            ..Default::default()
        });
        fs::create_dir_all(dir.path().join("data")).await.unwrap();
        fs::write(
            dir.path().join("data/settings.json"),
            serde_json::to_vec(&json!({
                "browser":{"stream":{
                    "desktop":{"max_width":1024,"quality":61,"max_fps":12},
                    "mobile":{"max_width":480,"quality":41,"max_fps":7}
                }}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            configured_screen_quality(&gw, false, Some("auto")).await,
            ScreenQuality {
                max_width: 1024,
                jpeg_quality: 61,
                max_fps: 12
            }
        );
        assert_eq!(
            configured_screen_quality(&gw, true, Some("auto")).await,
            ScreenQuality {
                max_width: 480,
                jpeg_quality: 41,
                max_fps: 7
            }
        );
        assert_eq!(
            configured_screen_quality(&gw, true, Some("high")).await,
            screen_quality(Some("high"))
        );
    }

    #[test]
    fn screen_profile_rejects_out_of_range_values() {
        let fallback = screen_quality(Some("auto"));
        assert_eq!(
            validate_screen_profile(
                Some(&json!({"max_width":0,"quality":70,"max_fps":15})),
                fallback
            ),
            fallback
        );
        assert_eq!(
            validate_screen_profile(
                Some(&json!({"max_width":1280,"quality":101,"max_fps":15})),
                fallback
            ),
            fallback
        );
    }

    #[test]
    fn screen_frame_header_uses_encoded_jpeg_dimensions_and_url() {
        let jpeg = MOCK_SCREEN_FRAMES[0].to_vec();
        let dimensions = jpeg_dimensions(&jpeg).unwrap();
        let event = SidecarEvent::Frame {
            seq: 4,
            jpeg,
            width: 1,
            height: 1,
            viewport_width: 1280,
            viewport_height: 720,
            timestamp: 99,
        };
        let (bytes, actual_width, actual_height) = sidecar_gateway_frame(
            7,
            &event,
            "tab-1",
            "https://example.test/login",
            screen_quality(Some("low")),
        )
        .unwrap();
        assert_eq!((actual_width, actual_height), dimensions);
        let header_len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&bytes[4..4 + header_len]).unwrap();
        assert_eq!(
            (
                header["w"].as_u64().unwrap() as u32,
                header["h"].as_u64().unwrap() as u32
            ),
            dimensions
        );
        assert_eq!(header["url"], "https://example.test/login");
        assert_eq!(jpeg_dimensions(&bytes[4 + header_len..]), Some(dimensions));
    }

    #[test]
    fn sidecar_input_scales_frame_pixels_to_viewport_coordinates() {
        let dimensions = Some((640, 360, 1280, 720));
        let mouse = sidecar_input(
            &json!({"type":"mouse","action":"move","x":320.0,"y":180.0,"button":"left","click_count":1}),
            dimensions,
        )
        .unwrap();
        assert_eq!(mouse.len(), 1);
        assert_eq!(mouse[0]["x"], 640.0);
        assert_eq!(mouse[0]["y"], 360.0);

        let wheel = sidecar_input(
            &json!({"type":"wheel","x":320.0,"y":180.0,"dx":10.0,"dy":20.0}),
            dimensions,
        )
        .unwrap();
        assert_eq!(wheel[0]["x"], 640.0);
        assert_eq!(wheel[0]["y"], 360.0);
        assert_eq!(wheel[0]["deltaX"], 20.0);
        assert_eq!(wheel[0]["deltaY"], 40.0);
    }

    #[test]
    fn sidecar_click_and_key_press_emit_physical_pairs() {
        let click = sidecar_input(
            &json!({"type":"mouse","action":"click","x":320.0,"y":180.0,"button":"left","click_count":1}),
            Some((640, 360, 1280, 720)),
        )
        .unwrap();
        assert_eq!(click.len(), 2);
        assert_eq!(click[0]["eventType"], "mousePressed");
        assert_eq!(click[1]["eventType"], "mouseReleased");
        assert_eq!(click[0]["x"], 640.0);
        assert_eq!(click[0]["y"], 360.0);

        let key = sidecar_input(
            &json!({"type":"key","action":"press","key":"Enter","code":"Enter","text":null,"modifiers":[]}),
            None,
        )
        .unwrap();
        assert_eq!(key.len(), 2);
        assert_eq!(key[0]["eventType"], "keyDown");
        assert_eq!(key[1]["eventType"], "keyUp");
    }

    #[test]
    fn sidecar_input_scales_touch_points() {
        let touch = sidecar_input(
            &json!({"type":"touch","action":"move","points":[{"x":0.0,"y":90.0},{"x":640.0,"y":360.0}]}),
            Some((640, 360, 1280, 720)),
        )
        .unwrap();
        assert_eq!(touch[0]["touchPoints"][0]["x"], 0.0);
        assert_eq!(touch[0]["touchPoints"][0]["y"], 180.0);
        assert_eq!(touch[0]["touchPoints"][1]["x"], 1280.0);
        assert_eq!(touch[0]["touchPoints"][1]["y"], 720.0);
    }

    #[test]
    fn mock_screen_frame_preserves_selected_tab_url() {
        let tabs = vec![json!({"tab_id":"tab-1","url":"https://example.test"})];
        let frame = mock_screen_frame(1, "tab-1", &tabs);
        let header_len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&frame[4..4 + header_len]).unwrap();
        assert_eq!(header["url"], "https://example.test");
        assert_eq!(
            (
                header["w"].as_u64().unwrap() as u32,
                header["h"].as_u64().unwrap() as u32
            ),
            jpeg_dimensions(&frame[4 + header_len..]).unwrap()
        );
    }

    #[test]
    fn screen_state_url_uses_selected_tab() {
        let state = json!({
            "type":"state",
            "state":{"tabs":[
                {"tab_id":"first","url":"https://first.test"},
                {"tab_id":"active","url":"https://active.test"}
            ]}
        });
        assert_eq!(screen_state_url(&state, "active"), "https://active.test");
        assert_eq!(screen_state_url(&state, "missing"), "about:blank");
    }

    #[test]
    fn screen_state_broadcast_driver_follows_takeover_lifecycle() {
        struct ScreenFake;
        impl macbot_browser::CliRunner for ScreenFake {
            fn run(
                &self,
                _: &std::path::Path,
                _: &[String],
            ) -> Result<String, macbot_browser::BrowserError> {
                Ok("{}".into())
            }
        }

        let mut browser = BrowserManager::new(
            macbot_browser::SessionConfig::default(),
            Arc::new(ScreenFake),
        );
        let tab = browser
            .open_tab("bot", "assignment", "https://example.test")
            .unwrap();
        let driver = |browser: &mut BrowserManager<ScreenFake>| {
            screen_state(browser, "bot", Some("assignment"), Some(&tab.tab_id), false)
                .unwrap()
                .0["state"]["driver"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(driver(&mut browser), "bot");
        browser.takeover_start("bot").unwrap();
        assert_eq!(driver(&mut browser), "user");
        browser.takeover_release("bot").unwrap();
        assert_eq!(driver(&mut browser), "bot");
    }

    #[tokio::test]
    async fn uploads_accept_files_larger_than_the_framework_default_limit() {
        let dir = tempfile::tempdir().unwrap();
        let gw = Gateway::new(GatewayConfig {
            home: dir.path().into(),
            password: Some("dev".into()),
            mock: true,
            ..Default::default()
        });
        let payload = vec![b'x'; 3 * 1024 * 1024];
        let mut body = b"--upload-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"large.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n".to_vec();
        body.extend_from_slice(&payload);
        body.extend_from_slice(b"\r\n--upload-test--\r\n");
        let response = gw
            .router()
            .oneshot(
                http::Request::builder()
                    .uri("/api/v1/uploads")
                    .method("POST")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "multipart/form-data; boundary=upload-test")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["file"]["size"], payload.len());
        let metadata: Value = serde_json::from_slice(
            &std::fs::read(
                dir.path()
                    .join("data/uploads")
                    .join(format!("{}.json", result["upload_id"].as_str().unwrap())),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(metadata, result["file"]);
        assert_eq!(metadata["name"], "large.bin");
        assert_eq!(metadata["mime"], "application/octet-stream");
        assert_eq!(
            std::fs::read(
                dir.path()
                    .join("uploads")
                    .join(result["upload_id"].as_str().unwrap())
            )
            .unwrap(),
            payload
        );
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
