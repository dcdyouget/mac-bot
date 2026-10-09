//! Feature pages shared by the desktop shell.
//!
//! The shell owns navigation, connection state, and the surrounding panes.
//! This module owns the feature page presentation and emits intent-like
//! actions so the shell can turn a click into an RPC or navigation event.

use chrono::{Duration as ChronoDuration, Utc};
use gpui_kit::StatefulInteractiveElement;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::chart::LineChart;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div, px,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::feature_i18n::text as t;
use crate::tokens::Tokens;

/// User intent emitted by feature pages.  The app shell translates RPC
/// actions into its connection client's request queue.
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub enum FeatureAction {
    Rpc { method: String, params: Value },
    Navigate(String),
    Trace(String),
    Computer(String),
    Toast(String),
}

impl FeatureAction {
    #[allow(dead_code)]
    fn method(&self) -> &str {
        match self {
            Self::Rpc { method, .. } => method,
            _ => "",
        }
    }

    #[allow(dead_code)]
    fn params(&self) -> &Value {
        match self {
            Self::Rpc { params, .. } => params,
            _ => panic!("params are only available for RPC actions"),
        }
    }
}

/// Data-backed page state. `data` is intentionally a JSON value: bootstrap
/// payloads can add fields without forcing a UI release to understand them.
pub struct FeaturePage {
    pub page: String,
    pub data: Value,
    pub inputs: BTreeMap<String, Entity<InputState>>,
    pub textareas: BTreeMap<String, Entity<TextareaState>>,
    pub selected_member_bot_ids: Vec<String>,
    pub last_action: Option<FeatureAction>,
}

/// JSON keys consumed by each feature page. Results from RPCs may include
/// extra fields; the renderer ignores fields outside this contract.
#[allow(dead_code)]
pub const FEATURE_DATA_CONTRACT: &[(&str, &[&str])] = &[
    (
        "workbench",
        &["running", "global_limit", "waiting", "bots", "done_today"],
    ),
    (
        "dashboard",
        &[
            "from",
            "to",
            "current",
            "heatmap",
            "timeseries",
            "breakdown",
            "dimension",
            "metric",
            "granularity",
            "split_io",
            "drill",
        ],
    ),
    (
        "skills",
        &["skills", "selected", "git_url", "git_subdir", "upload_id"],
    ),
    (
        "bot",
        &[
            "id",
            "name",
            "label",
            "description",
            "model",
            "max_parallel",
            "browser_mode",
            "avatar",
            "hidden",
            "models",
        ],
    ),
    ("group", &["name", "goal", "member_bot_ids", "bots"]),
    (
        "routine",
        &[
            "id",
            "bot_id",
            "project_id",
            "name",
            "instructions",
            "schedules",
            "timezone",
            "enabled",
            "runs",
            "routines",
            "bots",
            "projects",
        ],
    ),
    ("search", &["results"]),
];

impl EventEmitter<FeatureAction> for FeaturePage {}

impl FeaturePage {
    /// Create an entity owned by the app shell.
    pub fn new(
        page: impl Into<String>,
        data: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page = page.into();
        let inputs = build_inputs(&page, &data, window, cx);
        let textareas = build_textareas(&page, &data, window, cx);
        let selected_member_bot_ids = member_ids(&data);
        Self {
            page: page.clone(),
            data,
            inputs,
            textareas,
            selected_member_bot_ids,
            last_action: None,
        }
    }

    /// Convenience constructor for shells that create the entity from `App`.
    #[allow(dead_code)]
    pub fn entity(
        page: impl Into<String>,
        data: Value,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(page, data, window, cx))
    }

    #[allow(dead_code)]
    pub fn from_data(
        page: impl Into<String>,
        data: Value,
        inputs: BTreeMap<String, Entity<InputState>>,
    ) -> Self {
        let selected_member_bot_ids = member_ids(&data);
        Self {
            page: page.into(),
            data,
            inputs,
            textareas: BTreeMap::new(),
            selected_member_bot_ids,
            last_action: None,
        }
    }

    pub fn emit_action(&mut self, action: FeatureAction, cx: &mut Context<Self>) {
        self.last_action = Some(action.clone());
        cx.emit(action);
    }

    /// Stable event entry point for the shell. Keeping it separate from the
    /// click handlers also lets keyboard shortcuts and RPC acknowledgements
    /// feed the same event stream.
    #[allow(dead_code)]
    pub fn event(&mut self, action: FeatureAction, cx: &mut Context<Self>) {
        self.emit_action(action, cx);
    }

    /// Replace a bootstrap/RPC payload and ask GPUI to render the new page.
    #[allow(dead_code)]
    pub fn update_data(&mut self, data: Value, cx: &mut Context<Self>) {
        self.data = merge_feature_data(&self.data, data);
        cx.notify();
    }

    /// Switch the visible feature and recreate its native editing controls.
    pub fn set_page(
        &mut self,
        page: impl Into<String>,
        data: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = page.into();
        self.inputs = build_inputs(&page, &data, window, cx);
        self.textareas = build_textareas(&page, &data, window, cx);
        self.selected_member_bot_ids = member_ids(&data);
        self.page = page;
        self.data = data;
        self.last_action = None;
        cx.notify();
    }

    /// Load a selected Bot/Skill/Routine object into the current page editor.
    pub fn load_selected(&mut self, selected: Value, window: &mut Window, cx: &mut Context<Self>) {
        let mut data = self.data.clone();
        if let Value::Object(object) = &mut data {
            object.insert("selected".into(), selected.clone());
            if let Value::Object(selected_object) = selected {
                for (key, value) in selected_object {
                    object.insert(key, value);
                }
            }
        } else {
            data = selected;
        }
        self.set_page(self.page.clone(), data, window, cx);
    }

    pub fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        render_page(
            &self.page,
            &self.data,
            &self.inputs,
            &self.textareas,
            &self.selected_member_bot_ids,
            window,
            cx,
        )
    }
}

#[allow(dead_code)]
fn merge_feature_data(previous: &Value, incoming: Value) -> Value {
    let Value::Object(incoming) = incoming else {
        return incoming;
    };
    let Some(previous) = previous.as_object() else {
        return Value::Object(incoming);
    };
    let mut merged = previous.clone();
    for (key, value) in incoming {
        if key == "selected" {
            match (merged.get_mut(&key), value) {
                (Some(Value::Object(existing)), Value::Object(selected)) => {
                    existing.extend(selected);
                }
                (Some(Value::Object(_)), Value::Null) => {}
                (_, value) => {
                    merged.insert(key, value);
                }
            }
        } else {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

/// Name used by the desktop shell when it stores this view as an entity.
/// Keeping an alias avoids a second state container and keeps `FeaturePage`
/// available to callers that want a more explicit type name.
#[allow(dead_code)]
pub type Features = FeaturePage;

impl Render for FeaturePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render(window, cx)
    }
}

fn input_fields(page: &str) -> &'static [&'static str] {
    match page {
        "skills" | "skill" => &["name", "query", "path", "git_url", "git_subdir"],
        "bot" | "bot_settings" => &[
            "name",
            "label",
            "model",
            "max_parallel",
            "browser_mode",
            "avatar",
        ],
        "group" | "new_group" => &["name"],
        "routine" | "routines" => &[
            "name",
            "bot_id",
            "project_id",
            "cron",
            "cron_1",
            "cron_2",
            "cron_3",
            "cron_4",
            "cron_5",
            "timezone",
        ],
        "dashboard" => &["from", "to"],
        "search" => &["query"],
        _ => &[],
    }
}

fn textarea_fields(page: &str) -> &'static [&'static str] {
    match page {
        "skills" | "skill" => &["content"],
        "bot" | "bot_settings" => &["description"],
        "group" | "new_group" => &["goal"],
        "routine" | "routines" => &["instructions"],
        _ => &[],
    }
}

fn input_placeholder(page: &str, field: &str) -> Option<&'static str> {
    match (page, field) {
        ("skills" | "skill", "name") => Some("skills.placeholder_name"),
        ("skills" | "skill", "query") => Some("skills.placeholder_query"),
        ("skills" | "skill", "path") => Some("skills.placeholder_path"),
        ("skills" | "skill", "git_url") => Some("skills.placeholder_git_url"),
        ("skills" | "skill", "git_subdir") => Some("skills.placeholder_git_subdir"),
        ("bot" | "bot_settings", "name") => Some("bot.placeholder_name"),
        ("bot" | "bot_settings", "label") => Some("bot.placeholder_label"),
        ("bot" | "bot_settings", "model") => Some("bot.placeholder_model"),
        ("bot" | "bot_settings", "max_parallel") => Some("bot.placeholder_parallel"),
        ("bot" | "bot_settings", "browser_mode") => Some("bot.placeholder_browser"),
        ("group" | "new_group", "name") => Some("group.placeholder_name"),
        ("routine" | "routines", "name") => Some("routine.placeholder_name"),
        ("routine" | "routines", "bot_id") => Some("routine.placeholder_bot"),
        ("routine" | "routines", "project_id") => Some("routine.placeholder_project"),
        ("routine" | "routines", "timezone") => Some("routine.placeholder_timezone"),
        _ => None,
    }
}

fn textarea_placeholder(page: &str, field: &str) -> Option<&'static str> {
    match (page, field) {
        ("skills" | "skill", "content") => Some("skills.placeholder_content"),
        ("bot" | "bot_settings", "description") => Some("bot.placeholder_description"),
        ("group" | "new_group", "goal") => Some("group.placeholder_goal"),
        ("routine" | "routines", "instructions") => Some("routine.placeholder_instructions"),
        _ => None,
    }
}

fn labeled_field<L: Into<String>, T: IntoElement>(label: L, field: T) -> gpui_kit::Div {
    let label = label.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().child(label))
        .child(field)
}

fn build_inputs(
    page: &str,
    data: &Value,
    window: &mut Window,
    cx: &mut Context<FeaturePage>,
) -> BTreeMap<String, Entity<InputState>> {
    input_fields(page)
        .iter()
        .copied()
        .map(|field| {
            let new_bot = matches!(page, "bot" | "bot_settings") && data.get("id").is_none();
            let schedule_index = field
                .strip_prefix("cron")
                .map(|suffix| suffix.strip_prefix('_').unwrap_or("0"))
                .and_then(|index| {
                    if index.is_empty() {
                        Some(0)
                    } else {
                        index.parse().ok()
                    }
                });
            let value = data
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    schedule_index.and_then(|index| {
                        data.get("schedules")
                            .and_then(Value::as_array)
                            .and_then(|schedules| schedules.get(index))
                            .and_then(|schedule| schedule.get("cron"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                });
            let initial = value.unwrap_or_else(|| {
                if new_bot {
                    return String::new();
                }
                if page == "dashboard" && (field == "from" || field == "to") {
                    let (from, to) = usage_period(data);
                    if field == "from" { from } else { to }
                } else {
                    String::new()
                }
            });
            let placeholder = input_placeholder(page, field).map(t);
            let state = cx.new(|cx| {
                let state = InputState::new(window, cx);
                let state = match placeholder {
                    Some(key) => state.placeholder(key),
                    None => state,
                };
                state.default_value(initial)
            });
            (field.to_string(), state)
        })
        .collect()
}

fn build_textareas(
    page: &str,
    data: &Value,
    window: &mut Window,
    cx: &mut Context<FeaturePage>,
) -> BTreeMap<String, Entity<TextareaState>> {
    textarea_fields(page)
        .iter()
        .copied()
        .map(|field| {
            let placeholder = textarea_placeholder(page, field).map(t);
            let initial = data
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let state = cx.new(|cx| {
                let state = TextareaState::new(window, cx);
                let state = match placeholder {
                    Some(key) => state.placeholder(key),
                    None => state,
                };
                state.default_value(initial)
            });
            (field.to_string(), state)
        })
        .collect()
}

fn input_state<'a>(
    inputs: &'a BTreeMap<String, Entity<InputState>>,
    field: &str,
) -> &'a Entity<InputState> {
    inputs
        .get(field)
        .or_else(|| inputs.values().next())
        .expect("feature page declares at least one input")
}

fn textarea_state<'a>(
    textareas: &'a BTreeMap<String, Entity<TextareaState>>,
    field: &str,
) -> &'a Entity<TextareaState> {
    textareas
        .get(field)
        .expect("feature page declares the requested textarea")
}

fn render_page(
    page: &str,
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    selected_member_bot_ids: &[String],
    _window: &mut Window,
    cx: &mut Context<FeaturePage>,
) -> AnyElement {
    let tokens = Tokens::get(&*cx);
    let content = match page {
        "workbench" => workbench(data, &tokens, cx).into_any_element(),
        "dashboard" => dashboard(data, inputs, &tokens, cx).into_any_element(),
        "skills" | "skill" => skills(data, inputs, textareas, &tokens, cx).into_any_element(),
        "bot" | "bot_settings" => {
            bot_editor(data, inputs, textareas, &tokens, cx).into_any_element()
        }
        "group" | "new_group" => group_editor(
            data,
            inputs,
            textareas,
            selected_member_bot_ids,
            &tokens,
            cx,
        )
        .into_any_element(),
        "routine" | "routines" => {
            routine_editor(data, inputs, textareas, &tokens, cx).into_any_element()
        }
        "search" => search(data, inputs, &tokens, cx).into_any_element(),
        _ => empty_page(page, &tokens).into_any_element(),
    };
    let scroll = div()
        .id(format!("feature-scroll-{page}"))
        .size_full()
        .min_h_0()
        .overflow_y_scroll()
        .child(div().w_full().min_w_0().flex_none().child(content));
    div()
        .id(page.to_string())
        .flex()
        .flex_col()
        .size_full()
        .bg(tokens.window)
        .text_color(tokens.primary)
        .child(scroll)
        .into_any_element()
}

