//! Protocol-independent client core for the Mac Bot desktop application.
//!
//! Known request and response discriminators are checked against the shared
//! protocol crate. Frames still remain `serde_json::Value` at the state edge,
//! so newly added fields survive a client/server version skew.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use http::header::{HeaderValue, AUTHORIZATION};
use macbot_protocol as protocol;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

pub type JsonValue = Value;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("websocket: {0}")]
    WebSocket(#[source] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http: {0}")]
    Http(#[source] Box<reqwest::Error>),
    #[error("request timed out")]
    RequestTimeout,
    #[error("event gap exceeded {0} buffered sequence values; reconnect required")]
    EventGapExceeded(usize),
    #[error("connection closed")]
    Closed,
    #[error("server error {code}: {message}")]
    Server { code: String, message: String },
    #[error("protocol: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Maximum number of future sequence values retained while waiting for a gap
/// to close. A larger gap is treated as a resync request rather than allowing
/// unbounded memory growth during a damaged or adversarial stream.
pub const MAX_BUFFERED_EVENTS: usize = 2048;

#[derive(Clone, Debug)]
pub struct ReconnectConfig {
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub jitter: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: 0.20,
        }
    }
}

impl ReconnectConfig {
    pub fn delay(&self, attempt: u32, random: f64) -> Duration {
        let exp = self
            .initial_delay
            .saturating_mul(2u32.saturating_pow(attempt.min(16)));
        let base = exp.min(self.max_delay);
        let factor = (1.0 + (random.clamp(0.0, 1.0) * 2.0 - 1.0) * self.jitter).max(0.0);
        Duration::from_secs_f64(base.as_secs_f64() * factor)
    }
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// A host:port, http(s) URL, or ws(s) URL. `/ws` is added when absent.
    pub endpoint: String,
    pub password: String,
    pub app_version: String,
    pub device_name: String,
    pub device_id: String,
    pub request_timeout: Duration,
    pub reconnect: ReconnectConfig,
    /// Persisted cursor from a previous session. It is only sent when
    /// `has_cached_state` is true; otherwise a fresh bootstrap is required.
    pub last_seq: u64,
    pub has_cached_state: bool,
    /// Optional address fallbacks for hosts that advertise several routes.
    pub addresses: Vec<String>,
    pub node_id: Option<String>,
}

impl ClientConfig {
    pub fn new(endpoint: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            password: password.into(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            device_name: hostname(),
            device_id: Uuid::now_v7().to_string(),
            request_timeout: Duration::from_secs(30),
            reconnect: ReconnectConfig::default(),
            last_seq: 0,
            has_cached_state: false,
            addresses: vec![],
            node_id: None,
        }
    }

    pub fn websocket_url(&self, path: &str) -> Result<String> {
        websocket_url(&self.endpoint, path)
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "Mac Bot".to_string())
}

pub fn websocket_url(endpoint: &str, path: &str) -> Result<String> {
    let raw = endpoint.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(CoreError::InvalidEndpoint(endpoint.to_string()));
    }
    let mut url = if raw.starts_with("ws://") || raw.starts_with("wss://") {
        raw.to_string()
    } else if let Some(rest) = raw.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = raw.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        format!("ws://{raw}")
    };
    replace_transport_path(&mut url, path);
    Ok(url)
}

pub fn http_url(endpoint: &str, path: &str) -> Result<String> {
    let raw = endpoint.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Err(CoreError::InvalidEndpoint(endpoint.to_string()));
    }
    let mut url = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else if let Some(rest) = raw.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if let Some(rest) = raw.strip_prefix("wss://") {
        format!("https://{rest}")
    } else {
        format!("http://{raw}")
    };
    replace_transport_path(&mut url, path);
    Ok(url)
}

fn replace_transport_path(url: &mut String, path: &str) {
    let scheme_end = path_prefix_index(url);
    let existing_path = url[scheme_end..]
        .find('/')
        .map(|offset| scheme_end + offset);
    match existing_path {
        None => url.push_str(path),
        Some(index) if &url[index..] == "/" || &url[index..] == "/ws" => {
            url.truncate(index);
            url.push_str(path);
        }
        Some(_) => {}
    }
}

/// HTTP RPC transport used for large payloads and scripts. It uses the same
/// bearer password as the WebSocket and deliberately returns unknown JSON
/// fields unchanged.
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    rpc_url: reqwest::Url,
    password: String,
}

impl HttpClient {
    pub fn from_config(config: &ClientConfig) -> Result<Self> {
        let rpc_url = http_url(&config.endpoint, "/api/v1/rpc")?
            .parse::<reqwest::Url>()
            .map_err(|error| CoreError::InvalidEndpoint(error.to_string()))?;
        Ok(Self {
            client: reqwest::Client::new(),
            rpc_url,
            password: config.password.clone(),
        })
    }

    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let response = self
            .client
            .post(self.rpc_url.clone())
            .bearer_auth(&self.password)
            .json(&json!({"method": method, "params": params}))
            .send()
            .await
            .map_err(|error| CoreError::Http(Box::new(error)))?;
        let frame = response
            .json::<Value>()
            .await
            .map_err(|error| CoreError::Http(Box::new(error)))?;
        if frame.get("ok").is_some() {
            response_result(&frame)
        } else {
            Ok(frame)
        }
    }
}

fn path_prefix_index(url: &str) -> usize {
    url.find("://").map(|i| i + 3).unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connected {
        hello: Value,
        resumed: bool,
        /// The address that completed the main websocket handshake. This is
        /// useful when `ClientConfig::addresses` contains fallbacks: the UI
        /// must open auxiliary connections against the same reachable host.
        endpoint: String,
    },
    Bootstrap(Value),
    Protocol(ProtocolEvent),
    Disconnected {
        error: Option<String>,
    },
    TransportError(String),
}

#[derive(Debug, Clone)]
pub struct ProtocolEvent {
    pub seq: Option<u64>,
    pub event: String,
    pub data: Value,
}

#[derive(Debug)]
struct RequestCommand {
    method: String,
    params: Value,
    response: oneshot::Sender<Result<Value>>,
}

#[derive(Debug)]
enum Command {
    Request(RequestCommand),
    Close,
}

/// A cheap, cloneable façade suitable for GPUI entities.  The background
/// worker owns the socket and is independent from the UI executor.
#[derive(Clone)]
pub struct Client {
    command_tx: mpsc::Sender<Command>,
}

pub struct ClientHandle {
    pub client: Client,
    pub events: mpsc::Receiver<ClientEvent>,
}

impl ClientHandle {
    pub fn spawn(config: ClientConfig) -> Self {
        Client::spawn(config)
    }

    pub async fn request(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        self.client.request(method, params).await
    }

    pub fn try_request(
        &self,
        method: impl Into<String>,
        params: Value,
    ) -> oneshot::Receiver<Result<Value>> {
        self.client.try_request(method, params)
    }

    pub async fn close(&self) {
        self.client.close().await;
    }
}

impl Client {
    /// Starts the worker and returns immediately; connection progress is
    /// delivered through `events`. The caller does not need to own a Tokio
    /// runtime after this function returns, as the desktop shell provides one.
    pub fn spawn(config: ClientConfig) -> ClientHandle {
        let (command_tx, command_rx) = mpsc::channel(128);
        let (event_tx, events) = mpsc::channel(256);
        let client = Self { command_tx };
        tokio::spawn(run_worker(config, command_rx, event_tx));
        ClientHandle { client, events }
    }

    pub async fn connect(config: ClientConfig) -> ClientHandle {
        Self::spawn(config)
    }

    pub async fn request(&self, method: impl Into<String>, params: Value) -> Result<Value> {
        self.try_request(method, params)
            .await
            .map_err(|_| CoreError::Closed)?
    }

    pub fn try_request(
        &self,
        method: impl Into<String>,
        mut params: Value,
    ) -> oneshot::Receiver<Result<Value>> {
        let method = method.into();
        if is_write_method(&method) {
            add_idempotency_key(&mut params);
        }
        let (response, receiver) = oneshot::channel();
        if let Err(error) = validate_rpc_params(&method, &params) {
            let _ = response.send(Err(error));
            return receiver;
        }
        let command = Command::Request(RequestCommand {
            method,
            params,
            response,
        });
        let sender = self.command_tx.clone();
        tokio::spawn(async move {
            let _ = sender.send(command).await;
        });
        receiver
    }

    pub async fn close(&self) {
        let _ = self.command_tx.send(Command::Close).await;
    }
}

fn is_write_method(method: &str) -> bool {
    matches!(
        method,
        "chat.send"
            | "chat.mark_read"
            | "chat.react"
            | "chat.set_pinned"
            | "chat.set_muted"
            | "bot.create"
            | "bot.update"
            | "bot.duplicate"
            | "bot.delete"
            | "bot.create_from_template"
            | "project.create"
            | "project.update"
            | "project.add_member"
            | "project.remove_member"
            | "project.confirm_done"
            | "project.request_changes"
            | "project.archive"
            | "project.reopen"
            | "assignment.stop"
            | "approval.decide"
            | "question.answer"
            | "loop.resolve"
            | "takeover.start"
            | "takeover.release"
            | "skill.create"
            | "skill.update"
            | "skill.delete"
            | "skill.set_enabled"
            | "skill.publish"
            | "skill.import"
            | "routine.create"
            | "routine.update"
            | "routine.delete"
            | "routine.set_enabled"
            | "routine.test_run"
            | "provider.create"
            | "provider.update"
            | "provider.delete"
            | "provider.test"
            | "model.upsert"
            | "model.delete"
            | "settings.update"
            | "device.register"
    )
}

fn add_idempotency_key(params: &mut Value) {
    if let Value::Object(map) = params {
        map.entry("client_request_id")
            .or_insert_with(|| Value::String(Uuid::now_v7().to_string()));
    }
}

