//! Coordination tools exposed to model runs.
//!
//! This module deliberately has no dependency on the HTTP gateway state.  The
//! gateway supplies small bridges for durable RPCs, agent-browser, and web
//! access when it builds an `ExecutionEngine`.  Keeping the bridges here makes
//! tool registration testable without starting a listener and keeps the
//! existing execution/send_msg bridge unchanged.

use crate::{adapter::ProductionBackend, backend::RuntimeExecution, GatewayState, RpcBackend};
use async_trait::async_trait;
use base64::Engine;
use macbot_browser::{BrowserAction, BrowserError, BrowserMode, SessionConfig};
use macbot_orchestrator::Orchestrator;
use macbot_tools::{Risk, Tool, ToolContext, ToolResult};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Identity injected by the gateway for one model run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollaborationIdentity {
    pub bot_id: String,
    pub chat_id: String,
    pub assignment_id: Option<String>,
    pub project_id: Option<String>,
    pub is_main: bool,
}

/// Durable coordination RPCs.  Production implements this with the gateway's
/// `ProductionBackend::call`, while tests can use the in-memory orchestrator.
#[async_trait]
pub trait CoordinationRpc: Send + Sync {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String>;
}

/// Lightweight bridge for tests and callers that intentionally operate on
/// the orchestrator directly.  Production should wrap backend RPC instead so
/// mutations also go through the adapter WAL/event path.
#[derive(Clone)]
pub struct OrchestratorRpc {
    pub orchestrator: Orchestrator,
}

#[async_trait]
impl CoordinationRpc for OrchestratorRpc {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.orchestrator
            .rpc(method, params)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Browser operations are kept behind a bridge so a tool can never select a
/// session or tab belonging to another Bot.  The implementation receives the
/// run identity and must enforce the Bot/assignment mapping.
#[async_trait]
pub trait BrowserToolBridge: Send + Sync {
    async fn call(
        &self,
        identity: &CollaborationIdentity,
        operation: &str,
        args: Value,
    ) -> Result<Value, String>;
}

/// Web access bridge.  A production implementation applies the configured
/// provider, timeout, response-size, and credential policy.
#[async_trait]
pub trait WebToolBridge: Send + Sync {
    async fn call(&self, operation: &str, args: Value) -> Result<Value, String>;
}

/// Runtime handoff for a bounded child execution.  The coordination RPC
/// creates the durable subagent handle first; the runtime then consumes this
/// request with an independent model/context and a child tool allowlist.
#[derive(Clone, Debug)]
pub struct SubagentDispatchRequest {
    pub subagent_id: String,
    pub assignment_id: String,
    pub parent_run_id: String,
    pub bot_id: String,
    pub chat_id: String,
    pub task: String,
    pub max_turns: u32,
}

#[async_trait]
pub trait SubagentDispatchBridge: Send + Sync {
    async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<Value, String>;
}

/// Production coordination bridge.  RPC mutations therefore use the same
/// write lock, operation WAL, snapshot, event sequence, and idempotency map as
/// client RPCs.
#[derive(Clone)]
pub struct ProductionCoordinationRpc {
    backend: Arc<ProductionBackend>,
    state: GatewayState,
    runtime: Option<RuntimeExecution>,
    assignment_id: Option<String>,
}

impl ProductionCoordinationRpc {
    pub fn new(backend: Arc<ProductionBackend>, state: GatewayState) -> Self {
        Self {
            backend,
            state,
            runtime: None,
            assignment_id: None,
        }
    }

    pub fn with_runtime(
        mut self,
        runtime: RuntimeExecution,
        assignment_id: Option<String>,
    ) -> Self {
        self.runtime = Some(runtime);
        self.assignment_id = assignment_id;
        self
    }
}

#[async_trait]
impl CoordinationRpc for ProductionCoordinationRpc {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let result = if method == "project.confirm_done" {
            if let Some(runtime) = &self.runtime {
                runtime
                    .confirm_project_for_coordination(&params, self.assignment_id.as_deref())
                    .await
            } else {
                self.backend.call(method, params, &self.state).await
            }
        } else if method == "takeover.request" {
            // This model-only action is deliberately not a public protocol
            // method. Keep it on the typed execution bridge so a client
            // cannot manufacture a takeover request for an arbitrary Bot.
            self.backend
                .execution_takeover_request(&self.state, params)
                .await
        } else {
            self.backend.call(method, params, &self.state).await
        };
        result.map_err(|error| error.to_string())
    }
}

/// Browser bridge with the security boundary at the Bot and assignment IDs.
/// Every operation selects the assignment-owned tab before invoking the CLI;
/// a model cannot name an arbitrary Bot session.
#[derive(Clone)]
pub struct ProductionBrowserBridge {
    state: GatewayState,
}

impl ProductionBrowserBridge {
    pub fn new(state: GatewayState) -> Self {
        Self { state }
    }

    /// Configure a Bot from the live orchestrator and settings snapshots.
    /// Both execution and screen connections use this entry point so a Bot
    /// that has not run since a restart still gets the same persisted session
    /// path and browser mode as a newly dispatched request.
    pub async fn configure_bot_from_snapshots(
        &self,
        bot_id: &str,
        orchestrator: &Value,
        settings: &Value,
    ) -> Result<(), String> {
        let mode = match orchestrator
            .get("bots")
            .and_then(Value::as_object)
            .and_then(|bots| bots.get(bot_id))
            .and_then(|bot| bot.get("browser_mode"))
            .and_then(Value::as_str)
            .unwrap_or("headless")
        {
            "attach" => macbot_protocol::BrowserMode::Attach,
            "headless_profile" => macbot_protocol::BrowserMode::HeadlessProfile,
            _ => macbot_protocol::BrowserMode::Headless,
        };
        let chrome_profile = settings
            .pointer("/browser/chrome_profile")
            .and_then(Value::as_str);
        self.configure_bot(bot_id, mode, chrome_profile).await
    }