fn page_header(
    title: impl Into<String>,
    subtitle: impl Into<String>,
    tokens: &Tokens,
) -> impl IntoElement {
    let title = title.into();
    let subtitle = subtitle.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .pb_4()
        .child(
            div()
                .text_xl()
                .font_weight(gpui_kit::gpui::FontWeight::BOLD)
                .child(title),
        )
        .child(div().text_sm().text_color(tokens.secondary).child(subtitle))
}

fn card(title: impl Into<String>, body: impl IntoElement, tokens: &Tokens) -> impl IntoElement {
    let title = title.into();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded_lg()
        .bg(tokens.window)
        .border_1()
        .border_color(tokens.border)
        .child(
            div()
                .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(body)
}

fn stat(label: impl Into<String>, value: impl Into<String>, tokens: &Tokens) -> impl IntoElement {
    let label = label.into();
    let value = value.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .min_w(px(130.))
        .rounded_md()
        .bg(tokens.window)
        .border_1()
        .border_color(tokens.border)
        .child(div().text_sm().text_color(tokens.secondary).child(label))
        .child(
            div()
                .text_lg()
                .font_weight(gpui_kit::gpui::FontWeight::BOLD)
                .child(value),
        )
}

fn rows(items: impl IntoIterator<Item = String>, tokens: &Tokens) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(items.into_iter().map(|item| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .py_2()
                .border_b_1()
                .border_color(tokens.border)
                .child(item)
        }))
}

fn action_button(
    label: &str,
    action: FeatureAction,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    action_button_with_id(format!("action-{label}"), label, action, cx)
}

fn action_button_with_id(
    id: String,
    label: &str,
    action: FeatureAction,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    Button::new(id)
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| this.emit_action(action.clone(), cx)))
}

/// RPC button that carries the current value from the visible gpui-kit input.
/// Keeping this in the feature layer makes edits useful before the shell has a
/// form reducer, while the resulting payload remains ordinary JSON.
fn input_rpc_button(
    label: &str,
    method: &str,
    params: Value,
    field: &str,
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let method = method.to_string();
    let field = field.to_string();
    let input = input_state(inputs, &field).clone();
    Button::new(format!("input-action-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            let value = input.read(cx).value().to_string();
            let mut payload = params.clone();
            if let Value::Object(object) = &mut payload {
                object.insert(field.clone(), Value::String(value));
            }
            this.emit_action(
                FeatureAction::Rpc {
                    method: method.clone(),
                    params: payload,
                },
                cx,
            );
        }))
}

fn skill_import_button(
    label: &str,
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let path = input_state(inputs, "path").clone();
    Button::new(format!("skill-import-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.emit_action(
                FeatureAction::Rpc {
                    method: "skill.import".into(),
                    params: skill_import_path_params(&path.read(cx).value()),
                },
                cx,
            );
        }))
}

fn skill_import_git_button(
    label: &str,
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let url = input_state(inputs, "git_url").clone();
    let subdir = input_state(inputs, "git_subdir").clone();
    Button::new(format!("skill-import-git-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            let subdir = subdir.read(cx).value().to_string();
            this.emit_action(
                FeatureAction::Rpc {
                    method: "skill.import".into(),
                    params: skill_import_git_params(
                        &url.read(cx).value(),
                        (!subdir.trim().is_empty()).then_some(subdir.as_str()),
                    ),
                },
                cx,
            );
        }))
}

fn skill_upload_button(label: &str, cx: &mut Context<FeaturePage>) -> impl IntoElement + use<> {
    let label = label.to_string();
    Button::new(format!("skill-upload-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.emit_action(FeatureAction::Navigate("skill/import/upload/".into()), cx);
        }))
}

pub fn skill_create_params(name: &str, content: &str) -> Value {
    json!({"name": name, "content": content})
}

pub fn skill_update_params(name: &str, content: &str) -> Value {
    skill_create_params(name, content)
}

pub fn skill_delete_params(name: &str) -> Value {
    json!({"name": name})
}

pub fn skill_enabled_params(name: &str, enabled: bool, bot_id: Option<&str>) -> Value {
    let mut params = json!({"name": name, "enabled": enabled});
    if let Some(bot_id) = bot_id.filter(|id| !id.is_empty()) {
        params["bot_id"] = json!(bot_id);
    }
    params
}

pub fn skill_publish_params(name: &str) -> Value {
    json!({"name": name})
}

pub fn skill_import_path_params(path: &str) -> Value {
    json!({"source": {"kind": "path", "path": path}})
}

pub fn skill_import_git_params(url: &str, subdir: Option<&str>) -> Value {
    let mut params = json!({"source": {"kind": "git", "url": url}});
    if let Some(subdir) = subdir {
        params["source"]["subdir"] = json!(subdir);
    }
    params
}

pub fn routine_test_params(routine_id: &str) -> Value {
    json!({"routine_id": routine_id})
}

pub fn routine_enabled_params(routine_id: &str, enabled: bool) -> Value {
    json!({"routine_id": routine_id, "enabled": enabled})
}

pub fn assignment_stop_params(assignment_id: &str) -> Value {
    json!({"assignment_id": assignment_id})
}

pub fn project_create_params(name: &str, goal: &str, member_bot_ids: &[String]) -> Value {
    json!({
        "name": name,
        "goal": goal,
        "member_bot_ids": member_bot_ids,
    })
}

pub fn bot_create_params(
    name: &str,
    label: &str,
    description: &str,
    model: &str,
    max_parallel: &str,
    browser_mode: &str,
) -> Value {
    let mut params = json!({"name": name});
    let object = params.as_object_mut().expect("bot params is an object");
    if !label.is_empty() {
        object.insert("label".into(), json!(label));
    }
    if !description.is_empty() {
        object.insert("description".into(), json!(description));
    }
    if !model.is_empty() {
        object.insert("model".into(), json!(model));
    }
    if let Ok(max_parallel) = max_parallel.parse::<u8>() {
        object.insert("max_parallel".into(), json!(max_parallel));
    }
    if !browser_mode.is_empty() {
        object.insert("browser_mode".into(), json!(browser_mode));
    }
    params
}

pub struct BotCreateOptions {
    pub tools: Value,
    pub notifications: bool,
    pub avatar: Option<Value>,
}

pub fn bot_create_params_with_options(mut params: Value, options: BotCreateOptions) -> Value {
    params["tools"] = options.tools;
    params["notifications"] = json!(options.notifications);
    if let Some(avatar) = options.avatar {
        params["avatar"] = avatar;
    }
    params
}

fn default_bot_tools() -> Value {
    json!({"files": true, "bash": true, "browser": false, "subagent": false, "web": false, "mcp": false})
}

pub fn bot_update_params(
    bot_id: &str,
    name: &str,
    label: &str,
    description: &str,
    model: &str,
    max_parallel: &str,
    browser_mode: &str,
) -> Value {
    let patch = bot_create_params(name, label, description, model, max_parallel, browser_mode);
    json!({"bot_id": bot_id, "patch": patch})
}

pub fn bot_avatar_update_params(bot_id: &str, avatar: Value) -> Value {
    json!({"bot_id": bot_id, "patch": {"avatar": avatar}})
}

pub fn bot_visibility_update_params(bot_id: &str, hidden: bool) -> Value {
    json!({"bot_id": bot_id, "patch": {"hidden": hidden}})
}

pub fn bot_notifications_update_params(bot_id: &str, enabled: bool) -> Value {
    json!({"bot_id": bot_id, "patch": {"notifications": enabled}})
}

pub fn bot_pinned_update_params(bot_id: &str, pinned: bool) -> Value {
    json!({"bot_id": bot_id, "patch": {"pinned": pinned}})
}

pub fn bot_tools_update_params(bot_id: &str, tools: Value) -> Value {
    json!({"bot_id": bot_id, "patch": {"tools": tools}})
}

pub fn bot_delete_params(bot_id: &str) -> Value {
    json!({"bot_id": bot_id})
}

pub fn bot_duplicate_params(bot_id: &str, name: &str) -> Value {
    json!({"bot_id": bot_id, "name": name})
}

pub fn routine_create_params(
    bot_id: &str,
    project_id: Option<&str>,
    name: &str,
    instructions: &str,
    schedules: Value,
    timezone: Option<&str>,
) -> Value {
    let mut params = json!({
        "bot_id": bot_id,
        "name": name,
        "instructions": instructions,
        "schedules": schedules,
    });
    if let Some(project_id) = project_id {
        params["project_id"] = json!(project_id);
    }
    if let Some(timezone) = timezone {
        params["timezone"] = json!(timezone);
    }
    params
}

#[allow(dead_code)]
pub fn routine_update_params(
    routine_id: &str,
    name: &str,
    instructions: &str,
    schedules: Value,
) -> Value {
    routine_update_params_with_context(routine_id, name, instructions, schedules, None, None)
}

pub fn routine_update_params_with_context(
    routine_id: &str,
    name: &str,
    instructions: &str,
    schedules: Value,
    project_id: Option<&str>,
    timezone: Option<&str>,
) -> Value {
    let mut patch = json!({
        "name": name,
        "instructions": instructions,
        "schedules": schedules,
    });
    if let Some(project_id) = project_id {
        patch["project_id"] = json!(project_id);
    }
    if let Some(timezone) = timezone {
        patch["timezone"] = json!(timezone);
    }
    json!({"routine_id": routine_id, "patch": patch})
}

/// Build a routine patch while preserving an explicit `null` project value.
/// The protocol uses `Patch<Id>` here, so clearing the project must be sent as
/// `project_id: null` instead of omitting the field.
pub fn routine_update_params_with_project_patch(
    routine_id: &str,
    name: &str,
    instructions: &str,
    schedules: Value,
    project_id: Option<Value>,
    timezone: Option<&str>,
) -> Value {
    let mut patch = json!({
        "name": name,
        "instructions": instructions,
        "schedules": schedules,
    });
    patch["project_id"] = project_id.unwrap_or(Value::Null);
    if let Some(timezone) = timezone {
        patch["timezone"] = json!(timezone);
    }
    json!({
        "routine_id": routine_id,
        "patch": patch,
    })
}

pub fn search_params(query: &str, kind: Option<&str>) -> Value {
    let mut params = json!({"query": query, "limit": 20});
    if let Some(kind) = kind {
        params["kinds"] = json!([kind]);
    }
    params
}

fn textarea_rpc_button(
    label: &str,
    method: &str,
    params: Value,
    field: &str,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    wrapper: Option<&str>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let method = method.to_string();
    let field = field.to_string();
    let wrapper = wrapper.map(str::to_string);
    let textarea = textarea_state(textareas, &field).clone();
    Button::new(format!("textarea-action-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            let value = textarea.read(cx).value().to_string();
            let mut payload = params.clone();
            if let Value::Object(object) = &mut payload {
                if let Some(wrapper) = &wrapper {
                    let mut nested = object
                        .remove(wrapper)
                        .and_then(|value| value.as_object().cloned())
                        .unwrap_or_default();
                    nested.insert(field.clone(), Value::String(value));
                    object.insert(wrapper.clone(), Value::Object(nested));
                } else {
                    object.insert(field.clone(), Value::String(value));
                }
            }
            this.emit_action(
                FeatureAction::Rpc {
                    method: method.clone(),
                    params: payload,
                },
                cx,
            );
        }))
}

fn skill_create_button(
    label: &str,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let name = input_state(inputs, "name").clone();
    let content = textarea_state(textareas, "content").clone();
    Button::new(format!("skill-create-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.emit_action(
                FeatureAction::Rpc {
                    method: "skill.create".into(),
                    params: skill_create_params(&name.read(cx).value(), &content.read(cx).value()),
                },
                cx,
            );
        }))
}

fn bot_save_button(
    label: &str,
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let updating = data.get("id").cloned();
    let name = input_state(inputs, "name").clone();
    let label_input = input_state(inputs, "label").clone();
    let description = textarea_state(textareas, "description").clone();
    let model = input_state(inputs, "model").clone();
    let parallel = input_state(inputs, "max_parallel").clone();
    let browser = input_state(inputs, "browser_mode").clone();
    Button::new(format!("bot-save-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            let name_value = name.read(cx).value().to_string();
            let label_value = label_input.read(cx).value().to_string();
            let description_value = description.read(cx).value().to_string();
            let model_value = model.read(cx).value().to_string();
            let parallel_value = parallel.read(cx).value().to_string();
            let browser_value = browser.read(cx).value().to_string();
            let (method, params) = if let Some(bot_id) = &updating {
                (
                    "bot.update",
                    bot_update_params(
                        bot_id.as_str().unwrap_or_default(),
                        &name_value,
                        &label_value,
                        &description_value,
                        &model_value,
                        &parallel_value,
                        &browser_value,
                    ),
                )
            } else {
                (
                    "bot.create",
                    bot_create_params_with_options(
                        bot_create_params(
                            &name_value,
                            &label_value,
                            &description_value,
                            &model_value,
                            &parallel_value,
                            &browser_value,
                        ),
                        BotCreateOptions {
                            tools: this
                                .data
                                .get("tools")
                                .cloned()
                                .unwrap_or_else(default_bot_tools),
                            notifications: this
                                .data
                                .get("notifications")
                                .and_then(Value::as_bool)
                                .unwrap_or(true),
                            avatar: this.data.get("avatar").cloned(),
                        },
                    ),
                )
            };
            this.emit_action(
                FeatureAction::Rpc {
                    method: method.into(),
                    params,
                },
                cx,
            );
        }))
}