async fn run_worker(
    config: ClientConfig,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<ClientEvent>,
) {
    let mut last_seq = config.last_seq;
    let mut pending: VecDeque<RequestCommand> = VecDeque::new();
    let mut attempt = 0u32;

    loop {
        while let Ok(command) = commands.try_recv() {
            if matches!(command, Command::Close) {
                return;
            }
            if let Command::Request(request) = command {
                pending.push_back(request);
            }
        }

        let mut endpoints = config.addresses.clone();
        if endpoints.is_empty() {
            endpoints.push(config.endpoint.clone());
        }
        let connected = {
            let mut last_error = None;
            let mut connected = None;
            for endpoint in endpoints {
                match connect_main(&config, &endpoint).await {
                    Ok((socket, hello)) => {
                        connected = Some((socket, hello, endpoint));
                        break;
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            connected.ok_or_else(|| last_error.unwrap_or(CoreError::Closed))
        };
        let (mut socket, hello, endpoint) = match connected {
            Ok(value) => {
                attempt = 0;
                value
            }
            Err(error) => {
                let _ = events
                    .send(ClientEvent::Disconnected {
                        error: Some(error.to_string()),
                    })
                    .await;
                if !wait_for_reconnect(&config, &mut commands, &mut pending, &mut attempt).await {
                    return;
                }
                continue;
            }
        };

        let resume_last_seq = if config.has_cached_state { last_seq } else { 0 };
        let resume_params = json!({
            "last_seq": resume_last_seq,
            "client": {"platform":"macos", "app_version":config.app_version, "device_name":config.device_name, "device_id":config.device_id}
        });
        let resume_id = Uuid::now_v7().to_string();
        let _ = socket
            .send(Message::Text(
                request_frame(&resume_id, "session.resume", resume_params).to_string(),
            ))
            .await;
        let (resume_result, resume_sync_seq) = match receive_response_with_meta(
            &mut socket,
            &resume_id,
            config.request_timeout,
            &events,
            &mut last_seq,
            true,
        )
        .await
        {
            Ok(receipt) => (Ok(receipt.value), receipt.sync_seq),
            Err(error) => (Err(error), None),
        };
        if resume_result.is_err() {
            let _ = events
                .send(ClientEvent::Disconnected {
                    error: resume_result.err().map(|e| e.to_string()),
                })
                .await;
            if !wait_for_reconnect(&config, &mut commands, &mut pending, &mut attempt).await {
                return;
            }
            continue;
        }

        // A server restart or state reset can leave the persisted client
        // cursor ahead of the server's current event log.  Such a response
        // must not be treated as a successful replay: doing so would keep
        // stale cached state and suppress the bootstrap that repairs it.
        let server_seq = hello.get("last_seq").and_then(Value::as_u64);
        let rolled_back = config.has_cached_state
            && (server_seq.is_some_and(|seq| seq < resume_last_seq)
                || resume_sync_seq.is_some_and(|seq| seq < resume_last_seq));
        if rolled_back {
            last_seq = 0;
        }
        let resumed = !rolled_back
            && resume_result
                .as_ref()
                .ok()
                .and_then(|result| result.get("mode"))
                .and_then(Value::as_str)
                == Some("replay");

        let _ = events
            .send(ClientEvent::Connected {
                hello,
                resumed,
                endpoint,
            })
            .await;
        if !resumed {
            let bootstrap_id = Uuid::now_v7().to_string();
            if let Err(error) = socket
                .send(Message::Text(
                    request_frame(&bootstrap_id, "bootstrap", json!({})).to_string(),
                ))
                .await
            {
                let _ = events
                    .send(ClientEvent::Disconnected {
                        error: Some(error.to_string()),
                    })
                    .await;
                if !wait_for_reconnect(&config, &mut commands, &mut pending, &mut attempt).await {
                    return;
                }
                continue;
            }
            match receive_response(
                &mut socket,
                &bootstrap_id,
                config.request_timeout,
                &events,
                &mut last_seq,
                false,
            )
            .await
            {
                Ok(bootstrap) => {
                    last_seq = bootstrap
                        .get("seq")
                        .and_then(Value::as_u64)
                        .unwrap_or(last_seq);
                    let _ = events.send(ClientEvent::Bootstrap(bootstrap)).await;
                }
                Err(error) => {
                    let _ = events
                        .send(ClientEvent::Disconnected {
                            error: Some(error.to_string()),
                        })
                        .await;
                    if !wait_for_reconnect(&config, &mut commands, &mut pending, &mut attempt).await
                    {
                        return;
                    }
                    continue;
                }
            }
        }

        match run_connected(
            config.request_timeout,
            &mut socket,
            &mut commands,
            &events,
            &mut pending,
            &mut last_seq,
        )
        .await
        {
            Ok(ConnectedExit::Closed) => return,
            Err(error) => {
                let _ = events
                    .send(ClientEvent::Disconnected {
                        error: Some(error.to_string()),
                    })
                    .await;
            }
        }
        let _ = events.send(ClientEvent::Disconnected { error: None }).await;
        if !wait_for_reconnect(&config, &mut commands, &mut pending, &mut attempt).await {
            return;
        }
    }
}

async fn wait_for_reconnect(
    config: &ClientConfig,
    commands: &mut mpsc::Receiver<Command>,
    pending: &mut VecDeque<RequestCommand>,
    attempt: &mut u32,
) -> bool {
    let keep_running = tokio::select! {
        command = commands.recv() => match command {
            Some(Command::Request(request)) => {
                pending.push_back(request);
                true
            }
            Some(Command::Close) | None => false,
        },
        _ = sleep(config.reconnect.delay(*attempt, rand::thread_rng().gen())) => true,
    };
    if keep_running {
        *attempt = (*attempt).saturating_add(1);
    }
    keep_running
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_main(config: &ClientConfig, endpoint: &str) -> Result<(Socket, Value)> {
    let url = websocket_url(endpoint, "/ws")?;
    let parsed = url
        .parse::<http::Uri>()
        .map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?;
    let mut request = parsed
        .into_client_request()
        .map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?;
    let auth = format!("Bearer {}", config.password);
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&auth).map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?,
    );
    let (mut socket, _) = connect_async(request)
        .await
        .map_err(|error| CoreError::WebSocket(Box::new(error)))?;
    let hello = loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let frame: Value = serde_json::from_str(&text)?;
                if frame.get("kind").and_then(Value::as_str) == Some("evt")
                    && frame.get("event").and_then(Value::as_str) == Some("hello")
                {
                    break frame.get("data").cloned().unwrap_or(Value::Null);
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| CoreError::WebSocket(Box::new(error)))?;
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(CoreError::WebSocket(Box::new(error))),
            None => return Err(CoreError::Closed),
        }
    };
    if let Some(expected) = config.node_id.as_deref() {
        if hello.get("node_id").and_then(Value::as_str) != Some(expected) {
            return Err(CoreError::InvalidEndpoint(
                "host node_id does not match saved Host".into(),
            ));
        }
    }
    Ok((socket, hello))
}

fn request_frame(id: &str, method: &str, params: Value) -> Value {
    // Decode known methods to exercise required-field validation, but always
    // put the original JSON on the wire. Re-serializing MethodParams would
    // silently drop client extensions and retry metadata such as
    // `client_request_id`; unknown future methods must remain sendable too.
    let _ = validate_rpc_params(method, &params);
    json!({"v":1,"kind":"req","id":id,"method":method,"params":params})
}

/// Validate required fields for methods known by the shared protocol while
/// leaving the original JSON untouched. Unknown method names remain valid so
/// a newer server can be used before this client learns its typed contract.
pub fn validate_rpc_params(method: &str, params: &Value) -> Result<()> {
    let Ok(method_type) =
        serde_json::from_value::<protocol::Method>(Value::String(method.to_owned()))
    else {
        return Ok(());
    };
    protocol::MethodParams::decode(&method_type, params.clone())
        .map(|_| ())
        .map_err(CoreError::Protocol)
}

/// Validate a result with the method-specific protocol type while returning
/// the original JSON value. Unknown response fields therefore remain
/// available to the state layer even when the typed contract is older.
pub fn decode_rpc_result(method: &str, result: Value) -> Result<Value> {
    let Ok(method_type) =
        serde_json::from_value::<protocol::Method>(Value::String(method.to_owned()))
    else {
        // A newer server may add an RPC before this client knows its typed
        // contract. Keep the raw result available to the UI in that case.
        return Ok(result);
    };
    match protocol::MethodResult::decode(&method_type, result.clone()) {
        Ok(_) => Ok(result),
        Err(error) => {
            // The shared protocol enums are intentionally closed. Validate a
            // compatibility copy with only the two extensible collections
            // relaxed, while returning the untouched wire JSON below.
            let mut compatible = result.clone();
            let changed = sanitize_compat_result(method, &mut compatible);
            if !changed {
                return Err(CoreError::Protocol(error));
            }
            protocol::MethodResult::decode(&method_type, compatible)
                .map_err(CoreError::Protocol)?;
            Ok(result)
        }
    }
}

const KNOWN_BLOCK_TYPES: &[&str] = &[
    "text",
    "image",
    "file",
    "task_card",
    "completion",
    "progress",
    "blocked",
    "question",
    "project_card",
    "review_card",
    "delegation",
    "approval",
    "approval_ref",
    "takeover_request",
    "bot_dm_ref",
    "system",
    "loop_paused",
];

const KNOWN_TRACE_TYPES: &[&str] = &[
    "run.start",
    "llm.request",
    "llm.response",
    "tool.start",
    "tool.end",
    "send_msg",
    "steer",
    "run.wait",
    "run.resume",
    "compaction",
    "run.end",
];

fn sanitize_compat_result(method: &str, value: &mut Value) -> bool {
    let mut changed = false;
    if matches!(
        method,
        "bootstrap" | "chat.history" | "chat.thread" | "chat.send" | "project.request_changes"
    ) {
        changed |= sanitize_unknown_message_blocks(value);
    }
    if method == "trace.history" {
        changed |= sanitize_unknown_trace_items(value);
    }
    changed
}

fn sanitize_unknown_message_blocks(value: &mut Value) -> bool {
    match value {
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= sanitize_unknown_message_blocks(value);
            }
            changed
        }
        Value::Object(object) => {
            let fallback = object
                .get("fallback_text")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let mut changed = false;
            if let Some(Value::Array(blocks)) = object.get_mut("blocks") {
                if let Some(fallback) = fallback.as_deref() {
                    for block in blocks {
                        let Some(block_type) = block.get("type").and_then(Value::as_str) else {
                            continue;
                        };
                        if !KNOWN_BLOCK_TYPES.contains(&block_type) {
                            *block = json!({"type": "text", "markdown": fallback});
                            changed = true;
                        }
                    }
                }
            }
            for child in object.values_mut() {
                changed |= sanitize_unknown_message_blocks(child);
            }
            changed
        }
        _ => false,
    }
}

fn sanitize_unknown_trace_items(value: &mut Value) -> bool {
    let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) else {
        return false;
    };
    let original_len = items.len();
    items.retain(|item| {
        item.get("type")
            .and_then(Value::as_str)
            .is_none_or(|item_type| KNOWN_TRACE_TYPES.contains(&item_type))
    });
    items.len() != original_len
}

async fn receive_response(
    socket: &mut Socket,
    id: &str,
    wait: Duration,
    events: &mpsc::Sender<ClientEvent>,
    last_seq: &mut u64,
    wait_sync: bool,
) -> Result<Value> {
    receive_response_with_meta(socket, id, wait, events, last_seq, wait_sync)
        .await
        .map(|receipt| receipt.value)
}

struct ResponseReceipt {
    value: Value,
    sync_seq: Option<u64>,
}

