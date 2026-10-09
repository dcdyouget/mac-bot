//! Settings presentation and semantic actions.
//!
//! The shell owns transport and navigation.  This view only owns retained
//! controls and turns user intent into protocol-shaped RPC events.

use chrono::Utc;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    form::{Field, Form},
    input::{Input, InputState, Textarea, TextareaState},
};
use gpui_kit::gpui::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_kit::prelude::FluentBuilder;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::settings_i18n::text as t;
use crate::tokens::Tokens;

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsAction {
    Rpc { method: String, params: Value },
    HostConnections,
    CheckUpdate,
    Theme(String),
    UpdateUrl(String),
    Local { key: String, value: Value },
    Notice(String),
}

struct ApprovalRuleEditor {
    id: String,
    created_at: String,
    kind: String,
    text: Entity<InputState>,
}

pub struct SettingsView {
    data: Value,
    inputs_synced: bool,
    selected_provider: Option<String>,
    selected_model: Option<String>,
    provider_name: Entity<InputState>,
    provider_url: Entity<InputState>,
    provider_key: Entity<InputState>,
    provider_headers: Entity<TextareaState>,
    model_id: Entity<InputState>,
    model_display: Entity<InputState>,
    model_context: Entity<InputState>,
    model_max_output: Entity<InputState>,
    model_caps: Entity<TextareaState>,
    model_price_in: Entity<InputState>,
    model_price_out: Entity<InputState>,
    model_price_cache_read: Entity<InputState>,
    model_price_cache_write: Entity<InputState>,
    global_limit: Entity<InputState>,
    bot_limit: Entity<InputState>,
    subagent_limit: Entity<InputState>,
    subagent_global: Entity<InputState>,
    loop_hops: Entity<InputState>,
    default_bot: Entity<InputState>,
    default_main: Entity<InputState>,
    default_subagent: Entity<InputState>,
    default_maintenance: Entity<InputState>,
    extra_dirs: Entity<TextareaState>,
    browser_mode: Entity<InputState>,
    chrome_profile: Entity<InputState>,
    stream_max_width: Entity<InputState>,
    stream_quality: Entity<InputState>,
    stream_max_fps: Entity<InputState>,
    update_url: Entity<InputState>,
    provider_kind: String,
    approval_mode: String,
    approval_rules: Vec<ApprovalRuleEditor>,
    rules_synced: bool,
    trace_full: bool,
    local_theme: String,
    notifications: bool,
    launch_at_login: bool,
}