fn group_create_button(
    label: &str,
    selected_member_bot_ids: &[String],
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let name = input_state(inputs, "name").clone();
    let goal = textarea_state(textareas, "goal").clone();
    let selected_member_bot_ids = selected_member_bot_ids.to_vec();
    Button::new(format!("group-create-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            if selected_member_bot_ids.is_empty() || selected_member_bot_ids.len() > 6 {
                this.emit_action(
                    FeatureAction::Toast(t("group.member_required").to_string()),
                    cx,
                );
                return;
            }
            this.emit_action(
                FeatureAction::Rpc {
                    method: "project.create".into(),
                    params: project_create_params(
                        &name.read(cx).value(),
                        &goal.read(cx).value(),
                        &selected_member_bot_ids,
                    ),
                },
                cx,
            );
        }))
}

fn routine_save_button(
    label: &str,
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let routine_id = data.get("id").cloned();
    let name = input_state(inputs, "name").clone();
    let bot_id = input_state(inputs, "bot_id").clone();
    let project_id = input_state(inputs, "project_id").clone();
    let instructions = textarea_state(textareas, "instructions").clone();
    let cron = input_state(inputs, "cron").clone();
    let cron_1 = input_state(inputs, "cron_1").clone();
    let cron_2 = input_state(inputs, "cron_2").clone();
    let cron_3 = input_state(inputs, "cron_3").clone();
    let cron_4 = input_state(inputs, "cron_4").clone();
    let cron_5 = input_state(inputs, "cron_5").clone();
    let timezone = input_state(inputs, "timezone").clone();
    Button::new(format!("routine-save-{label}"))
        .label(label)
        .primary()
        .on_click(cx.listener(move |this, _, _, cx| {
            let name = name.read(cx).value().to_string();
            let bot_id = bot_id.read(cx).value().to_string();
            let project_id = project_id.read(cx).value().to_string();
            let instructions = instructions.read(cx).value().to_string();
            let timezone = timezone.read(cx).value().to_string();
            let schedules = [
                cron.clone(),
                cron_1.clone(),
                cron_2.clone(),
                cron_3.clone(),
                cron_4.clone(),
                cron_5.clone(),
            ]
            .iter()
            .map(|state| state.read(cx).value().to_string())
            .filter(|value| !value.trim().is_empty())
            .map(|value| json!({"cron": value, "label": value}))
            .collect::<Vec<_>>();
            if schedules.is_empty() {
                this.emit_action(
                    FeatureAction::Toast(t("routine.schedule_required").to_string()),
                    cx,
                );
                return;
            }
            let (method, params) = if let Some(routine_id) = &routine_id {
                (
                    "routine.update",
                    routine_update_params_with_project_patch(
                        routine_id.as_str().unwrap_or_default(),
                        &name,
                        &instructions,
                        json!(schedules),
                        (!project_id.trim().is_empty()).then(|| json!(project_id)),
                        (!timezone.trim().is_empty()).then_some(timezone.as_str()),
                    ),
                )
            } else {
                if bot_id.trim().is_empty() {
                    this.emit_action(
                        FeatureAction::Toast(t("routine.bot_required").to_string()),
                        cx,
                    );
                    return;
                }
                let params = routine_create_params(
                    &bot_id,
                    (!project_id.trim().is_empty()).then_some(project_id.as_str()),
                    &name,
                    &instructions,
                    json!(schedules),
                    (!timezone.trim().is_empty()).then_some(timezone.as_str()),
                );
                ("routine.create", params)
            };
            this.emit_action(
                FeatureAction::Rpc {
                    method: method.into(),
                    params,
                },
                cx,
            );
        }))
}

/// Build the protocol payload for a timeseries request. Dashboard controls use
/// this helper instead of putting query-string-only state on the wire.
pub fn usage_timeseries_params(
    data: &Value,
    dimension: &str,
    metric: &str,
    split_io: bool,
) -> Value {
    let (from, to) = usage_period(data);
    json!({
        "from": from,
        "to": to,
        "granularity": data.get("granularity").and_then(Value::as_str).unwrap_or("day"),
        "dimension": dimension,
        "metric": metric,
        "split_io": split_io,
        "top": data.get("top").and_then(Value::as_u64).unwrap_or(6),
    })
}

fn dashboard_dimension(data: &Value) -> &str {
    match data.get("dimension").and_then(Value::as_str) {
        Some(value @ ("model" | "bot" | "project")) => value,
        _ => "model",
    }
}

/// Build the protocol payload for a breakdown request, preserving a selected
/// drill object only when it already matches the server's shape.
pub fn usage_breakdown_params(data: &Value, dimension: &str) -> Value {
    let (from, to) = usage_period(data);
    let drill = data.get("drill").filter(|value| {
        value.as_object().is_some_and(|object| {
            if object.len() != 1 {
                return false;
            }
            object
                .get("bot_id")
                .or_else(|| object.get("project_id"))
                .and_then(Value::as_str)
                .is_some()
        })
    });
    let mut params = json!({"from": from, "to": to, "dimension": dimension});
    if let Some(drill) = drill {
        params["drill"] = drill.clone();
    }
    params
}

pub fn usage_summary_params(data: &Value) -> Value {
    let (from, to) = usage_period(data);
    json!({"from": from, "to": to})
}

pub fn usage_heatmap_params(data: &Value, mode: &str, metric: &str) -> Value {
    let (from, to) = if mode == "calendar" {
        let to = Utc::now();
        (
            (to - ChronoDuration::days(370)).to_rfc3339(),
            to.to_rfc3339(),
        )
    } else {
        usage_period(data)
    };
    json!({"mode": mode, "from": from, "to": to, "metric": metric})
}

fn usage_period(data: &Value) -> (String, String) {
    let now = Utc::now();
    let to = data
        .get("to")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| now.to_rfc3339());
    let from = data
        .get("from")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| (now - ChronoDuration::days(30)).to_rfc3339());
    (from, to)
}

fn dashboard_rpc_button(
    label: &str,
    method: &str,
    params: Value,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    dashboard_rpc_button_with_id(format!("dashboard-rpc-{label}"), label, method, params, cx)
}

fn dashboard_rpc_button_with_id(
    id: String,
    label: &str,
    method: &str,
    params: Value,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    action_button_with_id(
        id,
        label,
        FeatureAction::Rpc {
            method: method.to_owned(),
            params,
        },
        cx,
    )
}

fn dashboard_period_rpc_button(
    label: &str,
    method: &str,
    inputs: &BTreeMap<String, Entity<InputState>>,
    build: fn(&str, &str) -> Value,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_owned();
    let method = method.to_owned();
    let from = input_state(inputs, "from").clone();
    let to = input_state(inputs, "to").clone();
    Button::new(format!("dashboard-period-{label}"))
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            let from_value = from.read(cx).value().to_string();
            let to_value = to.read(cx).value().to_string();
            this.data["from"] = json!(from_value.clone());
            this.data["to"] = json!(to_value.clone());
            let params = build(&from_value, &to_value);
            this.emit_action(
                FeatureAction::Rpc {
                    method: method.clone(),
                    params,
                },
                cx,
            );
        }))
}

fn dashboard_dimension_button(
    label: &str,
    dimension: &str,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_owned();
    let dimension = dimension.to_owned();
    Button::new(format!("dashboard-dimension-{dimension}"))
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.data["dimension"] = json!(dimension.clone());
            let snapshot = this.data.clone();
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.timeseries".into(),
                    params: usage_timeseries_params(
                        &snapshot,
                        &dimension,
                        snapshot
                            .get("metric")
                            .and_then(Value::as_str)
                            .unwrap_or("tokens"),
                        snapshot
                            .get("split_io")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    ),
                },
                cx,
            );
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.breakdown".into(),
                    params: usage_breakdown_params(&snapshot, &dimension),
                },
                cx,
            );
            cx.notify();
        }))
}

fn dashboard_metric_button(
    label: &str,
    metric: &str,
    split_io: bool,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_owned();
    let metric = metric.to_owned();
    Button::new(format!("dashboard-metric-{metric}-{split_io}"))
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.data["metric"] = json!(metric.clone());
            this.data["split_io"] = json!(split_io);
            let snapshot = this.data.clone();
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.timeseries".into(),
                    params: usage_timeseries_params(
                        &snapshot,
                        dashboard_dimension(&snapshot),
                        &metric,
                        split_io,
                    ),
                },
                cx,
            );
        }))
}

fn dashboard_heatmap_metric_button(
    label: &str,
    metric: &str,
    mode: &str,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_owned();
    let metric = metric.to_owned();
    let mode = mode.to_owned();
    Button::new(format!("dashboard-heatmap-{mode}-{metric}"))
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.data["metric"] = json!(metric.clone());
            let snapshot = this.data.clone();
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.heatmap".into(),
                    params: usage_heatmap_params(&snapshot, &mode, &metric),
                },
                cx,
            );
        }))
}

fn usage_summary_period(from: &str, to: &str) -> Value {
    json!({"from": from, "to": to})
}

fn usage_heatmap_period(from: &str, to: &str) -> Value {
    let now = Utc::now();
    let _ = (from, to);
    json!({
        "mode": "calendar",
        "from": (now - ChronoDuration::days(370)).to_rfc3339(),
        "to": now.to_rfc3339(),
        "metric": "tokens"
    })
}

fn workbench(data: &Value, tokens: &Tokens, cx: &mut Context<FeaturePage>) -> impl IntoElement {
    let running = data.get("running").and_then(Value::as_u64).unwrap_or(0);
    let limit = data
        .get("global_limit")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let waiting = array(data, "waiting");
    let reviews = waiting
        .iter()
        .filter(|item| string(item, "kind", "") == "review")
        .count();
    let approvals = waiting
        .iter()
        .filter(|item| string(item, "kind", "") == "approval")
        .count();
    let questions = waiting
        .iter()
        .filter(|item| string(item, "kind", "") == "question")
        .count();
    let takeovers = waiting
        .iter()
        .filter(|item| string(item, "kind", "") == "takeover")
        .count();
    let group_mode = query_value(data, "by").unwrap_or_else(|| "bot".to_owned());
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for job in workbench_jobs(data)
        .into_iter()
        .filter(|job| !is_finished_assignment(job))
    {
        let key = workbench_group_key(&job, &group_mode, data);
        groups.entry(key).or_default().push(job);
    }
    let active_groups =
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(groups.iter().map(|(key, jobs)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                            .child(format!("{} · {}", workbench_group_label(&group_mode), key)),
                    )
                    .children(jobs.iter().enumerate().map(|(index, job)| {
                        workbench_job_row(job, data, tokens, cx, &format!("{key}-{index}"))
                    }))
            }));
    let done = workbench_done_jobs(data);
    let body = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_button(
                    t("workbench.by_bot"),
                    FeatureAction::Navigate("workbench?by=bot".into()),
                    cx,
                ))
                .child(action_button(
                    t("workbench.by_project"),
                    FeatureAction::Navigate("workbench?by=project".into()),
                    cx,
                ))
                .child(action_button(
                    t("workbench.by_status"),
                    FeatureAction::Navigate("workbench?by=status".into()),
                    cx,
                )),
        )
        .child(div().flex().gap_3().children([
            stat(
                t("workbench.running"),
                &format!("{running} / {limit}"),
                tokens,
            ),
            stat(t("workbench.review"), &reviews.to_string(), tokens),
            stat(t("workbench.approval"), &approvals.to_string(), tokens),
            stat(t("workbench.question"), &questions.to_string(), tokens),
            stat(t("workbench.takeover"), &takeovers.to_string(), tokens),
        ]))
        .child(workbench_waiting(&waiting, tokens, cx))
        .child(card(t("workbench.title"), active_groups, tokens))
        .child(card(
            t("workbench.done_today"),
            rows(
                done.iter().map(|job| workbench_done_label(job, data)),
                tokens,
            ),
            tokens,
        ));
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(
            t("workbench.title"),
            &format!(
                "{} {running} / {} {limit}",
                t("workbench.running"),
                t("workbench.global_limit")
            ),
            tokens,
        ))
        .child(body)
}

fn is_finished_assignment(job: &Value) -> bool {
    matches!(
        job.get("status").and_then(Value::as_str),
        Some("done" | "failed" | "cancelled")
    )
}

fn workbench_done_jobs(data: &Value) -> Vec<Value> {
    let explicit = array(data, "done_today");
    if !explicit.is_empty() {
        return explicit;
    }
    array(data, "jobs")
        .into_iter()
        .filter(is_finished_assignment)
        .collect()
}

fn workbench_group_key(job: &Value, mode: &str, data: &Value) -> String {
    match mode {
        "project" => workbench_entity_label(
            data,
            "projects",
            job.get("project_id").and_then(Value::as_str),
            t("workbench.no_project"),
        ),
        "status" => workbench_status_label(job.get("status").and_then(Value::as_str)),
        _ => {
            let bot_id = job.get("bot_id").and_then(Value::as_str);
            let bot = string(job, "bot", "");
            if !bot.is_empty() {
                bot
            } else {
                workbench_entity_label(data, "all_bots", bot_id, t("common.unknown"))
            }
        }
    }
}

fn workbench_entity_label(
    data: &Value,
    collection: &str,
    id: Option<&str>,
    fallback: &str,
) -> String {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return fallback.to_owned();
    };
    array(data, collection)
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        .map(|item| string(item, "name", &string(item, "label", id)))
        .unwrap_or_else(|| id.to_owned())
}