    /// Apply the persisted Bot browser settings without ever handing the
    /// user's live Chrome profile to agent-browser.  The BrowserManager makes
    /// an app-owned copy on first use and keeps assignment state under the
    /// service home directory.
    pub async fn configure_bot(
        &self,
        bot_id: &str,
        mode: macbot_protocol::BrowserMode,
        chrome_profile: Option<&str>,
    ) -> Result<(), String> {
        let mode = match mode {
            macbot_protocol::BrowserMode::Headless => BrowserMode::Headless,
            macbot_protocol::BrowserMode::HeadlessProfile => BrowserMode::HeadlessProfile,
            macbot_protocol::BrowserMode::Attach => BrowserMode::Attach,
        };
        let mut config = SessionConfig {
            mode,
            chrome_profile: chrome_profile
                .filter(|profile| !profile.is_empty())
                .map(str::to_owned),
            isolated_profile_root: Some(self.state.home.join("browser/profiles")),
            state_path: Some(
                self.state
                    .home
                    .join("browser/sessions")
                    .join(format!("{}.json", safe_path_component(bot_id))),
            ),
            ..SessionConfig::default()
        };
        // Headless mode must not accidentally inherit a profile path.
        if matches!(config.mode, BrowserMode::Headless) {
            config.chrome_profile = None;
            config.profile_source = None;
            config.isolated_profile_root = None;
        }
        self.state
            .browser
            .lock()
            .await
            .set_bot_config(bot_id, config)
            .map_err(browser_message)
    }
}

fn safe_path_component(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty() {
        output.push_str("bot");
    }
    output
}

#[async_trait]
impl BrowserToolBridge for ProductionBrowserBridge {
    async fn call(
        &self,
        identity: &CollaborationIdentity,
        operation: &str,
        args: Value,
    ) -> Result<Value, String> {
        let bot_id = identity.bot_id.as_str();
        let mut browser = self.state.browser.lock().await;
        let assignment_id = browser_scope(identity, &args)?;
        if operation == "browser_open" {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser_open.url is required".to_owned())?;
            let tab = browser
                .open_tab(bot_id, &assignment_id, url)
                .map_err(browser_message)?;
            return serde_json::to_value(tab).map_err(|error| error.to_string());
        }
        if operation == "browser_tabs" {
            let tabs = browser
                .tabs_for_assignment(bot_id, &assignment_id)
                .map_err(browser_message)?;
            let driver = browser
                .driver_for_assignment(bot_id, Some(&assignment_id))
                .map_err(browser_message)?;
            return Ok(json!({"tabs":tabs,"driver":driver}));
        }
        if operation == "browser_stream" {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser_stream.action is required".to_owned())?;
            return match action {
                "enable" => browser
                    .stream_enable_for_assignment(
                        bot_id,
                        &assignment_id,
                        args.get("port")
                            .and_then(Value::as_u64)
                            .map(|port| port as u16),
                    )
                    .map_err(browser_message),
                "disable" => browser
                    .stream_disable_for_assignment(bot_id, &assignment_id)
                    .map_err(browser_message),
                "status" => browser
                    .stream_status_for_assignment(bot_id, &assignment_id)
                    .map_err(browser_message),
                _ => Err("browser_stream.action must be enable, disable, or status".into()),
            };
        }
        if operation == "browser_screenshot" {
            let tab_id = tab_id(&browser, bot_id, &assignment_id, &args)?;
            let jpeg = browser
                .screenshot(bot_id, &tab_id)
                .map_err(browser_message)?;
            return Ok(json!({
                "tab_id": tab_id,
                "mime": "image/jpeg",
                "data": base64::engine::general_purpose::STANDARD.encode(jpeg),
                "driver": browser.driver_for_assignment(bot_id, Some(&assignment_id)).map_err(browser_message)?,
            }));
        }
        let (command, command_args) = browser_command(operation, &args)?;
        let result = browser
            .enqueue(
                bot_id,
                BrowserAction {
                    assignment_id,
                    operation: command,
                    args: command_args,
                },
            )
            .map_err(browser_message)?;
        Ok(result.unwrap_or_else(|| json!({"queued":true})))
    }
}

/// A private Bot chat can run browser tools before an assignment exists. Keep
/// that run in a stable, server-owned scope derived from the chat, and reject
/// caller-supplied arbitrary assignment ids. Assignment runs remain pinned to
/// their injected assignment id.
fn browser_scope(identity: &CollaborationIdentity, args: &Value) -> Result<String, String> {
    let scope = identity
        .assignment_id
        .clone()
        .unwrap_or_else(|| format!("dm_{}", safe_path_component(&identity.chat_id)));
    if let Some(requested) = args.get("assignment_id").and_then(Value::as_str) {
        if requested != scope {
            return Err("browser assignment does not belong to this run".into());
        }
    }
    Ok(scope)
}

fn browser_message(error: BrowserError) -> String {
    error.to_string()
}

fn tab_id<R: macbot_browser::CliRunner>(
    browser: &macbot_browser::BrowserManager<R>,
    bot_id: &str,
    assignment_id: &str,
    args: &Value,
) -> Result<String, String> {
    browser
        .tab_for_assignment(
            bot_id,
            assignment_id,
            args.get("tab_id").and_then(Value::as_str),
        )
        .map(|tab| tab.tab_id)
        .map_err(browser_message)
}