async fn receive_response_with_meta(
    socket: &mut Socket,
    id: &str,
    wait: Duration,
    events: &mpsc::Sender<ClientEvent>,
    last_seq: &mut u64,
    wait_sync: bool,
) -> Result<ResponseReceipt> {
    timeout(wait, async {
        let mut response = None;
        let mut sync_seen = false;
        let mut sync_seq = None;
        let mut out_of_order = BTreeSet::new();
        while let Some(message) = socket.next().await {
            match message.map_err(|error| CoreError::WebSocket(Box::new(error)))? {
                Message::Text(text) => {
                    let frame: Value = serde_json::from_str(&text)?;
                    if frame.get("kind").and_then(Value::as_str) == Some("res")
                        && frame.get("id").and_then(Value::as_str) == Some(id)
                    {
                        let result = response_result(&frame)?;
                        let replay = result.get("mode").and_then(Value::as_str) == Some("replay");
                        if !wait_sync || !replay {
                            return Ok(ResponseReceipt {
                                value: result,
                                sync_seq,
                            });
                        }
                        if sync_seen {
                            return Ok(ResponseReceipt {
                                value: result,
                                sync_seq,
                            });
                        }
                        response = Some(result);
                    }
                    if frame.get("kind").and_then(Value::as_str) == Some("evt") {
                        let seq = frame.get("seq").and_then(Value::as_u64);
                        if let Some(seq) = seq {
                            advance_contiguous(last_seq, seq, &mut out_of_order)?;
                        }
                        if let Some(event) = frame.get("event").and_then(Value::as_str) {
                            let mut completed = None;
                            if event == "sync.done" {
                                sync_seen = true;
                                if let Some(seq) = frame
                                    .get("data")
                                    .and_then(|data| data.get("seq"))
                                    .and_then(Value::as_u64)
                                {
                                    sync_seq = Some(seq);
                                    advance_contiguous(last_seq, seq, &mut out_of_order)?;
                                }
                                completed = response.take();
                            }
                            let _ = events
                                .send(ClientEvent::Protocol(ProtocolEvent {
                                    seq,
                                    event: event.into(),
                                    data: frame.get("data").cloned().unwrap_or(Value::Null),
                                }))
                                .await;
                            if let Some(result) = completed {
                                return Ok(ResponseReceipt {
                                    value: result,
                                    sync_seq,
                                });
                            }
                        }
                    }
                }
                Message::Ping(payload) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|error| CoreError::WebSocket(Box::new(error)))?,
                Message::Close(_) => return Err(CoreError::Closed),
                _ => {}
            }
        }
        if let Some(result) = response {
            return Ok(ResponseReceipt {
                value: result,
                sync_seq,
            });
        }
        Err(CoreError::Closed)
    })
    .await
    .map_err(|_| CoreError::RequestTimeout)?
}

fn response_result(frame: &Value) -> Result<Value> {
    if frame.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
    }
    let error = frame.get("error").cloned().unwrap_or_default();
    Err(CoreError::Server {
        code: error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("internal")
            .to_string(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("request failed")
            .to_string(),
    })
}

/// Advance only through a contiguous event prefix. Persisting the largest
/// observed sequence would make a reconnect skip an earlier event when a
/// frame arrives out of order or a frame is lost before the socket closes.
fn advance_contiguous(cursor: &mut u64, seq: u64, out_of_order: &mut BTreeSet<u64>) -> Result<()> {
    if seq <= *cursor {
        return Ok(());
    }
    out_of_order.insert(seq);
    while out_of_order.remove(&cursor.saturating_add(1)) {
        *cursor = cursor.saturating_add(1);
    }
    if out_of_order.len() > MAX_BUFFERED_EVENTS {
        out_of_order.clear();
        return Err(CoreError::EventGapExceeded(MAX_BUFFERED_EVENTS));
    }
    Ok(())
}

#[derive(Debug)]
enum ConnectedExit {
    Closed,
}

async fn run_connected(
    request_timeout: Duration,
    socket: &mut Socket,
    commands: &mut mpsc::Receiver<Command>,
    events: &mpsc::Sender<ClientEvent>,
    pending: &mut VecDeque<RequestCommand>,
    last_seq: &mut u64,
) -> Result<ConnectedExit> {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut in_flight: Option<(String, RequestCommand, Instant)> = None;
    let mut out_of_order = BTreeSet::new();
    loop {
        if in_flight.is_none() {
            if let Some(request) = pending.pop_front() {
                let id = Uuid::now_v7().to_string();
                let frame = request_frame(&id, &request.method, request.params.clone());
                match socket.send(Message::Text(frame.to_string())).await {
                    Ok(()) => in_flight = Some((id, request, Instant::now())),
                    Err(error) => {
                        pending.push_front(request);
                        return Err(CoreError::WebSocket(Box::new(error)));
                    }
                }
            }
        }
        tokio::select! {
            _ = heartbeat.tick() => if let Err(error) = socket.send(Message::Ping(Vec::new())).await {
                requeue(&mut in_flight, pending);
                return Err(CoreError::WebSocket(Box::new(error)));
            },
            timed_out = async {
                if let Some((_, _, started)) = in_flight.as_ref() {
                    sleep(request_timeout.saturating_sub(started.elapsed())).await;
                    true
                } else {
                    std::future::pending::<bool>().await
                }
            } => if timed_out {
                if let Some((_, request, _)) = in_flight.take() {
                    let _ = request.response.send(Err(CoreError::RequestTimeout));
                }
            },
            command = commands.recv() => match command {
                Some(Command::Request(request)) => pending.push_back(request),
                Some(Command::Close) | None => return Ok(ConnectedExit::Closed),
            },
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let frame: Value = serde_json::from_str(&text)?;
                    match frame.get("kind").and_then(Value::as_str) {
                        Some("res") => {
                            let response_id = frame.get("id").and_then(Value::as_str);
                            if response_id
                                == in_flight
                                    .as_ref()
                                    .map(|(id, _, _)| id.as_str())
                            {
                                if let Some((_, request, _)) = in_flight.take() {
                                    let result = response_result(&frame);
                                    if request.method == "bootstrap" {
                                        if let Ok(bootstrap) = &result {
                                            reset_cursor_from_bootstrap(
                                                last_seq,
                                                &mut out_of_order,
                                                bootstrap,
                                            );
                                        }
                                    }
                                    let _ = request.response.send(result);
                                }
                            }
                        }
                        Some("evt") => {
                            let seq = frame.get("seq").and_then(Value::as_u64);
                            if let Some(value) = seq {
                                advance_contiguous(last_seq, value, &mut out_of_order)?;
                            }
                            if let Some(event) = frame.get("event").and_then(Value::as_str) {
                                if event == "sync.done" {
                                    if let Some(seq) = frame.get("data").and_then(|data| data.get("seq")).and_then(Value::as_u64) {
                                        advance_contiguous(last_seq, seq, &mut out_of_order)?;
                                    }
                                }
                                let _ = events.send(ClientEvent::Protocol(ProtocolEvent { seq, event: event.into(), data: frame.get("data").cloned().unwrap_or(Value::Null) })).await;
                            }
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Ping(payload))) => if let Err(error) = socket.send(Message::Pong(payload)).await {
                    requeue(&mut in_flight, pending);
                    return Err(CoreError::WebSocket(Box::new(error)));
                },
                Some(Ok(Message::Close(_))) | None => { requeue(&mut in_flight, pending); return Err(CoreError::Closed); },
                Some(Ok(_)) => {}
                Some(Err(error)) => { requeue(&mut in_flight, pending); return Err(CoreError::WebSocket(Box::new(error))); },
            }
        }
    }
}

fn requeue(
    in_flight: &mut Option<(String, RequestCommand, Instant)>,
    pending: &mut VecDeque<RequestCommand>,
) {
    if let Some((_, request, _)) = in_flight.take() {
        pending.push_front(request);
    }
}

fn reset_cursor_from_bootstrap(
    last_seq: &mut u64,
    out_of_order: &mut BTreeSet<u64>,
    bootstrap: &Value,
) {
    if let Some(seq) = bootstrap.get("seq").and_then(Value::as_u64) {
        *last_seq = seq;
    }
    out_of_order.clear();
}

/// State held by the UI. Unknown objects and fields survive round trips.
#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub last_seq: u64,
    pub hello: Option<Value>,
    pub bots: BTreeMap<String, Value>,
    pub chats: BTreeMap<String, Value>,
    pub projects: BTreeMap<String, Value>,
    pub messages: BTreeMap<String, Value>,
    pub assignments: BTreeMap<String, Value>,
    pub announcements: BTreeMap<String, Value>,
    pub approvals: BTreeMap<String, Value>,
    pub questions: BTreeMap<String, Value>,
    pub skills: BTreeMap<String, Value>,
    pub routines: BTreeMap<String, Value>,
    pub providers: BTreeMap<String, Value>,
    pub models: BTreeMap<String, Value>,
    pub message_deltas: BTreeMap<String, String>,
    pub typing: BTreeMap<String, bool>,
    pub bot_status: BTreeMap<String, Value>,
    pub settings: Option<Value>,
    pub pending: Option<Value>,
    /// Set when the ordered event gap exceeded the bounded buffer. The UI
    /// should request a fresh `bootstrap` before presenting later events.
    pub needs_resync: bool,
    buffered_events: BTreeMap<u64, ProtocolEvent>,
}

impl AppState {
    /// Serializes the in-memory state for a local cache. Secrets are never
    /// part of this value; it contains only the protocol bootstrap objects.
    pub fn to_bootstrap_cache(&self) -> Value {
        let values = |map: &BTreeMap<String, Value>| Value::Array(map.values().cloned().collect());
        json!({
            "seq": self.last_seq,
            "hello": self.hello,
            "bots": values(&self.bots),
            "chats": values(&self.chats),
            "projects": values(&self.projects),
            "messages": values(&self.messages),
            "assignments": values(&self.assignments),
            "announcements": values(&self.announcements),
            "approvals": values(&self.approvals),
            "questions": values(&self.questions),
            "skills": values(&self.skills),
            "routines": values(&self.routines),
            "providers": values(&self.providers),
            "models": values(&self.models),
            "settings": self.settings,
            "pending": self.pending,
            "needs_resync": self.needs_resync,
        })
    }

    pub fn from_bootstrap_cache(value: Value) -> Self {
        let needs_resync = value
            .get("needs_resync")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut state = Self::default();
        state.apply_bootstrap(value);
        state.needs_resync = needs_resync;
        state
    }

    pub fn apply_bootstrap(&mut self, value: Value) {
        self.last_seq = value.get("seq").and_then(Value::as_u64).unwrap_or(0);
        self.hello = value.get("hello").cloned();
        self.settings = value.get("settings").cloned();
        self.pending = value.get("pending").cloned();
        self.bots = indexed(value.get("bots"));
        self.chats = indexed(value.get("chats"));
        self.projects = indexed(value.get("projects"));
        self.messages = indexed(value.get("messages"));
        self.assignments = indexed(value.get("assignments"));
        self.announcements = indexed_by(value.get("announcements"), "project_id");
        self.approvals = indexed(value.get("approvals"));
        self.questions = indexed(value.get("questions"));
        self.skills = indexed_by(value.get("skills"), "name");
        self.routines = indexed(value.get("routines"));
        self.providers = indexed(value.get("providers"));
        self.models = indexed_by(value.get("models"), "ref");
        if let Some(pending) = self.pending.as_ref() {
            self.approvals = indexed(pending.get("approvals"));
            self.questions = indexed(pending.get("questions"));
        }
        self.buffered_events.clear();
        self.needs_resync = false;
    }