fn workbench_group_label(mode: &str) -> &'static str {
    match mode {
        "project" => t("workbench.by_project"),
        "status" => t("workbench.by_status"),
        _ => t("workbench.by_bot"),
    }
}

fn workbench_status_label(status: Option<&str>) -> String {
    match status.unwrap_or_default() {
        "queued" => t("workbench.status_queued").to_owned(),
        "working" | "running" => t("workbench.status_working").to_owned(),
        "waiting" => t("workbench.status_waiting").to_owned(),
        "paused" => t("workbench.status_paused").to_owned(),
        "failed" => t("workbench.status_failed").to_owned(),
        "done" => t("workbench.status_done").to_owned(),
        "cancelled" => t("workbench.status_cancelled").to_owned(),
        _ => t("common.unknown").to_owned(),
    }
}

fn workbench_job_row(
    job: &Value,
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
    row_key: &str,
) -> impl IntoElement + use<> {
    let name = string(job, "title", t("common.unnamed_task"));
    let bot = {
        let bot = string(job, "bot", "");
        if bot.is_empty() {
            workbench_entity_label(
                data,
                "all_bots",
                job.get("bot_id").and_then(Value::as_str),
                t("common.unknown"),
            )
        } else {
            bot
        }
    };
    let project = workbench_entity_label(
        data,
        "projects",
        job.get("project_id").and_then(Value::as_str),
        t("workbench.no_project"),
    );
    let status_raw = job
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let status = workbench_status_label(Some(status_raw));
    let elapsed = workbench_elapsed(job);
    let usage = job
        .get("usage")
        .and_then(|usage| usage.get("input_tokens").and_then(Value::as_u64))
        .unwrap_or(0)
        + job
            .get("usage")
            .and_then(|usage| usage.get("output_tokens").and_then(Value::as_u64))
            .unwrap_or(0);
    let model = workbench_model_label(job, data);
    let subagents = job
        .get("subagents_active")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let assignment_id = string(job, "id", "");
    let mut actions = div().flex().gap_1().child(action_button_with_id(
        format!("workbench-{row_key}-detail"),
        t("workbench.detail"),
        FeatureAction::Trace(assignment_id.clone()),
        cx,
    ));
    if !assignment_id.is_empty() {
        let stop = FeatureAction::Rpc {
            method: "assignment.stop".into(),
            params: assignment_stop_params(&assignment_id),
        };
        actions = actions.child(action_button_with_id(
            format!("workbench-{row_key}-stop"),
            t("workbench.stop"),
            stop.clone(),
            cx,
        ));
        if status_raw == "queued" {
            actions = actions.child(action_button_with_id(
                format!("workbench-{row_key}-cancel"),
                t("workbench.cancel"),
                stop,
                cx,
            ));
        }
        if status_raw == "failed" {
            actions = actions.child(action_button_with_id(
                format!("workbench-{row_key}-retry"),
                t("workbench.retry"),
                FeatureAction::Navigate(format!("assignment/{assignment_id}/retry")),
                cx,
            ));
        }
    }
    div()
        .flex()
        .items_center()
        .justify_between()
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(format!("{bot} · {name}"))
                .child(div().text_sm().text_color(tokens.secondary).child(format!(
                    "{project} · {status} · {elapsed} · {usage} tok · {model} · {} {}",
                    t("workbench.subagents"),
                    subagents
                ))),
        )
        .child(actions)
}

fn workbench_done_label(job: &Value, data: &Value) -> String {
    let project = workbench_entity_label(
        data,
        "projects",
        job.get("project_id").and_then(Value::as_str),
        t("workbench.no_project"),
    );
    format!(
        "✓ {} · {}",
        project,
        string(job, "title", t("common.unnamed_task"))
    )
}

fn workbench_elapsed(job: &Value) -> String {
    if let Some(elapsed) = job.get("elapsed").and_then(Value::as_str)
        && !elapsed.trim().is_empty()
    {
        return elapsed.to_owned();
    }
    let started = job
        .get("started_at")
        .or_else(|| job.get("created_at"))
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
    let Some(started) = started else {
        return t("common.unknown").to_owned();
    };
    let minutes = (Utc::now() - started.with_timezone(&Utc))
        .num_minutes()
        .max(0);
    if minutes == 0 {
        t("common.just_now").to_owned()
    } else {
        format!("{minutes} {}", t("workbench.minutes"))
    }
}

fn workbench_model_label(job: &Value, data: &Value) -> String {
    let model = job.get("model");
    let model_ref = model.and_then(Value::as_str).or_else(|| {
        model
            .and_then(|value| value.get("ref"))
            .and_then(Value::as_str)
    });
    if let Some(model_ref) = model_ref {
        if let Some(model_item) = array(data, "models").iter().find(|item| {
            item.get("ref")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                == Some(model_ref)
        }) {
            return string(
                model_item,
                "display_name",
                &string(model_item, "label", &string(model_item, "name", model_ref)),
            );
        }
        return model_ref.to_owned();
    }
    model
        .and_then(|value| {
            value
                .get("display_name")
                .or_else(|| value.get("label"))
                .or_else(|| value.get("name"))
        })
        .and_then(Value::as_str)
        .unwrap_or(t("common.unknown"))
        .to_owned()
}

#[allow(clippy::collapsible_if)]
fn workbench_waiting(
    waiting: &[Value],
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let mut container = div().flex().flex_col().gap_2();
    for (item_index, item) in waiting.iter().enumerate() {
        let kind = string(item, "kind", "");
        let mut row = div()
            .flex()
            .items_center()
            .justify_between()
            .py_2()
            .border_b_1()
            .border_color(tokens.border)
            .child(waiting_title(item, &kind));
        match kind.as_str() {
            "review" => {
                if let Some(project_id) = item.get("project_id").and_then(Value::as_str) {
                    row = row
                        .child(action_button_with_id(
                            format!("workbench-waiting-{item_index}-confirm"),
                            t("workbench.confirm"),
                            FeatureAction::Rpc {
                                method: "project.confirm_done".into(),
                                params: json!({"project_id": project_id}),
                            },
                            cx,
                        ))
                        .child(action_button_with_id(
                            format!("workbench-waiting-{item_index}-changes"),
                            t("workbench.changes"),
                            FeatureAction::Navigate(format!("project/{project_id}/changes")),
                            cx,
                        ));
                }
            }
            "approval" => {
                if let Some(approval) = item.get("approval") {
                    if let Some(approval_id) = approval.get("id").and_then(Value::as_str) {
                        for (label, decision) in [
                            (t("workbench.allow_once"), "allow_once"),
                            (t("workbench.always_allow"), "always_allow"),
                            (t("workbench.deny"), "deny"),
                        ] {
                            row = row.child(action_button_with_id(
                                format!("workbench-waiting-{item_index}-approval-{decision}"),
                                label,
                                FeatureAction::Rpc {
                                    method: "approval.decide".into(),
                                    params: json!({"approval_id": approval_id, "decision": decision}),
                                },
                                cx,
                            ));
                        }
                    }
                }
            }
            "question" => {
                if let Some(question) = item.get("question") {
                    if let Some(question_id) = question.get("id").and_then(Value::as_str) {
                        if let Some(options) = question.get("options").and_then(Value::as_array) {
                            for (index, option) in options.iter().enumerate() {
                                row = row.child(action_button_with_id(
                                    format!("workbench-waiting-{item_index}-question-{index}"),
                                    option.as_str().unwrap_or(t("common.confirm")),
                                    FeatureAction::Rpc {
                                        method: "question.answer".into(),
                                        params: json!({"question_id": question_id, "option_index": index}),
                                    },
                                    cx,
                                ));
                            }
                        }
                        if question
                            .get("allow_free_text")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
                            row = row.child(action_button_with_id(
                                format!("workbench-waiting-{item_index}-answer"),
                                t("workbench.answer"),
                                FeatureAction::Navigate(format!("question/{question_id}")),
                                cx,
                            ));
                        }
                    }
                }
            }
            "takeover" => {
                let bot_id = item
                    .get("bot_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                row = row.child(action_button_with_id(
                    format!("workbench-waiting-{item_index}-takeover"),
                    t("workbench.takeover"),
                    FeatureAction::Rpc {
                        method: "takeover.start".into(),
                        params: json!({"bot_id": bot_id}),
                    },
                    cx,
                ));
            }
            _ => {}
        }
        container = container.child(row);
    }
    container
}

fn waiting_title(item: &Value, kind: &str) -> String {
    match kind {
        "approval" => item
            .get("approval")
            .map(|approval| {
                format!(
                    "{} · {}",
                    t("workbench.approval"),
                    string(approval, "summary", t("common.unknown"))
                )
            })
            .unwrap_or_else(|| t("workbench.approval").to_string()),
        "question" => item
            .get("question")
            .map(|question| {
                format!(
                    "{} · {}",
                    t("workbench.question"),
                    string(question, "text", t("common.unknown"))
                )
            })
            .unwrap_or_else(|| t("workbench.question").to_string()),
        "takeover" => format!(
            "{} · {}",
            t("workbench.takeover"),
            string(item, "reason", t("common.unknown"))
        ),
        _ => t("workbench.review").to_string(),
    }
}

