//! Durable provider/model registry.
//!
//! The registry stores only protocol objects. API keys are handed to the
//! injected secret store before a provider mutation is admitted and never
//! enter the WAL, snapshots, replies, or event payloads.

use super::{ApiKind, Error as ProviderError, HttpProvider, ProviderConfig, SecretStore};
use chrono::{SecondsFormat, Utc};
use macbot_protocol::{
    ApiKind as ProtocolApiKind, EmptyParams, Model, ModelCaps, ModelDeleteParams, ModelPrice,
    ModelRefreshResult, ModelResult, ModelUpsertParams, Patch, Provider, ProviderCreateParams,
    ProviderIdParams, ProviderListResult, ProviderPatch, ProviderResult, ProviderTestResult,
    ProviderUpdateParams,
};
use macbot_store::{Event, Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("provider: {0}")]
    Provider(#[from] ProviderError),
    #[error("invalid parameters: {0}")]
    Invalid(String),
    #[error("provider not found: {0}")]
    ProviderNotFound(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
}

pub type Result<T> = std::result::Result<T, RegistryError>;

#[derive(Debug, Clone)]
pub struct ProviderReply {
    pub result: Value,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RegistrySnapshot {
    providers: Vec<Provider>,
    models: Vec<Model>,
    #[serde(default)]
    idempotency: BTreeMap<String, Value>,
}

pub struct ProviderRegistry {
    store: Store,
    secrets: Arc<dyn SecretStore>,
    providers: BTreeMap<String, Provider>,
    models: BTreeMap<String, Model>,
    idempotency: BTreeMap<String, Value>,
}

impl ProviderRegistry {
    /// Attach to an existing process store. No second process lock is taken.
    pub fn from_store(store: Store, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        let mut state = RegistrySnapshot::default();
        match store.read_snapshot::<RegistrySnapshot>("data/provider-registry.json") {
            Ok(Some(snapshot)) => state = snapshot,
            Ok(None) | Err(StoreError::Snapshot { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        for event in store.read_jsonl::<Event>("data/events/events.jsonl")? {
            if let Some(metadata) = event.data.get("_registry") {
                state = serde_json::from_value(metadata.clone()).map_err(|error| {
                    RegistryError::Invalid(format!("registry metadata: {error}"))
                })?;
            }
        }
        Ok(Self {
            store,
            secrets,
            providers: state
                .providers
                .into_iter()
                .map(|provider| (provider.id.clone(), provider))
                .collect(),
            models: state
                .models
                .into_iter()
                .map(|model| (model.r#ref.clone(), model))
                .collect(),
            idempotency: state.idempotency,
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Return the injected credential backend to runtime integrations. The
    /// backend stores credentials outside registry snapshots and never exposes
    /// their values through provider RPC results.
    pub fn secret_store(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }

    pub fn providers(&self) -> impl Iterator<Item = &Provider> {
        self.providers.values()
    }

    pub fn models(&self) -> impl Iterator<Item = &Model> {
        self.models.values()
    }

    /// Resolve the persisted catalog reference before starting an execution.
    /// The returned provider owns the shared secret store; no credential is
    /// copied into the request, durable journal, or protocol object.
    pub fn resolve_model(&self, model_ref: &str) -> Result<(Model, Arc<HttpProvider>)> {
        let model = self
            .models
            .get(model_ref)
            .ok_or_else(|| RegistryError::ModelNotFound(model_ref.to_string()))?;
        if !model.enabled {
            return Err(RegistryError::Invalid(format!(
                "model is disabled: {model_ref}"
            )));
        }
        let provider = HttpProvider::new(
            self.provider_config(&model.provider_id)?,
            self.secrets.clone(),
        );
        Ok((model.clone(), Arc::new(provider)))
    }

    pub async fn rpc(
        &mut self,
        method: &str,
        params: Value,
        in_use_models: &[String],
    ) -> Result<ProviderReply> {
        match method {
            "provider.list" => self.provider_list(params),
            "provider.create" => self.provider_create(params),
            "provider.update" => self.provider_update(params),
            "provider.delete" => {
                let id = params["provider_id"].as_str().unwrap_or("");
                if in_use_models
                    .iter()
                    .any(|reference| reference.starts_with(&format!("{id}/")))
                {
                    return Err(RegistryError::Conflict("provider has models in use".into()));
                }
                self.provider_delete(params)
            }
            "provider.test" => self.provider_test(params).await,
            "model.refresh" => self.model_refresh(params).await,
            "model.upsert" => self.model_upsert(params),
            "model.delete" => self.model_delete(params, in_use_models),
            _ => Err(RegistryError::Invalid(format!(
                "unknown provider method: {method}"
            ))),
        }
    }

    fn provider_list(&self, params: Value) -> Result<ProviderReply> {
        decode::<EmptyParams>(params)?;
        let providers = self
            .providers
            .values()
            .cloned()
            .map(|mut provider| {
                provider.has_key = self.secrets.get(&provider.id).ok().flatten().is_some();
                provider
            })
            .collect::<Vec<_>>();
        let models = self.models.values().cloned().collect::<Vec<_>>();
        let result = serde_json::to_value(ProviderListResult { providers, models })
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        Ok(ProviderReply {
            result,
            events: Vec::new(),
        })
    }

    fn provider_create(&mut self, params: Value) -> Result<ProviderReply> {
        let typed: ProviderCreateParams = decode(params)?;
        if let Some(reply) = self.cached(&typed.meta.client_request_id) {
            return Ok(reply);
        }
        validate_name(&typed.name)?;
        validate_base_url(&typed.base_url)?;
        let headers = typed.headers.unwrap_or_default();
        validate_headers(&headers)?;
        let id = format!("prv_{}", uuid::Uuid::now_v7());
        if let Some(key) = typed.api_key.as_deref() {
            if key.is_empty() {
                return Err(RegistryError::Invalid("api_key must not be empty".into()));
            }
            self.secrets.set(&id, key)?;
        }
        let now = timestamp();
        let provider = Provider {
            id: id.clone(),
            name: typed.name,
            api_kind: typed.api_kind,
            base_url: typed.base_url,
            has_key: typed.api_key.is_some(),
            headers,
            created_at: now.clone(),
            updated_at: now,
        };
        self.providers.insert(id.clone(), provider.clone());
        let result = serde_json::to_value(ProviderResult {
            provider: provider.clone(),
        })
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let event_data = json!({
            "provider": provider,
            "models": self.models_for(&id),
        });
        self.persist_mutation(
            "provider.updated",
            event_data,
            result,
            typed.meta.client_request_id,
        )
    }

    fn provider_update(&mut self, params: Value) -> Result<ProviderReply> {
        let typed: ProviderUpdateParams = decode(params.clone())?;
        if let Some(reply) = self.cached(&typed.meta.client_request_id) {
            return Ok(reply);
        }
        let mut provider = self
            .providers
            .get(&typed.provider_id)
            .cloned()
            .ok_or_else(|| RegistryError::ProviderNotFound(typed.provider_id.clone()))?;
        validate_provider_patch(&typed.patch, &params)?;
        if let Some(name) = typed.patch.name.clone() {
            validate_name(&name)?;
            provider.name = name;
        }
        if let Some(base_url) = typed.patch.base_url.clone() {
            validate_base_url(&base_url)?;
            provider.base_url = base_url;
        }
        if params
            .get("patch")
            .and_then(Value::as_object)
            .is_some_and(|patch| patch.contains_key("headers"))
        {
            provider.headers = match params["patch"].get("headers") {
                Some(Value::Null) => BTreeMap::new(),
                Some(value) => {
                    let headers: BTreeMap<String, String> = decode(value.clone())?;
                    validate_headers(&headers)?;
                    headers
                }
                None => BTreeMap::new(),
            };
        }
        if params
            .get("patch")
            .and_then(Value::as_object)
            .is_some_and(|patch| patch.contains_key("api_key"))
        {
            match params["patch"].get("api_key") {
                Some(Value::String(key)) => {
                    if key.is_empty() {
                        return Err(RegistryError::Invalid("api_key must not be empty".into()));
                    }
                    self.secrets.set(&provider.id, key)?;
                    provider.has_key = true;
                }
                Some(Value::Null) => {
                    self.secrets.delete(&provider.id)?;
                    provider.has_key = false;
                }
                _ => {
                    return Err(RegistryError::Invalid(
                        "api_key must be a string or null".into(),
                    ))
                }
            }
        }
        provider.updated_at = timestamp();
        self.providers.insert(provider.id.clone(), provider.clone());
        let result = serde_json::to_value(ProviderResult {
            provider: provider.clone(),
        })
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        self.persist_mutation(
            "provider.updated",
            json!({"provider":provider,"models":self.models_for(&typed.provider_id)}),
            result,
            typed.meta.client_request_id,
        )
    }

    fn provider_delete(&mut self, params: Value) -> Result<ProviderReply> {
        let typed: ProviderIdParams = decode(params)?;
        if let Some(reply) = self.cached(&typed.meta.client_request_id) {
            return Ok(reply);
        }
        let provider = self
            .providers
            .get(&typed.provider_id)
            .cloned()
            .ok_or_else(|| RegistryError::ProviderNotFound(typed.provider_id.clone()))?;
        // Remove the credential first. An unavailable keychain must leave the
        // provider visible so a later retry can complete the deletion.
        self.secrets.delete(&provider.id)?;
        self.providers.remove(&typed.provider_id);
        self.models
            .retain(|_, model| model.provider_id != provider.id);
        let result = json!({});
        self.persist_mutation(
            "provider.deleted",
            json!({"provider_id":provider.id}),
            result,
            typed.meta.client_request_id,
        )
    }

    async fn provider_test(&self, params: Value) -> Result<ProviderReply> {
        let typed: ProviderIdParams = decode(params.clone())?;
        let provider = self.provider_config(&typed.provider_id)?;
        let started = Instant::now();
        let http = HttpProvider::new(provider, self.secrets.clone());
        let model = self
            .models
            .values()
            .find(|model| model.provider_id == typed.provider_id && model.enabled)
            .map(|model| model.model_id.clone());
        let result = async {
            let model = match model {
                Some(model) => model,
                None => http
                    .models()
                    .await?
                    .into_iter()
                    .find_map(|remote| {
                        remote
                            .get("id")
                            .or_else(|| remote.get("name"))
                            .and_then(Value::as_str)
                            .map(|id| id.trim_start_matches("models/").to_owned())
                    })
                    .ok_or_else(|| {
                        ProviderError::Response(
                            "no available model; register one with model.upsert".into(),
                        )
                    })?,
            };
            http.test(&model).await
        }
        .await;
        let value = ProviderTestResult {
            ok: result.is_ok(),
            latency_ms: started.elapsed().as_millis() as u64,
            error: result.err().map(|error| error.to_string()),
        };
        Ok(ProviderReply {
            result: serde_json::to_value(value)
                .map_err(|error| RegistryError::Invalid(error.to_string()))?,
            events: Vec::new(),
        })
    }

    async fn model_refresh(&mut self, params: Value) -> Result<ProviderReply> {
        let typed: ProviderIdParams = decode(params)?;
        if let Some(reply) = self.cached(&typed.meta.client_request_id) {
            return Ok(reply);
        }
        let provider = self.provider_config(&typed.provider_id)?;
        let remote_models = HttpProvider::new(provider, self.secrets.clone())
            .models()
            .await?;
        let mut refreshed = Vec::new();
        for remote in remote_models {
            let Some(model_id) = remote
                .get("id")
                .or_else(|| remote.get("name"))
                .and_then(Value::as_str)
                .map(|id| id.trim_start_matches("models/"))
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let reference = format!("{}/{}", typed.provider_id, model_id);
            let old = self.models.get(&reference);
            refreshed.push(Model {
                r#ref: reference.clone(),
                provider_id: typed.provider_id.clone(),
                model_id: model_id.to_string(),
                display_name: old
                    .map(|model| model.display_name.clone())
                    .unwrap_or_else(|| model_id.to_string()),
                context_window: old.map(|model| model.context_window).unwrap_or(128_000),
                max_output: old.map(|model| model.max_output).unwrap_or(8_192),
                caps: old.map(|model| model.caps.clone()).unwrap_or(ModelCaps {
                    vision: true,
                    tools: true,
                    reasoning: false,
                }),
                price: old.and_then(|model| model.price.clone()),
                enabled: old.map(|model| model.enabled).unwrap_or(true),
            });
        }
        self.models
            .retain(|_, model| model.provider_id != typed.provider_id);
        for model in &refreshed {
            self.models.insert(model.r#ref.clone(), model.clone());
        }
        let result = serde_json::to_value(ModelRefreshResult {
            models: refreshed.clone(),
        })
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        self.persist_mutation(
            "provider.updated",
            json!({"provider":self.providers[&typed.provider_id],"models":refreshed}),
            result,
            typed.meta.client_request_id,
        )
    }

    fn model_upsert(&mut self, params: Value) -> Result<ProviderReply> {
        let typed: ModelUpsertParams = decode(params)?;
        if let Some(reply) = self.cached(&typed.meta.client_request_id) {
            return Ok(reply);
        }
        if !self.providers.contains_key(&typed.provider_id) {
            return Err(RegistryError::ProviderNotFound(typed.provider_id));
        }
        validate_model_parts(&typed.model_id, typed.context_window, typed.max_output)?;
        let expected_ref = format!("{}/{}", typed.provider_id, typed.model_id);
        let model_ref = match typed.r#ref {
            Patch::Unset | Patch::Null => expected_ref.clone(),
            Patch::Value(reference) if reference == expected_ref => reference,
            Patch::Value(reference) => {
                return Err(RegistryError::Invalid(format!(
                    "model ref must be {expected_ref}, got {reference}"
                )))
            }
        };
        let old = self.models.get(&model_ref);
        let price = match typed.price {
            Patch::Unset => old.and_then(|model| model.price.clone()),
            Patch::Null => None,
            Patch::Value(price) => {
                validate_price(&price)?;
                Some(price)
            }
        };
        let model = Model {
            r#ref: model_ref.clone(),
            provider_id: typed.provider_id.clone(),
            model_id: typed.model_id.clone(),
            display_name: typed.display_name.unwrap_or_else(|| {
                old.map(|model| model.display_name.clone())
                    .unwrap_or_else(|| typed.model_id.clone())
            }),
            context_window: typed
                .context_window
                .or_else(|| old.map(|model| model.context_window))
                .unwrap_or(128_000),
            max_output: typed
                .max_output
                .or_else(|| old.map(|model| model.max_output))
                .unwrap_or(8_192),
            caps: typed
                .caps
                .or_else(|| old.map(|model| model.caps.clone()))
                .unwrap_or(ModelCaps {
                    vision: true,
                    tools: true,
                    reasoning: false,
                }),
            price,
            enabled: typed
                .enabled
                .or_else(|| old.map(|model| model.enabled))
                .unwrap_or(true),
        };
        self.models.insert(model_ref, model.clone());
        let result = serde_json::to_value(ModelResult {
            model: model.clone(),
        })
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        self.persist_mutation(
            "provider.updated",
            json!({"provider":self.providers[&typed.provider_id],"models":self.models_for(&typed.provider_id)}),
            result,
            typed.meta.client_request_id,
        )
    }

    fn model_delete(&mut self, params: Value, in_use_models: &[String]) -> Result<ProviderReply> {
        let typed: ModelDeleteParams = decode(params)?;
        if let Some(reply) = self.cached(&typed.meta.client_request_id) {
            return Ok(reply);
        }
        if in_use_models
            .iter()
            .any(|reference| reference == &typed.r#ref)
        {
            return Err(RegistryError::Conflict(format!(
                "model {} is in use",
                typed.r#ref
            )));
        }
        let model = self
            .models
            .get(&typed.r#ref)
            .cloned()
            .ok_or_else(|| RegistryError::ModelNotFound(typed.r#ref.clone()))?;
        let provider = self
            .providers
            .get(&model.provider_id)
            .cloned()
            .ok_or_else(|| RegistryError::ProviderNotFound(model.provider_id.clone()))?;
        self.models.remove(&typed.r#ref);
        let result = json!({});
        self.persist_mutation(
            "provider.updated",
            json!({"provider":provider,"models":self.models_for(&model.provider_id)}),
            result,
            typed.meta.client_request_id,
        )
    }

    fn provider_config(&self, id: &str) -> Result<ProviderConfig> {
        let provider = self
            .providers
            .get(id)
            .ok_or_else(|| RegistryError::ProviderNotFound(id.to_string()))?;
        Ok(ProviderConfig {
            id: provider.id.clone(),
            name: provider.name.clone(),
            api_kind: local_api_kind(provider.api_kind.clone()),
            base_url: provider.base_url.clone(),
            headers: provider.headers.clone(),
        })
    }

    fn models_for(&self, provider_id: &str) -> Vec<Model> {
        self.models
            .values()
            .filter(|model| model.provider_id == provider_id)
            .cloned()
            .collect()
    }

    fn cached(&self, request_id: &Option<String>) -> Option<ProviderReply> {
        request_id.as_ref().and_then(|id| {
            self.idempotency.get(id).map(|result| ProviderReply {
                result: result.clone(),
                events: Vec::new(),
            })
        })
    }

    fn persist_mutation(
        &mut self,
        event_name: &str,
        mut event_data: Value,
        result: Value,
        request_id: Option<String>,
    ) -> Result<ProviderReply> {
        let mut idempotency = self.idempotency.clone();
        if let Some(request_id) = request_id {
            idempotency.insert(request_id, result.clone());
        }
        let metadata = serde_json::to_value(RegistrySnapshot {
            providers: self.providers.values().cloned().collect(),
            models: self.models.values().cloned().collect(),
            idempotency,
        })
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        event_data["_registry"] = metadata.clone();
        let event = self.store.append_event(event_name, event_data)?;
        let snapshot = RegistrySnapshot::from_value(metadata)?;
        self.store
            .write_snapshot("data/providers.json", &snapshot.providers)?;
        self.store
            .write_snapshot("data/models.json", &snapshot.models)?;
        self.store
            .write_snapshot("data/provider-registry.json", &snapshot)?;
        self.idempotency = snapshot.idempotency;
        Ok(ProviderReply {
            result,
            events: vec![strip_registry(event)],
        })
    }
}

impl RegistrySnapshot {
    fn from_value(value: Value) -> Result<Self> {
        serde_json::from_value(value)
            .map_err(|error| RegistryError::Invalid(format!("registry metadata: {error}")))
    }
}

fn strip_registry(mut event: Event) -> Event {
    if let Some(object) = event.data.as_object_mut() {
        object.remove("_registry");
    }
    event
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| RegistryError::Invalid(error.to_string()))
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(RegistryError::Invalid("name must not be empty".into()));
    }
    Ok(())
}

fn validate_base_url(base_url: &str) -> Result<()> {
    let url = reqwest::Url::parse(base_url)
        .map_err(|error| RegistryError::Invalid(format!("invalid base_url: {error}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RegistryError::Invalid(
            "base_url must be an http(s) URL without credentials".into(),
        ));
    }
    Ok(())
}

fn validate_headers(headers: &BTreeMap<String, String>) -> Result<()> {
    for name in headers.keys() {
        let normalized = name.to_ascii_lowercase();
        if normalized == "authorization"
            || normalized.contains("api-key")
            || normalized.contains("token")
            || normalized.contains("secret")
            || normalized.contains("password")
        {
            return Err(RegistryError::Invalid(
                "credential headers must be supplied through api_key".into(),
            ));
        }
    }
    Ok(())
}

fn validate_provider_patch(patch: &ProviderPatch, params: &Value) -> Result<()> {
    let object = params
        .get("patch")
        .and_then(Value::as_object)
        .ok_or_else(|| RegistryError::Invalid("patch must be an object".into()))?;
    for field in ["name", "base_url"] {
        if object.get(field).is_some_and(Value::is_null) {
            return Err(RegistryError::Invalid(format!("{field} cannot be null")));
        }
    }
    if patch.name.is_none()
        && patch.base_url.is_none()
        && patch.api_key.is_none()
        && patch.headers.is_none()
        && object.is_empty()
    {
        return Err(RegistryError::Invalid("provider patch is empty".into()));
    }
    Ok(())
}

fn validate_model_parts(
    model_id: &str,
    context_window: Option<u64>,
    max_output: Option<u64>,
) -> Result<()> {
    if model_id.trim().is_empty() || model_id.chars().any(char::is_control) {
        return Err(RegistryError::Invalid(
            "model_id must be non-empty and contain no control characters".into(),
        ));
    }
    if context_window == Some(0) || max_output == Some(0) {
        return Err(RegistryError::Invalid(
            "model limits must be positive".into(),
        ));
    }
    Ok(())
}

fn validate_price(price: &ModelPrice) -> Result<()> {
    let values = [
        price.input_per_mtok,
        price.output_per_mtok,
        price.cache_read_per_mtok,
        price.cache_write_per_mtok,
    ];
    if values
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(RegistryError::Invalid(
            "model price must be finite and non-negative".into(),
        ));
    }
    Ok(())
}

fn local_api_kind(kind: ProtocolApiKind) -> ApiKind {
    match kind {
        ProtocolApiKind::OpenaiCompletions => ApiKind::OpenaiCompletions,
        ProtocolApiKind::OpenaiResponses => ApiKind::OpenaiResponses,
        ProtocolApiKind::AnthropicMessages => ApiKind::AnthropicMessages,
        ProtocolApiKind::GoogleGenerative => ApiKind::GoogleGenerative,
    }
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySecrets;
    use axum::{routing::get, Json, Router};
    use std::sync::Arc;
    use tempfile::tempdir;

    fn registry() -> (tempfile::TempDir, ProviderRegistry) {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let registry =
            ProviderRegistry::from_store(store, Arc::new(MemorySecrets::default())).unwrap();
        (dir, registry)
    }

    fn create_params(cid: &str, base_url: &str) -> Value {
        json!({"name":"Fake","api_kind":"openai-completions","base_url":base_url,"api_key":"secret-value","client_request_id":cid})
    }

    #[tokio::test]
    async fn model_ref_keeps_namespaced_provider_model_id() {
        let (_dir, mut registry) = registry();
        let created = registry
            .rpc(
                "provider.create",
                create_params("p", "http://127.0.0.1:1/v1"),
                &[],
            )
            .await
            .unwrap();
        let provider_id = created.result["provider"]["id"].as_str().unwrap();
        let reply = registry
            .rpc(
                "model.upsert",
                json!({"provider_id":provider_id,"model_id":"vendor/model"}),
                &[],
            )
            .await
            .unwrap();
        let reference = reply.result["model"]["ref"].as_str().unwrap();
        let (model, _) = registry.resolve_model(reference).unwrap();
        assert_eq!(model.model_id, "vendor/model");
        assert_eq!(model.r#ref, format!("{provider_id}/vendor/model"));
    }
    #[tokio::test]
    async fn crud_wal_restart_and_idempotency_never_persist_key() {
        let (dir, mut registry) = registry();
        let created = registry
            .rpc(
                "provider.create",
                create_params("create-1", "http://127.0.0.1:1/v1"),
                &[],
            )
            .await
            .unwrap();
        let provider: Provider =
            serde_json::from_value(created.result["provider"].clone()).unwrap();
        assert!(!created.result.to_string().contains("secret-value"));
        assert!(!created.events[0].data.to_string().contains("secret-value"));
        registry
            .rpc("model.upsert", json!({"provider_id":provider.id,"model_id":"fake","client_request_id":"model-1","price":{"input_per_mtok":1.0,"output_per_mtok":2.0,"cache_read_per_mtok":0.0,"cache_write_per_mtok":0.0}}), &[])
            .await
            .unwrap();
        let duplicate = registry
            .rpc(
                "provider.create",
                create_params("create-1", "http://bad.invalid"),
                &[],
            )
            .await
            .unwrap();
        assert_eq!(duplicate.result, created.result);
        let event_count = registry.store.events_since(0).unwrap().len();
        assert_eq!(event_count, 2);
        drop(registry);
        let store = Store::open(dir.path()).unwrap();
        let registry =
            ProviderRegistry::from_store(store, Arc::new(MemorySecrets::default())).unwrap();
        assert_eq!(registry.providers().count(), 1);
        assert_eq!(registry.models().count(), 1);
        let data = std::fs::read_dir(dir.path().join("data"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect::<String>();
        assert!(!data.contains("secret-value"));
    }

    #[tokio::test]
    async fn delete_rejects_in_use_model_and_accepts_after_release() {
        let (_dir, mut registry) = registry();
        let created = registry
            .rpc(
                "provider.create",
                create_params("p", "http://127.0.0.1:1/v1"),
                &[],
            )
            .await
            .unwrap();
        let provider: Provider =
            serde_json::from_value(created.result["provider"].clone()).unwrap();
        let model_ref = format!("{}/fake", provider.id);
        registry
            .rpc(
                "model.upsert",
                json!({"provider_id":provider.id,"model_id":"fake"}),
                &[],
            )
            .await
            .unwrap();
        let error = registry
            .rpc(
                "model.delete",
                json!({"ref":model_ref}),
                &[format!("{}/fake", provider.id)],
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RegistryError::Conflict(_)));
    }

    #[tokio::test]
    async fn refresh_uses_local_fake_models_endpoint() {
        let app = Router::new().route(
            "/models",
            get(|| async { Json(json!({"data":[{"id":"fake-a"},{"id":"fake-b"}]})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (_dir, mut registry) = registry();
        let created = registry
            .rpc(
                "provider.create",
                create_params("p", &format!("http://{addr}")),
                &[],
            )
            .await
            .unwrap();
        let provider: Provider =
            serde_json::from_value(created.result["provider"].clone()).unwrap();
        let refreshed = registry
            .rpc("model.refresh", json!({"provider_id":provider.id}), &[])
            .await
            .unwrap();
        assert_eq!(refreshed.result["models"].as_array().unwrap().len(), 2);
        server.abort();
    }
}