fn browser_command(operation: &str, args: &Value) -> Result<(String, Vec<String>), String> {
    if operation == "browser_nav" {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("open");
        return match action {
            "reload" | "back" | "forward" => {
                if args.get("url").is_some() || args.get("args").is_some() {
                    return Err(format!("browser_nav.{action} takes no url or args"));
                }
                Ok((action.to_owned(), Vec::new()))
            }
            "open" => {
                let list = args.get("args").and_then(Value::as_array);
                let url = args.get("url").and_then(Value::as_str);
                let target = match (url, list) {
                    (Some(url), None) => Some(url),
                    (None, Some(items)) if items.len() == 1 => items[0].as_str(),
                    _ => None,
                }
                .filter(|url| !url.trim().is_empty());
                let target =
                    target.ok_or("browser_nav.open requires one url or one URL in args")?;
                Ok(("open".into(), vec![target.to_owned()]))
            }
            _ => Err("browser_nav.action must be open, reload, back or forward".into()),
        };
    }
    let list = match args.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(value) => Ok(value.clone()),
                Value::Number(_) | Value::Bool(_) => Ok(item.to_string()),
                _ => Err("browser args must contain only strings, numbers or booleans".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("browser args must be an array".into()),
    };
    let command = match operation {
        "browser_snapshot" => "snapshot",
        "browser_act" => args
            .get("action")
            .and_then(Value::as_str)
            .ok_or("browser_act.action is required")?,
        "browser_get" => "get",
        "browser_wait" => "wait",
        "browser_eval" => "eval",
        _ => return Err(format!("unsupported browser operation {operation}")),
    };
    let mut command_args = list;
    if command_args.is_empty() && operation == "browser_wait" {
        if let Some(selector) = args.get("selector").and_then(Value::as_str) {
            command_args.push(selector.to_owned());
        } else if let Some(duration) = args.get("timeout").and_then(Value::as_u64) {
            command_args.push(duration.to_string());
        } else {
            return Err("browser_wait requires args, selector or timeout milliseconds".into());
        }
    }
    if command_args.is_empty() && operation == "browser_get" {
        let property = args
            .get("property")
            .and_then(Value::as_str)
            .ok_or("browser_get requires args or property")?;
        command_args.push(property.to_owned());
        if let Some(selector) = args.get("selector").and_then(Value::as_str) {
            command_args.push(selector.to_owned());
        }
    }
    if command_args.is_empty() {
        for key in ["property", "selector", "url", "expression"] {
            if let Some(value) = args.get(key).and_then(Value::as_str) {
                command_args.push(value.to_owned());
                break;
            }
        }
    }
    if command == "eval" && command_args.is_empty() {
        return Err("browser_eval.expression is required".into());
    }
    Ok((command.to_owned(), command_args))
}

/// Configuration supplied by the gateway after loading settings and secrets.
#[derive(Clone, Debug, Default)]
pub struct WebSearchConfig {
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
}

/// Secret lookup is deliberately separate from JSON settings.  Production
/// wires this to the provider Keychain-backed SecretStore; the API key is
/// fetched immediately before a request and is never placed in persisted
/// settings or request headers stored in a snapshot.
pub trait WebCredentialStore: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<String>, String>;
}

pub struct ProviderWebCredentials {
    pub secrets: Arc<dyn macbot_providers::SecretStore>,
}

impl WebCredentialStore for ProviderWebCredentials {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        self.secrets
            .get(provider)
            .map_err(|error| error.to_string())
    }
}

/// Real HTTP bridge for `web_fetch` and configured `web_search`.
#[derive(Clone)]
pub struct ReqwestWebBridge {
    client: reqwest::Client,
    search: Arc<RwLock<WebSearchConfig>>,
    credentials: Option<Arc<dyn WebCredentialStore>>,
    max_bytes: usize,
}

impl ReqwestWebBridge {
    pub fn new(search: WebSearchConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .user_agent("MacBot/0.1")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            search: Arc::new(RwLock::new(search)),
            credentials: None,
            max_bytes: 2 * 1024 * 1024,
        })
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes.max(16 * 1024);
        self
    }

    pub fn with_credentials(mut self, credentials: Arc<dyn WebCredentialStore>) -> Self {
        self.credentials = Some(credentials);
        self
    }
}

#[async_trait]
impl WebToolBridge for ReqwestWebBridge {
    async fn call(&self, operation: &str, args: Value) -> Result<Value, String> {
        match operation {
            "web_fetch" => self.fetch(args).await,
            "web_search" => self.search(args).await,
            _ => Err(format!("unsupported web operation {operation}")),
        }
    }
}

impl ReqwestWebBridge {
    async fn fetch(&self, args: Value) -> Result<Value, String> {
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "web_fetch.url is required".to_owned())?;
        let parsed = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("web_fetch only supports http(s) URLs".into());
        }
        let response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("web_fetch returned HTTP {status}"));
        }
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        let truncated = bytes.len() > self.max_bytes;
        let body = String::from_utf8_lossy(&bytes[..bytes.len().min(self.max_bytes)]);
        let markdown = html_to_markdown(&body);
        Ok(json!({"url":url,"markdown":markdown,"truncated":truncated}))
    }

    async fn search(&self, args: Value) -> Result<Value, String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| "web_search.query is required".to_owned())?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 20);
        let config = self.search.read().await.clone();
        let endpoint = config
            .endpoint
            .ok_or_else(|| "web search is not configured".to_owned())?;
        let provider = config.provider.as_deref().unwrap_or("searxng");
        let api_key = if let Some(credentials) = &self.credentials {
            credentials.get(provider)?
        } else {
            config.api_key.clone()
        };
        let limit_text = limit.to_string();
        let mut request = if provider == "tavily" {
            let mut body = json!({"query":query,"max_results":limit});
            if let Some(api_key) = api_key.clone() {
                body["api_key"] = json!(api_key);
            }
            self.client.post(endpoint).json(&body)
        } else {
            let query_key = "q";
            self.client
                .get(endpoint)
                .query(&[(query_key, query), ("limit", limit_text.as_str())])
        };
        if provider == "brave" {
            if let Some(api_key) = api_key.clone() {
                request = request.header("X-Subscription-Token", api_key);
            }
        } else if provider != "tavily" {
            if let Some(api_key) = api_key {
                request = request.bearer_auth(api_key);
            }
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("web_search returned HTTP {status}"));
        }
        let value: Value = response.json().await.map_err(|error| error.to_string())?;
        Ok(json!({"query":query,"results":search_results(&value, limit as usize)}))
    }
}