fn dashboard(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let days = data
        .get("heatmap")
        .and_then(|heatmap| heatmap.get("days"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| array(data, "days").len());
    let details = dashboard_details(data);
    let heat = heatmap(data, inputs, tokens, cx);
    let trend = trend_chart(data, tokens, cx);
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(
            t("dashboard.title"),
            t("dashboard.period"),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(Input::new(input_state(inputs, "from")).id("dashboard-from-input"))
                .child(Input::new(input_state(inputs, "to")).id("dashboard-to-input"))
                .child(dashboard_period_rpc_button(
                    t("dashboard.refresh_summary"),
                    "usage.summary",
                    inputs,
                    usage_summary_period,
                    cx,
                ))
                .child(dashboard_period_rpc_button(
                    t("dashboard.refresh_heatmap"),
                    "usage.heatmap",
                    inputs,
                    usage_heatmap_period,
                    cx,
                )),
        )
        .child(div().flex().gap_3().children([
            stat(
                t("dashboard.tokens"),
                &dashboard_metric_with_delta(data, "tokens", "0"),
                tokens,
            ),
            stat(
                t("dashboard.requests"),
                &dashboard_metric_with_delta(data, "requests", "0"),
                tokens,
            ),
            stat(
                t("dashboard.cost"),
                &dashboard_metric_with_delta(data, "cost", t("common.unknown")),
                tokens,
            ),
            stat(
                t("dashboard.tasks"),
                &dashboard_metric_with_delta(data, "tasks_done", "0"),
                tokens,
            ),
            stat(
                t("dashboard.cache"),
                &dashboard_cache_with_delta(data),
                tokens,
            ),
        ]))
        .child(card(t("dashboard.heatmap"), heat, tokens))
        .child(card(t("dashboard.trend"), trend, tokens))
        .child(card(
            t("dashboard.breakdown"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div().flex().gap_2().children([
                        dashboard_dimension_button(t("dashboard.model"), "model", cx)
                            .into_any_element(),
                        dashboard_dimension_button(t("dashboard.bot"), "bot", cx)
                            .into_any_element(),
                        dashboard_dimension_button(t("dashboard.project"), "project", cx)
                            .into_any_element(),
                    ]),
                )
                .child(
                    div().flex().gap_2().children([
                        dashboard_rpc_button(
                            t("dashboard.refresh_summary"),
                            "usage.summary",
                            usage_summary_params(data),
                            cx,
                        )
                        .into_any_element(),
                        dashboard_heatmap_metric_button(
                            t("dashboard.refresh_heatmap"),
                            "tokens",
                            "calendar",
                            cx,
                        )
                        .into_any_element(),
                        dashboard_heatmap_metric_button(
                            t("dashboard.metric_cost"),
                            "cost",
                            "calendar",
                            cx,
                        )
                        .into_any_element(),
                        dashboard_heatmap_metric_button(
                            t("dashboard.metric_requests"),
                            "requests",
                            "calendar",
                            cx,
                        )
                        .into_any_element(),
                        dashboard_heatmap_metric_button(
                            t("dashboard.weekhour"),
                            "tokens",
                            "weekhour",
                            cx,
                        )
                        .into_any_element(),
                    ]),
                )
                .child(
                    div().flex().gap_2().children([
                        dashboard_metric_button(t("dashboard.metric_tokens"), "tokens", false, cx)
                            .into_any_element(),
                        dashboard_metric_button(t("dashboard.metric_cost"), "cost", false, cx)
                            .into_any_element(),
                        dashboard_metric_button(
                            t("dashboard.metric_requests"),
                            "requests",
                            false,
                            cx,
                        )
                        .into_any_element(),
                        dashboard_metric_button(t("dashboard.split_io"), "tokens", true, cx)
                            .into_any_element(),
                    ]),
                )
                .child(action_button(
                    t("dashboard.export"),
                    FeatureAction::Navigate("usage/export.csv".into()),
                    cx,
                ))
                .child(dashboard_detail_rows(&details, data, tokens, cx))
                .child(div().text_sm().text_color(tokens.secondary).child(format!(
                    "{} {}",
                    days,
                    t("dashboard.days")
                ))),
            tokens,
        ))
}

fn dashboard_detail_rows(
    details: &[Value],
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let dimension = dashboard_dimension(data).to_owned();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(details.iter().enumerate().map(|(index, item)| {
            let label = string(item, "label", &string(item, "source", t("common.unknown")));
            let usage = item.get("usage").unwrap_or(item);
            let input = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cached = usage
                .get("cache_read_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let requests = usage.get("requests").and_then(Value::as_u64).unwrap_or(0);
            let cost = usage.get("cost").and_then(json_value_text);
            let key = string(item, "key", "");
            let drill = if key.is_empty() {
                None
            } else if dimension == "bot" {
                Some(json!({"bot_id": key}))
            } else if dimension == "project" {
                Some(json!({"project_id": key}))
            } else {
                None
            };
            let drill_button = drill.map(|drill| {
                let mut drill_data = data.clone();
                if let Value::Object(object) = &mut drill_data {
                    object.insert("drill".into(), drill);
                }
                dashboard_rpc_button_with_id(
                    format!("dashboard-drill-{dimension}-{key}-{index}"),
                    t("dashboard.drill"),
                    "usage.breakdown",
                    usage_breakdown_params(&drill_data, &dimension),
                    cx,
                )
            });
            let cost_label = cost.map(|cost| format!(" · ¥{cost}")).unwrap_or_default();
            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .py_2()
                .border_b_1()
                .border_color(tokens.border)
                .child(format!(
                "{label}   in {input} · out {output} · cache {cached} · {requests} req{cost_label}"
            ));
            if let Some(drill_button) = drill_button {
                row = row.child(drill_button);
            }
            row
        }))
}

fn heatmap(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let heatmap_data = data.get("heatmap").unwrap_or(data);
    let heatmap_data = heatmap_data
        .get("calendar")
        .or_else(|| heatmap_data.get("weekhour"))
        .unwrap_or(heatmap_data);
    let thresholds = heatmap_data
        .get("thresholds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let selected_day = data
        .get("selected_day")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let from_input = inputs.get("from").cloned();
    let to_input = inputs.get("to").cloned();
    if let Some(days) = heatmap_data.get("days").and_then(Value::as_array) {
        if days.is_empty() {
            return div()
                .text_sm()
                .text_color(tokens.secondary)
                .child(t("dashboard.no_data"));
        }
        let weeks = days.chunks(7).map(|week| week.to_vec()).collect::<Vec<_>>();
        let mut previous_month = String::new();
        let month_labels = weeks
            .iter()
            .map(|week| {
                let month = week
                    .first()
                    .and_then(|day| day.get("date"))
                    .and_then(Value::as_str)
                    .and_then(|date| date.get(5..7))
                    .unwrap_or_default()
                    .to_owned();
                if month.is_empty() || month == previous_month {
                    String::new()
                } else {
                    previous_month = month.clone();
                    format!(
                        "{}{}",
                        month.trim_start_matches('0'),
                        t("dashboard.month_suffix")
                    )
                }
            })
            .collect::<Vec<_>>();
        let weekday_labels = [
            t("dashboard.weekday_mon"),
            t("dashboard.weekday_tue"),
            t("dashboard.weekday_wed"),
            t("dashboard.weekday_thu"),
            t("dashboard.weekday_fri"),
            t("dashboard.weekday_sat"),
            t("dashboard.weekday_sun"),
        ];
        let month_row = div()
            .flex()
            .gap_1()
            .child(div().w(px(24.)).h(px(16.)))
            .children(month_labels.iter().map(|month| {
                div()
                    .w(px(18.))
                    .text_xs()
                    .text_color(tokens.secondary)
                    .child(month.clone())
            }));
        let weekday_column = div()
            .flex()
            .flex_col()
            .gap_1()
            .children(weekday_labels.iter().map(|label| {
                div()
                    .w(px(24.))
                    .h(px(18.))
                    .text_xs()
                    .text_color(tokens.secondary)
                    .child(*label)
            }));
        let weeks_grid = div()
            .flex()
            .gap_1()
            .child(weekday_column)
            .children(weeks.iter().map(|week| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(week.iter().map(|day| {
                        let from_input = from_input.clone();
                        let to_input = to_input.clone();
                        let value = day.get("value").and_then(Value::as_f64).unwrap_or(0.);
                        let date = day
                            .get("date")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let cost = day
                            .get("cost")
                            .and_then(json_value_text)
                            .unwrap_or_else(|| t("common.unknown").to_owned());
                        let top_bot = day
                            .get("top_bot_id")
                            .and_then(Value::as_str)
                            .unwrap_or(t("common.unknown"))
                            .to_owned();
                        let color = heatmap_color(value, &thresholds, tokens);
                        let selected = date == selected_day;
                        div()
                            .id(format!("heatmap-day-{date}"))
                            .size(px(if selected { 18. } else { 14. }))
                            .rounded_sm()
                            .bg(color)
                            .border_1()
                            .border_color(if selected { tokens.primary } else { color })
                            .tooltip({
                                let tooltip = format!(
                                    "{date} · {} {} · {} {} · {} {top_bot}",
                                    t("dashboard.tokens"),
                                    value,
                                    t("dashboard.cost"),
                                    cost,
                                    t("dashboard.top_bot")
                                );
                                move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.data["selected_day"] = json!(date.clone());
                                let from = format!("{date}T00:00:00Z");
                                let to = format!("{date}T23:59:59Z");
                                this.data["from"] = json!(from.clone());
                                this.data["to"] = json!(to.clone());
                                if let Some(input) = &from_input {
                                    input.update(cx, |state, cx| {
                                        state.set_value(from.clone(), window, cx);
                                    });
                                }
                                if let Some(input) = &to_input {
                                    input.update(cx, |state, cx| {
                                        state.set_value(to.clone(), window, cx);
                                    });
                                }
                                let snapshot = this.data.clone();
                                this.emit_action(
                                    FeatureAction::Rpc {
                                        method: "usage.timeseries".into(),
                                        params: usage_day_timeseries_params(&snapshot, &date),
                                    },
                                    cx,
                                );
                                this.emit_action(
                                    FeatureAction::Rpc {
                                        method: "usage.summary".into(),
                                        params: usage_summary_params(&snapshot),
                                    },
                                    cx,
                                );
                                this.emit_action(
                                    FeatureAction::Rpc {
                                        method: "usage.breakdown".into(),
                                        params: usage_breakdown_params(
                                            &snapshot,
                                            dashboard_dimension(&snapshot),
                                        ),
                                    },
                                    cx,
                                );
                            }))
                    }))
            }));
        return div()
            .flex()
            .flex_col()
            .gap_1()
            .child(month_row)
            .child(weeks_grid);
    }
    let matrix = heatmap_data
        .get("matrix")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if matrix.is_empty() {
        return div()
            .text_sm()
            .text_color(tokens.secondary)
            .child(t("dashboard.no_data"));
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(matrix.iter().map(|row| {
            div()
                .flex()
                .gap_1()
                .children(row.as_array().into_iter().flatten().map(|value| {
                    let color = heatmap_color(value.as_f64().unwrap_or(0.), &thresholds, tokens);
                    div().size(px(14.)).rounded_sm().bg(color)
                }))
        }))
}

fn heatmap_color(value: f64, thresholds: &[Value], tokens: &Tokens) -> gpui_kit::Hsla {
    if value == 0. {
        tokens.bot
    } else if thresholds.len() >= 3 {
        match value {
            value if value <= thresholds[0].as_f64().unwrap_or(value) => tokens.sidebar,
            value if value <= thresholds[1].as_f64().unwrap_or(value) => tokens.success,
            value if value <= thresholds[2].as_f64().unwrap_or(value) => tokens.accent,
            _ => tokens.primary,
        }
    } else {
        tokens.accent
    }
}

fn usage_day_timeseries_params(data: &Value, day: &str) -> Value {
    let mut params = usage_timeseries_params(
        data,
        dashboard_dimension(data),
        data.get("metric")
            .and_then(Value::as_str)
            .unwrap_or("tokens"),
        data.get("split_io")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    params["from"] = json!(format!("{day}T00:00:00Z"));
    params["to"] = json!(format!("{day}T23:59:59Z"));
    params
}

fn trend_chart(data: &Value, tokens: &Tokens, cx: &mut Context<FeaturePage>) -> AnyElement {
    let series = data
        .get("timeseries")
        .and_then(|value| value.get("series"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !series.is_empty() {
        let hidden = data
            .get("hidden_series")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut variants = Vec::new();
        for (index, item) in series.iter().enumerate() {
            let label = string(item, "label", &format!("series-{index}"));
            let input = item.get("input_values").and_then(Value::as_array);
            let output = item.get("output_values").and_then(Value::as_array);
            if input.is_some() || output.is_some() {
                if let Some(values) = input {
                    variants.push((
                        format!("{label} · {}", t("dashboard.input")),
                        values.clone(),
                        series_color(tokens, variants.len()),
                        false,
                    ));
                }
                if let Some(values) = output {
                    variants.push((
                        format!("{label} · {}", t("dashboard.output")),
                        values.clone(),
                        series_color(tokens, variants.len()),
                        true,
                    ));
                }
            } else if let Some(values) = item.get("values").and_then(Value::as_array) {
                variants.push((
                    label,
                    values.clone(),
                    series_color(tokens, variants.len()),
                    false,
                ));
            }
        }
        if variants.len() > 6 {
            let retained = variants.drain(..5).collect::<Vec<_>>();
            let other_values = aggregate_series_values(
                &variants
                    .iter()
                    .map(|(_, values, _, _)| values.clone())
                    .collect::<Vec<_>>(),
            );
            variants = retained;
            variants.push((
                t("dashboard.other").to_owned(),
                other_values,
                series_color(tokens, 5),
                false,
            ));
        }
        let legend = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(variants.iter().map(|(label, _, color, dashed)| {
                let label_for_event = label.clone();
                let is_hidden = hidden
                    .iter()
                    .any(|value| value.as_str() == Some(label.as_str()));
                Button::new(format!("usage-legend-{label}"))
                    .label(format!(
                        "{} {}{}",
                        if is_hidden { "○" } else { "●" },
                        if *dashed { "┄ " } else { "" },
                        label
                    ))
                    .text_color(*color)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut selected = this
                            .data
                            .get("hidden_series")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if let Some(index) = selected
                            .iter()
                            .position(|value| value.as_str() == Some(label_for_event.as_str()))
                        {
                            selected.remove(index);
                        } else {
                            selected.push(json!(label_for_event.clone()));
                        }
                        this.data["hidden_series"] = Value::Array(selected);
                        cx.notify();
                    }))
            }));
        let visible = variants
            .iter()
            .filter(|(label, _, _, _)| {
                !hidden
                    .iter()
                    .any(|value| value.as_str() == Some(label.as_str()))
            })
            .collect::<Vec<_>>();
        let (mut y_min, mut y_max) = visible
            .iter()
            .flat_map(|(_, values, _, _)| values.iter().filter_map(Value::as_f64))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), value| {
                (min.min(value), max.max(value))
            });
        if !y_min.is_finite() || !y_max.is_finite() {
            y_min = 0.;
            y_max = 1.;
        } else if (y_max - y_min).abs() < f64::EPSILON {
            y_max = y_min + 1.;
        }
        let axis_labels = data
            .get("timeseries")
            .and_then(|timeseries| {
                timeseries
                    .get("buckets")
                    .or_else(|| timeseries.get("labels"))
                    .or_else(|| timeseries.get("dates"))
            })
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let charts = visible
            .iter()
            .enumerate()
            .map(|(index, (label, values, color, dashed))| {
                let points = values
                    .iter()
                    .enumerate()
                    .map(|(point, value)| (point, value.as_f64().unwrap_or(0.) as f32))
                    .collect::<Vec<_>>();
                let labels = axis_labels.clone();
                let chart = LineChart::new(points)
                    .id(format!("usage-timeseries-{index}"))
                    .x(move |(point, _)| {
                        labels
                            .get(*point)
                            .cloned()
                            .unwrap_or_else(|| point.to_string())
                    })
                    .y(|(_, value)| *value)
                    .stroke(*color)
                    .y_domain(y_min as f32, y_max as f32)
                    .x_tick_count(6)
                    .y_axis(index == 0)
                    .x_axis(index == 0)
                    .grid(index == 0)
                    .interactive(index == 0)
                    .name(label.clone());
                let chart = if *dashed { chart } else { chart.dot() };
                div().absolute().inset_0().child(chart)
            });
        return div()
            .flex()
            .flex_col()
            .gap_2()
            .child(legend)
            .child(div().relative().h(px(220.)).w_full().children(charts))
            .into_any_element();
    }
    let values = array(data, "trend");
    if values.is_empty() {
        return div()
            .text_sm()
            .text_color(tokens.secondary)
            .child(t("dashboard.no_data"))
            .into_any_element();
    }
    let points = values
        .iter()
        .enumerate()
        .map(|(index, value)| (index, value.as_f64().unwrap_or(0.) as f32))
        .collect::<Vec<_>>();
    div()
        .h(px(150.))
        .child(
            LineChart::new(points)
                .id("usage-timeseries")
                .x(|(index, _)| index.to_string())
                .y(|(_, value)| *value)
                .stroke(tokens.accent)
                .dot(),
        )
        .into_any_element()
}

fn series_color(tokens: &Tokens, index: usize) -> gpui_kit::Hsla {
    match index % 6 {
        0 => tokens.accent,
        1 => tokens.success,
        2 => tokens.attention,
        3 => tokens.danger,
        4 => tokens.primary,
        _ => tokens.secondary,
    }
}

fn aggregate_series_values(series: &[Vec<Value>]) -> Vec<Value> {
    let length = series.iter().map(Vec::len).max().unwrap_or(0);
    (0..length)
        .map(|index| {
            json!(
                series
                    .iter()
                    .filter_map(|values| values.get(index).and_then(Value::as_f64))
                    .sum::<f64>()
            )
        })
        .collect()
}