impl EventEmitter<SettingsAction> for SettingsView {}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let local = crate::local_settings::load().unwrap_or_default();
        let update_url = input_state(window, cx, t("update_url_placeholder"));
        let configured_url = std::env::var("MACBOT_UPDATE_URL")
            .ok()
            .or_else(|| crate::update::load_update_url().ok().flatten());
        if let Some(url) = configured_url {
            set_input(&update_url, url, window, cx);
        }
        Self {
            data: Value::Null,
            inputs_synced: false,
            selected_provider: None,
            selected_model: None,
            provider_name: input_state(window, cx, t("provider_name")),
            provider_url: input_state(window, cx, t("provider_url")),
            provider_key: input_state(window, cx, t("provider_key")),
            provider_headers: textarea_state(window, cx, "{}"),
            model_id: input_state(window, cx, t("model_id")),
            model_display: input_state(window, cx, t("model_display")),
            model_context: input_state(window, cx, "32768"),
            model_max_output: input_state(window, cx, "4096"),
            model_caps: textarea_state(
                window,
                cx,
                r#"{"vision":false,"tools":true,"reasoning":false}"#,
            ),
            model_price_in: input_state(window, cx, "0"),
            model_price_out: input_state(window, cx, "0"),
            model_price_cache_read: input_state(window, cx, "0"),
            model_price_cache_write: input_state(window, cx, "0"),
            global_limit: input_state(window, cx, "8"),
            bot_limit: input_state(window, cx, "3"),
            subagent_limit: input_state(window, cx, "4"),
            subagent_global: input_state(window, cx, "8"),
            loop_hops: input_state(window, cx, "8"),
            default_bot: input_state(window, cx, "provider/model"),
            default_main: input_state(window, cx, "provider/model"),
            default_subagent: input_state(window, cx, t("default_placeholder")),
            default_maintenance: input_state(window, cx, "provider/model"),
            extra_dirs: textarea_state(window, cx, "/path/to/workspace"),
            browser_mode: input_state(window, cx, "headless"),
            chrome_profile: input_state(window, cx, "Default"),
            stream_max_width: input_state(window, cx, "1280"),
            stream_quality: input_state(window, cx, "75"),
            stream_max_fps: input_state(window, cx, "15"),
            update_url,
            provider_kind: "openai-completions".into(),
            approval_mode: "require".into(),
            approval_rules: Vec::new(),
            rules_synced: false,
            trace_full: false,
            local_theme: local.theme,
            notifications: local.notifications,
            launch_at_login: local.launch_at_login,
        }
    }

    pub fn update_data(&mut self, data: Value, cx: &mut Context<Self>) {
        if let Some(local) = data
            .get("local")
            .or_else(|| data.get("local_settings"))
            .filter(|value| value.is_object())
            && let Ok(local) =
                serde_json::from_value::<crate::local_settings::LocalSettings>(local.clone())
        {
            self.local_theme = local.theme;
            self.notifications = local.notifications;
            self.launch_at_login = local.launch_at_login;
        }
        self.data = data;
        if self.selected_provider.is_none() {
            self.selected_provider = list(&self.data, "providers")
                .first()
                .and_then(|value| id(value))
                .map(str::to_owned);
        }
        if self.selected_model.is_none() {
            self.selected_model = list(&self.data, "models")
                .first()
                .and_then(|value| model_ref(value))
                .map(str::to_owned);
        }
        let settings = self.data.get("settings").unwrap_or(&self.data);
        self.approval_mode = string(settings, &["approvals", "mode"], "require");
        self.trace_full = boolean(settings, &["trace", "save_full_requests"], false);
        cx.notify();
    }

    /// Populate retained controls from the server snapshot once.
    ///
    /// Settings events can arrive while the user is editing a field.  Keep
    /// the initial hydration separate from `update_data` so those events do
    /// not replace in-progress edits.  Provider and model selection use the
    /// same prefill path as an explicit user selection.
    pub fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inputs_synced {
            return;
        }
        // `sync_views` publishes the provider list and settings result
        // independently.  Do not hydrate defaults while `settings.get` is
        // still pending, or the later server snapshot would be mistaken for
        // an edit and ignored.
        let settings = match self.data.get("settings") {
            Some(settings) if settings.is_object() => settings,
            Some(_) => return,
            None if self.data.get("concurrency").is_some() => &self.data,
            None => return,
        };
        self.inputs_synced = true;

        let concurrency = settings.get("concurrency").unwrap_or(&Value::Null);
        let models = settings.get("models").unwrap_or(&Value::Null);
        let skills = settings.get("skills").unwrap_or(&Value::Null);
        let browser = settings.get("browser").unwrap_or(&Value::Null);
        let stream = browser.get("stream").unwrap_or(&Value::Null);
        let desktop_stream = stream.get("desktop").unwrap_or(&Value::Null);
        let global = value_string(concurrency.get("global"), "8");
        let bot_default = value_string(concurrency.get("bot_default"), "3");
        let subagent_per_run = value_string(concurrency.get("subagent_per_run"), "4");
        let subagent_global = value_string(concurrency.get("subagent_global"), "8");
        let loop_hops = value_string(concurrency.get("loop_hops"), "8");
        let default_bot = value_string(models.get("bot_default"), "");
        let default_main = value_string(models.get("main"), "");
        let default_subagent = value_string(models.get("subagent"), "");
        let default_maintenance = value_string(models.get("maintenance"), "");
        let extra_dirs = skills
            .get("extra_dirs")
            .and_then(Value::as_array)
            .map(|dirs| {
                dirs.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let browser_mode = value_string(browser.get("default_mode"), "headless");
        let chrome_profile = value_string(browser.get("chrome_profile"), "Default");
        let stream_max_width = value_string(desktop_stream.get("max_width"), "1280");
        let stream_quality = value_string(desktop_stream.get("quality"), "75");
        let stream_max_fps = value_string(desktop_stream.get("max_fps"), "15");
        let server_rules = settings
            .get("approvals")
            .and_then(|approvals| approvals.get("rules"))
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .map(|rule| {
                        (
                            rule.get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(new_rule_id),
                            rule.get("created_at")
                                .and_then(Value::as_str)
                                .filter(|created_at| !created_at.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(new_rule_created_at),
                            match rule.get("kind").and_then(Value::as_str) {
                                Some("auto_allow") => "auto_allow".to_owned(),
                                _ => "ask_first".to_owned(),
                            },
                            rule.get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        set_input(&self.global_limit, global, window, cx);
        set_input(&self.bot_limit, bot_default, window, cx);
        set_input(&self.subagent_limit, subagent_per_run, window, cx);
        set_input(&self.subagent_global, subagent_global, window, cx);
        set_input(&self.loop_hops, loop_hops, window, cx);
        set_input(&self.default_bot, default_bot, window, cx);
        set_input(&self.default_main, default_main, window, cx);
        set_input(&self.default_subagent, default_subagent, window, cx);
        set_input(&self.default_maintenance, default_maintenance, window, cx);
        set_textarea(&self.extra_dirs, extra_dirs, window, cx);
        set_input(&self.browser_mode, browser_mode, window, cx);
        set_input(&self.chrome_profile, chrome_profile, window, cx);
        set_input(&self.stream_max_width, stream_max_width, window, cx);
        set_input(&self.stream_quality, stream_quality, window, cx);
        set_input(&self.stream_max_fps, stream_max_fps, window, cx);

        if !self.rules_synced {
            self.approval_rules = server_rules
                .into_iter()
                .map(|(id, created_at, kind, text)| {
                    let text_state = input_state(window, cx, t("approval_rule_text_placeholder"));
                    set_input(&text_state, text, window, cx);
                    ApprovalRuleEditor {
                        id,
                        created_at,
                        kind,
                        text: text_state,
                    }
                })
                .collect();
            self.rules_synced = true;
        }

        let model_ref = self.selected_model.clone();
        if let Some(model_ref) = model_ref {
            self.select_model(model_ref, window, cx);
        }
        // A model can point at a different provider than the first provider
        // in the snapshot.  Resolve the model first, then prefill that
        // provider so the two editor sections stay in sync.
        if let Some(provider_id) = self.selected_provider.clone() {
            self.select_provider(provider_id, window, cx);
        }
        cx.notify();
    }

    fn emit_rpc(&mut self, method: &str, params: Value, cx: &mut Context<Self>) {
        cx.emit(SettingsAction::Rpc {
            method: method.into(),
            params,
        });
    }

    fn select_provider(
        &mut self,
        provider_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_provider = Some(provider_id.clone());
        // API keys are write-only in the protocol.  Never carry a key typed
        // for one provider into another provider's update request.
        set_input(&self.provider_key, String::new(), window, cx);
        if let Some(provider) = list(&self.data, "providers")
            .into_iter()
            .find(|provider| id(provider) == Some(provider_id.as_str()))
        {
            set_input(
                &self.provider_name,
                string_value(provider, "name", ""),
                window,
                cx,
            );
            set_input(
                &self.provider_url,
                string_value(provider, "base_url", ""),
                window,
                cx,
            );
            self.provider_kind = string_value(provider, "api_kind", "openai-completions");
            let headers = provider
                .get("headers")
                .cloned()
                .unwrap_or_else(|| json!({}));
            set_textarea(&self.provider_headers, headers.to_string(), window, cx);
        }
        cx.notify();
    }

    fn select_model(&mut self, reference: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_model = Some(reference.clone());
        if let Some(model) = list(&self.data, "models")
            .into_iter()
            .find(|model| model_ref(model) == Some(reference.as_str()))
        {
            set_input(
                &self.model_id,
                string_value(model, "model_id", ""),
                window,
                cx,
            );
            set_input(
                &self.model_display,
                string_value(model, "display_name", ""),
                window,
                cx,
            );
            set_input(
                &self.model_context,
                model
                    .get("context_window")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .to_string(),
                window,
                cx,
            );
            set_input(
                &self.model_max_output,
                model
                    .get("max_output")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .to_string(),
                window,
                cx,
            );
            set_textarea(
                &self.model_caps,
                model
                    .get("caps")
                    .cloned()
                    .unwrap_or_else(|| json!({}))
                    .to_string(),
                window,
                cx,
            );
            let price = model.get("price").unwrap_or(&Value::Null);
            set_input(
                &self.model_price_in,
                price
                    .get("input_per_mtok")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.)
                    .to_string(),
                window,
                cx,
            );
            set_input(
                &self.model_price_out,
                price
                    .get("output_per_mtok")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.)
                    .to_string(),
                window,
                cx,
            );
            set_input(
                &self.model_price_cache_read,
                price
                    .get("cache_read_per_mtok")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.)
                    .to_string(),
                window,
                cx,
            );
            set_input(
                &self.model_price_cache_write,
                price
                    .get("cache_write_per_mtok")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.)
                    .to_string(),
                window,
                cx,
            );
            self.selected_provider = model
                .get("provider_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        cx.notify();
    }

    fn provider_params(&self, cx: &App) -> Value {
        let headers = parse_json(self.provider_headers.read(cx).value().as_ref())
            .unwrap_or_else(|| json!({}));
        let mut params = json!({"name": self.provider_name.read(cx).value(), "api_kind": self.provider_kind,
            "base_url": self.provider_url.read(cx).value(), "headers": headers});
        let key = self.provider_key.read(cx).value();
        if !key.trim().is_empty() {
            params["api_key"] = json!(key);
        }
        params
    }

    fn provider_patch(&self, cx: &App) -> Value {
        let headers = parse_json(self.provider_headers.read(cx).value().as_ref())
            .unwrap_or_else(|| json!({}));
        let mut patch = json!({"name": self.provider_name.read(cx).value(),
            "base_url": self.provider_url.read(cx).value(), "headers": headers});
        let key = self.provider_key.read(cx).value();
        if !key.trim().is_empty() {
            patch["api_key"] = json!(key);
        }
        patch
    }

    fn model_params(&self, cx: &App) -> Value {
        let caps =
            parse_json(self.model_caps.read(cx).value().as_ref()).unwrap_or_else(|| json!({}));
        let provider_id = self.selected_provider.clone().or_else(|| {
            list(&self.data, "providers")
                .first()
                .and_then(|provider| id(provider))
                .map(str::to_owned)
        });
        let mut params = json!({"provider_id": provider_id.unwrap_or_default(),
            "model_id": self.model_id.read(cx).value(), "display_name": self.model_display.read(cx).value(),
            "context_window": number(self.model_context.read(cx).value().as_ref()),
            "max_output": number(self.model_max_output.read(cx).value().as_ref()), "caps": caps,
            "price": {"input_per_mtok": decimal(self.model_price_in.read(cx).value().as_ref()),
                "output_per_mtok": decimal(self.model_price_out.read(cx).value().as_ref()),
                "cache_read_per_mtok": decimal(self.model_price_cache_read.read(cx).value().as_ref()),
                "cache_write_per_mtok": decimal(self.model_price_cache_write.read(cx).value().as_ref())}, "enabled": true});
        if let Some(reference) = self.selected_model.as_deref() {
            params["ref"] = json!(reference);
        }
        params
    }

    fn provider_validation_notice(&self, cx: &App) -> Option<&'static str> {
        if parse_json(self.provider_headers.read(cx).value().as_ref())
            .is_none_or(|value| !value.is_object())
        {
            return Some("provider_headers_invalid");
        }
        None
    }

    fn model_validation_notice(&self, cx: &App) -> Option<&'static str> {
        if parse_json(self.model_caps.read(cx).value().as_ref())
            .is_none_or(|value| !value.is_object())
        {
            return Some("model_caps_invalid");
        }
        for state in [&self.model_context, &self.model_max_output] {
            if !valid_integer(state.read(cx).value().as_ref()) {
                return Some("settings_number_invalid");
            }
        }
        for state in [
            &self.model_price_in,
            &self.model_price_out,
            &self.model_price_cache_read,
            &self.model_price_cache_write,
        ] {
            if !valid_decimal(state.read(cx).value().as_ref()) {
                return Some("model_price_invalid");
            }
        }
        None
    }

    fn settings_validation_notice(&self, cx: &App) -> Option<&'static str> {
        for state in [
            &self.global_limit,
            &self.bot_limit,
            &self.subagent_limit,
            &self.subagent_global,
            &self.loop_hops,
            &self.stream_max_width,
            &self.stream_quality,
            &self.stream_max_fps,
        ] {
            if !valid_integer(state.read(cx).value().as_ref()) {
                return Some("settings_number_invalid");
            }
        }
        None
    }

    fn settings_patch(&self, cx: &App) -> Value {
        let dirs = self
            .extra_dirs
            .read(cx)
            .value()
            .split('\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let settings = self.data.get("settings").unwrap_or(&self.data);
        let rules = if self.rules_synced {
            Value::Array(
                self.approval_rules
                    .iter()
                    .map(|rule| {
                        json!({
                            "id": rule.id,
                            "created_at": rule.created_at,
                            "kind": rule.kind,
                            "text": rule.text.read(cx).value(),
                        })
                    })
                    .collect(),
            )
        } else {
            settings
                .get("approvals")
                .and_then(|approvals| approvals.get("rules"))
                .cloned()
                .unwrap_or_else(|| json!([]))
        };
        json!({"concurrency": {"global": number(self.global_limit.read(cx).value().as_ref()),
            "bot_default": number(self.bot_limit.read(cx).value().as_ref()),
            "subagent_per_run": number(self.subagent_limit.read(cx).value().as_ref()),
            "subagent_global": number(self.subagent_global.read(cx).value().as_ref()),
            "loop_hops": number(self.loop_hops.read(cx).value().as_ref())},
                "models": {"bot_default": value_or_null(self.default_bot.read(cx).value().as_ref()),
                "main": value_or_null(self.default_main.read(cx).value().as_ref()),
                "subagent": value_or_null(self.default_subagent.read(cx).value().as_ref()),
                "maintenance": value_or_null(self.default_maintenance.read(cx).value().as_ref())},
            "approvals": {"mode": self.approval_mode, "rules": rules},
            "skills": {"extra_dirs": dirs},
            "browser": {"default_mode": self.browser_mode.read(cx).value(),
                "chrome_profile": self.chrome_profile.read(cx).value(),
                "stream": {"desktop": {"max_width": number(self.stream_max_width.read(cx).value().as_ref()),
                    "quality": number(self.stream_quality.read(cx).value().as_ref()),
                    "max_fps": number(self.stream_max_fps.read(cx).value().as_ref())}}},
            "trace": {"save_full_requests": self.trace_full}})
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = Tokens::get(&*cx);
        let providers = list(&self.data, "providers");
        let models = list(&self.data, "models");
        let provider_buttons = providers.iter().enumerate().map(|(index, provider)| {
            let provider_id = id(provider).unwrap_or_default().to_owned();
            let title = string_value(provider, "name", &provider_id);
            let selected = self.selected_provider.as_deref() == Some(provider_id.as_str());
            let id_for_click = provider_id.clone();
            Button::new(format!("provider-{index}"))
                .label(title)
                .when(selected, |button| button.primary())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_provider(id_for_click.clone(), window, cx);
                }))
        });
        let model_buttons = models.iter().enumerate().map(|(index, model)| {
            let reference = model_ref(model).unwrap_or_default().to_owned();
            let title = string_value(model, "display_name", &reference);
            let selected = self.selected_model.as_deref() == Some(reference.as_str());
            let ref_for_click = reference.clone();
            Button::new(format!("model-{index}"))
                .label(title)
                .when(selected, |button| button.primary())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_model(ref_for_click.clone(), window, cx);
                }))
        });
        let kind_buttons = [
            ("openai-completions", "provider_kind_openai_completions"),
            ("openai-responses", "provider_kind_openai_responses"),
            ("anthropic-messages", "provider_kind_anthropic"),
            ("google-generative", "provider_kind_google"),
        ]
        .into_iter()
        .map(|(kind, label)| {
            let active = self.provider_kind == kind;
            Button::new(format!("api-kind-{kind}"))
                .label(t(label))
                .when(active, |b| b.primary())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.provider_kind = kind.into();
                    cx.notify();
                }))
        });
        let provider_id = self.selected_provider.clone();
        let model_ref_for_delete = self.selected_model.clone();
        let provider_actions = div()
            .flex()
            .gap_2()
            .child(
                Button::new("provider-new")
                    .label(t("provider_new"))
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.selected_provider = None;
                        set_input(&this.provider_name, String::new(), window, cx);
                        set_input(&this.provider_url, String::new(), window, cx);
                        set_input(&this.provider_key, String::new(), window, cx);
                        set_textarea(&this.provider_headers, "{}".into(), window, cx);
                        this.provider_kind = "openai-completions".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("provider-save")
                    .label(if provider_id.is_some() {
                        t("provider_update")
                    } else {
                        t("provider_save")
                    })
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(key) = this.provider_validation_notice(cx) {
                            cx.emit(SettingsAction::Notice(t(key).into()));
                            return;
                        }
                        let create_params = this.provider_params(cx);
                        let method = if this.selected_provider.is_some() {
                            "provider.update"
                        } else {
                            "provider.create"
                        };
                        let params = if let Some(provider_id) = this.selected_provider.clone() {
                            json!({"provider_id": provider_id, "patch": this.provider_patch(cx)})
                        } else {
                            create_params
                        };
                        this.emit_rpc(method, params, cx);
                    })),
            )
            .child(
                Button::new("provider-test")
                    .label(t("provider_test"))
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(id) = this.selected_provider.clone() {
                            this.emit_rpc("provider.test", json!({"provider_id": id}), cx);
                        }
                    })),
            )
            .child(
                Button::new("provider-refresh")
                    .label(t("provider_refresh"))
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(id) = this.selected_provider.clone() {
                            this.emit_rpc("model.refresh", json!({"provider_id": id}), cx);
                        }
                    })),
            )
            .child(
                Button::new("provider-delete")
                    .label(t("provider_delete"))
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(id) = this.selected_provider.clone() {
                            this.emit_rpc("provider.delete", json!({"provider_id": id}), cx);
                        }
                    })),
            );
        let provider_form = Form::vertical()
            .child(
                Field::new()
                    .label(t("provider_name"))
                    .child(Input::new(&self.provider_name).id("settings-provider-name")),
            )
            .child(
                Field::new()
                    .label(t("provider_kind"))
                    .child(div().flex().gap_1().children(kind_buttons)),
            )
            .child(
                Field::new()
                    .label(t("provider_url"))
                    .child(Input::new(&self.provider_url).id("settings-provider-url")),
            )
            .child(
                Field::new().label(t("provider_key")).child(
                    Input::new(&self.provider_key)
                        .id("settings-provider-key")
                        .mask_toggle(),
                ),
            )
            .child(
                Field::new()
                    .label(t("provider_headers"))
                    .child(Textarea::new(&self.provider_headers).h(px(80.))),
            )
            .child(Field::new().child(provider_actions));
        let model_actions = div()
            .flex()
            .gap_2()
            .child(
                Button::new("model-new")
                    .label(t("model_new"))
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.selected_model = None;
                        set_input(&this.model_id, String::new(), window, cx);
                        set_input(&this.model_display, String::new(), window, cx);
                        set_input(&this.model_context, "32768".into(), window, cx);
                        set_input(&this.model_max_output, "4096".into(), window, cx);
                        set_textarea(
                            &this.model_caps,
                            r#"{"vision":false,"tools":true,"reasoning":false}"#.into(),
                            window,
                            cx,
                        );
                        set_input(&this.model_price_in, "0".into(), window, cx);
                        set_input(&this.model_price_out, "0".into(), window, cx);
                        set_input(&this.model_price_cache_read, "0".into(), window, cx);
                        set_input(&this.model_price_cache_write, "0".into(), window, cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("model-save")
                    .label(t("model_save"))
                    .primary()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(key) = this.model_validation_notice(cx) {
                            cx.emit(SettingsAction::Notice(t(key).into()));
                            return;
                        }
                        let params = this.model_params(cx);
                        this.emit_rpc("model.upsert", params, cx);
                    })),
            )
            .child(
                Button::new("model-delete")
                    .label(t("model_delete"))
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(reference) = this
                            .selected_model
                            .clone()
                            .or_else(|| model_ref_for_delete.clone())
                        {
                            this.emit_rpc("model.delete", json!({"ref": reference}), cx);
                        }
                    })),
            );
        let model_form = Form::vertical()
            .child(
                Field::new()
                    .label(t("model_id"))
                    .child(Input::new(&self.model_id).id("settings-model-id")),
            )
            .child(
                Field::new()
                    .label(t("model_display"))
                    .child(Input::new(&self.model_display).id("settings-model-display")),
            )
            .child(
                Field::new()
                    .label(t("model_context"))
                    .child(Input::new(&self.model_context).id("settings-model-context")),
            )
            .child(
                Field::new()
                    .label(t("model_max_output"))
                    .child(Input::new(&self.model_max_output).id("settings-model-max-output")),
            )
            .child(
                Field::new()
                    .label(t("model_caps"))
                    .child(Textarea::new(&self.model_caps).h(px(65.))),
            )
            .child(
                Field::new()
                    .label(t("model_price_in"))
                    .child(Input::new(&self.model_price_in).id("settings-model-price-in")),
            )
            .child(
                Field::new()
                    .label(t("model_price_out"))
                    .child(Input::new(&self.model_price_out).id("settings-model-price-out")),
            )
            .child(Field::new().label(t("model_price_cache_read")).child(
                Input::new(&self.model_price_cache_read).id("settings-model-price-cache-read"),
            ))
            .child(Field::new().label(t("model_price_cache_write")).child(
                Input::new(&self.model_price_cache_write).id("settings-model-price-cache-write"),
            ))
            .child(Field::new().child(model_actions));
        let runtime_form = Form::vertical()
            .child(number_field(
                t("global_limit"),
                &self.global_limit,
                "settings-global-limit",
            ))
            .child(number_field(
                t("bot_limit"),
                &self.bot_limit,
                "settings-bot-limit",
            ))
            .child(number_field(
                t("subagent_limit"),
                &self.subagent_limit,
                "settings-subagent-limit",
            ))
            .child(number_field(
                t("subagent_global"),
                &self.subagent_global,
                "settings-subagent-global",
            ))
            .child(number_field(
                t("loop_hops"),
                &self.loop_hops,
                "settings-loop-hops",
            ));
        let approval_mode = div()
            .flex()
            .gap_2()
            .child(
                Button::new("approval-require")
                    .label(t("approval_require"))
                    .when(self.approval_mode == "require", |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.approval_mode = "require".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("approval-always")
                    .label(t("approval_always"))
                    .when(self.approval_mode == "always_allow", |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.approval_mode = "always_allow".into();
                        cx.notify();
                    })),
            );
        let approval_rule_rows = self.approval_rules.iter().enumerate().map(|(index, rule)| {
            let ask_first = rule.kind == "ask_first";
            let auto_allow = rule.kind == "auto_allow";
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new(format!("approval-rule-{index}-ask"))
                        .label(t("approval_rule_ask"))
                        .when(ask_first, |button| button.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(rule) = this.approval_rules.get_mut(index) {
                                rule.kind = "ask_first".into();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(format!("approval-rule-{index}-allow"))
                        .label(t("approval_rule_allow"))
                        .when(auto_allow, |button| button.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(rule) = this.approval_rules.get_mut(index) {
                                rule.kind = "auto_allow".into();
                            }
                            cx.notify();
                        })),
                )
                .child(Input::new(&rule.text).id(format!("approval-rule-text-{index}")))
                .child(
                    Button::new(format!("approval-rule-{index}-delete"))
                        .label(t("approval_rule_delete"))
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if index < this.approval_rules.len() {
                                this.approval_rules.remove(index);
                            }
                            cx.notify();
                        })),
                )
        });
        let approval_rules = div()
            .flex()
            .flex_col()
            .gap_2()
            .children(approval_rule_rows)
            .child(
                Button::new("approval-rule-add")
                    .label(t("approval_rule_add"))
                    .outline()
                    .on_click(cx.listener(|this, _, window, cx| {
                        let text = input_state(window, cx, t("approval_rule_text_placeholder"));
                        this.approval_rules.push(ApprovalRuleEditor {
                            id: new_rule_id(),
                            created_at: new_rule_created_at(),
                            kind: "ask_first".into(),
                            text,
                        });
                        cx.notify();
                    })),
            );
        let approval_form = Form::vertical()
            .child(Field::new().label(t("approval_mode")).child(approval_mode))
            .child(
                Field::new()
                    .label(t("approval_rules"))
                    .child(approval_rules),
            );
        let save_settings = Button::new("settings-save")
            .label(t("save"))
            .primary()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(key) = this.settings_validation_notice(cx) {
                    cx.emit(SettingsAction::Notice(t(key).into()));
                    return;
                }
                if this
                    .approval_rules
                    .iter()
                    .any(|rule| rule.text.read(cx).value().trim().is_empty())
                {
                    cx.emit(SettingsAction::Notice(
                        t("approval_rule_text_required").into(),
                    ));
                    return;
                }
                let patch = this.settings_patch(cx);
                this.emit_rpc("settings.update", json!({"patch": patch}), cx);
            }));
        div()
            .id("settings-view")
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .overflow_y_scroll()
            .bg(tokens.window)
            .text_color(tokens.primary)
            .child(header(t("title"), t("subtitle"), &tokens))
            .child(section(
                t("providers"),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(provider_buttons)
                    .child(provider_form),
                &tokens,
            ))
            .child(section(
                t("models"),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(model_buttons)
                    .child(model_form),
                &tokens,
            ))
            .child(section(t("runtime"), runtime_form, &tokens))
            .child(section(
                t("default_models"),
                Form::vertical()
                    .child(
                        Field::new()
                            .label(t("default_bot"))
                            .child(Input::new(&self.default_bot).id("settings-default-bot")),
                    )
                    .child(
                        Field::new()
                            .label(t("default_main"))
                            .child(Input::new(&self.default_main).id("settings-default-main")),
                    )
                    .child(
                        Field::new().label(t("default_subagent")).child(
                            Input::new(&self.default_subagent).id("settings-default-subagent"),
                        ),
                    )
                    .child(Field::new().label(t("default_maintenance")).child(
                        Input::new(&self.default_maintenance).id("settings-default-maintenance"),
                    )),
                &tokens,
            ))
            .child(section(t("tools"), approval_form, &tokens))
            .child(section(
                t("browser"),
                Form::vertical()
                    .child(
                        Field::new()
                            .label(t("browser_mode"))
                            .child(Input::new(&self.browser_mode).id("settings-browser-mode")),
                    )
                    .child(
                        Field::new()
                            .label(t("chrome_profile"))
                            .child(Input::new(&self.chrome_profile).id("settings-chrome-profile")),
                    )
                    .child(number_field(
                        t("stream_max_width"),
                        &self.stream_max_width,
                        "settings-stream-max-width",
                    ))
                    .child(number_field(
                        t("stream_quality"),
                        &self.stream_quality,
                        "settings-stream-quality",
                    ))
                    .child(number_field(
                        t("stream_max_fps"),
                        &self.stream_max_fps,
                        "settings-stream-max-fps",
                    ))
                    .child(
                        Field::new().label(t("screen_permission")).child(
                            div()
                                .text_sm()
                                .text_color(tokens.secondary)
                                .child(t("screen_permission_native")),
                        ),
                    ),
                &tokens,
            ))
            .child(section(
                t("extra_dirs"),
                Form::vertical()
                    .child(
                        Field::new()
                            .label(t("extra_dirs"))
                            .child(Textarea::new(&self.extra_dirs).h(px(80.))),
                    )
                    .child(
                        Field::new().label(t("trace")).child(
                            Checkbox::new("trace-full")
                                .checked(self.trace_full)
                                .label(t("trace_full"))
                                .on_click(cx.listener(|this, checked, _, cx| {
                                    this.trace_full = *checked;
                                    cx.notify();
                                })),
                        ),
                    ),
                &tokens,
            ))
            .child(section(
                t("general"),
                Form::vertical()
                    .child(
                        Field::new().label(t("language")).child(
                            div()
                                .text_sm()
                                .text_color(tokens.secondary)
                                .child(t("language_zh_cn")),
                        ),
                    )
                    .child(
                        Field::new().label(t("notifications")).child(
                            Checkbox::new("settings-notifications")
                                .checked(self.notifications)
                                .label(t("notifications_enabled"))
                                .on_click(cx.listener(|this, checked, _, cx| {
                                    this.notifications = *checked;
                                    cx.emit(SettingsAction::Local {
                                        key: "notifications".into(),
                                        value: Value::Bool(*checked),
                                    });
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(
                        Field::new().label(t("launch_at_login")).child(
                            Checkbox::new("settings-launch-at-login")
                                .checked(self.launch_at_login)
                                .label(t("launch_at_login_enabled"))
                                .on_click(cx.listener(|this, checked, _, cx| {
                                    this.launch_at_login = *checked;
                                    cx.emit(SettingsAction::Local {
                                        key: "launch_at_login".into(),
                                        value: Value::Bool(*checked),
                                    });
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(
                        Field::new().label(t("update_url")).child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(Input::new(&self.update_url).id("settings-update-url"))
                                .child(
                                    Button::new("settings-update-url-save")
                                        .label(t("update_url_save"))
                                        .outline()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.emit(SettingsAction::UpdateUrl(
                                                this.update_url.read(cx).value().trim().to_owned(),
                                            ));
                                        })),
                                ),
                        ),
                    ),
                &tokens,
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(save_settings)
                    .child(
                        Button::new("theme-system")
                            .label(t("theme_system"))
                            .when(self.local_theme == "system", |b| b.primary())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.local_theme = "system".into();
                                cx.emit(SettingsAction::Theme("system".into()));
                                cx.emit(SettingsAction::Local {
                                    key: "theme".into(),
                                    value: Value::String("system".into()),
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("theme-light")
                            .label(t("theme_light"))
                            .when(self.local_theme == "light", |b| b.primary())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.local_theme = "light".into();
                                cx.emit(SettingsAction::Theme("light".into()));
                                cx.emit(SettingsAction::Local {
                                    key: "theme".into(),
                                    value: Value::String("light".into()),
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("theme-dark")
                            .label(t("theme_dark"))
                            .when(self.local_theme == "dark", |b| b.primary())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.local_theme = "dark".into();
                                cx.emit(SettingsAction::Theme("dark".into()));
                                cx.emit(SettingsAction::Local {
                                    key: "theme".into(),
                                    value: Value::String("dark".into()),
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("settings-host")
                            .label(t("host"))
                            .outline()
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(SettingsAction::HostConnections)),
                            ),
                    )
                    .child(
                        Button::new("settings-update")
                            .label(t("check_update"))
                            .outline()
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(SettingsAction::CheckUpdate)),
                            ),
                    )
                    .child(div().text_sm().text_color(tokens.secondary).child(format!(
                                "{} · v{} · {}",
                                t("about"),
                                crate::update::current_version(),
                                std::env::var("MACBOT_UPDATE_URL")
                                    .ok()
                                    .filter(|url| !url.trim().is_empty())
                                    .or_else(|| {
                                        crate::update::load_update_url().ok().flatten()
                                    })
                                    .unwrap_or_else(|| t("update_unconfigured").to_string())
                            ))),
            )
    }
}

fn input_state(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    placeholder: &'static str,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}
fn set_input(
    state: &Entity<InputState>,
    value: String,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) {
    state.update(cx, |state, cx| state.set_value(value, window, cx));
}
fn textarea_state(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    placeholder: &'static str,
) -> Entity<TextareaState> {
    cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder))
}
fn set_textarea(
    state: &Entity<TextareaState>,
    value: String,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) {
    state.update(cx, |state, cx| state.set_value(value, window, cx));
}
fn number_field(label: &'static str, state: &Entity<InputState>, id: &'static str) -> Field {
    Field::new().label(label).child(Input::new(state).id(id))
}
fn header(title: &'static str, subtitle: &'static str, tokens: &Tokens) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .pb_2()
        .child(div().text_xl().child(title))
        .child(div().text_sm().text_color(tokens.secondary).child(subtitle))
}
fn section(title: &'static str, body: impl IntoElement, tokens: &Tokens) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(tokens.border)
        .bg(tokens.sidebar)
        .child(div().text_lg().child(title))
        .child(body)
}
fn list<'a>(data: &'a Value, key: &str) -> Vec<&'a Value> {
    data.get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}
fn id(value: &Value) -> Option<&str> {
    value.get("id").and_then(Value::as_str)
}
fn model_ref(value: &Value) -> Option<&str> {
    value.get("ref").and_then(Value::as_str)
}
fn string_value(value: &Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}
fn value_string(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => fallback.to_owned(),
    }
}
fn new_rule_id() -> String {
    format!("client-{}", Uuid::new_v4())
}
fn new_rule_created_at() -> String {
    Utc::now().to_rfc3339()
}
fn string(value: &Value, path: &[&str], fallback: &str) -> String {
    path.iter()
        .try_fold(value, |value, key| value.get(*key))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}
fn boolean(value: &Value, path: &[&str], fallback: bool) -> bool {
    path.iter()
        .try_fold(value, |value, key| value.get(*key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}
fn number(value: &str) -> Value {
    value
        .trim()
        .parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::from(0))
}
fn decimal(value: &str) -> Value {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.)
        .and_then(serde_json::Number::from_f64)
        .map(Value::Number)
        .unwrap_or_else(|| Value::from(0))
}
fn valid_integer(value: &str) -> bool {
    value.trim().parse::<i64>().is_ok_and(|value| value >= 0)
}
fn valid_decimal(value: &str) -> bool {
    value
        .trim()
        .parse::<f64>()
        .is_ok_and(|value| value.is_finite() && value >= 0.)
}
fn value_or_null(value: &str) -> Value {
    if value.trim().is_empty() {
        Value::Null
    } else {
        Value::String(value.trim().to_owned())
    }
}
fn parse_json(value: &str) -> Option<Value> {
    serde_json::from_str(value).ok()
}