fn search_results(value: &Value, limit: usize) -> Vec<Value> {
    let candidates = value
        .pointer("/web/results")
        .or_else(|| value.get("results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    candidates
        .into_iter()
        .take(limit)
        .map(|item| {
            json!({
                "title": item.get("title").cloned().unwrap_or(Value::Null),
                "url": item.get("url").or_else(|| item.get("link")).cloned().unwrap_or(Value::Null),
                "snippet": item.get("description").or_else(|| item.get("snippet")).or_else(|| item.get("content")).cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn html_to_markdown(input: &str) -> String {
    let mut output = String::with_capacity(input.len().min(64 * 1024));
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in input.chars() {
        if ch == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "p" | "div" | "br" | "li" | "h1" | "h2" | "h3" | "tr"
                ) {
                    output.push('\n');
                }
            } else {
                tag.push(ch);
            }
            continue;
        }
        output.push(ch);
    }
    decode_entities(&output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Tool factory used by gateway execution setup.
#[derive(Clone)]
pub struct CollaborationTools {
    rpc: Arc<dyn CoordinationRpc>,
    identity: CollaborationIdentity,
    browser: Option<Arc<dyn BrowserToolBridge>>,
    web: Option<Arc<dyn WebToolBridge>>,
    subagent: Option<Arc<dyn SubagentDispatchBridge>>,
}

impl CollaborationTools {
    pub fn new(
        rpc: Arc<dyn CoordinationRpc>,
        identity: CollaborationIdentity,
        browser: Option<Arc<dyn BrowserToolBridge>>,
        web: Option<Arc<dyn WebToolBridge>>,
    ) -> Self {
        Self {
            rpc,
            identity,
            browser,
            web,
            subagent: None,
        }
    }

    pub fn with_subagent_dispatch(mut self, bridge: Arc<dyn SubagentDispatchBridge>) -> Self {
        self.subagent = Some(bridge);
        self
    }

    /// Return only tools allowed for this run.  `send_msg` is intentionally
    /// omitted: ExecutionEngine injects its built-in durable run_id+call_id
    /// implementation and forwards the admitted message to the group bridge.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut names = vec!["web_fetch", "web_search", "routine", "question"];
        if self.identity.is_main {
            names.extend([
                "list_bots",
                "create_project",
                "project_create",
                "assign",
                "delegate",
                "project_status",
                "get_status",
                "request_review",
                "finish_project",
                "propose_bot",
                "notify_user",
                "remind",
            ]);
        } else {
            names.extend([
                "browser_open",
                "browser_snapshot",
                "browser_act",
                "browser_get",
                "browser_wait",
                "browser_screenshot",
                "browser_tabs",
                "browser_nav",
                "browser_eval",
                "browser_stream",
                "request_takeover",
                "subagent",
            ]);
        }
        names
            .into_iter()
            .map(|name| {
                Arc::new(CollaborationTool {
                    name: name.to_owned(),
                    factory: self.clone(),
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}

struct CollaborationTool {
    name: String,
    factory: CollaborationTools,
}

#[async_trait]
impl Tool for CollaborationTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        description(&self.name)
    }

    fn schema(&self) -> Value {
        schema(&self.name)
    }

    fn risk(&self, args: &Value) -> Risk {
        risk(&self.name, args)
    }

    async fn call(&self, context: &ToolContext, args: Value) -> ToolResult {
        self.factory.invoke(&self.name, context, args).await
    }
}

impl CollaborationTools {
    async fn invoke(&self, name: &str, context: &ToolContext, args: Value) -> ToolResult {
        let args = match object(args) {
            Ok(args) => args,
            Err(error) => return ToolResult::error(error),
        };
        let result = match name {
            "list_bots" => self.rpc.call("bot.list", json!({})).await,
            "create_project" | "project_create" => {
                if let Err(error) = require_nonempty_string_array(&args, "member_bot_ids") {
                    return ToolResult::error(error);
                }
                if let Err(error) = require_nonempty_string_array(&args, "flow") {
                    return ToolResult::error(error);
                }
                self.rpc.call("project.create", Value::Object(args)).await
            }
            "assign" => {
                self.rpc
                    .call("assignment.create", self.assignment_args(args))
                    .await
            }
            "delegate" => self.rpc.call("delegate", self.delegate_args(args)).await,
            "project_status" | "get_status" => self.rpc.call("project.status", args.into()).await,
            "request_review" => {
                let mut args = args;
                if let Some(project_id) = &self.identity.project_id {
                    args.entry("project_id")
                        .or_insert_with(|| json!(project_id));
                }
                self.rpc.call("project.request_review", args.into()).await
            }
            "finish_project" => self.finish_project(args).await,
            "request_takeover" => self.request_takeover(args).await,
            "propose_bot" => {
                let mut args = args;
                args.entry("bot_id")
                    .or_insert_with(|| json!(self.identity.bot_id));
                args.entry("chat_id")
                    .or_insert_with(|| json!(self.identity.chat_id));
                self.rpc.call("propose_bot", args.into()).await
            }
            "notify_user" | "remind" => self.rpc.call("send_msg", self.notify_args(args)).await,
            "routine" => self.invoke_routine(args).await,
            "subagent" => self.invoke_subagent(context, args).await,
            "browser_open" | "browser_snapshot" | "browser_act" | "browser_get"
            | "browser_wait" | "browser_screenshot" | "browser_tabs" | "browser_nav"
            | "browser_eval" | "browser_stream" => {
                let Some(browser) = &self.browser else {
                    return ToolResult::error("browser bridge is unavailable");
                };
                browser.call(&self.identity, name, args.into()).await
            }
            "web_fetch" | "web_search" => {
                let Some(web) = &self.web else {
                    return ToolResult::error("web bridge is unavailable");
                };
                web.call(name, args.into()).await
            }
            _ => Err(format!("unknown collaboration tool {name}")),
        };
        match result {
            Ok(value) => tool_value(value),
            Err(error) => ToolResult::error(error),
        }
    }

    async fn finish_project(&self, mut args: Map<String, Value>) -> Result<Value, String> {
        let project_id = args
            .get("project_id")
            .and_then(Value::as_str)
            .or(self.identity.project_id.as_deref())
            .ok_or_else(|| "project_id is required".to_owned())?;
        let current = self
            .rpc
            .call("project.status", json!({"project_id":project_id}))
            .await?;
        if current["project"]["status"].as_str() != Some("review") {
            return Err("project can be finished only after user confirmation".into());
        }
        args.insert("project_id".into(), json!(project_id));
        self.rpc
            .call("project.confirm_done", Value::Object(args))
            .await
    }

    async fn request_takeover(&self, args: Map<String, Value>) -> Result<Value, String> {
        let scope = self
            .identity
            .assignment_id
            .clone()
            .unwrap_or_else(|| format!("dm_{}", safe_path_component(&self.identity.chat_id)));
        let reason = args
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("用户接管浏览器")
            .to_owned();
        let result = self
            .rpc
            .call(
                "takeover.request",
                json!({
                    "bot_id": self.identity.bot_id,
                    "chat_id": self.identity.chat_id,
                    "assignment_id": scope,
                    "reason": reason,
                }),
            )
            .await?;
        let mut result = object(result)?;
        result.insert(
            "wait".into(),
            json!({"reason":"takeover","message_id":null}),
        );
        Ok(Value::Object(result))
    }

    fn assignment_args(&self, mut args: Map<String, Value>) -> Value {
        args.entry("origin_chat_id")
            .or_insert_with(|| json!(self.identity.chat_id));
        args.entry("from")
            .or_insert_with(|| json!(if self.identity.is_main { "main" } else { "bot" }));
        if let Some(project_id) = &self.identity.project_id {
            args.entry("project_id")
                .or_insert_with(|| json!(project_id));
        }
        Value::Object(args)
    }

    fn delegate_args(&self, mut args: Map<String, Value>) -> Value {
        args.entry("origin_chat_id")
            .or_insert_with(|| json!(self.identity.chat_id));
        Value::Object(args)
    }

    fn notify_args(&self, mut args: Map<String, Value>) -> Value {
        args.entry("bot_id")
            .or_insert_with(|| json!(self.identity.bot_id));
        args.entry("chat_id")
            .or_insert_with(|| json!(self.identity.chat_id));
        args.entry("intent").or_insert_with(|| json!("ack"));
        Value::Object(args)
    }

    async fn invoke_routine(&self, mut args: Map<String, Value>) -> Result<Value, String> {
        let action = args
            .remove("action")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| "routine.action is required".to_owned())?;
        let method = match action.as_str() {
            "list" => "routine.list",
            "create" => "routine.create",
            "update" => "routine.update",
            "delete" => "routine.delete",
            "enable" | "set_enabled" => "routine.set_enabled",
            _ => {
                return Err("routine.action must be list, create, update, delete, or enable".into())
            }
        };
        if !self.identity.is_main {
            args.entry("bot_id")
                .or_insert_with(|| json!(self.identity.bot_id));
        }
        self.rpc.call(method, Value::Object(args)).await
    }

    async fn invoke_subagent(
        &self,
        context: &ToolContext,
        mut args: Map<String, Value>,
    ) -> Result<Value, String> {
        let action = args
            .remove("action")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| "subagent.action is required".to_owned())?;
        args.entry("assignment_id")
            .or_insert_with(|| json!(self.identity.assignment_id));
        args.entry("parent_run_id")
            .or_insert_with(|| json!(context.run_id));
        args.entry("bot_id")
            .or_insert_with(|| json!(self.identity.bot_id));
        args.entry("chat_id")
            .or_insert_with(|| json!(self.identity.chat_id));
        let method = match action.as_str() {
            "start" => "subagent.start",
            "finish" => "subagent.finish",
            _ => return Err("subagent.action must be start or finish".into()),
        };
        let result = self.rpc.call(method, Value::Object(args.clone())).await?;
        if action == "start" {
            if let Some(dispatch) = &self.subagent {
                let subagent_id = result
                    .get("id")
                    .or_else(|| result.get("subagent_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "subagent.start did not return an id".to_owned())?;
                let assignment_id = args
                    .get("assignment_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "subagent assignment_id is required".to_owned())?;
                let task = args
                    .get("task")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "subagent task is required".to_owned())?;
                let child = dispatch
                    .dispatch(SubagentDispatchRequest {
                        subagent_id: subagent_id.to_owned(),
                        assignment_id: assignment_id.to_owned(),
                        parent_run_id: context.run_id.clone(),
                        bot_id: self.identity.bot_id.clone(),
                        chat_id: self.identity.chat_id.clone(),
                        task: task.to_owned(),
                        max_turns: args
                            .get("max_turns")
                            .and_then(Value::as_u64)
                            .unwrap_or(8)
                            .min(32) as u32,
                    })
                    .await?;
                return Ok(json!({"subagent":result,"dispatch":child}));
            }
        }
        Ok(result)
    }
}

fn object(value: Value) -> Result<Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "tool arguments must be a JSON object".into())
}

fn require_nonempty_string_array(args: &Map<String, Value>, key: &str) -> Result<(), String> {
    let values = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} must be a non-empty array of strings"))?;
    if values.is_empty()
        || values
            .iter()
            .any(|value| value.as_str().is_none_or(|value| value.trim().is_empty()))
    {
        return Err(format!("{key} must be a non-empty array of strings"));
    }
    Ok(())
}

fn tool_value(value: Value) -> ToolResult {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolResult {
        content: vec![macbot_tools::Part::Text { text }],
        details: value,
        is_error: false,
    }
}

fn description(name: &str) -> &'static str {
    match name {
        "list_bots" => "List available worker Bots.",
        "create_project" | "project_create" => "Create a project group. Use member_bot_ids with Bot IDs from list_bots and a non-empty ordered flow of plan steps. After success, send a real opening message in the new project chat with send_msg and mention the first role; the task card alone is not an opening.",
        "assign" => "Assign a project task to a worker Bot.",
        "delegate" => "Delegate a small task to a worker Bot without creating a project.",
        "project_status" | "get_status" => "Read project status and announcement.",
        "request_review" => "Move a project to review and notify the user.",
        "finish_project" => "Confirm a project is complete after user approval.",
        "propose_bot" => "Propose a new Bot and wait for user approval.",
        "notify_user" | "remind" => "Send a coordination notice to the user chat.",
        "routine" => "Create, update, list, enable, or delete a routine.",
        "subagent" => "Start or finish a bounded subagent task.",
        "request_takeover" => "Request that the user take over the browser.",
        "question" | "ask_user" => "Ask the user a question and wait for an answer.",
        "web_fetch" => "Fetch a web page and return readable content.",
        "browser_nav" => "Navigate the current task tab: action open with url, or action reload/back/forward without url or args.",
        "browser_act" => "Run an agent-browser interaction command on the current task tab. Examples: action fill with args [selector, text], click with args [selector], dialog with args [accept], set with args [viewport, width, height]. Argument values may be strings, numbers or booleans; numeric viewport dimensions are preserved.",
        "browser_wait" => "Wait on the current task tab. Use timeout as milliseconds, selector to wait for an element, or args matching agent-browser wait (for example [--text, Welcome]).",
        "browser_get" => "Read the current task tab. Use property (text/html/value/title/url/count) and optional selector, or args matching agent-browser get.",
        "web_search" => "Search the configured web provider.",
        _ if name.starts_with("browser_") => "Operate the current Bot browser session.",
        _ => "Coordination tool.",
    }
}

fn schema(name: &str) -> Value {
    let object = |properties: Value, required: &[&str]| json!({"type":"object","properties":properties,"required":required,"additionalProperties":true});
    match name {
        "list_bots" => object(json!({"include_hidden":{"type":"boolean"}}), &[]),
        "create_project" | "project_create" => object(
            json!({"name":{"type":"string"},"goal":{"type":"string"},"member_bot_ids":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},"members":{"type":"array","items":{"type":"string"}},"flow":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},"deadline":{"type":"string"}}),
            &["name", "goal", "member_bot_ids", "flow"],
        ),
        "assign" => object(
            json!({"bot_id":{"type":"string"},"instruction":{"type":"string"},"title":{"type":"string"},"project_id":{"type":"string"}}),
            &["bot_id", "instruction"],
        ),
        "delegate" => object(
            json!({"bot_id":{"type":"string"},"instruction":{"type":"string"},"title":{"type":"string"}}),
            &["bot_id", "instruction"],
        ),
        "project_status" | "get_status" => {
            object(json!({"project_id":{"type":"string"}}), &["project_id"])
        }
        "request_review" | "finish_project" => object(
            json!({"project_id":{"type":"string"},"summary":{"type":"string"}}),
            &["project_id"],
        ),
        "propose_bot" => object(
            json!({"name":{"type":"string"},"label":{"type":"string"},"description":{"type":"string"}}),
            &["name"],
        ),
        "notify_user" | "remind" => object(
            json!({"text":{"type":"string"},"intent":{"type":"string"},"reply_to":{"type":"string"}}),
            &["text"],
        ),
        "routine" => object(
            json!({"action":{"type":"string","enum":["list","create","update","delete","enable"]},"routine_id":{"type":"string"},"name":{"type":"string"},"instructions":{"type":"string"},"schedules":{"type":"array"},"timezone":{"type":"string"},"patch":{"type":"object"},"enabled":{"type":"boolean"},"project_id":{"type":"string"},"bot_id":{"type":"string"}}),
            &["action"],
        ),
        "subagent" => object(
            json!({"action":{"type":"string","enum":["start","finish"]},"task":{"type":"string"},"subagent_id":{"type":"string"}}),
            &["action"],
        ),
        "web_fetch" => object(
            json!({"url":{"type":"string"},"prompt":{"type":"string"}}),
            &["url"],
        ),
        "web_search" => object(
            json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20}}),
            &["query"],
        ),
        "request_takeover" => object(json!({"reason":{"type":"string"}}), &["reason"]),
        "question" | "ask_user" => object(
            json!({"question":{"type":"string"},"prompt":{"type":"string"}}),
            &["question"],
        ),
        "browser_nav" => object(
            json!({"assignment_id":{"type":"string"},"tab_id":{"type":"string"},"url":{"type":"string"},"action":{"type":"string","enum":["open","reload","back","forward"],"description":"Defaults to open, which requires url. reload/back/forward operate on the current task tab and take no url or args."},"args":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":1}}),
            &[],
        ),
        "browser_wait" => object(
            json!({"assignment_id":{"type":"string"},"tab_id":{"type":"string"},"selector":{"type":"string"},"timeout":{"type":"integer","minimum":0,"description":"Milliseconds to wait when args and selector are absent."},"args":{"type":"array","items":{"type":"string"}}}),
            &[],
        ),
        "browser_get" => object(
            json!({"assignment_id":{"type":"string"},"tab_id":{"type":"string"},"property":{"type":"string"},"selector":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}}),
            &[],
        ),
        _ if name.starts_with("browser_") => object(
            json!({"assignment_id":{"type":"string"},"tab_id":{"type":"string"},"url":{"type":"string"},"action":{"type":"string"},"port":{"type":"integer"},"args":{"type":"array","items":{"type":["string","number","boolean"]}},"expression":{"type":"string"},"timeout":{"type":"integer"}}),
            &[],
        ),
        _ => object(json!({}), &[]),
    }
}