fn skills(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let route_query = skill_route_query(data);
    let filter = query_value(data, "filter").filter(|value| {
        matches!(
            value.as_str(),
            "all" | "builtin" | "user" | "draft" | "disabled"
        )
    });
    let query = route_query
        .unwrap_or_else(|| input_state(inputs, "query").read(cx).value().to_string())
        .to_lowercase();
    let skills = array(data, "skills")
        .into_iter()
        .filter(|skill| skill_matches_filter(skill, filter.as_deref(), &query))
        .collect::<Vec<_>>();
    let selected = data.get("selected").unwrap_or(&Value::Null);
    let selected_name = selected
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let selected_builtin = !selected_name.is_empty() && skill_source_kind(selected) == "builtin";
    let list = div()
        .flex()
        .flex_col()
        .gap_1()
        .children(skills.iter().enumerate().map(|(index, skill)| {
            let id = string(skill, "name", &format!("skill-{index}"));
            let name = string(skill, "name", t("skills.unnamed"));
            let enabled = skill
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .w_full()
                .p_2()
                .rounded_md()
                .bg(
                    if selected.get("name").and_then(Value::as_str) == Some(id.as_str()) {
                        tokens.bot
                    } else {
                        tokens.window
                    },
                )
                .child(div().flex_1().min_w_0().truncate().child(format!(
                    "ϟ {name} · {} · {}",
                    skill_source(skill),
                    skill_invocations(skill)
                )))
                .child({
                    let mut actions = div().flex().flex_wrap().gap_2().flex_shrink_0();
                    actions = actions.child(action_button_with_id(
                        format!("skill-{index}-toggle"),
                        if enabled {
                            t("skills.disable")
                        } else {
                            t("skills.enable")
                        },
                        FeatureAction::Rpc {
                            method: "skill.set_enabled".into(),
                            params: skill_enabled_params(&id, !enabled, None),
                        },
                        cx,
                    ));
                    actions = actions.child(action_button_with_id(
                        format!("skill-{index}-edit"),
                        t("skills.edit"),
                        FeatureAction::Navigate(format!("skill/{id}")),
                        cx,
                    ));
                    if skill_source(skill) == t("skills.draft") {
                        actions = actions.child(action_button_with_id(
                            format!("skill-{index}-publish"),
                            t("skills.publish"),
                            FeatureAction::Rpc {
                                method: "skill.publish".into(),
                                params: skill_publish_params(&id),
                            },
                            cx,
                        ));
                    }
                    actions
                })
        }));
    let editor = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(labeled_field(
            t("skills.field_name"),
            Input::new(input_state(inputs, "name"))
                .id("skill-name-input")
                .disabled(selected_builtin),
        ))
        .child(labeled_field(
            t("skills.field_content"),
            Textarea::new(textarea_state(textareas, "content"))
                .h(px(240.))
                .disabled(selected_builtin),
        ))
        .child(skill_create_button(t("skills.new"), inputs, textareas, cx))
        .child(card(
            t("skills.import_group"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(labeled_field(
                    t("skills.field_path"),
                    Input::new(input_state(inputs, "path")).id("skill-import-path-input"),
                ))
                .child(skill_import_button(t("skills.import"), inputs, cx))
                .child(labeled_field(
                    t("skills.field_git_url"),
                    Input::new(input_state(inputs, "git_url")).id("skill-import-git-url-input"),
                ))
                .child(labeled_field(
                    t("skills.field_git_subdir"),
                    Input::new(input_state(inputs, "git_subdir"))
                        .id("skill-import-git-subdir-input"),
                ))
                .child(skill_import_git_button(t("skills.import_git"), inputs, cx))
                .child(skill_upload_button(t("skills.import_upload"), cx)),
            tokens,
        ))
        .child(card(
            t("skills.description"),
            div().child(string(selected, "description", t("skills.select_hint"))),
            tokens,
        ))
        .child(card(
            t("skills.instructions"),
            TextView::markdown(
                "skill-preview-markdown",
                string(selected, "content", t("skills.markdown_hint")),
            ),
            tokens,
        ))
        .child(card(
            t("skills.files"),
            rows(
                array(selected, "files")
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
                tokens,
            ),
            tokens,
        ))
        .child(card(
            t("skills.bot_scope"),
            skill_bot_controls(selected, data, cx),
            tokens,
        ))
        .child({
            let mut actions = div().flex().gap_2().child(action_button(
                t("skills.preview"),
                FeatureAction::Toast(t("skills.preview_ready").to_string()),
                cx,
            ));
            if !selected_name.is_empty() && !selected_builtin {
                actions = actions
                    .child(textarea_rpc_button(
                        t("settings.save"),
                        "skill.update",
                        skill_update_params(&selected_name, ""),
                        "content",
                        textareas,
                        None,
                        cx,
                    ))
                    .child(action_button(
                        t("skills.delete"),
                        FeatureAction::Rpc {
                            method: "skill.delete".into(),
                            params: skill_delete_params(&selected_name),
                        },
                        cx,
                    ));
            }
            actions
        });
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(t("skills.title"), t("skills.subtitle"), tokens))
        .child(
            div()
                .flex()
                .gap_2()
                .child(labeled_field(
                    t("skills.field_search"),
                    Input::new(input_state(inputs, "query")).id("skill-search-input"),
                ))
                .child(skill_search_button(inputs, cx))
                .child(skill_filter_button(t("skills.all"), None, cx))
                .child(skill_filter_button(
                    t("skills.builtin"),
                    Some("builtin"),
                    cx,
                ))
                .child(skill_filter_button(t("skills.user"), Some("user"), cx))
                .child(skill_filter_button(t("skills.draft"), Some("draft"), cx))
                .child(skill_filter_button(
                    t("skills.disabled"),
                    Some("disabled"),
                    cx,
                )),
        )
        .child(
            div()
                .flex()
                .gap_4()
                .child(div().w(px(280.)).flex_shrink_0().child(card(
                    t("skills.title"),
                    list,
                    tokens,
                )))
                .child(editor.flex_1().min_w_0()),
        )
}

fn skill_bot_controls(
    selected: &Value,
    data: &Value,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let name = selected
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if name.is_empty() {
        return div().child(t("skills.select_hint"));
    }
    let disabled = selected
        .get("disabled_bot_ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    div().flex().flex_col().gap_1().children(
        array(data, "bots")
            .iter()
            .filter(|bot| !bot.get("is_main").and_then(Value::as_bool).unwrap_or(false))
            .filter_map(|bot| {
                let bot_id = bot.get("id").and_then(Value::as_str)?.to_owned();
                let bot_name = bot
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&bot_id)
                    .to_owned();
                let is_enabled = !disabled
                    .iter()
                    .any(|id| id.as_str() == Some(bot_id.as_str()));
                let name = name.clone();
                Some(
                    Checkbox::new(format!("skill-bot-{bot_id}"))
                        .label(bot_name)
                        .checked(is_enabled)
                        .on_change(cx.listener(move |this, next, _, cx| {
                            this.emit_action(
                                FeatureAction::Rpc {
                                    method: "skill.set_enabled".into(),
                                    params: skill_enabled_params(&name, *next, Some(&bot_id)),
                                },
                                cx,
                            );
                        })),
                )
            }),
    )
}

fn skill_filter_button(
    label: &str,
    filter: Option<&str>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let target = filter
        .map(|filter| format!("skills?filter={filter}"))
        .unwrap_or_else(|| "skills?filter=all".to_owned());
    action_button(label, FeatureAction::Navigate(target), cx)
}

fn skill_route_query(data: &Value) -> Option<String> {
    let raw = data.get("filter").and_then(Value::as_str)?;
    raw.split('&')
        .find_map(|part| part.strip_prefix("query=").map(str::to_owned))
        .filter(|query| !query.is_empty())
}

fn skill_search_button(
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let query = input_state(inputs, "query").clone();
    Button::new("skills-search-button")
        .label(t("skills.search"))
        .on_click(cx.listener(move |this, _, _, cx| {
            let query = query.read(cx).value().to_string();
            this.emit_action(FeatureAction::Navigate(format!("skills?query={query}")), cx);
        }))
}

fn query_value(data: &Value, key: &str) -> Option<String> {
    let query = data.get(key).and_then(Value::as_str)?;
    query
        .split('&')
        .find_map(|part| part.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

fn skill_matches_filter(skill: &Value, filter: Option<&str>, query: &str) -> bool {
    let name = string(skill, "name", "").to_lowercase();
    let description = string(skill, "description", "").to_lowercase();
    if !query.trim().is_empty() && !name.contains(query) && !description.contains(query) {
        return false;
    }
    match filter {
        Some("disabled") => !skill
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        Some("builtin" | "user" | "draft") => skill_source_kind(skill) == filter.unwrap(),
        _ => true,
    }
}

fn skill_source_kind(skill: &Value) -> &str {
    match skill
        .get("source")
        .and_then(|source| {
            source
                .as_str()
                .or_else(|| source.get("kind").and_then(Value::as_str))
        })
        .unwrap_or("user")
    {
        "imported" => "user",
        source => source,
    }
}

fn skill_source(skill: &Value) -> &'static str {
    let source = skill
        .get("source")
        .and_then(|source| {
            source
                .as_str()
                .or_else(|| source.get("kind").and_then(Value::as_str))
        })
        .unwrap_or("user");
    match source {
        "builtin" => t("skills.builtin"),
        "draft" => t("skills.draft"),
        _ => t("skills.user"),
    }
}

fn skill_invocations(skill: &Value) -> String {
    skill
        .get("invocations_7d")
        .and_then(|value| value.get("total"))
        .or_else(|| skill.get("invocations"))
        .map(Value::to_string)
        .unwrap_or_else(|| "0".to_owned())
}

fn bot_editor(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let title = if data.get("id").is_some() {
        t("bot.edit")
    } else {
        t("bot.new")
    };
    let body = rows(
        [
            format!(
                "{}   {}",
                t("bot.name"),
                string(data, "name", t("bot.default_name"))
            ),
            format!(
                "{}   {}",
                t("bot.label"),
                string(data, "label", t("bot.default_label"))
            ),
            format!(
                "{}   {}",
                t("bot.description"),
                string(data, "description", t("bot.default_description"))
            ),
            format!(
                "{}   {}",
                t("settings.model"),
                string(data, "model", t("bot.default_model"))
            ),
            format!(
                "{}   {}",
                t("bot.parallel"),
                string(data, "max_parallel", "3")
            ),
            format!(
                "{}   {}",
                t("bot.browser"),
                string(data, "browser_mode", t("bot.default_browser"))
            ),
        ],
        tokens,
    );
    let model_picker = bot_model_picker(data, inputs, cx);
    let avatar_picker = bot_avatar_picker(data, tokens, cx);
    let bot_actions = bot_actions(data, cx);
    let tool_picker = if data
        .get("is_main")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        div().into_any_element()
    } else {
        bot_tools_picker(data, cx).into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(title, t("bot.subtitle"), tokens))
        .child(labeled_field(
            t("bot.name"),
            Input::new(input_state(inputs, "name")).id("bot-name-input"),
        ))
        .child(labeled_field(
            t("bot.label"),
            Input::new(input_state(inputs, "label")).id("bot-label-input"),
        ))
        .child(labeled_field(
            t("bot.description"),
            Textarea::new(textarea_state(textareas, "description")).h(px(120.)),
        ))
        .child(labeled_field(
            t("settings.model"),
            Input::new(input_state(inputs, "model")).id("bot-model-input"),
        ))
        .child(labeled_field(
            t("bot.parallel"),
            Input::new(input_state(inputs, "max_parallel")).id("bot-parallel-input"),
        ))
        .child(labeled_field(
            t("bot.browser"),
            Input::new(input_state(inputs, "browser_mode")).id("bot-browser-input"),
        ))
        .child(labeled_field(t("bot.model_options"), model_picker))
        .child(labeled_field(
            t("bot.browser"),
            bot_browser_picker(inputs, cx),
        ))
        .child(labeled_field(t("settings.tools"), tool_picker))
        .child(bot_notification_toggle(data, cx))
        .child(avatar_picker)
        .child(if data.get("id").is_some() {
            card(title, body, tokens).into_any_element()
        } else {
            div().into_any_element()
        })
        .child(bot_save_button(
            t("settings.save"),
            data,
            inputs,
            textareas,
            cx,
        ))
        .child(bot_actions)
}

fn bot_notification_toggle(
    data: &Value,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let bot_id = data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let enabled = data
        .get("notifications")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Checkbox::new("bot-notifications")
        .label(t("bot.notifications"))
        .checked(enabled)
        .on_change(cx.listener(move |this, next, _, cx| {
            if bot_id.is_empty() {
                this.data["notifications"] = json!(*next);
                cx.notify();
            } else {
                this.emit_action(
                    FeatureAction::Rpc {
                        method: "bot.update".into(),
                        params: bot_notifications_update_params(&bot_id, *next),
                    },
                    cx,
                );
            }
        }))
}

fn bot_model_picker(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let model = input_state(inputs, "model").clone();
    let models = array(data, "models");
    div()
        .flex()
        .gap_1()
        .children(models.iter().filter_map(|item| {
            let value = item
                .as_str()
                .map(str::to_owned)
                .or_else(|| item.get("ref").and_then(Value::as_str).map(str::to_owned))?;
            let label = item
                .get("display_name")
                .and_then(Value::as_str)
                .or_else(|| item.get("label").and_then(Value::as_str))
                .or_else(|| item.get("name").and_then(Value::as_str))
                .unwrap_or(&value)
                .to_owned();
            let model = model.clone();
            Some(
                Button::new(format!("bot-model-option-{value}"))
                    .label(label)
                    .on_click(cx.listener(move |_this, _, window, cx| {
                        model.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
                    })),
            )
        }))
}