    pub fn apply_event(&mut self, event: ProtocolEvent) {
        if self.needs_resync {
            return;
        }
        let Some(seq) = event.seq else {
            self.apply_event_now(&event);
            return;
        };
        if seq <= self.last_seq {
            return;
        }
        self.buffered_events.insert(seq, event);
        while let Some(next) = self.buffered_events.remove(&(self.last_seq + 1)) {
            self.apply_event_now(&next);
            self.last_seq += 1;
        }
        if self.buffered_events.len() > MAX_BUFFERED_EVENTS {
            self.buffered_events.clear();
            self.needs_resync = true;
        }
    }

    fn apply_event_now(&mut self, event: &ProtocolEvent) {
        let data = &event.data;
        if event.event == "sync.done" {
            // A resume can finish while an earlier durable event is still
            // missing from the stream. Keep the existing ordered-event
            // contract, but recover from a bounded single-event hole instead
            // of leaving all later events buffered indefinitely.
            let server_seq = data.get("seq").and_then(Value::as_u64);
            if server_seq.is_some_and(|seq| seq > self.last_seq) && !self.buffered_events.is_empty()
            {
                self.buffered_events.clear();
                self.needs_resync = true;
            }
            return;
        }
        let base = match event.event.as_str() {
            // Question lifecycle events carry the same canonical question
            // object as created/updated events. Keep the client cache in
            // sync so answered cards stop presenting pending controls.
            "question.asked" | "question.answered" => Some("question"),
            _ => event
                .event
                .strip_suffix(".created")
                .or_else(|| event.event.strip_suffix(".updated")),
        };
        if let Some(kind) = base {
            if let Some(object) = first_object(data) {
                let key = match kind {
                    // Bootstrap indexes announcements by project_id, so
                    // updates must use the same stable key even when the
                    // server also includes an announcement record id.
                    "announcement" => object.get("project_id").or_else(|| object.get("id")),
                    "skill" => object.get("name").or_else(|| object.get("id")),
                    "model" => object.get("ref").or_else(|| object.get("id")),
                    _ => object
                        .get("id")
                        .or_else(|| object.get("name"))
                        .or_else(|| object.get("ref"))
                        .or_else(|| object.get("project_id")),
                };
                if let Some(id) = key.and_then(Value::as_str) {
                    match kind {
                        "bot" => {
                            merge(&mut self.bots, id, object.clone());
                        }
                        "chat" => {
                            merge(&mut self.chats, id, object.clone());
                        }
                        "project" => {
                            merge(&mut self.projects, id, object.clone());
                        }
                        "message" => {
                            merge(&mut self.messages, id, object.clone());
                        }
                        "assignment" => {
                            merge(&mut self.assignments, id, object.clone());
                        }
                        "announcement" => {
                            merge(&mut self.announcements, id, object.clone());
                        }
                        "approval" => {
                            merge(&mut self.approvals, id, object.clone());
                        }
                        "question" => {
                            merge(&mut self.questions, id, object.clone());
                        }
                        "skill" => {
                            merge(&mut self.skills, id, object.clone());
                        }
                        "routine" => {
                            merge(&mut self.routines, id, object.clone());
                        }
                        "model" => {
                            merge(&mut self.models, id, object.clone());
                        }
                        "provider" => {
                            merge(&mut self.providers, id, object.clone());
                        }
                        "settings" => self.settings = Some(object.clone()),
                        _ => {}
                    }
                }
            }
        }
        if event.event.ends_with(".deleted") {
            let kind = event.event.trim_end_matches(".deleted");
            let id = match kind {
                "announcement" => data.get("project_id").or_else(|| data.get("id")),
                "model" => data.get("ref").or_else(|| data.get("id")),
                "skill" => data.get("name").or_else(|| data.get("id")),
                _ => data
                    .get("id")
                    .or_else(|| data.get("bot_id"))
                    .or_else(|| data.get("chat_id"))
                    .or_else(|| data.get("project_id")),
            };
            if let Some(id) = id.and_then(Value::as_str) {
                match kind {
                    "bot" => {
                        self.bots.remove(id);
                    }
                    "chat" => {
                        self.chats.remove(id);
                    }
                    "project" => {
                        self.projects.remove(id);
                    }
                    "message" => {
                        self.messages.remove(id);
                    }
                    "assignment" => {
                        self.assignments.remove(id);
                    }
                    "approval" => {
                        self.approvals.remove(id);
                    }
                    "question" => {
                        self.questions.remove(id);
                    }
                    "skill" => {
                        self.skills.remove(id);
                    }
                    "routine" => {
                        self.routines.remove(id);
                    }
                    "provider" => {
                        self.providers.remove(id);
                    }
                    "announcement" => {
                        self.announcements.remove(id);
                    }
                    "model" => {
                        self.models.remove(id);
                    }
                    _ => {}
                }
            }
        }
        match event.event.as_str() {
            "message.updated" => {
                if let Some(message) = data.get("message") {
                    if message.get("streaming").and_then(Value::as_bool) == Some(false) {
                        if let Some(id) = message.get("id").and_then(Value::as_str) {
                            self.message_deltas.remove(id);
                        }
                    }
                }
            }
            "read.updated" => {
                if let (Some(chat_id), Some(seq)) = (
                    data.get("chat_id").and_then(Value::as_str),
                    data.get("last_read_seq"),
                ) {
                    if let Some(Value::Object(map)) = self.chats.get_mut(chat_id) {
                        map.insert("last_read_seq".into(), seq.clone());
                    }
                }
            }
            "message.delta" => {
                if let (Some(id), Some(text)) = (
                    data.get("message_id").and_then(Value::as_str),
                    data.get("text").and_then(Value::as_str),
                ) {
                    self.message_deltas
                        .entry(id.into())
                        .or_default()
                        .push_str(text);
                }
            }
            "typing" => {
                if let (Some(chat), Some(bot), Some(on)) = (
                    data.get("chat_id").and_then(Value::as_str),
                    data.get("bot_id").and_then(Value::as_str),
                    data.get("on").and_then(Value::as_bool),
                ) {
                    self.typing.insert(format!("{chat}:{bot}"), on);
                }
            }
            "bot.status" => {
                if let (Some(id), Some(status)) = (
                    data.get("bot_id").and_then(Value::as_str),
                    data.get("status"),
                ) {
                    self.bot_status.insert(id.into(), status.clone());
                }
            }
            "settings.updated" => {
                if let Some(settings) = data.get("settings") {
                    self.settings = Some(settings.clone());
                }
            }
            "provider.updated" => {
                if let Some(provider) = data.get("provider") {
                    if let Some(id) = provider.get("id").and_then(Value::as_str) {
                        merge(&mut self.providers, id, provider.clone());
                    }
                }
                if let Some(models) = data.get("models").and_then(Value::as_array) {
                    for model in models {
                        if let Some(id) = model.get("ref").and_then(Value::as_str) {
                            merge(&mut self.models, id, model.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn indexed(value: Option<&Value>) -> BTreeMap<String, Value> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .map(|id| (id.to_string(), item.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn indexed_by(value: Option<&Value>, key: &str) -> BTreeMap<String, Value> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get(key)
                        .and_then(Value::as_str)
                        .map(|id| (id.to_string(), item.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn first_object(value: &Value) -> Option<&Value> {
    [
        "bot",
        "chat",
        "project",
        "message",
        "assignment",
        "announcement",
        "approval",
        "question",
        "skill",
        "routine",
        "provider",
        "model",
        "settings",
    ]
    .iter()
    .find_map(|key| value.get(*key))
}

fn merge(map: &mut BTreeMap<String, Value>, id: &str, value: Value) {
    if let Some(previous) = map.get_mut(id) {
        merge_values(previous, value);
    } else {
        map.insert(id.to_string(), value);
    }
}

fn merge_values(into: &mut Value, update: Value) {
    match (into, update) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                target.insert(key, value);
            }
        }
        (slot, update) => *slot = update,
    }
}

#[derive(Clone, Debug, Default)]
pub struct TraceTimeline {
    pub items: BTreeMap<u64, Value>,
    pub first_aseq: Option<u64>,
    pub last_aseq: Option<u64>,
    pub has_more_before: bool,
    pub live: bool,
    fragments: BTreeMap<String, TraceFragment>,
}

#[derive(Clone, Debug, Default)]
struct TraceFragment {
    text: String,
    thinking: String,
    completed_text: Option<String>,
    completed_thinking: Option<String>,
}

impl TraceTimeline {
    pub fn apply_history(&mut self, result: &Value) {
        if let Some(items) = result.get("items").and_then(Value::as_array) {
            for item in items {
                self.apply_item(item.clone());
            }
        }
        if let Some(first) = result.get("first_aseq").and_then(Value::as_u64) {
            self.first_aseq = Some(self.first_aseq.map_or(first, |current| current.min(first)));
        }
        if let Some(last) = result.get("last_aseq").and_then(Value::as_u64) {
            self.last_aseq = Some(self.last_aseq.map_or(last, |current| current.max(last)));
        }
        self.has_more_before = result
            .get("has_more_before")
            .and_then(Value::as_bool)
            .unwrap_or(self.has_more_before);
        self.live = result
            .get("live")
            .and_then(Value::as_bool)
            .unwrap_or(self.live);
    }

    pub fn apply_item(&mut self, item: Value) {
        let aseq = item.get("aseq").and_then(Value::as_u64);
        if let Some(aseq) = aseq {
            self.items.insert(aseq, item.clone());
            self.first_aseq = Some(self.first_aseq.map_or(aseq, |old| old.min(aseq)));
            self.last_aseq = Some(self.last_aseq.map_or(aseq, |old| old.max(aseq)));
        }
        if item.get("type").and_then(Value::as_str) == Some("llm.response") {
            if let Some(data) = item.get("data") {
                if let Some(request_id) = data.get("request_id").and_then(Value::as_str) {
                    let fragment = self.fragments.entry(request_id.to_string()).or_default();
                    fragment.completed_text =
                        data.get("text").and_then(Value::as_str).map(str::to_string);
                    fragment.completed_thinking = data
                        .get("thinking")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
        }
    }

    pub fn apply_delta(&mut self, request_id: &str, channel: &str, text: &str) {
        let fragment = self.fragments.entry(request_id.to_string()).or_default();
        match channel {
            "thinking" => fragment.thinking.push_str(text),
            _ => fragment.text.push_str(text),
        }
    }

    pub fn rendered_request(&self, request_id: &str) -> Option<(&str, &str)> {
        let fragment = self.fragments.get(request_id)?;
        Some((
            fragment.completed_text.as_deref().unwrap_or(&fragment.text),
            fragment
                .completed_thinking
                .as_deref()
                .unwrap_or(&fragment.thinking),
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ScreenFrameHeader {
    pub seq: u64,
    pub tab_id: String,
    pub w: u32,
    pub h: u32,
    pub ts: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenFrame {
    pub header: ScreenFrameHeader,
    pub jpeg: Vec<u8>,
}

pub fn parse_screen_frame(bytes: &[u8]) -> Result<ScreenFrame> {
    if bytes.len() < 4 {
        return Err(CoreError::InvalidEndpoint(
            "screen frame is missing header length".into(),
        ));
    }
    let header_len = u32::from_be_bytes(bytes[..4].try_into().expect("four bytes")) as usize;
    let end = 4usize
        .checked_add(header_len)
        .ok_or_else(|| CoreError::InvalidEndpoint("screen header overflow".into()))?;
    if end > bytes.len() {
        return Err(CoreError::InvalidEndpoint(
            "screen frame header is truncated".into(),
        ));
    }
    let header = serde_json::from_slice::<ScreenFrameHeader>(&bytes[4..end])?;
    Ok(ScreenFrame {
        header,
        jpeg: bytes[end..].to_vec(),
    })
}

pub fn screen_ack(seq: u64) -> Value {
    json!({"type":"ack", "seq":seq})
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ScreenCommand {
    #[serde(rename = "ack")]
    Ack { seq: u64 },
    #[serde(rename = "switch_tab")]
    SwitchTab { tab_id: String },
    #[serde(rename = "input")]
    Input { event: Value },
}

#[derive(Clone, Debug)]
pub enum ScreenEvent {
    State(Value),
    Frame(ScreenFrame),
    Closed,
    Error(String),
}

#[derive(Clone)]
pub struct ScreenClient {
    command_tx: mpsc::Sender<ScreenControl>,
}

enum ScreenControl {
    Send(ScreenCommand),
    Close,
}

pub struct ScreenHandle {
    pub client: ScreenClient,
    pub events: mpsc::Receiver<ScreenEvent>,
}

impl ScreenHandle {
    pub fn spawn(
        config: ClientConfig,
        bot_id: impl Into<String>,
        quality: impl Into<String>,
        tab_id: Option<String>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel(64);
        let (event_tx, events) = mpsc::channel(64);
        let client = ScreenClient { command_tx };
        tokio::spawn(run_screen(
            config,
            bot_id.into(),
            quality.into(),
            tab_id,
            command_rx,
            event_tx,
        ));
        Self { client, events }
    }

    pub async fn close(&self) -> Result<()> {
        self.client.close().await
    }
}

impl ScreenClient {
    pub async fn ack(&self, seq: u64) -> Result<()> {
        self.send(ScreenCommand::Ack { seq }).await
    }
    pub async fn switch_tab(&self, tab_id: impl Into<String>) -> Result<()> {
        self.send(ScreenCommand::SwitchTab {
            tab_id: tab_id.into(),
        })
        .await
    }
    pub async fn input(&self, event: Value) -> Result<()> {
        self.send(ScreenCommand::Input { event }).await
    }
    pub async fn close(&self) -> Result<()> {
        self.command_tx
            .send(ScreenControl::Close)
            .await
            .map_err(|_| CoreError::Closed)
    }
    async fn send(&self, command: ScreenCommand) -> Result<()> {
        self.command_tx
            .send(ScreenControl::Send(command))
            .await
            .map_err(|_| CoreError::Closed)
    }
}

async fn run_screen(
    config: ClientConfig,
    bot_id: String,
    quality: String,
    tab_id: Option<String>,
    mut commands: mpsc::Receiver<ScreenControl>,
    events: mpsc::Sender<ScreenEvent>,
) {
    let result: Result<()> = async {
        let mut url = config.websocket_url("/ws/screen")?.parse::<reqwest::Url>().map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?;
        url.query_pairs_mut().append_pair("bot_id", &bot_id).append_pair("quality", &quality);
        if let Some(tab_id) = tab_id.as_deref() { url.query_pairs_mut().append_pair("tab_id", tab_id); }
        let mut request = url.as_str().into_client_request().map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?;
        let auth = format!("Bearer {}", config.password);
        request.headers_mut().insert(AUTHORIZATION, HeaderValue::from_str(&auth).map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?);
        let (mut socket, _) = connect_async(request).await.map_err(|error| CoreError::WebSocket(Box::new(error)))?;
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(ScreenControl::Send(command)) => socket.send(Message::Text(serde_json::to_string(&command)?)).await.map_err(|error| CoreError::WebSocket(Box::new(error)))?,
                    Some(ScreenControl::Close) => { let _ = socket.close(None).await; return Ok(()); }
                    None => {
                        let _ = socket.close(None).await;
                        return Ok(());
                    }
                },
                message = socket.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        let envelope: Value = serde_json::from_str(&text)?;
                        if envelope.get("type").and_then(Value::as_str) != Some("state") {
                            continue;
                        }
                        let Some(state) = envelope.get("state").filter(|value| value.is_object())
                        else {
                            continue;
                        };
                        let _ = events.send(ScreenEvent::State(state.clone())).await;
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let frame = parse_screen_frame(&bytes)?;
                        let _ = events.send(ScreenEvent::Frame(frame)).await;
                    }
                    Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.map_err(|error| CoreError::WebSocket(Box::new(error)))?,
                    Some(Ok(Message::Close(_))) | None => return Err(CoreError::Closed),
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(CoreError::WebSocket(Box::new(error))),
                }
            }
        }
    }.await;
    match result {
        Ok(()) | Err(CoreError::Closed) => {
            let _ = events.send(ScreenEvent::Closed).await;
        }
        Err(error) => {
            let _ = events.send(ScreenEvent::Error(error.to_string())).await;
        }
    }
}

pub async fn screen_request(
    config: &ClientConfig,
    bot_id: &str,
    quality: &str,
    tab_id: Option<&str>,
) -> Result<reqwest::RequestBuilder> {
    let base = config.websocket_url("/ws/screen")?;
    let mut url =
        reqwest::Url::parse(&base).map_err(|e| CoreError::InvalidEndpoint(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("bot_id", bot_id)
        .append_pair("quality", quality);
    if let Some(tab_id) = tab_id {
        url.query_pairs_mut().append_pair("tab_id", tab_id);
    }
    Ok(reqwest::Client::new()
        .get(url)
        .bearer_auth(&config.password))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    async fn loopback_server(node_id: &str, resume_mode: &str, bootstrap: bool) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let node_id = node_id.to_string();
        let resume_mode = resume_mode.to_string();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            socket.send(Message::Text(json!({
                "v": 1, "kind": "evt", "event": "hello",
                "data": {"protocol": 1, "node_id": node_id, "server_version": "test", "last_seq": 3}
            }).to_string())).await.unwrap();
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                return;
            };
            let request: Value = serde_json::from_str(text.as_ref()).unwrap();
            socket.send(Message::Text(json!({"v":1,"kind":"res","id":request["id"],"ok":true,"result":{"mode":resume_mode}}).to_string())).await.unwrap();
            if bootstrap {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    return;
                };
                let request: Value = serde_json::from_str(text.as_ref()).unwrap();
                socket.send(Message::Text(json!({"v":1,"kind":"res","id":request["id"],"ok":true,"result":{"seq":3,"hello":{"node_id":node_id},"bots":[],"chats":[],"projects":[],"settings":{},"pending":{}}}).to_string())).await.unwrap();
            } else {
                socket
                    .send(Message::Text(
                        json!({"v":1,"kind":"evt","event":"sync.done","data":{"seq":3}})
                            .to_string(),
                    ))
                    .await
                    .unwrap();
            }
        });
        format!("127.0.0.1:{}", address.port())
    }

    #[test]
    fn arbitrary_endpoints_get_ws_path() {
        assert_eq!(
            websocket_url("127.0.0.1:7789", "/ws").unwrap(),
            "ws://127.0.0.1:7789/ws"
        );
        assert_eq!(
            websocket_url("https://bot.example/ws", "/ws").unwrap(),
            "wss://bot.example/ws"
        );
        assert_eq!(
            websocket_url("wss://bot.example", "/ws/screen").unwrap(),
            "wss://bot.example/ws/screen"
        );
        assert_eq!(
            websocket_url("wss://bot.example/ws", "/ws/screen").unwrap(),
            "wss://bot.example/ws/screen"
        );
        assert_eq!(
            http_url("ws://bot.example:7788", "/api/v1/rpc").unwrap(),
            "http://bot.example:7788/api/v1/rpc"
        );
    }

    #[test]
    fn reconnect_delay_has_bounded_jitter() {
        let cfg = ReconnectConfig::default();
        assert_eq!(cfg.delay(0, 0.5), Duration::from_secs(1));
        assert!(cfg.delay(10, 0.0) <= Duration::from_secs(24));
        assert!(cfg.delay(10, 1.0) <= Duration::from_secs(36));
    }

    #[test]
    fn ordered_events_merge_and_deduplicate() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({"seq": 2, "bots": [{"id":"bot_a", "name":"old", "hidden":false}], "chats":[], "projects":[]}));
        state.apply_event(ProtocolEvent {
            seq: Some(4),
            event: "bot.updated".into(),
            data: json!({"bot":{"id":"bot_a","name":"new"}}),
        });
        assert_eq!(state.bots["bot_a"]["name"], "old");
        state.apply_event(ProtocolEvent {
            seq: Some(3),
            event: "bot.updated".into(),
            data: json!({"bot":{"id":"bot_a","name":"new"}}),
        });
        assert_eq!(state.last_seq, 4);
        assert_eq!(state.bots["bot_a"]["name"], "new");
        state.apply_event(ProtocolEvent {
            seq: Some(4),
            event: "bot.updated".into(),
            data: json!({"bot":{"id":"bot_a","name":"bad"}}),
        });
        assert_eq!(state.bots["bot_a"]["name"], "new");
    }

    #[test]
    fn question_answered_event_updates_cached_question_state() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({
            "seq": 1,
            "questions": [{"id":"question-1","state":"pending","text":"继续？"}]
        }));
        state.apply_event(ProtocolEvent {
            seq: Some(2),
            event: "question.answered".into(),
            data: json!({
                "question": {
                    "id":"question-1",
                    "state":"answered",
                    "text":"继续？",
                    "answer":{"text":"继续"}
                }
            }),
        });
        assert_eq!(state.questions["question-1"]["state"], "answered");
        assert_eq!(state.questions["question-1"]["answer"]["text"], "继续");
    }

    #[test]
    fn trace_cursor_unblocks_later_project_updates_without_trace_subscription() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({"seq":40,"projects":[{"id":"p","status":"active"}]}));
        state.apply_event(ProtocolEvent {
            seq: Some(42),
            event: "project.updated".into(),
            data: json!({"project":{"id":"p","status":"review"}}),
        });
        assert_eq!(state.projects["p"]["status"], "active");
        state.apply_event(ProtocolEvent {
            seq: Some(41),
            event: "sync.cursor".into(),
            data: json!({"seq":41}),
        });
        assert_eq!(state.last_seq, 42);
        assert_eq!(state.projects["p"]["status"], "review");
        assert!(state.buffered_events.is_empty());
    }

    #[test]
    fn sync_done_with_a_single_event_gap_requests_bootstrap_resync() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({
            "seq": 40,
            "projects": [{"id":"p","status":"active"}]
        }));
        state.apply_event(ProtocolEvent {
            seq: Some(42),
            event: "project.updated".into(),
            data: json!({"project":{"id":"p","status":"review"}}),
        });
        assert!(!state.needs_resync);
        assert_eq!(state.projects["p"]["status"], "active");

        state.apply_event(ProtocolEvent {
            seq: None,
            event: "sync.done".into(),
            data: json!({"seq": 42}),
        });

        assert!(state.needs_resync);
        assert!(state.buffered_events.is_empty());
    }

    #[test]
    fn sync_done_after_contiguous_events_does_not_request_resync() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({
            "seq": 40,
            "projects": [{"id":"p","status":"active"}]
        }));
        state.apply_event(ProtocolEvent {
            seq: Some(41),
            event: "sync.cursor".into(),
            data: json!({"seq": 41}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(42),
            event: "project.updated".into(),
            data: json!({"project":{"id":"p","status":"review"}}),
        });
        state.apply_event(ProtocolEvent {
            seq: None,
            event: "sync.done".into(),
            data: json!({"seq": 42}),
        });

        assert!(!state.needs_resync);
        assert_eq!(state.projects["p"]["status"], "review");
    }