fn risk(name: &str, args: &Value) -> Risk {
    match name {
        "list_bots" | "project_status" | "get_status" => Risk::Read,
        "web_fetch" | "web_search" | "browser_snapshot" | "browser_get" | "browser_wait"
        | "browser_tabs" | "browser_screenshot" | "browser_stream" => Risk::External,
        "browser_open" | "browser_act" | "browser_nav" | "browser_eval" | "request_takeover"
        | "question" | "ask_user" => Risk::External,
        "routine" => match args.get("action").and_then(Value::as_str) {
            Some("list") => Risk::Read,
            _ => Risk::Write,
        },
        "subagent" => Risk::Write,
        _ => Risk::Write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRpc(Mutex<Vec<(String, Value)>>);
    #[async_trait]
    impl CoordinationRpc for FakeRpc {
        async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            self.0.lock().unwrap().push((method.into(), params));
            Ok(json!({"ok":true}))
        }
    }

    #[tokio::test]
    async fn role_tool_sets_keep_main_and_worker_capabilities_separate() {
        let rpc = Arc::new(FakeRpc::default());
        let main = CollaborationTools::new(
            rpc.clone(),
            CollaborationIdentity {
                is_main: true,
                ..Default::default()
            },
            None,
            None,
        );
        let worker = CollaborationTools::new(
            rpc,
            CollaborationIdentity {
                bot_id: "bot-1".into(),
                is_main: false,
                ..Default::default()
            },
            None,
            None,
        );
        let main_names: Vec<_> = main
            .tools()
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        let worker_names: Vec<_> = worker
            .tools()
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        assert!(main_names.iter().any(|name| name == "create_project"));
        assert!(!main_names.iter().any(|name| name == "browser_open"));
        assert!(!main_names.iter().any(|name| name == "subagent"));
        assert!(worker_names.iter().any(|name| name == "browser_open"));
        assert!(!worker_names.iter().any(|name| name == "create_project"));
        assert!(worker_names.iter().any(|name| name == "subagent"));
        assert!(!worker_names.iter().any(|name| name == "send_msg"));
    }

    #[test]
    fn project_creation_guides_real_group_opening() {
        let text = description("create_project");
        assert!(text.contains("real opening message"));
        assert!(text.contains("mention the first role"));
        assert!(text.contains("send_msg"));
        assert_eq!(
            schema("notify_user").pointer("/properties/reply_to/type"),
            Some(&json!("string"))
        );
    }

    #[tokio::test]
    async fn assign_and_routine_inject_run_identity() {
        let rpc = Arc::new(FakeRpc::default());
        let factory = CollaborationTools::new(
            rpc.clone(),
            CollaborationIdentity {
                bot_id: "bot-1".into(),
                chat_id: "chat-1".into(),
                assignment_id: Some("asg-1".into()),
                project_id: Some("project-1".into()),
                is_main: false,
            },
            None,
            None,
        );
        let context = ToolContext::new("/tmp", "run-1", "/tmp/runs");
        factory
            .invoke(
                "assign",
                &context,
                json!({"bot_id":"bot-2","instruction":"help"}),
            )
            .await;
        factory
            .invoke("routine", &context, json!({"action":"list"}))
            .await;
        factory
            .invoke(
                "subagent",
                &context,
                json!({"action":"start","task":"inspect"}),
            )
            .await;
        let calls = rpc.0.lock().unwrap();
        assert_eq!(calls[0].0, "assignment.create");
        assert_eq!(calls[0].1["origin_chat_id"], "chat-1");
        assert_eq!(calls[0].1["project_id"], "project-1");
        assert_eq!(calls[1].1["bot_id"], "bot-1");
        assert_eq!(calls[2].1["assignment_id"], "asg-1");
    }

    #[tokio::test]
    async fn create_project_requires_bot_ids_and_ordered_flow() {
        let rpc = Arc::new(FakeRpc::default());
        let factory = CollaborationTools::new(
            rpc.clone(),
            CollaborationIdentity {
                bot_id: "main".into(),
                chat_id: "chat_main".into(),
                is_main: true,
                ..Default::default()
            },
            None,
            None,
        );
        let context = ToolContext::new("/tmp", "run-1", "/tmp/runs");

        let missing = factory
            .invoke(
                "create_project",
                &context,
                json!({"name":"x","goal":"y","flow":["编码"]}),
            )
            .await;
        assert!(missing.is_error);

        let empty_flow = factory
            .invoke(
                "create_project",
                &context,
                json!({"name":"x","goal":"y","member_bot_ids":["bot-1"],"flow":[]}),
            )
            .await;
        assert!(empty_flow.is_error);

        let valid = factory
            .invoke(
                "create_project",
                &context,
                json!({"name":"x","goal":"y","member_bot_ids":["bot-1"],"flow":["调研","实现"]}),
            )
            .await;
        assert!(!valid.is_error);
        let calls = rpc.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "project.create");
        assert_eq!(calls[0].1["member_bot_ids"], json!(["bot-1"]));
        assert_eq!(calls[0].1["flow"], json!(["调研", "实现"]));
    }

    #[tokio::test]
    async fn browser_stream_is_dispatched_through_the_assignment_bridge() {
        #[derive(Default)]
        struct FakeBrowser(Mutex<Vec<(String, Value)>>);
        #[async_trait]
        impl BrowserToolBridge for FakeBrowser {
            async fn call(
                &self,
                _identity: &CollaborationIdentity,
                operation: &str,
                args: Value,
            ) -> Result<Value, String> {
                self.0.lock().unwrap().push((operation.to_owned(), args));
                Ok(json!({"enabled":true,"port":19123}))
            }
        }
        let bridge = Arc::new(FakeBrowser::default());
        let factory = CollaborationTools::new(
            Arc::new(FakeRpc::default()),
            CollaborationIdentity {
                bot_id: "bot-1".into(),
                assignment_id: Some("asg-1".into()),
                is_main: false,
                ..Default::default()
            },
            Some(bridge.clone()),
            None,
        );
        let result = factory
            .invoke(
                "browser_stream",
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"action":"enable","port":19123}),
            )
            .await;
        assert!(!result.is_error);
        let calls = bridge.0.lock().unwrap();
        assert_eq!(calls[0].0, "browser_stream");
        assert_eq!(calls[0].1["action"], "enable");
    }

    #[tokio::test]
    async fn takeover_requests_are_pending_and_private_browser_scope_is_stable() {
        let rpc = Arc::new(FakeRpc::default());
        let factory = CollaborationTools::new(
            rpc.clone(),
            CollaborationIdentity {
                bot_id: "bot-1".into(),
                chat_id: "dm_bot-1".into(),
                is_main: false,
                ..Default::default()
            },
            None,
            None,
        );
        let result = factory
            .invoke(
                "request_takeover",
                &ToolContext::new("/tmp", "run-1", "/tmp/runs"),
                json!({"reason":"登录需要用户操作"}),
            )
            .await;
        assert!(!result.is_error);
        assert_eq!(result.details["wait"]["reason"], "takeover");
        let calls = rpc.0.lock().unwrap();
        assert_eq!(calls[0].0, "takeover.request");
        assert_eq!(calls[0].1["assignment_id"], "dm_dm_bot-1");

        let identity = CollaborationIdentity {
            bot_id: "bot-1".into(),
            chat_id: "dm_bot-1".into(),
            is_main: false,
            ..Default::default()
        };
        assert_eq!(browser_scope(&identity, &json!({})).unwrap(), "dm_dm_bot-1");
        assert!(browser_scope(&identity, &json!({"assignment_id":"other"})).is_err());
    }

    #[test]
    fn schemas_and_risk_are_stable() {
        assert_eq!(risk("list_bots", &json!({})), Risk::Read);
        assert_eq!(risk("browser_act", &json!({})), Risk::External);
        assert_eq!(risk("routine", &json!({"action":"list"})), Risk::Read);
        let project_schema = schema("create_project");
        assert!(project_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "name"));
        assert!(project_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "member_bot_ids"));
        assert!(project_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "flow"));
        assert_eq!(
            project_schema["properties"]["member_bot_ids"]["minItems"],
            1
        );
        assert_eq!(project_schema["properties"]["flow"]["minItems"], 1);
    }

    #[test]
    fn browser_commands_are_scoped_to_supported_agent_browser_operations() {
        assert_eq!(
            browser_command("browser_nav", &json!({"url":"https://example.com"})).unwrap(),
            ("open".into(), vec!["https://example.com".to_owned()])
        );
        assert!(browser_command("browser_eval", &json!({})).is_err());
        assert!(browser_command("browser_act", &json!({"action":"click","args":["#go"]})).is_ok());
        assert_eq!(
            browser_command(
                "browser_act",
                &json!({"action":"set","args":["viewport",360,800]})
            )
            .unwrap(),
            (
                "set".into(),
                vec!["viewport".into(), "360".into(), "800".into()]
            )
        );
        assert_eq!(
            browser_command(
                "browser_act",
                &json!({"action":"fill","args":["#input", ""]})
            )
            .unwrap(),
            ("fill".into(), vec!["#input".into(), "".into()])
        );
        for args in [
            json!(["viewport", null, 800]),
            json!(["#input", {}]),
            json!([[]]),
            json!("#input"),
        ] {
            assert!(browser_command("browser_act", &json!({"action":"set","args":args})).is_err());
        }

        for action in ["reload", "back", "forward"] {
            assert_eq!(
                browser_command("browser_nav", &json!({"action":action,"tab_id":"t2"})).unwrap(),
                (action.into(), vec![])
            );
            assert!(browser_command(
                "browser_nav",
                &json!({"action":action,"url":"https://example.com"})
            )
            .is_err());
        }
        assert_eq!(
            browser_command("browser_nav", &json!({"args":["https://example.com"]})).unwrap(),
            ("open".into(), vec!["https://example.com".into()])
        );
        for invalid in [
            json!({}),
            json!({"url":""}),
            json!({"action":"close"}),
            json!({"args":[42]}),
            json!({"action":"reload","args":[]}),
        ] {
            assert!(browser_command("browser_nav", &invalid).is_err());
        }
        assert_eq!(
            browser_command("browser_wait", &json!({"timeout":1000})).unwrap(),
            ("wait".into(), vec!["1000".into()])
        );
        assert_eq!(
            browser_command("browser_wait", &json!({"args":["--text","Welcome"]})).unwrap(),
            ("wait".into(), vec!["--text".into(), "Welcome".into()])
        );
        assert!(browser_command("browser_wait", &json!({"timeout":-1})).is_err());
        assert_eq!(
            browser_command(
                "browser_get",
                &json!({"property":"text","selector":"#error"})
            )
            .unwrap(),
            ("get".into(), vec!["text".into(), "#error".into()])
        );
    }

    #[test]
    fn web_fetch_strips_markup_and_search_normalizes_provider_shapes() {
        assert_eq!(
            html_to_markdown("<h1>Hello</h1><p>A &amp; B</p>"),
            "Hello\nA & B"
        );
        let results = search_results(
            &json!({"web":{"results":[{"title":"One","url":"https://one","description":"first"}]}}),
            10,
        );
        assert_eq!(results[0]["title"], "One");
        assert_eq!(results[0]["snippet"], "first");
    }

    #[tokio::test]
    async fn subagent_dispatch_receives_independent_context_metadata() {
        #[derive(Default)]
        struct Dispatch(Mutex<Vec<SubagentDispatchRequest>>);
        #[async_trait]
        impl SubagentDispatchBridge for Dispatch {
            async fn dispatch(&self, request: SubagentDispatchRequest) -> Result<Value, String> {
                self.0.lock().unwrap().push(request);
                Ok(json!({"run_id":"child-run"}))
            }
        }
        struct StartRpc;
        #[async_trait]
        impl CoordinationRpc for StartRpc {
            async fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
                if method == "subagent.start" {
                    Ok(json!({"id":"sub-1"}))
                } else {
                    Ok(json!({}))
                }
            }
        }
        let dispatch = Arc::new(Dispatch::default());
        let factory = CollaborationTools::new(
            Arc::new(StartRpc),
            CollaborationIdentity {
                bot_id: "bot-1".into(),
                chat_id: "chat-1".into(),
                assignment_id: Some("assignment-1".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .with_subagent_dispatch(dispatch.clone());
        let context = ToolContext::new("/tmp", "parent-run", "/tmp/runs");
        let result = factory
            .invoke(
                "subagent",
                &context,
                json!({"action":"start","task":"inspect","max_turns":4}),
            )
            .await;
        assert!(!result.is_error);
        let calls = dispatch.0.lock().unwrap();
        assert_eq!(calls[0].subagent_id, "sub-1");
        assert_eq!(calls[0].parent_run_id, "parent-run");
        assert_eq!(calls[0].max_turns, 4);
    }
}