fn bot_browser_picker(
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let browser = input_state(inputs, "browser_mode").clone();
    div().flex().gap_1().children(
        [
            ("headless", "bot.browser_headless"),
            ("headless_profile", "bot.browser_profile"),
            ("attach", "bot.browser_attach"),
        ]
        .into_iter()
        .map(|(value, label_key)| {
            let browser = browser.clone();
            Button::new(format!("bot-browser-option-{value}"))
                .label(t(label_key))
                .on_click(cx.listener(move |_this, _, window, cx| {
                    browser.update(cx, |state, cx| {
                        state.set_value(value.to_owned(), window, cx)
                    });
                }))
        }),
    )
}

fn bot_tools_picker(data: &Value, cx: &mut Context<FeaturePage>) -> impl IntoElement + use<> {
    let bot_id = data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let tools = data.get("tools").cloned().unwrap_or_else(default_bot_tools);
    div().flex().flex_wrap().gap_2().children(
        ["files", "bash", "browser", "subagent", "web", "mcp"]
            .into_iter()
            .map(|key| {
                let checked = tools.get(key).and_then(Value::as_bool).unwrap_or(false);
                let bot_id = bot_id.clone();
                let key = key.to_owned();
                Checkbox::new(format!("bot-tool-{key}"))
                    .label(t(match key.as_str() {
                        "files" => "settings.tool.files",
                        "bash" => "settings.tool.shell",
                        "browser" => "settings.tool.browser",
                        "subagent" => "settings.tool.subagent",
                        "web" => "settings.tool.web",
                        _ => "settings.tool.mcp",
                    }))
                    .checked(checked)
                    .on_change(cx.listener(move |this, next, _, cx| {
                        if bot_id.is_empty() {
                            let mut next_tools = this
                                .data
                                .get("tools")
                                .cloned()
                                .unwrap_or_else(default_bot_tools);
                            next_tools[key.clone()] = json!(*next);
                            this.data["tools"] = next_tools;
                            cx.notify();
                        } else {
                            let mut next_tools = this
                                .data
                                .get("tools")
                                .cloned()
                                .unwrap_or_else(default_bot_tools);
                            next_tools[key.clone()] = json!(*next);
                            this.data["tools"] = next_tools.clone();
                            this.emit_action(
                                FeatureAction::Rpc {
                                    method: "bot.update".into(),
                                    params: bot_tools_update_params(&bot_id, next_tools),
                                },
                                cx,
                            );
                        }
                    }))
            }),
    )
}

fn bot_avatar_picker(
    data: &Value,
    _tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let bot_id = data
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    let mut buttons = Vec::new();
    for color in 0u8..10 {
        buttons.push(
            bot_avatar_button(
                &format!("🎨{color}"),
                &bot_id,
                json!({"kind":"bean","color":color}),
                cx,
            )
            .into_any_element(),
        );
    }
    for emoji in ["🤖", "🦊", "🐼", "🐙", "🦄", "🚀"] {
        buttons.push(
            bot_avatar_button(emoji, &bot_id, json!({"kind":"emoji","emoji":emoji}), cx)
                .into_any_element(),
        );
    }
    div().flex().gap_1().children(buttons)
}

fn bot_avatar_button(
    label: &str,
    bot_id: &str,
    avatar: Value,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_owned();
    let bot_id = bot_id.to_owned();
    Button::new(format!("bot-avatar-{label}"))
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            if bot_id.is_empty() {
                this.data["avatar"] = avatar.clone();
                cx.notify();
            } else {
                this.emit_action(
                    FeatureAction::Rpc {
                        method: "bot.update".into(),
                        params: bot_avatar_update_params(&bot_id, avatar.clone()),
                    },
                    cx,
                );
            }
        }))
}

fn bot_actions(data: &Value, cx: &mut Context<FeaturePage>) -> impl IntoElement + use<> {
    let Some(bot_id) = data.get("id").and_then(Value::as_str) else {
        return div();
    };
    let bot_id = bot_id.to_owned();
    let hidden = data.get("hidden").and_then(Value::as_bool).unwrap_or(false);
    let pinned = data.get("pinned").and_then(Value::as_bool).unwrap_or(false);
    if data
        .get("is_main")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return div();
    }
    let duplicate_name = format!(
        "{} {}",
        string(data, "name", t("bot.default_name")),
        t("bot.copy_suffix")
    );
    div()
        .flex()
        .gap_2()
        .child(action_button(
            if pinned { t("bot.unpin") } else { t("bot.pin") },
            FeatureAction::Rpc {
                method: "bot.update".into(),
                params: bot_pinned_update_params(&bot_id, !pinned),
            },
            cx,
        ))
        .child(action_button(
            if hidden { t("bot.show") } else { t("bot.hide") },
            FeatureAction::Rpc {
                method: "bot.update".into(),
                params: bot_visibility_update_params(&bot_id, !hidden),
            },
            cx,
        ))
        .child(action_button(
            t("bot.delete"),
            FeatureAction::Rpc {
                method: "bot.delete".into(),
                params: bot_delete_params(&bot_id),
            },
            cx,
        ))
        .child(action_button(
            t("bot.duplicate"),
            FeatureAction::Rpc {
                method: "bot.duplicate".into(),
                params: bot_duplicate_params(&bot_id, &duplicate_name),
            },
            cx,
        ))
}

fn group_editor(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    selected_member_bot_ids: &[String],
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let member_values = array(data, "members");
    let members = member_values
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let available_bots = array(data, "bots");
    let selected_count = selected_member_bot_ids.len();
    let member_checks = div().flex().flex_col().gap_1().children(
        available_bots
            .iter()
            .filter(|bot| {
                !bot.get("hidden").and_then(Value::as_bool).unwrap_or(false)
                    && !bot.get("is_main").and_then(Value::as_bool).unwrap_or(false)
            })
            .map(|bot| {
                let bot_id = string(bot, "id", "");
                let bot_name = string(bot, "name", t("common.unknown"));
                let checked = selected_member_bot_ids.iter().any(|id| id == &bot_id);
                let bot_id_for_event = bot_id.clone();
                Checkbox::new(format!("group-member-{bot_id}"))
                    .label(bot_name)
                    .checked(checked)
                    .disabled(!checked && selected_count >= 6)
                    .on_change(cx.listener(move |this, next, _, cx| {
                        if *next {
                            if !this.selected_member_bot_ids.contains(&bot_id_for_event) {
                                this.selected_member_bot_ids.push(bot_id_for_event.clone());
                            }
                        } else {
                            this.selected_member_bot_ids
                                .retain(|id| id != &bot_id_for_event);
                        }
                        cx.notify();
                    }))
            }),
    );
    let body = rows(
        [
            format!(
                "{}   {}",
                t("group.name"),
                string(data, "name", t("group.default_name"))
            ),
            format!(
                "{}   {}",
                t("group.goal"),
                string(data, "goal", t("group.goal_hint"))
            ),
            format!(
                "{}   {}",
                t("group.members"),
                if members.is_empty() {
                    t("group.main_bot").to_string()
                } else {
                    members.join("、")
                }
            ),
        ],
        tokens,
    );
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(t("group.new"), t("group.subtitle"), tokens))
        .child(labeled_field(
            t("group.name"),
            Input::new(input_state(inputs, "name")).id("group-name-input"),
        ))
        .child(labeled_field(
            t("group.goal"),
            Textarea::new(textarea_state(textareas, "goal")).h(px(120.)),
        ))
        .child(div().text_sm().child(t("group.members")))
        .child(member_checks)
        .child(card(t("group.new"), body, tokens))
        .child(group_create_button(
            t("group.create"),
            selected_member_bot_ids,
            inputs,
            textareas,
            cx,
        ))
}

fn routine_editor(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let routine_id = data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let routine_list = routines_list(data, tokens, cx);
    let history = array(data, "runs")
        .iter()
        .map(|h| {
            format!(
                "{} · {} · {}",
                string(h, "at", t("common.unknown")),
                routine_trigger(h),
                string(h, "status", t("routine.done"))
            )
        })
        .collect::<Vec<_>>();
    let summary = if routine_id.is_empty() {
        div().into_any_element()
    } else {
        card(
            t("routine.title"),
            rows(
                [
                    format!(
                        "{}   {}",
                        t("routine.instruction"),
                        string(data, "instructions", t("routine.default_instruction"))
                    ),
                    format!("{}   {}", t("routine.schedule"), routine_schedule(data)),
                    format!(
                        "{}   {}",
                        t("routine.active"),
                        if data.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
                            t("common.confirm")
                        } else {
                            t("common.cancel")
                        }
                    ),
                ],
                tokens,
            ),
            tokens,
        )
        .into_any_element()
    };
    let routine_actions = if routine_id.is_empty() {
        div()
    } else {
        div()
            .flex()
            .gap_2()
            .child(action_button(
                t("routine.test_run"),
                FeatureAction::Rpc {
                    method: "routine.test_run".into(),
                    params: routine_test_params(&routine_id),
                },
                cx,
            ))
            .child(action_button(
                t("routine.toggle"),
                FeatureAction::Rpc {
                    method: "routine.set_enabled".into(),
                    params: routine_enabled_params(
                        &routine_id,
                        !data.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                    ),
                },
                cx,
            ))
            .child(action_button(
                t("routine.delete"),
                FeatureAction::Rpc {
                    method: "routine.delete".into(),
                    params: routine_test_params(&routine_id),
                },
                cx,
            ))
    };
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(
            t("routine.title"),
            string(data, "name", t("routine.default_name")),
            tokens,
        ))
        .child(routine_list)
        .child(labeled_field(
            t("routine.title"),
            Input::new(input_state(inputs, "name")).id("routine-name-input"),
        ))
        .child(labeled_field(
            t("routine.bot"),
            Input::new(input_state(inputs, "bot_id")).id("routine-bot-input"),
        ))
        .child(labeled_field(
            t("routine.project"),
            Input::new(input_state(inputs, "project_id")).id("routine-project-input"),
        ))
        .child(routine_selectors(data, inputs, cx))
        .child(labeled_field(
            t("routine.instruction"),
            Textarea::new(textarea_state(textareas, "instructions")).h(px(180.)),
        ))
        .child(routine_schedule_inputs(data, inputs, cx))
        .child(routine_add_schedule_button(cx))
        .child(labeled_field(
            t("routine.timezone"),
            Input::new(input_state(inputs, "timezone")).id("routine-timezone-input"),
        ))
        .child(summary)
        .child(card(
            t("routine.history"),
            rows(history.iter().cloned(), tokens),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(routine_save_button(
                    t("settings.save"),
                    data,
                    inputs,
                    textareas,
                    cx,
                ))
                .child(routine_actions),
        )
}

fn routine_schedule_inputs(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    _cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let slots = data
        .get("schedule_slots")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            let schedules = array(data, "schedules").len();
            schedules.max(1) as u64
        })
        .clamp(1, 6);
    let fields = ["cron", "cron_1", "cron_2", "cron_3", "cron_4", "cron_5"];
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(
            fields
                .iter()
                .take(slots as usize)
                .enumerate()
                .map(|(index, field)| {
                    labeled_field(
                        format!("{} {}", t("routine.schedule_slot"), index + 1),
                        Input::new(input_state(inputs, field)).id(format!("routine-{field}-input")),
                    )
                }),
        )
}

fn routine_add_schedule_button(cx: &mut Context<FeaturePage>) -> impl IntoElement + use<> {
    Button::new("routine-add-schedule")
        .label(t("routine.add_schedule"))
        .on_click(cx.listener(|this, _, _, cx| {
            let current = this
                .data
                .get("schedule_slots")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            this.data["schedule_slots"] = json!(current.min(5) + 1);
            cx.notify();
        }))
}

fn routine_selectors(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let bot_input = input_state(inputs, "bot_id").clone();
    let project_input = input_state(inputs, "project_id").clone();
    let bots = array(data, "bots");
    let projects = array(data, "projects");
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().flex().gap_1().children(bots.iter().filter_map(|bot| {
            let id = bot.get("id").and_then(Value::as_str)?.to_owned();
            let label = bot
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            let input = bot_input.clone();
            Some(
                Button::new(format!("routine-bot-{id}"))
                    .label(label)
                    .on_click(cx.listener(move |_this, _, window, cx| {
                        input.update(cx, |state, cx| state.set_value(id.clone(), window, cx));
                    })),
            )
        })))
        .child(
            div()
                .flex()
                .gap_1()
                .children(projects.iter().filter_map(|project| {
                    let id = project.get("id").and_then(Value::as_str)?.to_owned();
                    let label = project
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned();
                    let input = project_input.clone();
                    Some(
                        Button::new(format!("routine-project-{id}"))
                            .label(label)
                            .on_click(cx.listener(move |_this, _, window, cx| {
                                input.update(cx, |state, cx| {
                                    state.set_value(id.clone(), window, cx)
                                });
                            })),
                    )
                })),
        )
}

fn routines_list(
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let routines = array(data, "routines");
    let selected_id = data.get("id").and_then(Value::as_str).unwrap_or_default();
    let mut list = div().flex().flex_col().gap_1();
    for routine in routines {
        let id = string(&routine, "id", "");
        let name = string(&routine, "name", t("routine.default_name"));
        let enabled = routine
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let label = format!("{}{}", if enabled { "● " } else { "○ " }, name);
        let selected_routine = routine.clone();
        let route = format!("routine/{id}");
        list = list.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .p_2()
                .rounded_md()
                .bg(if id == selected_id {
                    tokens.bot
                } else {
                    tokens.window
                })
                .child(
                    Button::new(format!("routine-select-{id}"))
                        .label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.load_selected(selected_routine.clone(), window, cx);
                            this.emit_action(FeatureAction::Navigate(route.clone()), cx);
                        })),
                ),
        );
    }
    list.child(action_button(
        t("routine.new"),
        FeatureAction::Navigate("routine/new".into()),
        cx,
    ))
}