    #[test]
    fn event_maps_keep_bootstrap_keys_for_announcements_skills_and_models() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({
            "seq": 1,
            "announcements": [{"id":"announcement-1","project_id":"project-1","title":"old"}],
            "skills": [{"id":"skill-1","name":"skill-name","enabled":false}],
            "models": [{"ref":"provider/model","display_name":"old"}]
        }));
        state.apply_event(ProtocolEvent {
            seq: Some(2),
            event: "announcement.updated".into(),
            data: json!({"announcement":{"id":"announcement-1","project_id":"project-1","title":"new"}}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(3),
            event: "skill.updated".into(),
            data: json!({"skill":{"id":"skill-1","name":"skill-name","enabled":true}}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(4),
            event: "model.updated".into(),
            data: json!({"model":{"ref":"provider/model","display_name":"new"}}),
        });
        assert_eq!(state.announcements["project-1"]["title"], "new");
        assert_eq!(state.skills["skill-name"]["enabled"], true);
        assert_eq!(state.models["provider/model"]["display_name"], "new");
        state.apply_event(ProtocolEvent {
            seq: Some(5),
            event: "announcement.deleted".into(),
            data: json!({"project_id":"project-1"}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(6),
            event: "skill.deleted".into(),
            data: json!({"name":"skill-name"}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(7),
            event: "model.deleted".into(),
            data: json!({"ref":"provider/model"}),
        });
        assert!(state.announcements.is_empty());
        assert!(state.skills.is_empty());
        assert!(state.models.is_empty());
    }

    #[test]
    fn transport_cursor_does_not_skip_an_event_gap() {
        let mut cursor = 2;
        let mut out_of_order = BTreeSet::new();
        advance_contiguous(&mut cursor, 4, &mut out_of_order).unwrap();
        assert_eq!(cursor, 2);
        advance_contiguous(&mut cursor, 3, &mut out_of_order).unwrap();
        assert_eq!(cursor, 4);
        advance_contiguous(&mut cursor, 3, &mut out_of_order).unwrap();
        assert_eq!(cursor, 4);
    }

    #[test]
    fn app_state_gap_flood_requests_resync_and_bootstrap_recovers() {
        let mut state = AppState::default();
        for seq in 2..=(MAX_BUFFERED_EVENTS as u64 + 2) {
            state.apply_event(ProtocolEvent {
                seq: Some(seq),
                event: "bot.updated".into(),
                data: json!({"bot":{"id":"bot_gap","name":seq}}),
            });
        }
        assert!(state.needs_resync);
        assert_eq!(state.last_seq, 0);
        let cached = state.to_bootstrap_cache();
        assert!(AppState::from_bootstrap_cache(cached).needs_resync);
        state.apply_event(ProtocolEvent {
            seq: Some(1),
            event: "bot.updated".into(),
            data: json!({"bot":{"id":"bot_gap","name":"must-wait"}}),
        });
        assert!(!state.bots.contains_key("bot_gap"));

        state.apply_bootstrap(json!({
            "seq": 40,
            "bots": [{"id":"bot_gap","name":"fresh"}],
            "chats": [], "projects": [], "messages": []
        }));
        assert!(!state.needs_resync);
        assert_eq!(state.last_seq, 40);
        state.apply_event(ProtocolEvent {
            seq: Some(41),
            event: "bot.updated".into(),
            data: json!({"bot":{"id":"bot_gap","name":"after-bootstrap"}}),
        });
        assert_eq!(state.bots["bot_gap"]["name"], "after-bootstrap");
    }

    #[test]
    fn transport_gap_flood_is_bounded_and_cleared_for_reconnect() {
        let mut cursor = 0;
        let mut out_of_order = BTreeSet::new();
        for seq in 2..=(MAX_BUFFERED_EVENTS as u64 + 1) {
            advance_contiguous(&mut cursor, seq, &mut out_of_order).unwrap();
        }
        assert_eq!(out_of_order.len(), MAX_BUFFERED_EVENTS);
        let error = advance_contiguous(
            &mut cursor,
            MAX_BUFFERED_EVENTS as u64 + 2,
            &mut out_of_order,
        )
        .unwrap_err();
        assert!(
            matches!(error, CoreError::EventGapExceeded(MAX_BUFFERED_EVENTS)),
            "unexpected transport result: {error:?}"
        );
        assert!(out_of_order.is_empty());
        assert_eq!(cursor, 0);
    }

    #[test]
    fn bootstrap_cursor_resets_transport_gap() {
        let mut cursor = 7;
        let mut out_of_order = BTreeSet::new();
        advance_contiguous(&mut cursor, 9, &mut out_of_order).unwrap();
        assert!(!out_of_order.is_empty());
        reset_cursor_from_bootstrap(&mut cursor, &mut out_of_order, &json!({"seq": 42}));
        assert_eq!(cursor, 42);
        assert!(out_of_order.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_transport_disconnects_on_gap_flood() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            for seq in 2..=(MAX_BUFFERED_EVENTS as u64 + 2) {
                socket
                    .send(Message::Text(
                        json!({
                            "v": 1,
                            "kind": "evt",
                            "seq": seq,
                            "event": "bot.updated",
                            "data": {"bot":{"id":"gap-bot","name":seq}}
                        })
                        .to_string(),
                    ))
                    .await
                    .unwrap();
            }
            sleep(Duration::from_millis(250)).await;
            let _ = socket.close(None).await;
        });
        let url = websocket_url(&endpoint, "/ws").unwrap();
        let (mut socket, _) = connect_async(url).await.unwrap();
        let (_command_tx, mut commands) = mpsc::channel(1);
        let (events, _received) = mpsc::channel(4096);
        let mut pending = VecDeque::new();
        let mut last_seq = 0;
        let error = run_connected(
            Duration::from_secs(1),
            &mut socket,
            &mut commands,
            &events,
            &mut pending,
            &mut last_seq,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, CoreError::EventGapExceeded(MAX_BUFFERED_EVENTS)),
            "unexpected transport result: {error:?}"
        );
        assert_eq!(last_seq, 0);
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_command_exits_connected_worker_without_reconnect() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _socket = accept_async(stream).await.unwrap();
            sleep(Duration::from_millis(100)).await;
        });
        let url = websocket_url(&endpoint, "/ws").unwrap();
        let (mut socket, _) = connect_async(url).await.unwrap();
        let (command_tx, mut commands) = mpsc::channel(1);
        command_tx.send(Command::Close).await.unwrap();
        let (events, _received) = mpsc::channel(8);
        let mut pending = VecDeque::new();
        let mut last_seq = 0;
        let result = run_connected(
            Duration::from_secs(1),
            &mut socket,
            &mut commands,
            &events,
            &mut pending,
            &mut last_seq,
        )
        .await
        .unwrap();
        assert!(matches!(result, ConnectedExit::Closed));
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn in_flight_request_timeout_is_independent_of_heartbeat_interval() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let _ = socket.next().await;
            sleep(Duration::from_millis(100)).await;
            let _ = socket.close(None).await;
        });
        let url = websocket_url(&endpoint, "/ws").unwrap();
        let (mut socket, _) = connect_async(url).await.unwrap();
        let (command_tx, mut commands) = mpsc::channel(1);
        let (response, receiver) = oneshot::channel();
        command_tx
            .send(Command::Request(RequestCommand {
                method: "ping".into(),
                params: json!({}),
                response,
            }))
            .await
            .unwrap();
        let (events, _received) = mpsc::channel(8);
        let mut pending = VecDeque::new();
        let mut last_seq = 0;
        let run = tokio::spawn(async move {
            run_connected(
                Duration::from_millis(20),
                &mut socket,
                &mut commands,
                &events,
                &mut pending,
                &mut last_seq,
            )
            .await
        });
        let error = timeout(Duration::from_millis(500), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, CoreError::RequestTimeout));
        let _ = run.await;
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unrelated_response_id_does_not_requeue_current_request() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(text))) = socket.next().await else {
                return;
            };
            let request: Value = serde_json::from_str(text.as_ref()).unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "v": 1,
                        "kind": "res",
                        "id": "unrelated-response",
                        "ok": true,
                        "result": {"ignored": true}
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "v": 1,
                        "kind": "res",
                        "id": request["id"],
                        "ok": true,
                        "result": {"accepted": true}
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            sleep(Duration::from_millis(100)).await;
            let _ = socket.close(None).await;
        });
        let url = websocket_url(&endpoint, "/ws").unwrap();
        let (mut socket, _) = connect_async(url).await.unwrap();
        let (command_tx, mut commands) = mpsc::channel(1);
        let (response, receiver) = oneshot::channel();
        command_tx
            .send(Command::Request(RequestCommand {
                method: "ping".into(),
                params: json!({}),
                response,
            }))
            .await
            .unwrap();
        let (events, _received) = mpsc::channel(8);
        let mut pending = VecDeque::new();
        let mut last_seq = 0;
        let run = tokio::spawn(async move {
            run_connected(
                Duration::from_secs(1),
                &mut socket,
                &mut commands,
                &events,
                &mut pending,
                &mut last_seq,
            )
            .await
        });
        let result = timeout(Duration::from_millis(500), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result["accepted"], true);
        let _ = run.await;
        server.await.unwrap();
    }

    #[test]
    fn bootstrap_and_transient_events_keep_ui_maps_in_sync() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({
            "seq": 1,
            "bots": [], "chats": [{"id":"chat_a","last_read_seq":1}], "projects": [],
            "messages": [], "assignments": [], "approvals": [], "questions": [],
            "skills": [], "routines": [], "providers": [], "models": []
        }));
        state.apply_event(ProtocolEvent {
            seq: None,
            event: "message.delta".into(),
            data: json!({"message_id":"msg_a","text":"hi"}),
        });
        state.apply_event(ProtocolEvent {
            seq: None,
            event: "typing".into(),
            data: json!({"chat_id":"chat_a","bot_id":"bot_a","on":true}),
        });
        state.apply_event(ProtocolEvent {
            seq: None,
            event: "read.updated".into(),
            data: json!({"chat_id":"chat_a","last_read_seq":3}),
        });
        assert_eq!(state.message_deltas["msg_a"], "hi");
        state.apply_event(ProtocolEvent {
            seq: None,
            event: "message.updated".into(),
            data: json!({"message":{"id":"msg_a","streaming":false,"fallback_text":"hi"}}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(2),
            event: "assignment.created".into(),
            data: json!({"assignment":{"id":"asg_a","status":"working"}}),
        });
        assert!(state.typing["chat_a:bot_a"]);
        assert_eq!(state.chats["chat_a"]["last_read_seq"], 3);
        assert_eq!(state.assignments["asg_a"]["status"], "working");
        assert!(!state.message_deltas.contains_key("msg_a"));
        assert_eq!(state.messages["msg_a"]["fallback_text"], "hi");
    }

    #[test]
    fn streaming_message_deltas_merge_in_sequence_and_finalize() {
        let mut state = AppState::default();
        state.apply_bootstrap(json!({
            "seq": 3,
            "messages": [{"id":"msg_stream","streaming":true}],
            "chats": [], "projects": []
        }));
        // The second chunk can arrive first on the wire; the state layer must
        // wait for the missing sequence before exposing either chunk.
        state.apply_event(ProtocolEvent {
            seq: Some(5),
            event: "message.delta".into(),
            data: json!({"message_id":"msg_stream","text":"world"}),
        });
        assert!(!state.message_deltas.contains_key("msg_stream"));
        state.apply_event(ProtocolEvent {
            seq: Some(4),
            event: "message.delta".into(),
            data: json!({"message_id":"msg_stream","text":"hello "}),
        });
        assert_eq!(state.message_deltas["msg_stream"], "hello world");
        state.apply_event(ProtocolEvent {
            seq: Some(6),
            event: "message.updated".into(),
            data: json!({
                "message":{"id":"msg_stream","streaming":false,"fallback_text":"hello world"}
            }),
        });
        assert!(!state.message_deltas.contains_key("msg_stream"));
        assert_eq!(state.messages["msg_stream"]["fallback_text"], "hello world");
        assert_eq!(state.last_seq, 6);
    }

    #[test]
    fn bootstrap_indexes_nested_pending_and_cache_round_trip() {
        let value = json!({
            "seq": 9,
            "bots": [], "chats": [], "projects": [], "messages": [], "assignments": [],
            "announcements": [{"project_id":"prj_a","highlights":[]}],
            "skills": [], "routines": [], "providers": [], "models": [], "settings": {"host_name":"test"},
            "pending": {"approvals":[{"id":"apr_a"}], "questions":[{"id":"que_a"}], "reviews":["prj_a"]}
        });
        let state = AppState::from_bootstrap_cache(value);
        assert!(state.approvals.contains_key("apr_a"));
        assert!(state.questions.contains_key("que_a"));
        assert!(state.announcements.contains_key("prj_a"));
        let cached = state.to_bootstrap_cache();
        assert_eq!(cached["seq"], 9);
        assert_eq!(cached["approvals"][0]["id"], "apr_a");
        assert_eq!(cached["pending"]["reviews"][0], "prj_a");
    }

    #[test]
    fn trace_cursor_dedup_and_response_replaces_delta() {
        let mut trace = TraceTimeline::default();
        trace.apply_delta("req_1", "text", "par");
        trace.apply_item(json!({"aseq":2,"type":"llm.response","data":{"request_id":"req_1","text":"complete","thinking":"reason"}}));
        trace.apply_item(json!({"aseq":2,"type":"llm.response","data":{"request_id":"req_1","text":"complete","thinking":"reason"}}));
        assert_eq!(trace.items.len(), 1);
        assert_eq!(
            trace.rendered_request("req_1"),
            Some(("complete", "reason"))
        );
    }

    #[test]
    fn trace_older_history_does_not_move_last_cursor_backwards() {
        let mut trace = TraceTimeline::default();
        trace.apply_history(&json!({"items":[{"aseq":10},{"aseq":20}],"first_aseq":10,"last_aseq":20,"has_more_before":true,"live":true}));
        trace.apply_history(&json!({"items":[{"aseq":1},{"aseq":2}],"first_aseq":1,"last_aseq":2,"has_more_before":false,"live":true}));
        assert_eq!(trace.first_aseq, Some(1));
        assert_eq!(trace.last_aseq, Some(20));
        assert!(trace.items.contains_key(&20));
    }

    #[test]
    fn trace_history_overlap_deduplicates_by_aseq_without_cursor_regression() {
        let mut trace = TraceTimeline::default();
        trace.apply_history(&json!({
            "items":[{"aseq":4,"type":"tool.start"},{"aseq":5,"type":"tool.end"},{"aseq":6,"type":"run.end"}],
            "first_aseq":4,"last_aseq":6,"has_more_before":true,"live":true
        }));
        trace.apply_history(&json!({
            "items":[{"aseq":2,"type":"run.start"},{"aseq":3,"type":"llm.request"},{"aseq":4,"type":"tool.start","replacement":true}],
            "first_aseq":2,"last_aseq":4,"has_more_before":false,"live":true
        }));
        assert_eq!(trace.items.len(), 5);
        assert_eq!(trace.first_aseq, Some(2));
        assert_eq!(trace.last_aseq, Some(6));
        assert_eq!(trace.items[&4]["replacement"], true);
        assert!(trace.items.contains_key(&6));
    }

    #[test]
    fn screen_parser_and_ack() {
        let header =
            json!({"seq":7,"tab_id":"tab","w":12,"h":9,"ts":10,"url":"https://example.com"})
                .to_string();
        let mut bytes = (header.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&[0xff, 0xd8, 0xff]);
        let frame = parse_screen_frame(&bytes).unwrap();
        assert_eq!(frame.header.seq, 7);
        assert_eq!(frame.jpeg, vec![0xff, 0xd8, 0xff]);
        assert_eq!(screen_ack(7), json!({"type":"ack","seq":7}));
    }

    #[test]
    fn write_params_get_stable_idempotency_key() {
        let mut params = json!({"chat_id":"chat"});
        add_idempotency_key(&mut params);
        let first = params["client_request_id"].clone();
        add_idempotency_key(&mut params);
        assert_eq!(first, params["client_request_id"]);
    }

    #[test]
    fn protocol_types_validate_wire_request_and_response_without_dropping_extensions() {
        let request = request_frame(
            "typed-request",
            "session.resume",
            json!({
                "last_seq": 7,
                "client": {
                    "platform": "macos",
                    "app_version": "test",
                    "device_name": "Mac mini",
                    "device_id": "device-1"
                }
            }),
        );
        let decoded: protocol::RpcRequest = serde_json::from_value(request).unwrap();
        assert_eq!(decoded.id, "typed-request");
        assert_eq!(decoded.method, protocol::Method::SessionResume);

        let result = json!({"mode":"replay", "server_extension":{"kept":true}});
        let preserved = decode_rpc_result("session.resume", result.clone()).unwrap();
        assert_eq!(preserved, result);
        let future_result = json!({"future_field": {"kept": true}});
        assert_eq!(
            decode_rpc_result("future.method", future_result.clone()).unwrap(),
            future_result
        );
    }

    #[test]
    fn unknown_message_blocks_are_fallbacked_for_history_and_preserved_on_wire() {
        let message = json!({
            "id":"msg_future", "chat_id":"chat_main", "seq":1,
            "sender":{"kind":"user"}, "created_at":"2026-10-09T10:19:02.312Z",
            "edited_at":null, "deleted":false, "reply_to":null, "thread_count":0,
            "mentions":[], "blocks":[{"type":"future_block","payload":{"v":1}}],
            "fallback_text":"Future block", "intent":null, "assignment_id":null,
            "streaming":false, "delivery":[], "reactions":[]
        });
        let history = json!({"messages":[message.clone()], "has_more":false});
        let preserved = decode_rpc_result("chat.history", history.clone()).unwrap();
        assert_eq!(preserved, history);

        let bootstrap = json!({
            "seq":1,
            "hello": serde_json::from_str::<Value>(include_str!("../../../../../protocol/fixtures/objects/hello.json")).unwrap(),
            "bots":[], "chats":[], "projects":[],
            "settings": serde_json::from_str::<Value>(include_str!("../../../../../protocol/fixtures/objects/settings.json")).unwrap(),
            "pending":{"approvals":[],"questions":[],"reviews":[]},
            "messages":[message]
        });
        let preserved_bootstrap = decode_rpc_result("bootstrap", bootstrap.clone()).unwrap();
        assert_eq!(preserved_bootstrap, bootstrap);

        let malformed = json!({"messages":[{"blocks":[{"type":"future_block"}],"fallback_text":"x"}],"has_more":false});
        assert!(decode_rpc_result("chat.history", malformed).is_err());
    }

    #[test]
    fn unknown_trace_items_are_ignored_for_validation_and_unknown_events_are_ignored_in_state() {
        let trace = json!({
            "items":[{
                "assignment_id":null, "chat_id":"chat_main", "run_id":"run_1",
                "aseq":9, "at":"2026-10-09T10:19:02.312Z", "type":"future.trace",
                "data":{"new_shape":true}
            }],
            "first_aseq":9, "last_aseq":9, "has_more_before":false, "live":true
        });
        let preserved = decode_rpc_result("trace.history", trace.clone()).unwrap();
        assert_eq!(preserved, trace);

        let mut state = AppState::default();
        state.apply_event(ProtocolEvent {
            seq: Some(1),
            event: "future.event".into(),
            data: json!({"new_shape":true}),
        });
        state.apply_event(ProtocolEvent {
            seq: Some(2),
            event: "bot.created".into(),
            data: json!({"bot":{"id":"bot_a","name":"main"}}),
        });
        assert_eq!(state.last_seq, 2);
        assert_eq!(state.bots["bot_a"]["name"], "main");
    }

    #[test]
    fn request_frame_preserves_retry_metadata_extensions_and_future_methods() {
        let params = json!({
            "chat_id": "chat-1",
            "text": "hello",
            "mentions": [],
            "reply_to": null,
            "attachments": [],
            "client_request_id": "retry-1",
            "future_field": {"kept": true}
        });
        let frame = request_frame("request-1", "chat.send", params.clone());
        assert_eq!(frame["params"], params);

        let future = request_frame(
            "request-2",
            "future.method",
            json!({"client_request_id":"retry-2","extension":true}),
        );
        assert_eq!(future["method"], "future.method");
        assert_eq!(future["params"]["extension"], true);
    }

    #[test]
    fn known_params_validate_required_fields_without_rejecting_extensions() {
        let mut params = json!({
            "chat_id": "chat-1",
            "text": "hello",
            "mentions": [],
            "future_field": {"kept": true}
        });
        add_idempotency_key(&mut params);
        assert!(validate_rpc_params("chat.send", &params).is_ok());
        assert!(validate_rpc_params("chat.send", &json!({"text":"missing chat"})).is_err());
        assert!(validate_rpc_params("future.method", &json!({"any":true})).is_ok());
    }

    #[test]
    fn disconnected_in_flight_request_is_requeued_with_same_idempotency_key() {
        let (response, _receiver) = oneshot::channel();
        let mut params = json!({"chat_id":"chat_a"});
        add_idempotency_key(&mut params);
        let key = params["client_request_id"].clone();
        let mut in_flight = Some((
            "transport_id".to_string(),
            RequestCommand {
                method: "chat.send".into(),
                params,
                response,
            },
            Instant::now(),
        ));
        let mut pending = VecDeque::new();
        requeue(&mut in_flight, &mut pending);
        assert_eq!(pending.front().unwrap().params["client_request_id"], key);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_resume_replay_and_reset_bootstrap() {
        let replay_endpoint = loopback_server("node_replay", "replay", false).await;
        let mut config = ClientConfig::new(&replay_endpoint, "dev");
        config.node_id = Some("node_replay".into());
        config.has_cached_state = true;
        config.last_seq = 2;
        let (mut socket, hello) = connect_main(&config, &replay_endpoint).await.unwrap();
        assert_eq!(hello["node_id"], "node_replay");
        let request_id = "resume_replay";
        socket
            .send(Message::Text(
                request_frame(request_id, "session.resume", json!({"last_seq":2})).to_string(),
            ))
            .await
            .unwrap();
        let (events, _receiver) = mpsc::channel(8);
        let mut last_seq = 2;
        let result = receive_response(
            &mut socket,
            request_id,
            Duration::from_secs(1),
            &events,
            &mut last_seq,
            true,
        )
        .await
        .unwrap();
        assert_eq!(result["mode"], "replay");
        assert_eq!(last_seq, 3);

        let reset_endpoint = loopback_server("node_reset", "reset", true).await;
        let mut config = ClientConfig::new(&reset_endpoint, "dev");
        config.node_id = Some("node_reset".into());
        let (mut socket, _) = connect_main(&config, &reset_endpoint).await.unwrap();
        socket
            .send(Message::Text(
                request_frame("resume_reset", "session.resume", json!({"last_seq":0})).to_string(),
            ))
            .await
            .unwrap();
        let (events, _receiver) = mpsc::channel(8);
        let mut last_seq = 0;
        let result = receive_response(
            &mut socket,
            "resume_reset",
            Duration::from_secs(1),
            &events,
            &mut last_seq,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result["mode"], "reset");
        socket
            .send(Message::Text(
                request_frame("bootstrap", "bootstrap", json!({})).to_string(),
            ))
            .await
            .unwrap();
        let bootstrap = receive_response(
            &mut socket,
            "bootstrap",
            Duration::from_secs(1),
            &events,
            &mut last_seq,
            false,
        )
        .await
        .unwrap();
        assert_eq!(bootstrap["seq"], 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_event_reports_successful_fallback_endpoint() {
        let endpoint = loopback_server("node_fallback", "reset", true).await;
        let mut config = ClientConfig::new("127.0.0.1:1", "dev");
        config.addresses = vec!["127.0.0.1:1".into(), endpoint.clone()];
        config.request_timeout = Duration::from_secs(1);
        config.reconnect.initial_delay = Duration::from_millis(5);
        config.reconnect.max_delay = Duration::from_millis(5);
        config.reconnect.jitter = 0.0;

        let handle = ClientHandle::spawn(config);
        let client = handle.client.clone();
        let mut events = handle.events;
        let mut connected_endpoint = None;
        let mut saw_bootstrap = false;
        while !saw_bootstrap {
            let event = timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .expect("client event stream closed");
            match event {
                ClientEvent::Connected { endpoint, .. } => {
                    connected_endpoint = Some(endpoint);
                }
                ClientEvent::Bootstrap(_) => saw_bootstrap = true,
                _ => {}
            }
        }
        assert_eq!(connected_endpoint.as_deref(), Some(endpoint.as_str()));
        client.close().await;
    }

    async fn assert_resume_watermark_rollback(hello_seq: u64, sync_seq: u64) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "v": 1,
                        "kind": "evt",
                        "event": "hello",
                        "data": {
                            "protocol": 1,
                            "node_id": "node_rollback",
                            "server_version": "test",
                            "last_seq": hello_seq
                        }
                    })
                    .to_string(),
                ))
                .await
                .unwrap();

            let Some(Ok(Message::Text(text))) = socket.next().await else {
                panic!("client closed before session.resume");
            };
            let resume: Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(resume["method"], "session.resume");
            assert_eq!(resume["params"]["last_seq"], 10);
            socket
                .send(Message::Text(
                    json!({
                        "v": 1,
                        "kind": "res",
                        "id": resume["id"],
                        "ok": true,
                        "result": {"mode": "replay"}
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "v": 1,
                        "kind": "evt",
                        "event": "sync.done",
                        "data": {"seq": sync_seq}
                    })
                    .to_string(),
                ))
                .await
                .unwrap();

            let Some(Ok(Message::Text(text))) = socket.next().await else {
                panic!("rollback resume did not request bootstrap");
            };
            let bootstrap: Value = serde_json::from_str(text.as_ref()).unwrap();
            assert_eq!(bootstrap["method"], "bootstrap");
            socket
                .send(Message::Text(
                    json!({
                        "v": 1,
                        "kind": "res",
                        "id": bootstrap["id"],
                        "ok": true,
                        "result": {
                            "seq": sync_seq,
                            "hello": {"node_id": "node_rollback"},
                            "bots": [],
                            "chats": [],
                            "projects": [],
                            "settings": {},
                            "pending": {}
                        }
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
        });

        let mut config = ClientConfig::new(&endpoint, "dev");
        config.node_id = Some("node_rollback".into());
        config.has_cached_state = true;
        config.last_seq = 10;
        config.request_timeout = Duration::from_secs(1);
        config.reconnect.initial_delay = Duration::from_millis(5);
        config.reconnect.max_delay = Duration::from_millis(5);
        config.reconnect.jitter = 0.0;
        let handle = ClientHandle::spawn(config);
        let client = handle.client.clone();
        let mut events = handle.events;
        let mut saw_connected = false;
        let mut saw_bootstrap = false;
        while !saw_bootstrap {
            let event = timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .expect("client event stream closed");
            match event {
                ClientEvent::Connected { resumed, .. } => {
                    assert!(!resumed, "watermark rollback must force a fresh bootstrap");
                    saw_connected = true;
                }
                ClientEvent::Bootstrap(value) => {
                    assert_eq!(value["seq"], sync_seq);
                    saw_bootstrap = true;
                }
                _ => {}
            }
        }
        assert!(saw_connected);
        client.close().await;
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resume_hello_watermark_rollback_forces_bootstrap() {
        assert_resume_watermark_rollback(3, 10).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resume_sync_watermark_rollback_forces_bootstrap() {
        assert_resume_watermark_rollback(10, 3).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_wrong_node_is_rejected() {
        let endpoint = loopback_server("unexpected", "replay", false).await;
        let mut config = ClientConfig::new(&endpoint, "dev");
        config.node_id = Some("expected".into());
        let error = connect_main(&config, &endpoint).await.unwrap_err();
        assert!(error.to_string().contains("node_id"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn screen_ack_is_sent_only_after_caller_renders_frame() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (ack_tx, mut ack_rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(
                    json!({"type":"future_state","ignored":true}).to_string(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "type":"state",
                        "state": {
                            "driver":"user",
                            "tabs":[{"tab_id":"tab","title":"App","url":"https://example.com","active":true}],
                            "width":2,
                            "height":2
                        }
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            let header =
                json!({"seq":42,"tab_id":"tab","w":2,"h":2,"ts":1,"url":"https://example.com"})
                    .to_string();
            let mut bytes = (header.len() as u32).to_be_bytes().to_vec();
            bytes.extend_from_slice(header.as_bytes());
            bytes.extend_from_slice(&[0xff, 0xd8, 0xff]);
            socket.send(Message::Binary(bytes)).await.unwrap();
            if let Ok(Some(Ok(Message::Text(text)))) =
                timeout(Duration::from_millis(100), socket.next()).await
            {
                let _ = ack_tx.send(text.to_string()).await;
                return;
            }
            if let Some(Ok(Message::Text(text))) = timeout(Duration::from_secs(1), socket.next())
                .await
                .unwrap()
            {
                let _ = ack_tx.send(text.to_string()).await;
            }
        });
        let config = ClientConfig::new(&endpoint, "dev");
        let handle = ScreenHandle::spawn(config, "bot_a", "auto", None);
        let mut events = handle.events;
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let ScreenEvent::State(state) = event else {
            panic!("expected normalized screen state");
        };
        assert_eq!(state["driver"], "user");
        assert_eq!(state["tabs"][0]["tab_id"], "tab");
        let event = timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let ScreenEvent::Frame(frame) = event else {
            panic!("expected screen frame");
        };
        assert_eq!(frame.header.seq, 42);
        sleep(Duration::from_millis(150)).await;
        assert!(ack_rx.try_recv().is_err());
        handle.client.ack(frame.header.seq).await.unwrap();
        assert_eq!(ack_rx.recv().await.unwrap(), r#"{"type":"ack","seq":42}"#);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_disconnect_retries_same_write_request_id() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (id_tx, mut id_rx) = mpsc::channel(2);
        tokio::spawn(async move {
            for connection in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                socket.send(Message::Text(json!({"v":1,"kind":"evt","event":"hello","data":{"protocol":1,"node_id":"node_retry"}}).to_string())).await.unwrap();
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    return;
                };
                let resume: Value = serde_json::from_str(text.as_ref()).unwrap();
                socket.send(Message::Text(json!({"v":1,"kind":"res","id":resume["id"],"ok":true,"result":{"mode":"reset"}}).to_string())).await.unwrap();
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    return;
                };
                let bootstrap: Value = serde_json::from_str(text.as_ref()).unwrap();
                socket.send(Message::Text(json!({"v":1,"kind":"res","id":bootstrap["id"],"ok":true,"result":{"seq":0,"hello":{},"bots":[],"chats":[],"projects":[],"settings":{},"pending":{}}}).to_string())).await.unwrap();
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    return;
                };
                let write: Value = serde_json::from_str(text.as_ref()).unwrap();
                let id = write["params"]["client_request_id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                id_tx.send(id).await.unwrap();
                if connection == 0 {
                    // Drop before a response: the client must keep this exact request.
                    continue;
                }
                socket.send(Message::Text(json!({"v":1,"kind":"res","id":write["id"],"ok":true,"result":{"message":{"id":"msg_retry"}}}).to_string())).await.unwrap();
            }
        });
        let mut config = ClientConfig::new(&endpoint, "dev");
        config.reconnect.initial_delay = Duration::from_millis(5);
        config.reconnect.max_delay = Duration::from_millis(5);
        config.reconnect.jitter = 0.0;
        config.request_timeout = Duration::from_secs(1);
        let handle = ClientHandle::spawn(config);
        let response = timeout(
            Duration::from_secs(3),
            handle.client.request(
                "chat.send",
                json!({"chat_id":"chat_retry","text":"hello","mentions":[]}),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response["message"]["id"], "msg_retry");
        let first_id = id_rx.recv().await.unwrap();
        let second_id = id_rx.recv().await.unwrap();
        assert_eq!(first_id, second_id);
        handle.close().await;
    }
}
