//! Provider adapters. Credentials are resolved only at the HTTP boundary.
pub mod registry;
pub mod secrets;
pub use secrets::{configured_secret_store, FileSecrets};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider response error: {0}")]
    Response(String),
    #[error("credential storage error: {0}")]
    Secret(String),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApiKind {
    #[serde(rename = "openai-completions")]
    OpenaiCompletions,
    #[serde(rename = "openai-responses")]
    OpenaiResponses,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    #[serde(rename = "google-generative")]
    GoogleGenerative,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub api_kind: ApiKind,
    pub base_url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}
pub trait SecretStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<String>>;
    fn set(&self, provider_id: &str, key: &str) -> Result<()>;
    fn delete(&self, provider_id: &str) -> Result<()>;
}
#[derive(Default)]
pub struct MemorySecrets(Mutex<BTreeMap<String, String>>);
impl SecretStore for MemorySecrets {
    fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn set(&self, id: &str, key: &str) -> Result<()> {
        self.0.lock().unwrap().insert(id.to_owned(), key.to_owned());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<()> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}
pub struct KeychainSecrets;
#[cfg(target_os = "macos")]
impl SecretStore for KeychainSecrets {
    fn get(&self, id: &str) -> Result<Option<String>> {
        match security_framework::passwords::get_generic_password("bot.mac.providers", id) {
            Ok(key) => String::from_utf8(key)
                .map(Some)
                .map_err(|_| Error::Secret("invalid credential encoding".into())),
            Err(e) if e.code() == -25300 => Ok(None),
            Err(e) => Err(Error::Secret(e.to_string())),
        }
    }
    fn set(&self, id: &str, key: &str) -> Result<()> {
        security_framework::passwords::set_generic_password("bot.mac.providers", id, key.as_bytes())
            .map_err(|e| Error::Secret(e.to_string()))
    }
    fn delete(&self, id: &str) -> Result<()> {
        match security_framework::passwords::delete_generic_password("bot.mac.providers", id) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(e) => Err(Error::Secret(e.to_string())),
        }
    }
}
#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainSecrets {
    fn get(&self, _: &str) -> Result<Option<String>> {
        Err(Error::Secret("macOS keychain required".into()))
    }
    fn set(&self, _: &str, _: &str) -> Result<()> {
        Err(Error::Secret("macOS keychain required".into()))
    }
    fn delete(&self, _: &str) -> Result<()> {
        Err(Error::Secret("macOS keychain required".into()))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<Value>,
    #[serde(default)]
    pub tools: Vec<Value>,
    pub max_output: u32,
    pub session_id: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TokenUsage,
    pub stop_reason: String,
    /// Provider-native assistant blocks, including signed thinking. Internal
    /// context only: these must be replayed to the same API after tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_content: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCallDelta {
        call_id: String,
        name: Option<String>,
        args: String,
    },
    Usage {
        usage: TokenUsage,
    },
    Stop {
        reason: String,
    },
}
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        request: ModelRequest,
        events: mpsc::Sender<ModelEvent>,
    ) -> Result<Completion>;
    async fn complete(&self, request: ModelRequest) -> Result<Completion> {
        let (tx, mut rx) = mpsc::channel(128);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let result = self.stream(request, tx).await;
        let _ = drain.await;
        result
    }
}
pub struct HttpProvider {
    config: ProviderConfig,
    secrets: Arc<dyn SecretStore>,
    client: reqwest::Client,
}
impl HttpProvider {
    pub fn new(config: ProviderConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            config,
            secrets,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(180))
                .build()
                .expect("HTTP client"),
        }
    }
    fn authenticated(&self, request: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let mut request = request;
        for (name, value) in &self.config.headers {
            request = request.header(name, value);
        }
        if let Some(key) = self.secrets.get(&self.config.id)? {
            request = match self.config.api_kind {
                ApiKind::AnthropicMessages => request.header("x-api-key", key),
                ApiKind::GoogleGenerative => request.header("x-goog-api-key", key),
                _ => request.bearer_auth(key),
            };
        }
        if self.config.api_kind == ApiKind::AnthropicMessages {
            request = request.header("anthropic-version", "2023-06-01");
        }
        Ok(request)
    }
    pub async fn models(&self) -> Result<Vec<Value>> {
        let base = self.config.base_url.trim_end_matches('/');
        let prefix = if self.config.api_kind == ApiKind::AnthropicMessages && !base.ends_with("/v1")
        {
            "/v1"
        } else {
            ""
        };
        let response = self
            .authenticated(self.client.get(format!("{base}{prefix}/models")))?
            .send()
            .await?;
        let value = checked_json(response).await?;
        Ok(value
            .get("data")
            .or_else(|| value.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
    pub async fn test(&self, model: &str) -> Result<Completion> {
        self.complete(ModelRequest {
            model: model.into(),
            messages: vec![json!({"role":"user","content":"Reply OK."})],
            max_output: 8,
            ..Default::default()
        })
        .await
    }
}
async fn checked_json(response: reqwest::Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err(Error::Response(format!(
            "HTTP {}",
            response.status().as_u16()
        )));
    }
    Ok(response.json().await?)
}

/// Build each wire request from the common role/content and function-call representation.
pub fn wire_request(kind: ApiKind, request: &ModelRequest) -> (String, Value) {
    let max_output = request.max_output.max(1);
    match kind {
        ApiKind::OpenaiCompletions => {
            let messages = request
                .messages
                .iter()
                .map(|message| {
                    let mut message = message.clone();
                    if let Some(object) = message.as_object_mut() {
                        object.remove("assistant_content");
                    }
                    message
                })
                .collect::<Vec<_>>();
            let mut body = json!({"model":request.model,"messages":messages,"max_tokens":max_output,"stream":true,"stream_options":{"include_usage":true}});
            if !request.tools.is_empty() {
                body["tools"] = json!(request.tools);
            }
            ("/chat/completions".into(), body)
        }
        ApiKind::OpenaiResponses => {
            let mut input = Vec::new();
            for message in &request.messages {
                if message["role"] == "tool" {
                    input.push(json!({"type":"function_call_output","call_id":message["tool_call_id"],"output":message["content"]}));
                } else {
                    if !message["content"].is_null() {
                        let content = if let Some(parts) = message["content"].as_array() {
                            json!(parts.iter().map(|part| match part["type"].as_str() {
                                Some("image_url") => json!({"type":"input_image","image_url":part["image_url"]["url"]}),
                                Some("text") => json!({"type":if message["role"]=="assistant" {"output_text"} else {"input_text"},"text":part["text"]}),
                                _ => part.clone(),
                            }).collect::<Vec<_>>())
                        } else {
                            message["content"].clone()
                        };
                        input.push(json!({"role":message["role"],"content":content}));
                    }
                    for call in message["tool_calls"].as_array().into_iter().flatten() {
                        input.push(json!({"type":"function_call","call_id":call["id"],"name":call["function"]["name"],"arguments":call["function"]["arguments"]}));
                    }
                }
            }
            let mut body = json!({"model":request.model,"input":input,"max_output_tokens":max_output,"stream":true,"store":false});
            if let Some(id) = &request.session_id {
                body["prompt_cache_key"] = json!(id);
            }
            if !request.tools.is_empty() {
                body["tools"] = json!(request
                    .tools
                    .iter()
                    .map(|t| {
                        let mut f = t["function"].clone();
                        f["type"] = json!("function");
                        f
                    })
                    .collect::<Vec<_>>());
            }
            ("/responses".into(), body)
        }
        ApiKind::AnthropicMessages => {
            let mut messages = Vec::new();
            let mut system = Vec::new();
            for message in &request.messages {
                let role = message["role"].as_str().unwrap_or("user");
                if role == "system" {
                    system.push(message["content"].as_str().unwrap_or("").to_owned());
                    continue;
                }
                let content = if role == "assistant"
                    && message["assistant_content"]["api_kind"] == "anthropic-messages"
                {
                    message["assistant_content"]["content"].clone()
                } else if role == "tool" {
                    json!([{"type":"tool_result","tool_use_id":message["tool_call_id"],"content":message["content"]}])
                } else {
                    let mut parts = Vec::new();
                    if let Some(text) = message["content"].as_str().filter(|s| !s.is_empty()) {
                        parts.push(json!({"type":"text","text":text}));
                    }
                    for part in message["content"].as_array().into_iter().flatten() {
                        if part["type"] == "text" {
                            parts.push(json!({"type":"text","text":part["text"]}));
                        } else if let Some(url) = part["image_url"]["url"].as_str() {
                            if let Some((mime, data)) = data_image(url) {
                                parts.push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}));
                            } else {
                                parts.push(
                                    json!({"type":"image","source":{"type":"url","url":url}}),
                                );
                            }
                        }
                    }
                    for call in message["tool_calls"].as_array().into_iter().flatten() {
                        let args = call["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or_else(|| json!({}));
                        parts.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":args}));
                    }
                    json!(parts)
                };
                messages.push(json!({"role":if role=="assistant" {"assistant"} else {"user"},"content":content}));
            }
            let mut body = json!({"model":request.model,"messages":messages,"system":system.join("\n\n"),"max_tokens":max_output,"stream":true});
            if !request.tools.is_empty() {
                body["tools"] = json!(request.tools.iter().map(|t| json!({"name":t["function"]["name"],"description":t["function"]["description"],"input_schema":t["function"]["parameters"]})).collect::<Vec<_>>());
            }
            ("/messages".into(), body)
        }
        ApiKind::GoogleGenerative => {
            let mut contents = Vec::new();
            let mut system = Vec::new();
            let mut call_names = BTreeMap::new();
            for message in &request.messages {
                let role = message["role"].as_str().unwrap_or("user");
                if role == "system" {
                    system.push(json!({"text":message["content"]}));
                    continue;
                }
                let mut parts = Vec::new();
                if role == "assistant"
                    && message["assistant_content"]["api_kind"] == "google-generative"
                {
                    for call in message["tool_calls"].as_array().into_iter().flatten() {
                        if let (Some(id), Some(name)) =
                            (call["id"].as_str(), call["function"]["name"].as_str())
                        {
                            call_names.insert(id, name);
                        }
                    }
                    contents.push(
                        json!({"role":"model","parts":message["assistant_content"]["content"]}),
                    );
                    continue;
                }
                if role == "tool" {
                    let id = message["tool_call_id"].as_str().unwrap_or("");
                    parts.push(json!({"functionResponse":{"name":call_names.get(id).copied().unwrap_or("tool"),"response":{"output":message["content"]}}}));
                } else {
                    if let Some(text) = message["content"].as_str().filter(|s| !s.is_empty()) {
                        parts.push(json!({"text":text}));
                    }
                    for part in message["content"].as_array().into_iter().flatten() {
                        if part["type"] == "text" {
                            parts.push(json!({"text":part["text"]}));
                        } else if let Some(url) = part["image_url"]["url"].as_str() {
                            if let Some((mime, data)) = data_image(url) {
                                parts.push(json!({"inlineData":{"mimeType":mime,"data":data}}));
                            } else {
                                parts.push(json!({"fileData":{"fileUri":url}}));
                            }
                        }
                    }
                    for call in message["tool_calls"].as_array().into_iter().flatten() {
                        let id = call["id"].as_str().unwrap_or("");
                        let name = call["function"]["name"].as_str().unwrap_or("");
                        call_names.insert(id, name);
                        let args = call["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or_else(|| json!({}));
                        parts.push(json!({"functionCall":{"name":name,"args":args}}));
                    }
                }
                contents.push(
                    json!({"role":if role=="assistant" {"model"} else {"user"},"parts":parts}),
                );
            }
            let mut body =
                json!({"contents":contents,"generationConfig":{"maxOutputTokens":max_output}});
            if !system.is_empty() {
                body["systemInstruction"] = json!({"parts":system});
            }
            if !request.tools.is_empty() {
                body["tools"] = json!([{"functionDeclarations":request.tools.iter().map(|t|t["function"].clone()).collect::<Vec<_>>()}]);
            }
            (
                format!(
                    "/models/{}:streamGenerateContent?alt=sse",
                    request.model.trim_start_matches("models/")
                ),
                body,
            )
        }
    }
}
fn data_image(url: &str) -> Option<(&str, &str)> {
    let (prefix, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    Some((prefix, data))
}

#[derive(Default)]
struct Accumulator {
    result: Completion,
    native_blocks: BTreeMap<String, Value>,
    google_parts: Vec<Value>,
    calls: BTreeMap<String, (String, String, String)>,
}
fn n(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}
fn usage(value: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: n(value, "input_tokens")
            .max(n(value, "prompt_tokens"))
            .max(n(value, "promptTokenCount"))
            + n(value, "cache_read_input_tokens")
            + n(value, "cache_creation_input_tokens"),
        output_tokens: n(value, "output_tokens")
            .max(n(value, "completion_tokens"))
            .max(n(value, "candidatesTokenCount")),
        cache_read_tokens: n(value, "cache_read_input_tokens")
            .max(n(&value["prompt_tokens_details"], "cached_tokens"))
            .max(n(&value["input_tokens_details"], "cached_tokens"))
            .max(n(value, "cachedContentTokenCount")),
        cache_write_tokens: n(value, "cache_creation_input_tokens"),
    }
}
impl Accumulator {
    fn parse(&mut self, kind: ApiKind, value: &Value) -> Result<Vec<ModelEvent>> {
        if value.get("error").is_some() || value["type"] == "error" {
            return Err(Error::Response("model stream reported an error".into()));
        }
        let mut events = Vec::new();
        match kind {
            ApiKind::OpenaiCompletions => {
                let choice = &value["choices"][0];
                let delta = &choice["delta"];
                self.text(delta["content"].as_str(), false, &mut events);
                self.text(
                    delta["reasoning_content"]
                        .as_str()
                        .or_else(|| delta["reasoning"].as_str()),
                    true,
                    &mut events,
                );
                for call in delta["tool_calls"].as_array().into_iter().flatten() {
                    let index = call["index"].as_u64().unwrap_or(0).to_string();
                    self.call(
                        index,
                        call["id"].as_str(),
                        call["function"]["name"].as_str(),
                        call["function"]["arguments"].as_str().unwrap_or(""),
                        &mut events,
                    );
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    self.result.stop_reason = reason.into();
                }
                if value["usage"].is_object() {
                    self.result.usage = usage(&value["usage"]);
                    events.push(ModelEvent::Usage {
                        usage: self.result.usage.clone(),
                    });
                }
            }
            ApiKind::OpenaiResponses => match value["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    self.text(value["delta"].as_str(), false, &mut events)
                }
                "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                    self.text(value["delta"].as_str(), true, &mut events)
                }
                "response.output_item.added" if value["item"]["type"] == "function_call" => {
                    self.call(
                        value["item"]["id"].as_str().unwrap_or("").into(),
                        value["item"]["call_id"].as_str(),
                        value["item"]["name"].as_str(),
                        value["item"]["arguments"].as_str().unwrap_or(""),
                        &mut events,
                    );
                }
                "response.function_call_arguments.delta" => self.call(
                    value["item_id"].as_str().unwrap_or("").into(),
                    None,
                    None,
                    value["delta"].as_str().unwrap_or(""),
                    &mut events,
                ),
                "response.completed" | "response.incomplete" => {
                    self.result.usage = usage(&value["response"]["usage"]);
                    self.result.stop_reason = value["response"]["status"]
                        .as_str()
                        .unwrap_or("completed")
                        .into();
                    events.push(ModelEvent::Usage {
                        usage: self.result.usage.clone(),
                    });
                }
                "response.failed" => return Err(Error::Response("model response failed".into())),
                _ => {}
            },
            ApiKind::AnthropicMessages => match value["type"].as_str().unwrap_or("") {
                "message_start" => {
                    self.result.usage = usage(&value["message"]["usage"]);
                }
                "content_block_start" => {
                    let block = &value["content_block"];
                    self.native_blocks
                        .insert(value["index"].to_string(), block.clone());
                    if block["type"] == "tool_use" {
                        let initial = block
                            .get("input")
                            .filter(|v| {
                                v.is_object() && v.as_object().is_some_and(|o| !o.is_empty())
                            })
                            .map(Value::to_string)
                            .unwrap_or_default();
                        self.call(
                            value["index"].to_string(),
                            block["id"].as_str(),
                            block["name"].as_str(),
                            &initial,
                            &mut events,
                        );
                    }
                    self.text(block["text"].as_str(), false, &mut events);
                    self.text(block["thinking"].as_str(), true, &mut events);
                }
                "content_block_delta" => {
                    if let Some(block) = self.native_blocks.get_mut(&value["index"].to_string()) {
                        for key in ["text", "thinking", "signature"] {
                            if let Some(delta) = value["delta"][key].as_str() {
                                let mut accumulated = block[key].as_str().unwrap_or("").to_owned();
                                accumulated.push_str(delta);
                                block[key] = json!(accumulated);
                            }
                        }
                    }
                    self.text(value["delta"]["text"].as_str(), false, &mut events);
                    self.text(value["delta"]["thinking"].as_str(), true, &mut events);
                    if let Some(args) = value["delta"]["partial_json"].as_str() {
                        self.call(value["index"].to_string(), None, None, args, &mut events);
                    }
                }
                "message_delta" => {
                    self.result.stop_reason = value["delta"]["stop_reason"]
                        .as_str()
                        .unwrap_or("end_turn")
                        .into();
                    self.result.usage.output_tokens = n(&value["usage"], "output_tokens");
                    events.push(ModelEvent::Usage {
                        usage: self.result.usage.clone(),
                    });
                }
                _ => {}
            },
            ApiKind::GoogleGenerative => {
                for part in value["candidates"][0]["content"]["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    self.google_parts.push(part.clone());
                    self.text(
                        part["text"].as_str(),
                        part["thought"].as_bool().unwrap_or(false),
                        &mut events,
                    );
                    if part["functionCall"].is_object() {
                        let id = format!("call_{}", uuid::Uuid::now_v7());
                        let call = &part["functionCall"];
                        self.call(
                            id.clone(),
                            Some(&id),
                            call["name"].as_str(),
                            &call["args"].to_string(),
                            &mut events,
                        );
                    }
                }
                if let Some(reason) = value["candidates"][0]["finishReason"].as_str() {
                    self.result.stop_reason = reason.into();
                }
                if value["usageMetadata"].is_object() {
                    self.result.usage = usage(&value["usageMetadata"]);
                    events.push(ModelEvent::Usage {
                        usage: self.result.usage.clone(),
                    });
                }
            }
        }
        Ok(events)
    }
    fn text(&mut self, text: Option<&str>, thinking: bool, events: &mut Vec<ModelEvent>) {
        if let Some(text) = text.filter(|s| !s.is_empty()) {
            if thinking {
                self.result.thinking.push_str(text);
                events.push(ModelEvent::ThinkingDelta { text: text.into() });
            } else {
                self.result.text.push_str(text);
                events.push(ModelEvent::TextDelta { text: text.into() });
            }
        }
    }
    fn call(
        &mut self,
        index: String,
        id: Option<&str>,
        name: Option<&str>,
        args: &str,
        events: &mut Vec<ModelEvent>,
    ) {
        let entry = self.calls.entry(index).or_insert_with(|| {
            (
                format!("call_{}", uuid::Uuid::now_v7()),
                String::new(),
                String::new(),
            )
        });
        if let Some(id) = id {
            entry.0 = id.into();
        }
        if let Some(name) = name {
            entry.1.push_str(name);
        }
        entry.2.push_str(args);
        events.push(ModelEvent::ToolCallDelta {
            call_id: entry.0.clone(),
            name: name.map(str::to_owned),
            args: args.into(),
        });
    }
    fn finish(mut self) -> Result<Completion> {
        for (index, (call_id, name, args)) in self.calls {
            let args = if args.is_empty() {
                json!({})
            } else {
                serde_json::from_str(&args)
                    .map_err(|_| Error::Response("invalid tool arguments JSON".into()))?
            };
            if let Some(block) = self.native_blocks.get_mut(&index) {
                if block["type"] == "tool_use" {
                    block["input"] = args.clone();
                }
            }
            self.result.tool_calls.push(ToolCall {
                call_id,
                name,
                args,
            });
        }
        if self.result.stop_reason.is_empty() {
            return Err(Error::Response(
                "model stream ended before stop event".into(),
            ));
        }
        if !self.native_blocks.is_empty() {
            let mut blocks = self.native_blocks.into_iter().collect::<Vec<_>>();
            blocks.sort_by_key(|(index, _)| index.parse::<u64>().unwrap_or(0));
            self.result.assistant_content = Some(
                json!({"api_kind":"anthropic-messages","content":blocks.into_iter().map(|(_, block)| block).collect::<Vec<_>>()}),
            );
        } else if !self.google_parts.is_empty() {
            self.result.assistant_content =
                Some(json!({"api_kind":"google-generative","content":self.google_parts}));
        }
        Ok(self.result)
    }
}
#[async_trait]
impl ModelProvider for HttpProvider {
    async fn stream(
        &self,
        request: ModelRequest,
        events: mpsc::Sender<ModelEvent>,
    ) -> Result<Completion> {
        let (mut path, body) = wire_request(self.config.api_kind, &request);
        if self.config.api_kind == ApiKind::AnthropicMessages
            && !self.config.base_url.trim_end_matches('/').ends_with("/v1")
        {
            path = format!("/v1{path}");
        }
        let response = self
            .authenticated(
                self.client
                    .post(format!(
                        "{}{}",
                        self.config.base_url.trim_end_matches('/'),
                        path
                    ))
                    .json(&body),
            )?
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::Response(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        let mut stream = response.bytes_stream().eventsource();
        let mut accumulator = Accumulator::default();
        while let Some(event) = stream.next().await {
            let event =
                event.map_err(|_| Error::Response("malformed or interrupted SSE stream".into()))?;
            if event.data == "[DONE]" {
                break;
            }
            let value: Value = serde_json::from_str(&event.data)
                .map_err(|_| Error::Response("invalid SSE JSON".into()))?;
            for event in accumulator.parse(self.config.api_kind, &value)? {
                let _ = events.send(event).await;
            }
        }
        let result = accumulator.finish()?;
        let _ = events
            .send(ModelEvent::Stop {
                reason: result.stop_reason.clone(),
            })
            .await;
        Ok(result)
    }
}
/// Deterministic model used by unit tests and mock-provider scenario tests.
pub struct MockProvider {
    script: Mutex<std::collections::VecDeque<Completion>>,
}
impl MockProvider {
    pub fn new(script: Vec<Completion>) -> Self {
        Self {
            script: Mutex::new(script.into()),
        }
    }
}
#[async_trait]
impl ModelProvider for MockProvider {
    async fn stream(
        &self,
        request: ModelRequest,
        events: mpsc::Sender<ModelEvent>,
    ) -> Result<Completion> {
        let result = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Completion {
                text: format!(
                    "Mock: {}",
                    request
                        .messages
                        .last()
                        .and_then(|m| m["content"].as_str())
                        .unwrap_or("OK")
                ),
                stop_reason: "stop".into(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                ..Default::default()
            });
        let _ = events
            .send(ModelEvent::TextDelta {
                text: result.text.clone(),
            })
            .await;
        let _ = events
            .send(ModelEvent::Usage {
                usage: result.usage.clone(),
            })
            .await;
        let _ = events
            .send(ModelEvent::Stop {
                reason: result.stop_reason.clone(),
            })
            .await;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_thinking_survives_tool_result_continuation() {
        let mut a = Accumulator::default();
        for event in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call1","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"file\"}"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
        ] {
            a.parse(ApiKind::AnthropicMessages, &event).unwrap();
        }
        let completion = a.finish().unwrap();
        let request = ModelRequest {
            messages: vec![
                json!({"role":"assistant","content":"","assistant_content":completion.assistant_content,"tool_calls":[{"id":"call1","function":{"name":"read","arguments":"{\"path\":\"file\"}"}}]}),
                json!({"role":"tool","tool_call_id":"call1","content":"found"}),
            ],
            ..Default::default()
        };
        let (_, wire) = wire_request(ApiKind::AnthropicMessages, &request);
        assert_eq!(
            wire["messages"][0]["content"][0],
            json!({"type":"thinking","thinking":"plan","signature":"signed"})
        );
        assert_eq!(
            wire["messages"][0]["content"][1]["input"],
            json!({"path":"file"})
        );
        assert_eq!(wire["messages"][1]["content"][0]["tool_use_id"], "call1");
    }
    #[test]
    fn google_thought_signature_survives_native_function_response() {
        let part =
            json!({"functionCall":{"name":"read","args":{"path":"f"}},"thoughtSignature":"signed"});
        let mut a = Accumulator::default();
        a.parse(
            ApiKind::GoogleGenerative,
            &json!({"candidates":[{"content":{"parts":[part]},"finishReason":"STOP"}]}),
        )
        .unwrap();
        let completion = a.finish().unwrap();
        let call = &completion.tool_calls[0];
        let request = ModelRequest {
            messages: vec![
                json!({"role":"assistant","assistant_content":completion.assistant_content,"tool_calls":[{"id":call.call_id,"function":{"name":call.name}}]}),
                json!({"role":"tool","tool_call_id":call.call_id,"content":"found"}),
            ],
            ..Default::default()
        };
        let (_, wire) = wire_request(ApiKind::GoogleGenerative, &request);
        assert_eq!(
            wire["contents"][0]["parts"][0]["thoughtSignature"],
            "signed"
        );
        assert_eq!(
            wire["contents"][1]["parts"][0]["functionResponse"]["name"],
            "read"
        );
    }
    #[test]
    fn split_tool_arguments_and_usage() {
        let mut a = Accumulator::default();
        a.parse(ApiKind::OpenaiCompletions,&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"pa"}}]}}]})).unwrap();
        a.parse(ApiKind::OpenaiCompletions,&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":4}}})).unwrap();
        let result = a.finish().unwrap();
        assert_eq!(result.tool_calls[0].args, json!({"path":"a"}));
        assert_eq!(result.tool_calls[0].call_id, "c1");
        assert_eq!(result.usage.cache_read_tokens, 4);
    }
    #[test]
    fn anthropic_request_contains_tool_results_and_images() {
        let request = ModelRequest {
            model: "m".into(),
            messages: vec![
                json!({"role":"system","content":"rules"}),
                json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}),
                json!({"role":"tool","tool_call_id":"c","content":"result"}),
            ],
            max_output: 10,
            ..Default::default()
        };
        let (_, body) = wire_request(ApiKind::AnthropicMessages, &request);
        assert_eq!(body["system"], "rules");
        assert_eq!(
            body["messages"][0]["content"][0]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "c");
    }
    #[test]
    fn responses_tool_call_id_is_preserved() {
        let mut a = Accumulator::default();
        a.parse(ApiKind::OpenaiResponses,&json!({"type":"response.output_item.added","item":{"type":"function_call","id":"item1","call_id":"call1","name":"read","arguments":""}})).unwrap();
        a.parse(ApiKind::OpenaiResponses,&json!({"type":"response.function_call_arguments.delta","item_id":"item1","delta":"{}"})).unwrap();
        a.parse(ApiKind::OpenaiResponses,&json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":3,"output_tokens":4}}})).unwrap();
        assert_eq!(a.finish().unwrap().tool_calls[0].call_id, "call1");
    }
    #[test]
    fn anthropic_cached_tokens_and_fragmented_tools_are_normalized() {
        let mut a = Accumulator::default();
        for value in [
            json!({"type":"message_start","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":20,"cache_creation_input_tokens":10}}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c1","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        ] {
            a.parse(ApiKind::AnthropicMessages, &value).unwrap();
        }
        let result = a.finish().unwrap();
        assert_eq!(
            result.usage,
            TokenUsage {
                input_tokens: 130,
                output_tokens: 5,
                cache_read_tokens: 20,
                cache_write_tokens: 10
            }
        );
        assert_eq!(
            result.tool_calls[0],
            ToolCall {
                call_id: "c1".into(),
                name: "read".into(),
                args: json!({"path":"a"})
            }
        );
    }
    #[test]
    fn google_tools_images_and_reasoning_use_the_common_contract() {
        let request = ModelRequest {
            model: "models/gemini-test".into(),
            messages: vec![
                json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}),
            ],
            max_output: 16,
            ..Default::default()
        };
        let (path, body) = wire_request(ApiKind::GoogleGenerative, &request);
        assert_eq!(path, "/models/gemini-test:streamGenerateContent?alt=sse");
        assert_eq!(
            body["contents"][0]["parts"][0]["inlineData"]["data"],
            "AA=="
        );
        let mut a = Accumulator::default();
        a.parse(ApiKind::GoogleGenerative,&json!({"candidates":[{"content":{"parts":[{"text":"thinking","thought":true},{"functionCall":{"name":"read","args":{"path":"a"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":5,"cachedContentTokenCount":10}})).unwrap();
        let result = a.finish().unwrap();
        assert_eq!(result.thinking, "thinking");
        assert_eq!(result.usage.input_tokens, 20);
        assert_eq!(result.tool_calls[0].args, json!({"path":"a"}));
    }
    #[test]
    fn interrupted_stream_and_malformed_tool_arguments_are_errors() {
        let mut a = Accumulator::default();
        a.parse(
            ApiKind::OpenaiCompletions,
            &json!({"choices":[{"delta":{"content":"partial"}}]}),
        )
        .unwrap();
        assert!(a.finish().is_err());
        let mut a = Accumulator::default();
        a.parse(ApiKind::OpenaiCompletions,&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":"{"}}]},"finish_reason":"tool_calls"}]})).unwrap();
        assert!(a.finish().is_err());
    }
    #[test]
    fn responses_converts_multimodal_content_and_tool_results() {
        let request = ModelRequest {
            model: "m".into(),
            messages: vec![
                json!({"role":"user","content":[{"type":"text","text":"inspect"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}),
                json!({"role":"tool","tool_call_id":"c1","content":"result"}),
            ],
            max_output: 16,
            ..Default::default()
        };
        let (_, body) = wire_request(ApiKind::OpenaiResponses, &request);
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(body["input"][1]["call_id"], "c1");
    }
    #[tokio::test]
    async fn anthropic_catalog_uses_versioned_route() {
        use axum::{routing::get, Router};
        let app = Router::new().route(
            "/anthropic/v1/models",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers["x-api-key"], "test-only");
                axum::Json(json!({"data":[{"id":"fake-model"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let secrets = Arc::new(MemorySecrets::default());
        secrets.set("p", "test-only").unwrap();
        let provider = HttpProvider::new(
            ProviderConfig {
                id: "p".into(),
                name: "fake".into(),
                api_kind: ApiKind::AnthropicMessages,
                base_url: format!("http://{addr}/anthropic"),
                headers: BTreeMap::new(),
            },
            secrets,
        );
        assert_eq!(provider.models().await.unwrap()[0]["id"], "fake-model");
        server.abort();
    }
    #[tokio::test]
    async fn anthropic_stream_uses_versioned_route_and_retains_signed_tool_context() {
        use axum::{routing::post, Router};
        let app = Router::new().route(
            "/anthropic/v1/messages",
            post(|headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| async move {
                assert_eq!(headers["x-api-key"], "test-only");
                assert_eq!(headers["anthropic-version"], "2023-06-01");
                assert_eq!(body["model"], "fake/model");
                assert_eq!(body["stream"], true);
                let events = [
                    json!({"type":"message_start","message":{"usage":{"input_tokens":4,"cache_read_input_tokens":2}}}),
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}),
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"fake-signature"}}),
                    json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-1","name":"read","input":{}}}),
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"README.md\"}"}}),
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
                    json!({"type":"message_stop"}),
                ];
                let sse = events.iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
                ([("content-type", "text/event-stream")], sse)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let secrets = Arc::new(MemorySecrets::default());
        secrets.set("p", "test-only").unwrap();
        for suffix in ["/anthropic", "/anthropic/v1/"] {
            let provider = HttpProvider::new(
                ProviderConfig {
                    id: "p".into(),
                    name: "fake".into(),
                    api_kind: ApiKind::AnthropicMessages,
                    base_url: format!("http://{addr}{suffix}"),
                    headers: BTreeMap::new(),
                },
                secrets.clone(),
            );
            let result = provider.test("fake/model").await.unwrap();
            assert_eq!(result.usage.input_tokens, 6);
            assert_eq!(result.usage.cache_read_tokens, 2);
            assert_eq!(result.usage.output_tokens, 3);
            assert_eq!(result.tool_calls[0].args, json!({"path":"README.md"}));
            assert_eq!(
                result.assistant_content.as_ref().unwrap()["content"][0]["signature"],
                "fake-signature"
            );
            let (_, body) = wire_request(
                ApiKind::AnthropicMessages,
                &ModelRequest {
                    model: "fake/model".into(),
                    messages: vec![
                        json!({"role":"assistant","assistant_content":result.assistant_content}),
                        json!({"role":"tool","tool_call_id":"call-1","content":"file contents"}),
                    ],
                    max_output: 16,
                    ..Default::default()
                },
            );
            assert_eq!(
                body["messages"][0]["content"][0]["signature"],
                "fake-signature"
            );
            assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "call-1");
        }
        server.abort();
    }
    #[tokio::test]
    async fn http_stream_against_local_fake_server() {
        use axum::{routing::post, Router};
        let app=Router::new().route("/v1/chat/completions",post(|headers:axum::http::HeaderMap, axum::Json(body):axum::Json<Value>|async move {
            assert_eq!(headers["authorization"],"Bearer test-only"); assert_eq!(body["stream"],true);
            ([("content-type","text/event-stream")],"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n")
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let secrets = Arc::new(MemorySecrets::default());
        secrets.set("p", "test-only").unwrap();
        let provider = HttpProvider::new(
            ProviderConfig {
                id: "p".into(),
                name: "test".into(),
                api_kind: ApiKind::OpenaiCompletions,
                base_url: format!("http://{addr}/v1"),
                headers: BTreeMap::new(),
            },
            secrets,
        );
        let result = provider.test("test-model").await.unwrap();
        assert_eq!(result.text, "hello");
        assert_eq!(result.usage.input_tokens, 2);
        server.abort();
    }
}