fn search_filter_button(
    label: &str,
    kind: Option<&str>,
    inputs: &BTreeMap<String, Entity<InputState>>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_string();
    let kind = kind.map(str::to_string);
    let query = input_state(inputs, "query").clone();
    Button::new(format!("search-filter-{label}"))
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            let params = search_params(&query.read(cx).value(), kind.as_deref());
            this.emit_action(
                FeatureAction::Rpc {
                    method: "search".into(),
                    params,
                },
                cx,
            );
        }))
}

fn search(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let result = array(data, "results");
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_6()
        .child(page_header(
            t("search.title"),
            t("search.placeholder"),
            tokens,
        ))
        .child(Input::new(input_state(inputs, "query")).id("search-input"))
        .child(input_rpc_button(
            t("search.title"),
            "search",
            json!({"limit": 20}),
            "query",
            inputs,
            cx,
        ))
        .child(
            div()
                .flex()
                .gap_2()
                .child(search_filter_button(t("search.all"), None, inputs, cx))
                .child(search_filter_button(
                    t("search.messages"),
                    Some("message"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.groups"),
                    Some("chat"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.bots"),
                    Some("bot"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.artifacts"),
                    Some("artifact"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.routines"),
                    Some("routine"),
                    inputs,
                    cx,
                )),
        )
        .child(card(
            t("search.title"),
            search_result_rows(&result, tokens, cx),
            tokens,
        ))
}

fn search_result_rows(
    results: &[Value],
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(results.iter().enumerate().map(|(index, result)| {
            let kind = string(result, "kind", "");
            let title = string(result, "title", t("common.unknown"));
            let snippet = string(result, "snippet", t("common.unknown"));
            let target = search_target(result, &kind);
            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .py_2()
                .border_b_1()
                .border_color(tokens.border)
                .child(format!("{kind}   {title}\n{snippet}"));
            if let Some(target) = target {
                row = row.child(action_button_with_id(
                    format!("search-open-{index}"),
                    t("search.open"),
                    FeatureAction::Navigate(target),
                    cx,
                ));
            }
            row
        }))
}

fn search_target(result: &Value, kind: &str) -> Option<String> {
    let id = result.get("id").and_then(Value::as_str)?;
    match kind {
        "message" => result
            .get("chat_id")
            .and_then(Value::as_str)
            .map(|chat_id| format!("chat/{chat_id}"))
            .or_else(|| Some(format!("chat/{id}"))),
        "chat" => Some(format!("chat/{id}")),
        "bot" => Some(format!("bot/{id}")),
        "routine" => Some(format!("routine/{id}")),
        "artifact" => Some(format!("artifact/{id}")),
        _ => None,
    }
}

fn empty_page(page: &str, tokens: &Tokens) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_6()
        .child(page_header(page, t("common.unknown"), tokens))
        .child(t("common.loading"))
}

fn workbench_jobs(data: &Value) -> Vec<Value> {
    let mut jobs = array(data, "jobs");
    if jobs.is_empty() {
        for bot in array(data, "bots") {
            jobs.extend(array(&bot, "assignments"));
        }
        jobs.extend(array(data, "done_today"));
    }
    jobs
}

fn routine_schedule(data: &Value) -> String {
    let schedules = array(data, "schedules")
        .iter()
        .map(|schedule| {
            string(
                schedule,
                "label",
                &string(schedule, "cron", t("routine.default_schedule")),
            )
        })
        .collect::<Vec<_>>();
    if schedules.is_empty() {
        t("routine.default_schedule").to_string()
    } else {
        schedules.join("、")
    }
}

fn routine_trigger(run: &Value) -> &'static str {
    match run.get("trigger").and_then(Value::as_str) {
        Some("schedule") => t("routine.trigger_schedule"),
        Some("test") => t("routine.trigger_test"),
        _ => t("common.unknown"),
    }
}

fn dashboard_details(data: &Value) -> Vec<Value> {
    if let Some(rows) = data
        .get("breakdown")
        .and_then(|value| value.get("rows"))
        .and_then(Value::as_array)
    {
        rows.clone()
    } else {
        array(data, "details")
    }
}

#[allow(dead_code)]
fn usage_heatmap_values(data: &Value) -> Vec<Value> {
    let heatmap = data.get("heatmap").unwrap_or(data);
    let heatmap = heatmap
        .get("calendar")
        .or_else(|| heatmap.get("weekhour"))
        .unwrap_or(heatmap);
    if let Some(days) = heatmap.get("days").and_then(Value::as_array) {
        return days
            .iter()
            .map(|day| day.get("value").cloned().unwrap_or_default())
            .collect();
    }
    if let Some(matrix) = heatmap.get("matrix").and_then(Value::as_array) {
        return matrix
            .iter()
            .flat_map(|row| row.as_array().cloned().unwrap_or_default())
            .collect();
    }
    array(data, "heatmap")
}

fn member_ids(data: &Value) -> Vec<String> {
    data.get("member_bot_ids")
        .or_else(|| data.get("members"))
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .filter_map(|member| {
                    member.as_str().map(str::to_owned).or_else(|| {
                        member
                            .get("bot_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                })
                .take(6)
                .collect()
        })
        .unwrap_or_default()
}

fn dashboard_metric(data: &Value, key: &str, fallback: &str) -> String {
    if let Some(value) = data.get(key)
        && let Some(text) = json_value_text(value)
    {
        return text;
    }
    let usage = data.get("current").or_else(|| {
        data.get("summary")
            .and_then(|summary| summary.get("current"))
    });
    if let Some(usage) = usage {
        if key == "tokens" {
            let input = usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            return (input + output).to_string();
        }
        if let Some(value) = usage.get(key).and_then(json_value_text) {
            return value;
        }
    }
    fallback.to_string()
}

fn dashboard_cache_hit_rate(data: &Value) -> String {
    if let Some(value) = data.get("cache_hit_rate") {
        return value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}%", value.as_f64().unwrap_or(0.)));
    }
    let usage = data.get("current").or_else(|| {
        data.get("summary")
            .and_then(|summary| summary.get("current"))
    });
    let Some(usage) = usage else {
        return t("common.unknown").to_owned();
    };
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_f64)
        .unwrap_or(0.);
    let cached = usage
        .get("cache_read_tokens")
        .and_then(Value::as_f64)
        .unwrap_or(0.);
    let total = input + cached;
    if total == 0. {
        "0%".to_owned()
    } else {
        format!("{:.0}%", cached / total * 100.)
    }
}

fn dashboard_cache_with_delta(data: &Value) -> String {
    let current = dashboard_cache_rate(data.get("current"));
    let previous = dashboard_cache_rate(data.get("previous"));
    let Some(current) = current else {
        return dashboard_cache_hit_rate(data);
    };
    let Some(previous) = previous else {
        return format!("{current:.0}%");
    };
    format!(
        "{current:.0}%  {}{:.0}pt",
        if current >= previous { "▲" } else { "▼" },
        (current - previous).abs()
    )
}

fn dashboard_cache_rate(value: Option<&Value>) -> Option<f64> {
    let usage = value?;
    let input = usage.get("input_tokens")?.as_f64()?;
    let cached = usage.get("cache_read_tokens")?.as_f64()?;
    let total = input + cached;
    (total > 0.).then_some(cached / total * 100.)
}

fn dashboard_metric_with_delta(data: &Value, key: &str, fallback: &str) -> String {
    let value = dashboard_metric(data, key, fallback);
    let Some(current) = dashboard_numeric_metric(data.get("current"), key) else {
        return value;
    };
    let Some(previous) = dashboard_numeric_metric(data.get("previous"), key) else {
        return value;
    };
    if previous == 0. {
        return value;
    }
    let delta = (current - previous) / previous * 100.;
    format!(
        "{value}  {}{:.0}%",
        if delta >= 0. { "▲" } else { "▼" },
        delta.abs()
    )
}

fn dashboard_numeric_metric(value: Option<&Value>, key: &str) -> Option<f64> {
    let value = value?;
    if key == "tokens" {
        return Some(value.get("input_tokens")?.as_f64()? + value.get("output_tokens")?.as_f64()?);
    }
    value.get(key).and_then(Value::as_f64)
}

fn array(data: &Value, key: &str) -> Vec<Value> {
    data.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn json_value_text(value: &Value) -> Option<String> {
    if value.is_null() {
        None
    } else if let Some(text) = value.as_str() {
        Some(text.to_owned())
    } else if value.is_number() {
        Some(value.to_string())
    } else {
        None
    }
}

fn string(data: &Value, key: &str, fallback: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_actions_keep_rpc_payload() {
        let action = FeatureAction::Rpc {
            method: "skill.update".into(),
            params: json!({"name":"demo"}),
        };
        assert_eq!(
            action,
            FeatureAction::Rpc {
                method: "skill.update".into(),
                params: json!({"name":"demo"})
            }
        );
    }

    #[test]
    fn safe_data_helpers_handle_missing_fields() {
        let value = json!({"items":[{"name":"one"}]});
        assert_eq!(array(&value, "items").len(), 1);
        assert_eq!(string(&value["items"][0], "name", "fallback"), "one");
        assert_eq!(string(&value, "missing", "fallback"), "fallback");
    }

    #[test]
    fn update_data_merge_preserves_selected_editor_context() {
        let previous = json!({
            "selected": {"id":"routine-1", "name":"日报", "instructions":"keep"},
            "routines": [{"id":"routine-1"}],
            "current": {"running": 1}
        });
        let merged = merge_feature_data(
            &previous,
            json!({"routines":[{"id":"routine-2"}], "current":{"running":2}}),
        );
        assert_eq!(merged["selected"]["instructions"], "keep");
        assert_eq!(merged["current"]["running"], 2);

        let merged = merge_feature_data(
            &previous,
            json!({"selected":{"name":"updated"}, "routines":[]}),
        );
        assert_eq!(merged["selected"]["id"], "routine-1");
        assert_eq!(merged["selected"]["name"], "updated");
    }

    #[test]
    fn protocol_payload_shapes_use_server_field_names() {
        let bot_update = FeatureAction::Rpc {
            method: "bot.update".into(),
            params: json!({"bot_id":"bot_1","patch":{"name":"编程"}}),
        };
        assert_eq!(bot_update.method(), "bot.update");
        assert_eq!(bot_update.params()["patch"]["name"], "编程");

        let routine_test = FeatureAction::Rpc {
            method: "routine.test_run".into(),
            params: json!({"routine_id":"rtn_1"}),
        };
        assert_eq!(routine_test.method(), "routine.test_run");
        assert!(routine_test.params().get("id").is_none());
    }

    #[test]
    fn rpc_helpers_cover_required_protocol_fields() {
        let bot = bot_create_params("coder", "工程师", "规则", "mock/model", "2", "headless");
        assert_eq!(bot["name"], "coder");
        assert_eq!(bot["max_parallel"], 2);
        assert!(bot.get("tools").is_none());

        let project = project_create_params("上线", "完成发布", &["bot_1".into()]);
        assert_eq!(project["member_bot_ids"], json!(["bot_1"]));
        assert_eq!(project["goal"], "完成发布");

        let routine = routine_create_params(
            "bot_1",
            Some("project_1"),
            "日报",
            "整理日报",
            json!([{"cron":"0 9 * * *","label":"09:00"}]),
            Some("Asia/Shanghai"),
        );
        assert_eq!(routine["bot_id"], "bot_1");
        assert_eq!(routine["schedules"][0]["cron"], "0 9 * * *");

        assert_eq!(
            search_params("发布", Some("artifact"))["kinds"],
            json!(["artifact"])
        );
        let period = json!({"from":"2026-09-01T00:00:00Z","to":"2026-10-01T00:00:00Z"});
        let timeseries = usage_timeseries_params(&period, "bot", "tokens", true);
        assert_eq!(timeseries["from"], period["from"]);
        assert_eq!(timeseries["dimension"], "bot");
        assert_eq!(timeseries["split_io"], true);
        let breakdown = usage_breakdown_params(&period, "project");
        assert_eq!(breakdown["dimension"], "project");
        assert!(breakdown.get("drill").is_none());
        assert_eq!(usage_summary_params(&period)["from"], period["from"]);
        assert_eq!(
            usage_heatmap_params(&period, "weekhour", "requests")["mode"],
            "weekhour"
        );
        assert_eq!(
            assignment_stop_params("assignment_1"),
            json!({"assignment_id":"assignment_1"})
        );
        assert_eq!(
            bot_tools_update_params("bot_1", json!({"files":true})),
            json!({"bot_id":"bot_1","patch":{"tools":{"files":true}}})
        );
        assert_eq!(
            skill_enabled_params("skill.demo", false, Some("bot_1")),
            json!({"name":"skill.demo","enabled":false,"bot_id":"bot_1"})
        );
    }

    #[test]
    fn usage_data_does_not_fabricate_empty_series() {
        assert!(usage_heatmap_values(&json!({})).is_empty());
        assert_eq!(dashboard_metric(&json!({}), "tokens", "0"), "0");
        assert_eq!(
            member_ids(&json!({"member_bot_ids":["bot_1", "bot_2"]})),
            ["bot_1", "bot_2"]
        );
    }
}
