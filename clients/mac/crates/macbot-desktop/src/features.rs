//! Feature pages shared by the desktop shell.
//!
//! The shell owns navigation, connection state, and the surrounding panes.
//! This module owns the feature page presentation and emits intent-like
//! actions so the shell can turn a click into an RPC or navigation event.

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, Utc};
use gpui_kit::StatefulInteractiveElement;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::chart::LineChart;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, EventEmitter, Hsla, InteractiveElement,
    IntoElement, ParentElement, PathBuilder, Render, Styled, Window, canvas, div, point, px,
};
use gpui_kit::prelude::FluentBuilder;
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
            "previous",
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
        "group" | "new_group" | "new" => &["name"],
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
        "new" => &["announcement"],
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
        ("new", "name") => Some("new.input.placeholder"),
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
        ("new", "announcement") => Some("new.note.placeholder"),
        ("routine" | "routines", "instructions") => Some("routine.placeholder_instructions"),
        _ => None,
    }
}

fn labeled_field<L: Into<String>, T: IntoElement>(
    label: L,
    field: T,
    tokens: &Tokens,
) -> gpui_kit::Div {
    let label = label.into();
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .child(div().text_size(px(tokens.desktop_type[1][0])).child(label))
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
        "bot/detail" | "bot_detail" => bot_detail(data, &tokens, cx).into_any_element(),
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
        "new" => new_dialog(
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
        _ => empty_page(page, &tokens, cx).into_any_element(),
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
        .gap(px(tokens.space[2]))
        .pb(px(tokens.space[6]))
        .child(
            div()
                .text_size(px(tokens.desktop_type[7][0]))
                .font_weight(gpui_kit::gpui::FontWeight::BOLD)
                .child(title),
        )
        .child(
            div()
                .text_size(px(tokens.desktop_type[1][0]))
                .text_color(tokens.secondary)
                .child(subtitle),
        )
}

fn card(title: impl Into<String>, body: impl IntoElement, tokens: &Tokens) -> impl IntoElement {
    let title = title.into();
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[4]))
        .p(px(tokens.space[5]))
        .rounded(px(tokens.radius[8]))
        .bg(tokens.elevated)
        .border(px(tokens.desktop_layout.divider))
        .border_color(tokens.border_subtle)
        .child(
            div()
                .text_size(px(tokens.desktop_type[2][0]))
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
        .gap(px(tokens.space[2]))
        .flex_1()
        .p(px(tokens.space[5]))
        .min_w(px(130.))
        .rounded(px(tokens.radius[8]))
        .bg(tokens.elevated)
        .border(px(tokens.desktop_layout.divider))
        .border_color(tokens.border_subtle)
        .child(
            div()
                .text_size(px(tokens.desktop_type[1][0]))
                .text_color(tokens.secondary)
                .child(label),
        )
        .child(
            div()
                .text_size(px(tokens.desktop_type[6][0]))
                .font_weight(gpui_kit::gpui::FontWeight::BOLD)
                .child(value),
        )
}

fn rows(items: impl IntoIterator<Item = String>, tokens: &Tokens) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .children(items.into_iter().map(|item| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .py(px(tokens.space[4]))
                .border_b(px(tokens.desktop_layout.divider))
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
        .ghost()
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

pub fn project_create_params(name: &str, announcement: &str, member_bot_ids: &[String]) -> Value {
    json!({
        "name": name,
        "announcement": announcement,
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

fn new_create_button(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    selected_member_bot_ids: &[String],
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let mode = data
        .get("new_mode")
        .and_then(Value::as_str)
        .unwrap_or("group")
        .to_owned();
    let name = input_state(inputs, "name").clone();
    let announcement = textarea_state(textareas, "announcement").clone();
    let selected_member_bot_ids = selected_member_bot_ids.to_vec();
    let pending = data
        .get("new_pending")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let label = if pending {
        t("new.submitting")
    } else if mode == "bot" {
        t("new.create_bot")
    } else {
        t("new.create_group")
    };
    Button::new("new-create")
        .label(label)
        .primary()
        .disabled(pending)
        .on_click(cx.listener(move |this, _, _, cx| {
            let name_value = name.read(cx).value().trim().to_owned();
            if name_value.is_empty() {
                this.emit_action(FeatureAction::Toast(t("new.name_required").to_owned()), cx);
                return;
            }
            if mode == "group" {
                if selected_member_bot_ids.is_empty() || selected_member_bot_ids.len() > 6 {
                    this.emit_action(
                        FeatureAction::Toast(t("new.members_required").to_owned()),
                        cx,
                    );
                    return;
                }
                this.emit_action(
                    FeatureAction::Rpc {
                        method: "project.create".into(),
                        params: project_create_params(
                            &name_value,
                            &announcement.read(cx).value(),
                            &selected_member_bot_ids,
                        ),
                    },
                    cx,
                );
            } else {
                this.emit_action(
                    FeatureAction::Rpc {
                        method: "bot.create".into(),
                        params: bot_create_params(&name_value, &name_value, "", "", "", ""),
                    },
                    cx,
                );
            }
        }))
}

fn new_dialog(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    textareas: &BTreeMap<String, Entity<TextareaState>>,
    selected_member_bot_ids: &[String],
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let name = input_state(inputs, "name").clone();
    let mode = data
        .get("new_mode")
        .and_then(Value::as_str)
        .unwrap_or("group");
    let name_value = name.read(cx).value().trim().to_owned();
    let group_label = t("new.group").replace("{name}", &name_value);
    let bot_label = t("new.bot").replace("{name}", &name_value);
    let available_bots = array(data, "bots")
        .iter()
        .filter(|bot| {
            !bot.get("hidden").and_then(Value::as_bool).unwrap_or(false)
                && !bot.get("is_main").and_then(Value::as_bool).unwrap_or(false)
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected_count = selected_member_bot_ids.len();
    let member_copy = t("new.members").replace("{count}", &selected_count.to_string());
    let member_rows = div()
        .flex()
        .flex_col()
        .gap_1()
        .children(available_bots.iter().map(|bot| {
            let bot_id = string(bot, "id", "").to_owned();
            let fallback_name = string(bot, "name", t("common.unknown")).to_owned();
            let bot_name = string(bot, "label", &fallback_name);
            let role = string(bot, "description", "");
            let checked = selected_member_bot_ids.iter().any(|id| id == &bot_id);
            let bot_id_for_event = bot_id.clone();
            let label = if role.trim().is_empty() {
                bot_name.to_owned()
            } else {
                format!("{bot_name} · {role}")
            };
            Checkbox::new(format!("new-member-{bot_id}"))
                .label(label)
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
        }));
    let error = data
        .get("new_error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let error_row = if error.is_empty() {
        div().into_any_element()
    } else {
        div()
            .text_color(tokens.danger)
            .text_sm()
            .child(error.to_owned())
            .into_any_element()
    };
    let pending = data
        .get("new_pending")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let footer_copy = if mode == "group" { t("new.footer") } else { "" };
    let group_option = if mode == "group" {
        Button::new("new-mode-group")
            .label(format!("{}  ·  {}", group_label, t("new.group.hint")))
            .primary()
            .disabled(pending)
            .on_click(cx.listener(|this, _, _, cx| {
                this.data["new_mode"] = json!("group");
                this.data["new_error"] = Value::Null;
                cx.notify();
            }))
    } else {
        Button::new("new-mode-group")
            .label(format!("{}  ·  {}", group_label, t("new.group.hint")))
            .outline()
            .disabled(pending)
            .on_click(cx.listener(|this, _, _, cx| {
                this.data["new_mode"] = json!("group");
                this.data["new_error"] = Value::Null;
                cx.notify();
            }))
    };
    let bot_option = if mode == "bot" {
        Button::new("new-mode-bot")
            .label(bot_label)
            .primary()
            .disabled(pending)
            .on_click(cx.listener(|this, _, _, cx| {
                this.data["new_mode"] = json!("bot");
                this.data["new_error"] = Value::Null;
                cx.notify();
            }))
    } else {
        Button::new("new-mode-bot")
            .label(bot_label)
            .outline()
            .disabled(pending)
            .on_click(cx.listener(|this, _, _, cx| {
                this.data["new_mode"] = json!("bot");
                this.data["new_error"] = Value::Null;
                cx.notify();
            }))
    };
    let group_fields = if mode == "group" {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child(member_copy))
            .child(member_rows)
            .child(labeled_field(
                t("new.note"),
                Textarea::new(textarea_state(textareas, "announcement")).h(px(52.)),
                tokens,
            ))
    } else {
        div()
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(tokens.window)
        .child(
            div()
                .w(px(560.))
                .flex()
                .flex_col()
                .gap_3()
                .rounded_lg()
                .border_1()
                .border_color(tokens.border)
                .bg(tokens.sidebar)
                .p_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                                .child(t("new.title")),
                        )
                        .child(
                            Button::new("new-routine")
                                .label(t("new.routine"))
                                .outline()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.emit_action(
                                        FeatureAction::Navigate("routines".into()),
                                        cx,
                                    );
                                })),
                        ),
                )
                .child(
                    div()
                        .h(px(52.))
                        .flex()
                        .items_center()
                        .child(Input::new(&name).id("new-name-input")),
                )
                .child(group_option)
                .child(bot_option)
                .child(group_fields)
                .child(error_row)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(tokens.secondary)
                                .child(footer_copy),
                        )
                        .child(
                            Button::new("new-cancel")
                                .label(t("new.cancel"))
                                .outline()
                                .disabled(pending)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let target = this
                                        .data
                                        .get("cancel_target")
                                        .and_then(Value::as_str)
                                        .unwrap_or("dashboard")
                                        .to_owned();
                                    this.emit_action(FeatureAction::Navigate(target), cx);
                                })),
                        )
                        .child(new_create_button(
                            data,
                            inputs,
                            textareas,
                            selected_member_bot_ids,
                            cx,
                        )),
                ),
        )
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
                    FeatureAction::Toast(t("routines.schedule_required").to_string()),
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
        "granularity": data.get("granularity").and_then(Value::as_str).unwrap_or("auto"),
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
            this.data["range"] = json!("custom");
            let params = build(&from_value, &to_value);
            let refresh_all = method == "usage.summary" || method == "usage.heatmap";
            this.emit_action(
                FeatureAction::Rpc {
                    method: method.clone(),
                    params,
                },
                cx,
            );
            if refresh_all {
                let snapshot = this.data.clone();
                if method != "usage.summary" {
                    this.emit_action(
                        FeatureAction::Rpc {
                            method: "usage.summary".into(),
                            params: usage_summary_params(&snapshot),
                        },
                        cx,
                    );
                }
                if method != "usage.heatmap" {
                    this.emit_action(
                        FeatureAction::Rpc {
                            method: "usage.heatmap".into(),
                            params: usage_heatmap_params(&snapshot, "calendar", "tokens"),
                        },
                        cx,
                    );
                }
                this.emit_action(
                    FeatureAction::Rpc {
                        method: "usage.timeseries".into(),
                        params: usage_timeseries_params(
                            &snapshot,
                            dashboard_dimension(&snapshot),
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
                        params: usage_breakdown_params(&snapshot, dashboard_dimension(&snapshot)),
                    },
                    cx,
                );
            }
        }))
}

fn dashboard_range_period(days: u32) -> (String, String) {
    let now = Local::now();
    let first_day = now.date_naive() - ChronoDuration::days(i64::from(days.saturating_sub(1)));
    let first_day_start = first_day
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time")
        .and_local_timezone(Local)
        .earliest()
        .unwrap_or(now);
    (
        first_day_start.with_timezone(&Utc).to_rfc3339(),
        now.with_timezone(&Utc).to_rfc3339(),
    )
}

fn dashboard_range_button(
    label: &str,
    range: &str,
    days: u32,
    inputs: &BTreeMap<String, Entity<InputState>>,
    selected_range: &str,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let label = label.to_owned();
    let range = range.to_owned();
    let from = input_state(inputs, "from").clone();
    let to = input_state(inputs, "to").clone();
    let selected = range == selected_range;
    Button::new(format!("dashboard-range-{range}"))
        .label(label)
        .when(selected, |button| button.primary())
        .when(!selected, |button| button.ghost())
        .on_click(cx.listener(move |this, _, window, cx| {
            let (from_value, to_value) = dashboard_range_period(days);
            from.update(cx, |state, cx| {
                state.set_value(from_value.clone(), window, cx);
            });
            to.update(cx, |state, cx| {
                state.set_value(to_value.clone(), window, cx);
            });
            this.data["from"] = json!(from_value);
            this.data["to"] = json!(to_value);
            this.data["range"] = json!(range.clone());
            let snapshot = this.data.clone();
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.summary".into(),
                    params: usage_summary_params(&snapshot),
                },
                cx,
            );
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.heatmap".into(),
                    params: usage_heatmap_params(&snapshot, "calendar", "tokens"),
                },
                cx,
            );
            this.emit_action(
                FeatureAction::Rpc {
                    method: "usage.timeseries".into(),
                    params: usage_timeseries_params(
                        &snapshot,
                        dashboard_dimension(&snapshot),
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
                    params: usage_breakdown_params(&snapshot, dashboard_dimension(&snapshot)),
                },
                cx,
            );
            cx.notify();
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
    let main_bot_exists = array(data, "all_bots")
        .iter()
        .any(|bot| bot.get("is_main").and_then(Value::as_bool).unwrap_or(false));
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
            .gap(px(tokens.space[4]))
            .children(groups.iter().map(|(key, jobs)| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(tokens.space[2]))
                    .child(
                        div()
                            .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                            .child(format!("{} · {}", workbench_group_label(&group_mode), key)),
                    )
                    .children(jobs.iter().enumerate().map(|(index, job)| {
                        workbench_job_row(job, data, tokens, cx, &format!("{key}-{index}"))
                    }))
            }));
    let active_groups = if groups.is_empty() {
        div()
            .p(px(tokens.space[6]))
            .text_color(tokens.secondary)
            .child(t("workbench.empty.running"))
            .into_any_element()
    } else {
        active_groups.into_any_element()
    };
    let done = workbench_done_jobs(data);
    let body = div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[5]))
        .child(
            div()
                .flex()
                .gap(px(tokens.space[4]))
                .child(workbench_mode_button(
                    t("workbench.view.by_bot"),
                    "bot",
                    &group_mode,
                    cx,
                ))
                .child(workbench_mode_button(
                    t("workbench.view.by_group"),
                    "project",
                    &group_mode,
                    cx,
                ))
                .child(workbench_mode_button(
                    t("workbench.view.by_status"),
                    "status",
                    &group_mode,
                    cx,
                )),
        )
        .child(
            div().flex().flex_wrap().gap(px(tokens.space[5])).children([
                div()
                    .px(px(tokens.space[4]))
                    .py(px(tokens.space[3]))
                    .text_size(px(tokens.desktop_type[2][0]))
                    .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                    .child(
                        t("workbench.concurrency")
                            .replace("{running}", &running.to_string())
                            .replace("{max}", &limit.to_string()),
                    ),
                stat(t("workbench.waiting_review"), &reviews.to_string(), tokens),
                stat(
                    t("workbench.waiting_approval"),
                    &approvals.to_string(),
                    tokens,
                ),
                stat(
                    t("workbench.waiting_questions"),
                    &questions.to_string(),
                    tokens,
                ),
                stat(t("block.takeover.title"), &takeovers.to_string(), tokens),
            ]),
        )
        .child(if main_bot_exists {
            div()
                .text_size(px(tokens.desktop_type[1][0]))
                .text_color(tokens.secondary)
                .child(t("workbench.main_not_counted"))
                .into_any_element()
        } else {
            div().into_any_element()
        })
        .child(workbench_waiting(&waiting, data, tokens, cx))
        .child(card(t("workbench.title"), active_groups, tokens))
        .child(card(
            &t("workbench.done_today").replace("{count}", &done.len().to_string()),
            if done.is_empty() {
                div()
                    .p(px(tokens.space[5]))
                    .text_color(tokens.secondary)
                    .child(t("workbench.empty.done_today"))
                    .into_any_element()
            } else {
                workbench_done_rows(&done, data, tokens, cx).into_any_element()
            },
            tokens,
        ));
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(
            t("workbench.title"),
            &t("workbench.running")
                .replace("{running}", &running.to_string())
                .replace("{limit}", &limit.to_string()),
            tokens,
        ))
        .child(body)
}

fn workbench_mode_button(
    label: &str,
    mode: &str,
    selected: &str,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let target = format!("workbench?by={mode}");
    Button::new(format!("workbench-view-{mode}"))
        .label(label)
        .when(selected == mode, |button| button.primary())
        .when(selected != mode, |button| button.ghost())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.emit_action(FeatureAction::Navigate(target.clone()), cx)
        }))
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
            t("workbench.unassigned"),
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
        "project" => t("workbench.view.by_group"),
        "status" => t("workbench.view.by_status"),
        _ => t("workbench.view.by_bot"),
    }
}

fn workbench_status_label(status: Option<&str>) -> String {
    let raw = status.unwrap_or_default();
    match raw {
        "queued" => t("status.queued").to_owned(),
        "working" | "running" => t("status.working").to_owned(),
        "waiting" => t("status.waiting_user").to_owned(),
        "paused" => t("workbench.status.paused").to_owned(),
        "failed" => t("status.failed").to_owned(),
        "done" => t("status.done").to_owned(),
        "cancelled" => t("status.cancelled").to_owned(),
        "" => t("common.unknown").to_owned(),
        _ => raw.to_owned(),
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
        t("workbench.unassigned"),
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
    let mut actions = div()
        .flex()
        .gap(px(tokens.space[2]))
        .child(action_button_with_id(
            format!("workbench-{row_key}-detail"),
            t("common.details"),
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
            t("common.stop"),
            stop.clone(),
            cx,
        ));
        if status_raw == "queued" {
            actions = actions.child(action_button_with_id(
                format!("workbench-{row_key}-cancel"),
                t("common.cancel"),
                stop,
                cx,
            ));
        }
        if status_raw == "failed" {
            actions = actions.child(action_button_with_id(
                format!("workbench-{row_key}-retry"),
                t("common.retry"),
                assignment_retry_action(job),
                cx,
            ));
        }
    }
    let project_id = job
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let chat_id = if project_id.is_empty() {
        string(job, "origin_chat_id", "")
    } else {
        array(data, "projects")
            .iter()
            .find(|project| string(project, "id", "") == project_id)
            .map(|project| string(project, "chat_id", ""))
            .filter(|chat_id| !chat_id.is_empty())
            .unwrap_or_else(|| string(job, "origin_chat_id", ""))
    };
    if !chat_id.is_empty() {
        actions = actions.child(action_button_with_id(
            format!("workbench-{row_key}-chat"),
            if project_id.is_empty() {
                t("workbench.open_dm")
            } else {
                t("follow.jump_group")
            },
            FeatureAction::Navigate(format!("chat/{chat_id}")),
            cx,
        ));
    }
    let bot_id = string(job, "bot_id", "");
    let bot_exists = array(data, "all_bots")
        .iter()
        .any(|candidate| string(candidate, "id", "") == bot_id && !bot_id.is_empty());
    let origin_chat_id = string(job, "origin_chat_id", "");
    if status_raw == "working" && bot_exists && !origin_chat_id.is_empty() {
        let mention_target = format!("chat/{origin_chat_id}?mention_bot={bot_id}");
        actions = actions.child(action_button_with_id(
            format!("workbench-{row_key}-mention"),
            t("trace.at_it"),
            FeatureAction::Navigate(mention_target),
            cx,
        ));
    }
    if status_raw == "blocked"
        && let Some(bot_id) = job.get("bot_id").and_then(Value::as_str)
        && !bot_id.is_empty()
    {
        actions = actions.child(
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[1]))
                .child(action_button_with_id(
                    format!("workbench-{row_key}-switch-model"),
                    t("block.blocked.switch_model"),
                    FeatureAction::Navigate(format!("bot_settings?bot_id={bot_id}")),
                    cx,
                ))
                .child(
                    div()
                        .text_size(px(tokens.desktop_type[1][0]))
                        .text_color(tokens.secondary)
                        .child(t("block.blocked.switch_model_hint")),
                ),
        );
    }
    div()
        .flex()
        .items_center()
        .justify_between()
        .py(px(tokens.space[4]))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[2]))
                .child(format!("{bot} · {name}"))
                .child(
                    div()
                        .text_size(px(tokens.desktop_type[1][0]))
                        .text_color(tokens.secondary)
                        .child(format!(
                            "{project} · {status} · {elapsed} · {usage} tok · {model} · {} {}",
                            t("trace.filter.subagents"),
                            subagents
                        )),
                ),
        )
        .child(actions)
}

fn workbench_done_label(job: &Value, data: &Value) -> String {
    let project = workbench_entity_label(
        data,
        "projects",
        job.get("project_id").and_then(Value::as_str),
        t("workbench.unassigned"),
    );
    format!(
        "✓ {} · {}",
        project,
        string(job, "title", t("common.unnamed_task"))
    )
}

fn workbench_done_rows(
    jobs: &[Value],
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .children(jobs.iter().map(|job| {
            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .py(px(tokens.space[4]))
                .border_b(px(tokens.desktop_layout.divider))
                .border_color(tokens.border)
                .child(workbench_done_label(job, data));
            if job.get("status").and_then(Value::as_str) == Some("failed")
                && let Some(assignment_id) = job.get("id").and_then(Value::as_str)
                && !assignment_id.is_empty()
            {
                row = row.child(action_button_with_id(
                    format!("workbench-done-{assignment_id}-retry"),
                    t("common.retry"),
                    assignment_retry_action(job),
                    cx,
                ));
            }
            row
        }))
}

fn assignment_retry_action(job: &Value) -> FeatureAction {
    let assignment_id = string(job, "id", "");
    let retry_version = job
        .get("finished_at")
        .and_then(Value::as_str)
        .or_else(|| job.get("started_at").and_then(Value::as_str))
        .or_else(|| job.get("created_at").and_then(Value::as_str))
        .unwrap_or("failed");
    FeatureAction::Rpc {
        method: "assignment.retry".into(),
        params: json!({
            "assignment_id": assignment_id,
            "client_request_id": format!("assignment-retry:{assignment_id}:{retry_version}"),
        }),
    }
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
        format!("{minutes} {}", t("common.minutes_short"))
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
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let mut container = div().flex().flex_col().gap(px(tokens.space[4]));
    for (item_index, item) in waiting.iter().enumerate() {
        let kind = string(item, "kind", "");
        let mut row = div()
            .flex()
            .items_center()
            .justify_between()
            .py(px(tokens.space[4]))
            .border_b(px(tokens.desktop_layout.divider))
            .border_color(tokens.border)
            .child(waiting_title(item, &kind));
        match kind.as_str() {
            "review" => {
                if let Some(project_id) = item.get("project_id").and_then(Value::as_str) {
                    row = row
                        .child(action_button_with_id(
                            format!("workbench-waiting-{item_index}-confirm"),
                            t("block.review_card.confirm"),
                            FeatureAction::Rpc {
                                method: "project.confirm_done".into(),
                                params: json!({"project_id": project_id}),
                            },
                            cx,
                        ))
                        .child(action_button_with_id(
                            format!("workbench-waiting-{item_index}-changes"),
                            t("block.review_card.request_changes"),
                            FeatureAction::Navigate(format!("project/{project_id}/changes")),
                            cx,
                        ));
                }
            }
            "approval" => {
                if let Some(approval) = item.get("approval") {
                    if let Some(approval_id) = approval.get("id").and_then(Value::as_str) {
                        let mut decisions = vec![
                            (t("block.approval.allow_once"), "allow_once"),
                            (t("block.approval.deny"), "deny"),
                        ];
                        if approval
                            .get("always_allow_supported")
                            .and_then(Value::as_bool)
                            .unwrap_or(true)
                        {
                            decisions.insert(1, (t("block.approval.always_allow"), "always_allow"));
                        }
                        for (label, decision) in decisions {
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
                        let options = question
                            .get("options")
                            .and_then(Value::as_array)
                            .map(Vec::as_slice)
                            .unwrap_or_default();
                        let multi = question
                            .get("multi")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let pending = question
                            .get("state")
                            .and_then(Value::as_str)
                            .map(|state| state == "pending")
                            .unwrap_or(true);
                        let mut selected = question_option_selection(data, question_id);
                        selected.retain(|index| (*index as usize) < options.len());
                        if multi {
                            for (index, option) in options.iter().enumerate() {
                                let index = index as u32;
                                let checked = selected.contains(&index);
                                let question_id = question_id.to_owned();
                                let label = format!(
                                    "{}{}",
                                    if checked { "☑ " } else { "□ " },
                                    option.as_str().unwrap_or_default()
                                );
                                row = row.child(
                                    Button::new(format!(
                                        "workbench-waiting-{item_index}-question-{index}"
                                    ))
                                    .label(label)
                                    .ghost()
                                    .disabled(!pending)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            let mut selected =
                                                question_option_selection(&this.data, &question_id);
                                            if selected.contains(&index) {
                                                selected.retain(|value| *value != index);
                                            } else {
                                                selected.push(index);
                                                selected.sort_unstable();
                                            }
                                            set_question_option_selection(
                                                &mut this.data,
                                                &question_id,
                                                selected,
                                            );
                                            cx.notify();
                                        },
                                    )),
                                );
                            }
                            if !options.is_empty() {
                                let selected_for_submit = selected.clone();
                                let question_id_for_submit = question_id.to_owned();
                                let request_id = question_request_id(question_id, &selected);
                                row = row.child(
                                    Button::new(format!(
                                        "workbench-waiting-{item_index}-question-submit"
                                    ))
                                    .label(t("common.submit"))
                                    .primary()
                                    .disabled(!pending || selected.is_empty())
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.emit_action(
                                                FeatureAction::Rpc {
                                                    method: "question.answer".into(),
                                                    params: json!({
                                                        "question_id": question_id_for_submit,
                                                        "option_indexes": selected_for_submit,
                                                        "client_request_id": format!(
                                                            "question-answer:{}:{}",
                                                            question_id_for_submit,
                                                            indexes
                                                        )
                                                    }),
                                                },
                                                cx,
                                            );
                                        },
                                    )),
                                );
                                let question_id_for_cancel = question_id.to_owned();
                                row = row.child(
                                    Button::new(format!(
                                        "workbench-waiting-{item_index}-question-cancel"
                                    ))
                                    .label(t("common.cancel"))
                                    .ghost()
                                    .disabled(!pending || selected.is_empty())
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            set_question_option_selection(
                                                &mut this.data,
                                                &question_id_for_cancel,
                                                Vec::new(),
                                            );
                                            cx.notify();
                                        },
                                    )),
                                );
                            }
                        } else if pending {
                            for (index, option) in options.iter().enumerate() {
                                let request_id = question_request_id(question_id, &[index as u32]);
                                row = row.child(action_button_with_id(
                                    format!("workbench-waiting-{item_index}-question-{index}"),
                                    option.as_str().unwrap_or_default(),
                                    FeatureAction::Rpc {
                                        method: "question.answer".into(),
                                        params: json!({
                                            "question_id": question_id,
                                            "option_index": index,
                                            "client_request_id": request_id
                                        }),
                                    },
                                    cx,
                                ));
                            }
                        }
                        if pending
                            && question
                                .get("allow_free_text")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        {
                            row = row.child(action_button_with_id(
                                format!("workbench-waiting-{item_index}-answer"),
                                t("block.question.other"),
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
                    t("block.takeover.title"),
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

fn question_option_selection(data: &Value, question_id: &str) -> Vec<u32> {
    data.get("workbench_question_selections")
        .and_then(Value::as_object)
        .and_then(|selections| selections.get(question_id))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|value| u32::try_from(value).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn question_request_id(question_id: &str, indexes: &[u32]) -> String {
    // The same pending answer keeps the same valid UUID across retries; changing
    // the selected options produces a different idempotency key.
    let material = format!(
        "{}:{}",
        question_id,
        indexes
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut first = 0xcbf29ce484222325u64;
    let mut second = 0x84222325cbf29ce4u64;
    for byte in material.bytes() {
        first = (first ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        second = (second ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&first.to_be_bytes());
    bytes[8..].copy_from_slice(&second.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn set_question_option_selection(data: &mut Value, question_id: &str, selected: Vec<u32>) {
    if !data.is_object() {
        *data = json!({});
    }
    if let Some(object) = data.as_object_mut() {
        let selections = object
            .entry("workbench_question_selections")
            .or_insert_with(|| json!({}));
        if !selections.is_object() {
            *selections = json!({});
        }
        if let Some(selections) = selections.as_object_mut() {
            selections.insert(question_id.to_owned(), json!(selected));
        }
    }
}

fn waiting_title(item: &Value, kind: &str) -> String {
    match kind {
        "approval" => item
            .get("approval")
            .map(|approval| {
                format!(
                    "{} · {}",
                    t("workbench.waiting_approval"),
                    string(approval, "summary", t("common.unknown"))
                )
            })
            .unwrap_or_else(|| t("workbench.waiting_approval").to_string()),
        "question" => item
            .get("question")
            .map(|question| {
                format!(
                    "{} · {}",
                    t("workbench.waiting_question"),
                    string(question, "text", t("common.unknown"))
                )
            })
            .unwrap_or_else(|| t("workbench.waiting_question").to_string()),
        "takeover" => format!(
            "{} · {}",
            t("block.takeover.title"),
            string(item, "reason", t("common.unknown"))
        ),
        _ => t("workbench.waiting_review").to_string(),
    }
}

fn dashboard(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let details = dashboard_details(data);
    let heat = heatmap(data, inputs, tokens, cx);
    let trend = trend_chart(data, tokens, cx);
    let selected_range = data.get("range").and_then(Value::as_str).unwrap_or("30d");
    let period_label = match selected_range {
        "today" => t("dashboard.range.today"),
        "7d" => t("dashboard.range.7d"),
        "30d" => t("dashboard.range.30d"),
        "90d" => t("dashboard.range.90d"),
        _ => "",
    };
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(t("dashboard.title"), period_label, tokens))
        .child(
            div()
                .flex()
                .gap(px(tokens.space[4]))
                .child(dashboard_range_button(
                    t("dashboard.range.today"),
                    "today",
                    1,
                    inputs,
                    selected_range,
                    cx,
                ))
                .child(dashboard_range_button(
                    t("dashboard.range.7d"),
                    "7d",
                    7,
                    inputs,
                    selected_range,
                    cx,
                ))
                .child(dashboard_range_button(
                    t("dashboard.range.30d"),
                    "30d",
                    30,
                    inputs,
                    selected_range,
                    cx,
                ))
                .child(dashboard_range_button(
                    t("dashboard.range.90d"),
                    "90d",
                    90,
                    inputs,
                    selected_range,
                    cx,
                )),
        )
        .child(
            div()
                .flex()
                .gap(px(tokens.space[4]))
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
        .child(div().flex().gap(px(tokens.space[5])).children([
            stat(
                t("dashboard.kpi.tokens"),
                &dashboard_metric_with_delta(data, "tokens", "0"),
                tokens,
            ),
            stat(
                t("dashboard.kpi.requests"),
                &dashboard_metric_with_delta(data, "requests", "0"),
                tokens,
            ),
            stat(
                t("dashboard.kpi.cost"),
                &dashboard_metric_with_delta(data, "cost", t("common.unknown")),
                tokens,
            ),
            stat(
                t("dashboard.kpi.tasks_done"),
                &dashboard_metric_with_delta(data, "tasks_done", "0"),
                tokens,
            ),
            stat(
                t("dashboard.kpi.cache_hit"),
                &dashboard_cache_with_delta(data),
                tokens,
            ),
        ]))
        .child(card(
            t("dashboard.heatmap"),
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[3]))
                .child(heat)
                .child(dashboard_heatmap_legend(tokens)),
            tokens,
        ))
        .child(card(dashboard_trend_title(data), trend, tokens))
        .child(card(
            t("dashboard.metric"),
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[4]))
                .child(
                    div().flex().gap(px(tokens.space[4])).children([
                        dashboard_dimension_button(t("dashboard.dimension.model"), "model", cx)
                            .into_any_element(),
                        dashboard_dimension_button(t("dashboard.dimension.bot"), "bot", cx)
                            .into_any_element(),
                        dashboard_dimension_button(t("dashboard.dimension.project"), "project", cx)
                            .into_any_element(),
                    ]),
                )
                .child(
                    div().flex().gap(px(tokens.space[4])).children([
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
                            t("dashboard.kpi.cost"),
                            "cost",
                            "calendar",
                            cx,
                        )
                        .into_any_element(),
                        dashboard_heatmap_metric_button(
                            t("dashboard.kpi.requests"),
                            "requests",
                            "calendar",
                            cx,
                        )
                        .into_any_element(),
                        dashboard_heatmap_metric_button(
                            t("dashboard.heatmap.week_hour"),
                            "tokens",
                            "weekhour",
                            cx,
                        )
                        .into_any_element(),
                    ]),
                )
                .child(
                    div().flex().gap(px(tokens.space[4])).children([
                        dashboard_metric_button(t("dashboard.kpi.tokens"), "tokens", false, cx)
                            .into_any_element(),
                        dashboard_metric_button(t("dashboard.kpi.cost"), "cost", false, cx)
                            .into_any_element(),
                        dashboard_metric_button(t("dashboard.kpi.requests"), "requests", false, cx)
                            .into_any_element(),
                        dashboard_metric_button(t("dashboard.split_io"), "tokens", true, cx)
                            .into_any_element(),
                    ]),
                )
                .child(action_button(
                    t("dashboard.export_csv"),
                    FeatureAction::Navigate("usage/export.csv".into()),
                    cx,
                ))
                .child(dashboard_detail_rows(&details, data, tokens, cx)),
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
        .gap(px(tokens.space[2]))
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
            let sparkline_values = item
                .get("sparkline")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let sparkline = if sparkline_values.len() >= 2 {
                let points = sparkline_values
                    .iter()
                    .enumerate()
                    .map(|(point, value)| (point, value.as_f64().unwrap_or(0.) as f32))
                    .collect::<Vec<_>>();
                div()
                    .w(px(96.))
                    .h(px(28.))
                    .child(
                        LineChart::new(points)
                            .id(format!("dashboard-sparkline-{dimension}-{index}"))
                            .x(|(point, _)| point.to_string())
                            .y(|(_, value)| *value)
                            .stroke(tokens.accent)
                            .x_axis(false)
                            .y_axis(false)
                            .grid(false)
                            .interactive(false),
                    )
                    .into_any_element()
            } else {
                div().w(px(96.)).h(px(28.)).into_any_element()
            };
            let phase_text = item
                .get("phases")
                .and_then(Value::as_object)
                .map(|phases| {
                    phases
                        .iter()
                        .filter_map(|(phase, value)| {
                            let value = value.as_f64()?;
                            (value > 0.)
                                .then(|| format!("{} {:.0}", dashboard_phase_label(phase), value))
                        })
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
                .unwrap_or_default();
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
                    t("dashboard.drill.phases"),
                    "usage.breakdown",
                    usage_breakdown_params(&drill_data, &dimension),
                    cx,
                )
            });
            let cost_label = cost.map(|cost| format!(" · ¥{cost}")).unwrap_or_default();
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(tokens.space[4]))
                .justify_between()
                .py(px(tokens.space[4]))
                .border_b(px(tokens.desktop_layout.divider))
                .border_color(tokens.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(tokens.space[2]))
                        .child(format!(
                            "{label}   {} {} · {} {} · {} {} · {} {}{cost_label}",
                            t("dashboard.input"),
                            input,
                            t("dashboard.output"),
                            output,
                            t("dashboard.cache_read"),
                            cached,
                            t("dashboard.kpi.requests"),
                            requests,
                        ))
                        .child(
                            div()
                                .text_size(px(tokens.desktop_type[0][0]))
                                .text_color(tokens.secondary)
                                .child(phase_text),
                        ),
                )
                .child(sparkline);
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
                .text_size(px(tokens.desktop_type[1][0]))
                .text_color(tokens.secondary)
                .child(t("dashboard.empty"));
        }
        let mut aligned_days = Vec::with_capacity(days.len() + 6);
        let offset = days
            .first()
            .and_then(|day| day.get("date"))
            .and_then(Value::as_str)
            .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .map(|date| date.weekday().num_days_from_monday() as usize)
            .unwrap_or(0);
        aligned_days.extend(std::iter::repeat_with(|| Value::Null).take(offset));
        aligned_days.extend(days.iter().cloned());
        let weeks = aligned_days
            .chunks(7)
            .map(|week| week.to_vec())
            .collect::<Vec<_>>();
        let mut previous_month = String::new();
        let month_labels = weeks
            .iter()
            .map(|week| {
                let month = week
                    .iter()
                    .find_map(|day| day.get("date"))
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
            .gap(px(tokens.space[1]))
            .child(div().w(px(20.)).h(px(16.)))
            .children(month_labels.iter().map(|month| {
                div()
                    .w(px(13.))
                    .text_size(px(tokens.desktop_type[0][0]))
                    .text_color(tokens.secondary)
                    .child(month.clone())
            }));
        let weekday_column =
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[1]))
                .children(weekday_labels.iter().map(|label| {
                    div()
                        .w(px(20.))
                        .h(px(16.))
                        .text_size(px(tokens.desktop_type[0][0]))
                        .text_color(tokens.secondary)
                        .child(*label)
                }));
        let weeks_grid = div()
            .flex()
            .gap(px(tokens.space[1]))
            .child(weekday_column)
            .children(weeks.iter().map(|week| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(tokens.space[1]))
                    .children(week.iter().map(|day| {
                        if day.is_null() {
                            return div().size(px(11.)).into_any_element();
                        }
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
                            .map(|id| dashboard_bot_label(data, id))
                            .unwrap_or_else(|| t("common.unknown").to_owned());
                        let color = heatmap_color(value, &thresholds, tokens);
                        let selected = date == selected_day;
                        div()
                            .id(format!("heatmap-day-{date}"))
                            .size(px(if selected { 13. } else { 11. }))
                            .rounded(px(tokens.radius[1]))
                            .bg(color)
                            .border(px(tokens.desktop_layout.divider))
                            .border_color(if selected { tokens.accent } else { color })
                            .tooltip({
                                let tooltip = format!(
                                    "{date} · {} {} · {} {} · {} {top_bot}",
                                    t("dashboard.kpi.tokens"),
                                    value,
                                    t("dashboard.kpi.cost"),
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
                            .into_any_element()
                    }))
            }));
        return div()
            .flex()
            .flex_col()
            .gap(px(tokens.space[2]))
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
            .text_size(px(tokens.desktop_type[1][0]))
            .text_color(tokens.secondary)
            .child(t("dashboard.empty"));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .children(matrix.iter().map(|row| {
            div().flex().gap(px(tokens.space[2])).children(
                row.as_array().into_iter().flatten().map(|value| {
                    let color = heatmap_color(value.as_f64().unwrap_or(0.), &thresholds, tokens);
                    div().size(px(14.)).rounded(px(tokens.radius[1])).bg(color)
                }),
            )
        }))
}

fn dashboard_heatmap_legend(tokens: &Tokens) -> impl IntoElement {
    let levels = [
        tokens.heat[0],
        tokens.heat[1],
        tokens.heat[2],
        tokens.heat[3],
        tokens.heat[4],
    ];
    div()
        .flex()
        .items_center()
        .gap(px(tokens.space[2]))
        .text_size(px(tokens.desktop_type[0][0]))
        .text_color(tokens.secondary)
        .child(t("dashboard.heatmap.less"))
        .children(
            levels
                .into_iter()
                .map(|color| div().size(px(10.)).rounded(px(tokens.radius[1])).bg(color)),
        )
        .child(t("dashboard.heatmap.more"))
}

fn heatmap_color(value: f64, thresholds: &[Value], tokens: &Tokens) -> gpui_kit::Hsla {
    let activity = [
        tokens.heat[1],
        tokens.heat[2],
        tokens.heat[3],
        tokens.heat[4],
    ];
    heatmap_palette_color(value, thresholds, tokens.heat[0], &activity)
}

fn heatmap_palette_color(
    value: f64,
    thresholds: &[Value],
    zero: gpui_kit::Hsla,
    activity: &[gpui_kit::Hsla; 4],
) -> gpui_kit::Hsla {
    if value <= 0. || !value.is_finite() {
        return zero;
    }
    let level = if thresholds.len() >= 3 {
        thresholds[..3]
            .iter()
            .position(|threshold| value <= threshold.as_f64().unwrap_or(value))
            .unwrap_or(3)
    } else {
        0
    };
    activity[level]
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

fn dashboard_trend_title(data: &Value) -> &'static str {
    let granularity = data
        .get("timeseries")
        .and_then(|timeseries| {
            timeseries.get("granularity").or_else(|| {
                timeseries
                    .get("result")
                    .and_then(|result| result.get("granularity"))
            })
        })
        .and_then(Value::as_str)
        .or_else(|| data.get("granularity").and_then(Value::as_str))
        .unwrap_or("day");
    match granularity {
        "hour" => t("dashboard.trend_hour"),
        "week" => t("dashboard.trend_week"),
        _ => t("dashboard.daily"),
    }
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
        let legend =
            div()
                .flex()
                .flex_wrap()
                .gap(px(tokens.space[2]))
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
        let granularity = data
            .get("timeseries")
            .and_then(|timeseries| timeseries.get("granularity"))
            .and_then(Value::as_str)
            .or_else(|| data.get("granularity").and_then(Value::as_str))
            .unwrap_or("auto");
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
                    .map(|label| format_bucket_label(label, granularity))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let lines = visible
            .iter()
            .map(|(_, values, color, dashed)| TrendLine {
                values: values
                    .iter()
                    .map(|value| value.as_f64().unwrap_or(0.))
                    .collect(),
                color: *color,
                dashed: *dashed,
            })
            .collect();
        return div()
            .flex()
            .flex_col()
            .gap(px(tokens.space[4]))
            .child(legend)
            .child(shared_trend_plot(
                lines,
                axis_labels,
                y_min,
                y_max,
                tokens.border,
                tokens.secondary,
                tokens,
            ))
            .into_any_element();
    }
    let values = array(data, "trend");
    if values.is_empty() {
        return div()
            .text_size(px(tokens.desktop_type[1][0]))
            .text_color(tokens.secondary)
            .child(t("dashboard.empty"))
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

#[derive(Clone)]
struct TrendLine {
    values: Vec<f64>,
    color: Hsla,
    dashed: bool,
}

fn shared_trend_plot(
    lines: Vec<TrendLine>,
    labels: Vec<String>,
    y_min: f64,
    y_max: f64,
    grid_color: Hsla,
    secondary_color: Hsla,
    tokens: &Tokens,
) -> AnyElement {
    let plot_lines = lines.clone();
    let grid_line_width = tokens.desktop_layout.divider;
    let plot = canvas(
        move |bounds: Bounds<gpui_kit::gpui::Pixels>, _, _| {
            let width = bounds.size.width.as_f32();
            let height = bounds.size.height.as_f32();
            let origin = bounds.origin;
            let left = 38.;
            let right = (width - 8.).max(left + 1.);
            let top = 8.;
            let bottom = (height - 24.).max(top + 1.);
            let span = (y_max - y_min).max(f64::EPSILON);
            let mut paths = Vec::new();
            // Paint the grid first so the data lines remain visually on top.
            for y_ratio in [0., 0.5, 1.] {
                let y = bottom - (bottom - top) * y_ratio;
                let mut builder = PathBuilder::stroke(px(grid_line_width));
                builder.move_to(point(origin.x + px(left), origin.y + px(y)));
                builder.line_to(point(origin.x + px(right), origin.y + px(y)));
                if let Ok(path) = builder.build() {
                    paths.push((path, grid_color));
                }
            }
            for line in &plot_lines {
                if line.values.len() < 2 {
                    continue;
                }
                let mut builder = PathBuilder::stroke(px(2.));
                if line.dashed {
                    builder = builder.dash_array(&[px(6.), px(4.)]);
                }
                for (index, value) in line.values.iter().enumerate() {
                    let x = if line.values.len() == 1 {
                        left
                    } else {
                        left + (right - left) * index as f32 / (line.values.len() - 1) as f32
                    };
                    let ratio = ((*value - y_min) / span).clamp(0., 1.) as f32;
                    let y = bottom - (bottom - top) * ratio;
                    let point = point(origin.x + px(x), origin.y + px(y));
                    if index == 0 {
                        builder.move_to(point);
                    } else {
                        builder.line_to(point);
                    }
                }
                if let Ok(path) = builder.build() {
                    paths.push((path, line.color));
                }
            }
            paths
        },
        move |_, paths, window, _| {
            for (path, color) in paths {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .inset_0();

    let mut tick_indexes = if labels.is_empty() {
        Vec::new()
    } else {
        let stride = ((labels.len() - 1) / 5).max(1);
        let mut indexes = (0..labels.len()).step_by(stride).collect::<Vec<_>>();
        if indexes.last().copied() != Some(labels.len() - 1) {
            indexes.push(labels.len() - 1);
        }
        indexes
    };
    tick_indexes.dedup();
    let x_labels = div()
        .absolute()
        .left(px(38.))
        .right(px(8.))
        .bottom_0()
        .h(px(20.))
        .flex()
        .justify_between()
        .children(tick_indexes.iter().map(|index| {
            div()
                .text_size(px(tokens.desktop_type[0][0]))
                .text_color(secondary_color)
                .child(labels.get(*index).cloned().unwrap_or_default())
        }));
    let y_labels = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom(px(24.))
        .w(px(34.))
        .flex()
        .flex_col()
        .justify_between()
        .children([
            div()
                .text_size(px(tokens.desktop_type[0][0]))
                .text_color(secondary_color)
                .child(format_chart_value(y_max)),
            div()
                .text_size(px(tokens.desktop_type[0][0]))
                .text_color(secondary_color)
                .child(format_chart_value(y_min)),
        ]);
    div()
        .relative()
        .h(px(240.))
        .w_full()
        .child(plot)
        .child(x_labels)
        .child(y_labels)
        .into_any_element()
}

fn format_bucket_label(label: &str, granularity: &str) -> String {
    let date = label.get(5..10).unwrap_or(label);
    if granularity == "hour" {
        let hour = label.get(11..13).unwrap_or_default();
        if !hour.is_empty() {
            return format!("{date} {hour}{}", t("common.hour_suffix"));
        }
    }
    date.to_owned()
}

fn format_chart_value(value: f64) -> String {
    if value.abs() >= 1_000_000. {
        format!("{:.1}M", value / 1_000_000.)
    } else if value.abs() >= 1_000. {
        format!("{:.1}K", value / 1_000.)
    } else {
        format!("{value:.0}")
    }
}

fn series_color(tokens: &Tokens, index: usize) -> gpui_kit::Hsla {
    tokens.chart[index % tokens.chart.len()]
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
    let previewing = data
        .get("previewing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let route_query = skill_route_query(data);
    let filter = query_value(data, "filter").filter(|value| {
        matches!(
            value.as_str(),
            "all" | "builtin" | "user" | "imported" | "draft" | "disabled"
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
    let selected_source = skill_source_kind(selected);
    let selected_builtin = !selected_name.is_empty() && selected_source == "builtin";
    let selected_draft = !selected_name.is_empty() && selected_source == "draft";
    let list_rows =
        div()
            .flex()
            .flex_col()
            .gap(px(tokens.space[2]))
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
                    .gap(px(tokens.space[4]))
                    .w_full()
                    .p(px(tokens.space[4]))
                    .rounded(px(tokens.radius[2]))
                    .bg(
                        if selected.get("name").and_then(Value::as_str) == Some(id.as_str()) {
                            tokens.selected
                        } else {
                            tokens.window
                        },
                    )
                    .child(div().flex_1().min_w_0().truncate().child(format!(
                        "ϟ {name} · {} · {}",
                        skill_source(skill),
                        skill_usage_label(skill)
                    )))
                    .child({
                        let mut actions = div()
                            .flex()
                            .flex_wrap()
                            .gap(px(tokens.space[4]))
                            .flex_shrink_0();
                        if skill_source_kind(skill) != "draft" {
                            actions = actions.child(action_button_with_id(
                                format!("skill-{index}-toggle"),
                                if enabled {
                                    t("skills.disable_for")
                                } else {
                                    t("common.enable")
                                },
                                FeatureAction::Rpc {
                                    method: "skill.set_enabled".into(),
                                    params: skill_enabled_params(&id, !enabled, None),
                                },
                                cx,
                            ));
                        }
                        actions = actions.child(action_button_with_id(
                            format!("skill-{index}-edit"),
                            t("common.edit"),
                            FeatureAction::Navigate(format!("skill/{id}")),
                            cx,
                        ));
                        if skill_source_kind(skill) == "draft" {
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
    let list = if skills.is_empty() {
        div()
            .p(px(tokens.space[5]))
            .text_color(tokens.secondary)
            .child(t("skills.no_results"))
    } else {
        list_rows
    };
    let selected_header = if selected_name.is_empty() {
        div().into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .gap(px(tokens.space[2]))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(tokens.desktop_type[5][0]))
                    .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                    .child(selected_name.clone()),
            )
            .child(
                div()
                    .text_size(px(tokens.desktop_type[0][0]))
                    .text_color(if selected_draft {
                        tokens.warning
                    } else {
                        tokens.secondary
                    })
                    .child(skill_source(selected)),
            )
            .into_any_element()
    };
    let editor = div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[4]))
        .child(selected_header)
        .child(labeled_field(
            t("skills.new.name"),
            Input::new(input_state(inputs, "name"))
                .id("skill-name-input")
                .disabled(selected_builtin),
            tokens,
        ))
        .child(if previewing {
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[4]))
                .child(
                    div()
                        .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                        .child(t("common.preview")),
                )
                .child(TextView::markdown(
                    "skill-edit-preview-markdown",
                    textarea_state(textareas, "content")
                        .read(cx)
                        .value()
                        .to_string(),
                ))
                .into_any_element()
        } else {
            labeled_field(
                t("skills.instructions"),
                Textarea::new(textarea_state(textareas, "content"))
                    .h(px(240.))
                    .disabled(selected_builtin),
                tokens,
            )
            .into_any_element()
        })
        .child(
            div()
                .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                .child(t("skills.new.mode.write")),
        )
        .child(skill_create_button(
            t("skills.new.create"),
            inputs,
            textareas,
            cx,
        ))
        .child(if selected_draft {
            div()
                .text_size(px(tokens.desktop_type[1][0]))
                .text_color(tokens.secondary)
                .child(t("skills.new.draft_hint"))
                .into_any_element()
        } else {
            div().into_any_element()
        })
        .child(card(
            t("skills.import_group"),
            div()
                .flex()
                .flex_col()
                .gap(px(tokens.space[4]))
                .child(labeled_field(
                    t("skills.field_path"),
                    Input::new(input_state(inputs, "path")).id("skill-import-path-input"),
                    tokens,
                ))
                .child(skill_import_button(t("skills.import"), inputs, cx))
                .child(labeled_field(
                    t("skills.field_git_url"),
                    Input::new(input_state(inputs, "git_url")).id("skill-import-git-url-input"),
                    tokens,
                ))
                .child(labeled_field(
                    t("skills.field_git_subdir"),
                    Input::new(input_state(inputs, "git_subdir"))
                        .id("skill-import-git-subdir-input"),
                    tokens,
                ))
                .child(skill_import_git_button(
                    t("skills.new.mode.git"),
                    inputs,
                    cx,
                ))
                .child(skill_upload_button(t("skills.new.mode.zip"), cx)),
            tokens,
        ))
        .child(card(
            t("skills.new.desc"),
            div().child(string(selected, "description", t("skills.select_hint"))),
            tokens,
        ))
        .child(card(
            t("skills.attached_files"),
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
            t("skills.disable_for"),
            skill_bot_controls(selected, data, tokens, cx),
            tokens,
        ))
        .child({
            let preview_label = if previewing {
                t("common.edit")
            } else {
                t("common.preview")
            };
            let mut actions = div().flex().gap(px(tokens.space[4])).child(
                Button::new("skill-toggle-preview")
                    .label(preview_label)
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let current = this
                            .data
                            .get("previewing")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        this.data["previewing"] = json!(!current);
                        cx.notify();
                    })),
            );
            if selected_draft {
                actions = actions
                    .child(textarea_rpc_button(
                        t("common.save"),
                        "skill.update",
                        skill_update_params(&selected_name, ""),
                        "content",
                        textareas,
                        None,
                        cx,
                    ))
                    .child(action_button(
                        t("skills.discard"),
                        FeatureAction::Rpc {
                            method: "skill.delete".into(),
                            params: skill_delete_params(&selected_name),
                        },
                        cx,
                    ))
                    .child(action_button(
                        t("skills.publish"),
                        FeatureAction::Rpc {
                            method: "skill.publish".into(),
                            params: skill_publish_params(&selected_name),
                        },
                        cx,
                    ));
            } else if !selected_name.is_empty() && !selected_builtin {
                actions = actions
                    .child(textarea_rpc_button(
                        t("common.save"),
                        "skill.update",
                        skill_update_params(&selected_name, ""),
                        "content",
                        textareas,
                        None,
                        cx,
                    ))
                    .child(action_button(
                        t("common.delete"),
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
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(t("skills.title"), t("skills.subtitle"), tokens))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(tokens.space[4]))
                .child(labeled_field(
                    t("skills.search"),
                    Input::new(input_state(inputs, "query")).id("skill-search-input"),
                    tokens,
                ))
                .child(skill_search_button(inputs, cx))
                .child(skill_filter_button(
                    t("common.all"),
                    None,
                    filter.as_deref(),
                    cx,
                ))
                .child(skill_filter_button(
                    t("skills.source.builtin"),
                    Some("builtin"),
                    filter.as_deref(),
                    cx,
                ))
                .child(skill_filter_button(
                    t("skills.source.mine"),
                    Some("user"),
                    filter.as_deref(),
                    cx,
                ))
                .child(skill_filter_button(
                    t("skills.source.imported"),
                    Some("imported"),
                    filter.as_deref(),
                    cx,
                ))
                .child(skill_filter_button(
                    t("skills.draft"),
                    Some("draft"),
                    filter.as_deref(),
                    cx,
                ))
                .child(skill_filter_button(
                    t("skills.disabled"),
                    Some("disabled"),
                    filter.as_deref(),
                    cx,
                )),
        )
        .child(
            div()
                .flex()
                .gap(px(tokens.space[6]))
                .child(
                    div()
                        .w(px(tokens.desktop_layout.listcol_width))
                        .flex_shrink_0()
                        .child(card(t("skills.title"), list, tokens)),
                )
                .child(editor.flex_1().min_w_0()),
        )
}

fn skill_bot_controls(
    selected: &Value,
    data: &Value,
    tokens: &Tokens,
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
    div().flex().flex_col().gap(px(tokens.space[2])).children(
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
    selected: Option<&str>,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let target = filter
        .map(|filter| format!("skills?filter={filter}"))
        .unwrap_or_else(|| "skills?filter=all".to_owned());
    let active = filter.unwrap_or("all") == selected.unwrap_or("all");
    let label = label.to_string();
    Button::new(format!("skill-filter-{}", filter.unwrap_or("all")))
        .label(label)
        .when(active, |button| button.primary())
        .when(!active, |button| button.ghost())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.emit_action(FeatureAction::Navigate(target.clone()), cx)
        }))
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
        Some("builtin" | "user" | "imported" | "draft") => {
            skill_source_kind(skill) == filter.unwrap()
        }
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
        "builtin" => t("skills.source.builtin"),
        "draft" => t("skills.draft"),
        "imported" => t("skills.source.imported"),
        _ => t("skills.source.mine"),
    }
}

fn skill_usage_label(skill: &Value) -> String {
    let count = skill_invocations(skill);
    let disabled_for_bots = skill
        .get("disabled_bot_ids")
        .and_then(Value::as_array)
        .is_some_and(|ids| !ids.is_empty());
    let enabled = skill
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if enabled && !disabled_for_bots {
        t("skills.usage").replace("{count}", &count)
    } else {
        t("skills.usage.partial").replace("{count}", &count)
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
        t("bot.settings").to_owned()
    } else {
        let name = input_state(inputs, "name").read(cx).value().to_string();
        let name = if name.trim().is_empty() {
            t("bot.default_name")
        } else {
            name.as_str()
        };
        t("new.bot").replace("{name}", name)
    };
    let bot_id = data.get("id").and_then(Value::as_str).unwrap_or_default();
    let body = rows(
        [
            format!(
                "{}   {}",
                t("bot.field.name"),
                string(data, "name", t("bot.default_name"))
            ),
            format!(
                "{}   {}",
                t("bot.field.label"),
                string(data, "label", t("bot.default_label"))
            ),
            format!(
                "{}   {}",
                t("bot.field.rules"),
                string(data, "description", t("bot.default_description"))
            ),
            format!(
                "{}   {}",
                t("bot.field.model"),
                string(data, "model", t("bot.default_model"))
            ),
            format!(
                "{}   {}",
                t("bot.field.max_parallel"),
                string(data, "max_parallel", "3")
            ),
            format!(
                "{}   {}",
                t("bot.field.browser_mode"),
                string(data, "browser_mode", t("bot.default_browser"))
            ),
        ],
        tokens,
    );
    let model_picker = bot_model_picker(data, inputs, tokens, cx);
    let avatar_picker = bot_avatar_picker(data, tokens, cx);
    let bot_actions = bot_actions(data, tokens, cx);
    let main_rule_picker = if data
        .get("is_main")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bot_main_rule_picker(data, tokens, cx).into_any_element()
    } else {
        div().into_any_element()
    };
    let tool_picker = if data
        .get("is_main")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        div().into_any_element()
    } else {
        bot_tools_picker(data, tokens, cx).into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(title.clone(), t("bot.subtitle"), tokens))
        .child(if data.get("id").is_some() {
            action_button(
                t("common.details"),
                FeatureAction::Navigate(format!("bot/{bot_id}")),
                cx,
            )
            .into_any_element()
        } else {
            div().into_any_element()
        })
        .child(labeled_field(
            t("bot.field.name"),
            Input::new(input_state(inputs, "name")).id("bot-name-input"),
            tokens,
        ))
        .child(labeled_field(
            t("bot.field.label"),
            Input::new(input_state(inputs, "label")).id("bot-label-input"),
            tokens,
        ))
        .child(labeled_field(
            t("bot.field.rules"),
            Textarea::new(textarea_state(textareas, "description")).h(px(120.)),
            tokens,
        ))
        .child(labeled_field(
            t("bot.field.model"),
            Input::new(input_state(inputs, "model")).id("bot-model-input"),
            tokens,
        ))
        .child(labeled_field(
            t("bot.field.max_parallel"),
            Input::new(input_state(inputs, "max_parallel")).id("bot-parallel-input"),
            tokens,
        ))
        .child(labeled_field(
            t("bot.field.browser_mode"),
            Input::new(input_state(inputs, "browser_mode")).id("bot-browser-input"),
            tokens,
        ))
        .child(labeled_field(
            t("bot.models.available"),
            model_picker,
            tokens,
        ))
        .child(labeled_field(
            t("bot.field.browser_mode"),
            bot_browser_picker(inputs, tokens, cx),
            tokens,
        ))
        .child(labeled_field(t("bot.field.tools"), tool_picker, tokens))
        .child(bot_notification_toggle(data, cx))
        .child(main_rule_picker)
        .child(avatar_picker)
        .child(if data.get("id").is_some() {
            card(title, body, tokens).into_any_element()
        } else {
            div().into_any_element()
        })
        .child(bot_save_button(
            t("common.save"),
            data,
            inputs,
            textareas,
            cx,
        ))
        .child(bot_actions)
}

fn bot_main_rule_picker(
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let rules = data.get("main_bot_rules").unwrap_or(&Value::Null);
    if data.get("main_bot_rules_loading").and_then(Value::as_bool) == Some(true) {
        return div()
            .text_color(tokens.secondary)
            .child(t("common.loading"));
    }
    if data
        .get("main_bot_rules_error")
        .is_some_and(|error| !error.is_null())
        || rules
            .get("auto_create_project")
            .and_then(Value::as_bool)
            .is_none()
    {
        return div()
            .flex()
            .items_center()
            .gap(px(tokens.space[3]))
            .text_color(tokens.secondary)
            .child(t("bot.main.rule.load_failed"))
            .child(action_button(
                t("common.retry"),
                FeatureAction::Rpc {
                    method: "settings.get".into(),
                    params: json!({}),
                },
                cx,
            ));
    }
    let auto_create = rules["auto_create_project"].as_bool().unwrap_or(false);
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .child(
            div()
                .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                .child(t("bot.main.rule")),
        )
        .child(
            div().flex().gap(px(tokens.space[2])).children([
                Button::new("main-bot-rule-auto")
                    .label(t("bot.main.rule.auto"))
                    .when(auto_create, |button| button.primary())
                    .when(!auto_create, |button| button.ghost())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emit_action(
                            FeatureAction::Rpc {
                                method: "settings.update".into(),
                                params: json!({"patch":{"main_bot":{"auto_create_project":true}}}),
                            },
                            cx,
                        )
                    })),
                Button::new("main-bot-rule-ask")
                    .label(t("bot.main.rule.ask"))
                    .when(!auto_create, |button| button.primary())
                    .when(auto_create, |button| button.ghost())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emit_action(
                            FeatureAction::Rpc {
                                method: "settings.update".into(),
                                params: json!({"patch":{"main_bot":{"auto_create_project":false}}}),
                            },
                            cx,
                        )
                    })),
            ]),
        )
}

fn bot_detail(data: &Value, tokens: &Tokens, cx: &mut Context<FeaturePage>) -> impl IntoElement {
    let detail = data.get("bot_detail").unwrap_or(data);
    let bot = detail
        .get("bot")
        .or_else(|| data.get("selected"))
        .unwrap_or(&Value::Null);
    let loading = detail.get("loading").unwrap_or(&Value::Null);
    let errors = detail.get("errors").unwrap_or(&Value::Null);
    let bot_id = string(bot, "id", &string(detail, "bot_id", ""));
    let bot_name = string(bot, "name", t("common.unknown"));
    let label = string(bot, "label", "");
    let model = string(bot, "model", t("bot.default_model"));
    let avatar = bot.get("avatar").unwrap_or(&Value::Null);
    let avatar_color = avatar.get("color").and_then(Value::as_u64).unwrap_or(0);
    let avatar_text = avatar
        .get("emoji")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| bot_name.chars().next().unwrap_or('B').to_string());
    let project_rows = array(detail, "projects")
        .into_iter()
        .filter(|project| project_has_bot(project, &bot_id))
        .map(|project| {
            let project_id = string(&project, "id", "");
            let project_name = string(&project, "name", t("common.unknown"));
            let target = format!("project/{project_id}");
            action_button(&project_name, FeatureAction::Navigate(target), cx).into_any_element()
        })
        .collect::<Vec<_>>();
    let assignments = array(detail, "assignments")
        .into_iter()
        .map(|assignment| {
            let assignment_id = string(&assignment, "id", "");
            let title = string(&assignment, "title", t("common.unnamed_task"));
            let status = bot_assignment_status(&assignment);
            action_button(
                &format!("{status} · {title}"),
                FeatureAction::Navigate(format!("assignment/{assignment_id}")),
                cx,
            )
            .into_any_element()
        })
        .collect::<Vec<_>>();
    let usage = bot_usage(detail, &bot_id);
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cost = usage
        .get("cost")
        .and_then(json_value_text)
        .unwrap_or_else(|| t("common.unknown").to_owned());
    let name_and_label = if label.is_empty() {
        model
    } else {
        format!("{label} · {model}")
    };
    let bot_id_for_settings = bot_id.clone();
    let dm_chat_id = string(bot, "dm_chat_id", "");
    let show_detail_error = |key: &str| errors.get(key).is_some();
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(
            if loading.get("bot").and_then(Value::as_bool) == Some(true) {
                div()
                    .text_color(tokens.secondary)
                    .child(t("common.loading"))
                    .into_any_element()
            } else if show_detail_error("bot") || bot_id.is_empty() {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.load_failed"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens.space[5]))
                    .child(
                        div()
                            .size(px(64.))
                            .rounded(px(tokens.radius[11]))
                            .bg(tokens.bean(avatar_color))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(tokens.desktop_type[7][0]))
                            .text_color(tokens.primary)
                            .child(avatar_text),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(tokens.space[2]))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(tokens.desktop_type[5][0]))
                                    .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                                    .child(bot_name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(tokens.desktop_type[1][0]))
                                    .text_color(tokens.secondary)
                                    .child(name_and_label),
                            ),
                    )
                    .into_any_element()
            },
        )
        .child(
            div()
                .flex()
                .gap(px(tokens.space[2]))
                .when(!dm_chat_id.is_empty(), |row| {
                    row.child(action_button(
                        t("bot.detail.open_chat"),
                        FeatureAction::Navigate(format!("chat/{dm_chat_id}")),
                        cx,
                    ))
                })
                .child(action_button(
                    t("bot.settings"),
                    FeatureAction::Navigate(format!("bot_settings?bot_id={bot_id_for_settings}")),
                    cx,
                )),
        )
        .child(card(
            t("bot.groups"),
            if loading.get("projects").and_then(Value::as_bool) == Some(true) {
                div()
                    .text_color(tokens.secondary)
                    .child(t("common.loading"))
                    .into_any_element()
            } else if show_detail_error("projects") {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.load_failed"))
                    .into_any_element()
            } else if project_rows.is_empty() {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.empty_groups"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(tokens.space[2]))
                    .children(project_rows)
                    .into_any_element()
            },
            tokens,
        ))
        .child(card(
            t("bot.history"),
            if loading.get("assignments").and_then(Value::as_bool) == Some(true) {
                div()
                    .text_color(tokens.secondary)
                    .child(t("common.loading"))
                    .into_any_element()
            } else if show_detail_error("assignments") {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.load_failed"))
                    .into_any_element()
            } else if assignments.is_empty() {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.empty_history"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(tokens.space[2]))
                    .children(assignments)
                    .into_any_element()
            },
            tokens,
        ))
        .child(card(
            t("bot.field.skills"),
            if loading.get("skills").and_then(Value::as_bool) == Some(true) {
                div()
                    .text_color(tokens.secondary)
                    .child(t("common.loading"))
                    .into_any_element()
            } else if show_detail_error("skills") {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.load_failed"))
                    .into_any_element()
            } else {
                bot_detail_skills(&array(detail, "skills"), &bot_id, tokens, cx).into_any_element()
            },
            tokens,
        ))
        .child(card(
            t("bot.week_usage"),
            if loading.get("weekly_usage").and_then(Value::as_bool) == Some(true) {
                div()
                    .text_color(tokens.secondary)
                    .child(t("common.loading"))
                    .into_any_element()
            } else if show_detail_error("weekly_usage") {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.load_failed"))
                    .into_any_element()
            } else if array(detail, "weekly_usage").is_empty() {
                div()
                    .text_color(tokens.secondary)
                    .child(t("bot.detail.empty_usage"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(tokens.space[4]))
                    .children([
                        stat(
                            t("dashboard.kpi.tokens"),
                            &(input_tokens + output_tokens).to_string(),
                            tokens,
                        ),
                        stat(t("dashboard.kpi.cost"), &cost, tokens),
                    ])
                    .into_any_element()
            },
            tokens,
        ))
}

fn bot_detail_skills(
    skills: &[Value],
    bot_id: &str,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    let active_count = skills
        .iter()
        .filter(|skill| skill_source_kind(skill) != "draft")
        .filter(|skill| bot_skill_enabled(skill, bot_id))
        .count();
    let header = t("bot.field.skills_all").replace("{count}", &active_count.to_string());
    let mut rows = div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .child(header);
    let mut visible = 0;
    for skill in skills {
        if skill_source_kind(skill) == "draft" {
            continue;
        }
        visible += 1;
        let name = string(skill, "name", "").to_owned();
        if name.is_empty() {
            continue;
        }
        let enabled = bot_skill_enabled(skill, bot_id);
        rows = rows.child(
            Checkbox::new(format!("bot-detail-skill-{name}"))
                .label(format!("{} · {}", name, skill_source(skill)))
                .checked(enabled)
                .on_change(cx.listener(move |this, next, _, cx| {
                    this.emit_action(
                        FeatureAction::Rpc {
                            method: "skill.set_enabled".into(),
                            params: skill_enabled_params(&name, *next, Some(&bot_id)),
                        },
                        cx,
                    );
                })),
        );
    }
    if visible == 0 {
        rows = rows.child(
            div()
                .text_color(tokens.secondary)
                .child(t("bot.detail.empty_skills")),
        );
    }
    rows.child(action_button(
        t("common.view_all"),
        FeatureAction::Navigate("skills".into()),
        cx,
    ))
}

fn bot_skill_enabled(skill: &Value, bot_id: &str) -> bool {
    skill
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        && !skill
            .get("disabled_bot_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(bot_id)))
}

fn project_has_bot(project: &Value, bot_id: &str) -> bool {
    if project.get("lead_bot_id").and_then(Value::as_str) == Some(bot_id) {
        return true;
    }
    let members = project
        .get("member_bot_ids")
        .or_else(|| project.get("members"))
        .and_then(Value::as_array);
    members.is_some_and(|members| {
        members.iter().any(|member| {
            member.as_str() == Some(bot_id)
                || member.get("bot_id").and_then(Value::as_str) == Some(bot_id)
        })
    })
}

fn bot_assignment_status(assignment: &Value) -> &'static str {
    match assignment.get("status").and_then(Value::as_str) {
        Some("queued") => t("status.queued"),
        Some("working") => t("status.working"),
        Some("waiting_user") => t("status.waiting_user"),
        Some("waiting_bot" | "blocked") => t("status.blocked"),
        Some("done") => t("status.done"),
        Some("failed") => t("status.failed"),
        Some("cancelled") => t("status.cancelled"),
        _ => t("common.unknown"),
    }
}

fn bot_usage(detail: &Value, bot_id: &str) -> Value {
    let weekly_usage = detail.get("weekly_usage").unwrap_or(&Value::Null);
    if let Some(rows) = weekly_usage.as_array() {
        return rows
            .iter()
            .find(|row| row.get("key").and_then(Value::as_str) == Some(bot_id))
            .or_else(|| (rows.len() == 1).then(|| &rows[0]))
            .and_then(|row| row.get("usage").cloned().or_else(|| Some(row.clone())))
            .unwrap_or(Value::Null);
    }
    if let Some(row) = array(weekly_usage, "rows")
        .iter()
        .find(|row| row.get("key").and_then(Value::as_str) == Some(bot_id))
    {
        return row.get("usage").cloned().unwrap_or_else(|| row.clone());
    }
    weekly_usage
        .get("usage")
        .cloned()
        .unwrap_or_else(|| weekly_usage.clone())
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
        .label(t("bot.field.notifications"))
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
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let model = input_state(inputs, "model").clone();
    let models = array(data, "models");
    div()
        .flex()
        .gap(px(tokens.space[2]))
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
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let browser = input_state(inputs, "browser_mode").clone();
    div().flex().gap(px(tokens.space[2])).children(
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

fn bot_tools_picker(
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let bot_id = data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let tools = data.get("tools").cloned().unwrap_or_else(default_bot_tools);
    div().flex().flex_wrap().gap(px(tokens.space[4])).children(
        ["files", "bash", "browser", "subagent", "web", "mcp"]
            .into_iter()
            .map(|key| {
                let checked = tools.get(key).and_then(Value::as_bool).unwrap_or(false);
                let bot_id = bot_id.clone();
                let key = key.to_owned();
                Checkbox::new(format!("bot-tool-{key}"))
                    .label(t(match key.as_str() {
                        "files" => "bot.tool.file",
                        "bash" => "bot.tool.bash",
                        "browser" => "bot.tool.browser",
                        "subagent" => "bot.tool.subagent",
                        "web" => "bot.tool.web",
                        _ => "bot.tool.mcp",
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
    tokens: &Tokens,
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
    div().flex().gap(px(tokens.space[2])).children(buttons)
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

fn bot_actions(
    data: &Value,
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
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
        .gap(px(tokens.space[4]))
        .child(action_button(
            if pinned {
                t("bot.action.unpin")
            } else {
                t("bot.action.pin")
            },
            FeatureAction::Rpc {
                method: "bot.update".into(),
                params: bot_pinned_update_params(&bot_id, !pinned),
            },
            cx,
        ))
        .child(action_button(
            if hidden {
                t("bot.action.show")
            } else {
                t("bot.action.hide")
            },
            FeatureAction::Rpc {
                method: "bot.update".into(),
                params: bot_visibility_update_params(&bot_id, !hidden),
            },
            cx,
        ))
        .child(action_button(
            t("bot.action.delete"),
            FeatureAction::Rpc {
                method: "bot.delete".into(),
                params: bot_delete_params(&bot_id),
            },
            cx,
        ))
        .child(action_button(
            t("bot.action.duplicate"),
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
    let member_checks = div().flex().flex_col().gap(px(tokens.space[2])).children(
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
                t("group.field.name"),
                string(data, "name", t("group.default_name"))
            ),
            format!(
                "{}   {}",
                t("group.announcement"),
                string(data, "goal", t("group.field.goal_hint"))
            ),
            format!(
                "{}   {}",
                t("group.members"),
                if members.is_empty() {
                    t("sidebar.main_bot.tag").to_string()
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
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(
            t("group.new.title"),
            t("group.subtitle"),
            tokens,
        ))
        .child(labeled_field(
            t("group.field.name"),
            Input::new(input_state(inputs, "name")).id("group-name-input"),
            tokens,
        ))
        .child(labeled_field(
            t("group.announcement"),
            Textarea::new(textarea_state(textareas, "goal")).h(px(120.)),
            tokens,
        ))
        .child(
            div()
                .text_size(px(tokens.desktop_type[1][0]))
                .child(t("group.members")),
        )
        .child(member_checks)
        .child(card(t("group.new.title"), body, tokens))
        .child(group_create_button(
            t("new.create_group"),
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
    let history = array(data, "runs");
    let summary = if routine_id.is_empty() {
        div().into_any_element()
    } else {
        let enabled = data.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let mut summary_rows = vec![
            format!(
                "{}   {}",
                t("routines.field.instruction"),
                string(data, "instructions", t("routine.default_instruction"))
            ),
            format!("{}   {}", t("routines.field.when"), routine_schedule(data)),
        ];
        let next_run = routine_next_run_label(data);
        if !next_run.is_empty() {
            summary_rows.push(next_run);
        }
        summary_rows.push(format!(
            "{}   {}",
            t("routines.status"),
            if enabled {
                t("routines.filter.running")
            } else {
                t("routines.paused")
            }
        ));
        card(t("routines.title"), rows(summary_rows, tokens), tokens).into_any_element()
    };
    let routine_actions = if routine_id.is_empty() {
        div()
    } else {
        div()
            .flex()
            .gap(px(tokens.space[4]))
            .child(action_button(
                t("routines.test_run"),
                FeatureAction::Rpc {
                    method: "routine.test_run".into(),
                    params: routine_test_params(&routine_id),
                },
                cx,
            ))
            .child(action_button(
                if data.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
                    t("routines.paused")
                } else {
                    t("common.enable")
                },
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
                t("common.delete"),
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
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(
            t("routines.title"),
            string(data, "name", t("routine.default_name")),
            tokens,
        ))
        .child(routine_list)
        .child(labeled_field(
            t("routines.field.name"),
            Input::new(input_state(inputs, "name")).id("routine-name-input"),
            tokens,
        ))
        .child(routine_selectors(data, inputs, tokens, cx))
        .child(labeled_field(
            t("routines.field.instruction"),
            Textarea::new(textarea_state(textareas, "instructions")).h(px(180.)),
            tokens,
        ))
        .child(routine_schedule_inputs(data, inputs, tokens, cx))
        .child(routine_add_schedule_button(cx))
        .child(labeled_field(
            t("routines.field.timezone_label"),
            Input::new(input_state(inputs, "timezone")).id("routine-timezone-input"),
            tokens,
        ))
        .child(summary)
        .child(card(
            t("routines.history"),
            routine_history_rows(&history, tokens, cx),
            tokens,
        ))
        .child(
            div()
                .flex()
                .gap(px(tokens.space[4]))
                .child(routine_save_button(
                    t("common.save"),
                    data,
                    inputs,
                    textareas,
                    cx,
                ))
                .child(routine_actions),
        )
}

fn routine_next_run_label(data: &Value) -> String {
    let Some(next_run) = data.get("next_run_at").and_then(Value::as_str) else {
        return String::new();
    };
    let Ok(next_run) = chrono::DateTime::parse_from_rfc3339(next_run) else {
        return String::new();
    };
    let bot_id = string(data, "bot_id", "");
    let bot = array(data, "bots")
        .iter()
        .find(|item| string(item, "id", "") == bot_id)
        .map(|item| string(item, "name", &bot_id))
        .unwrap_or_else(|| bot_id.to_owned());
    t("routines.next_run")
        .replace(
            "{time}",
            &next_run
                .with_timezone(&Local)
                .format("%m/%d %H:%M")
                .to_string(),
        )
        .replace("{bot}", &bot)
}

fn routine_history_rows(
    runs: &[Value],
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    if runs.is_empty() {
        return div()
            .text_color(tokens.secondary)
            .child(t("routines.history.empty"));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .children(runs.iter().enumerate().map(|(index, run)| {
            let status = match run.get("status").and_then(Value::as_str) {
                Some("done") => t("routines.run.ok"),
                Some("running") => t("status.working"),
                Some("failed") => t("status.failed"),
                Some("cancelled") => t("status.cancelled"),
                _ => t("common.unknown"),
            };
            let assignment_id = run
                .get("assignment_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut row = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(tokens.space[3]))
                .child(format!(
                    "{} · {} · {status}",
                    routine_time_label(
                        run.get("started_at")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    ),
                    routine_trigger(run)
                ));
            if !assignment_id.is_empty() {
                row = row.child(action_button_with_id(
                    format!("routine-run-trace-{index}"),
                    t("routines.run.trace"),
                    FeatureAction::Trace(assignment_id.to_owned()),
                    cx,
                ));
            }
            row
        }))
}

fn routine_schedule_inputs(
    data: &Value,
    inputs: &BTreeMap<String, Entity<InputState>>,
    tokens: &Tokens,
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
    div().flex().flex_col().gap(px(tokens.space[2])).children(
        fields
            .iter()
            .take(slots as usize)
            .enumerate()
            .map(|(index, field)| {
                labeled_field(
                    format!("{} {}", t("routines.field.schedule_slot"), index + 1),
                    Input::new(input_state(inputs, field)).id(format!("routine-{field}-input")),
                    tokens,
                )
            }),
    )
}

fn routine_add_schedule_button(cx: &mut Context<FeaturePage>) -> impl IntoElement + use<> {
    Button::new("routine-add-schedule")
        .label(t("routines.field.add_time"))
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
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement + use<> {
    let bot_input = input_state(inputs, "bot_id").clone();
    let project_input = input_state(inputs, "project_id").clone();
    let bots = array(data, "bots");
    let projects = array(data, "projects");
    let selected_bot_id = bot_input.read(cx).value().to_string();
    let selected_project_id = project_input.read(cx).value().to_string();
    let selected_bot = bots
        .iter()
        .find(|bot| string(bot, "id", "") == selected_bot_id)
        .map(|bot| string(bot, "name", &selected_bot_id))
        .unwrap_or_else(|| selected_bot_id.clone());
    let selected_project = projects
        .iter()
        .find(|project| string(project, "id", "") == selected_project_id)
        .map(|project| string(project, "name", &selected_project_id));
    let dm_label = t("routines.deliver.dm").replace("{bot}", &selected_bot);
    let destination = selected_project
        .as_ref()
        .map(|name| format!("{} · {name}", t("routines.deliver.group")))
        .unwrap_or_else(|| dm_label.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[3]))
        .child(
            div()
                .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                .child(t("routines.field.bot")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(tokens.space[2]))
                .children(bots.iter().filter_map(|bot| {
                    let id = bot.get("id").and_then(Value::as_str)?.to_owned();
                    let label = bot
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned();
                    let input = bot_input.clone();
                    let selected = id == selected_bot_id;
                    Some(
                        Button::new(format!("routine-bot-{id}"))
                            .label(label)
                            .when(selected, |button| button.primary())
                            .when(!selected, |button| button.ghost())
                            .on_click(cx.listener(move |_this, _, window, cx| {
                                input.update(cx, |state, cx| {
                                    state.set_value(id.clone(), window, cx)
                                });
                            })),
                    )
                })),
        )
        .child(
            div()
                .font_weight(gpui_kit::gpui::FontWeight::SEMIBOLD)
                .child(t("routines.field.deliver_to")),
        )
        .child(
            div()
                .text_size(px(tokens.desktop_type[1][0]))
                .text_color(tokens.secondary)
                .child(destination),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(tokens.space[2]))
                .child({
                    let input = project_input.clone();
                    let selected = selected_project_id.is_empty();
                    Button::new("routine-deliver-dm")
                        .label(dm_label)
                        .when(selected, |button| button.primary())
                        .when(!selected, |button| button.ghost())
                        .on_click(cx.listener(move |_this, _, window, cx| {
                            input
                                .update(cx, |state, cx| state.set_value(String::new(), window, cx));
                        }))
                })
                .children(projects.iter().filter_map(|project| {
                    let id = project.get("id").and_then(Value::as_str)?.to_owned();
                    let label = project
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned();
                    let input = project_input.clone();
                    let selected = id == selected_project_id;
                    Some(
                        Button::new(format!("routine-deliver-group-{id}"))
                            .label(label)
                            .when(selected, |button| button.primary())
                            .when(!selected, |button| button.ghost())
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
    let filter = query_value(data, "filter").unwrap_or_else(|| "all".to_owned());
    let filtered = routines
        .iter()
        .filter(|routine| match filter.as_str() {
            "running" => routine
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            "paused" => !routine
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            _ => true,
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut list = div().flex().flex_col().gap(px(tokens.space[2]));
    for routine in filtered {
        let id = string(&routine, "id", "");
        let name = string(&routine, "name", t("routine.default_name"));
        let enabled = routine
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let label = format!(
            "{} · {name}",
            if enabled {
                t("routines.filter.running")
            } else {
                t("routines.paused")
            }
        );
        let selected_routine = routine.clone();
        let route = format!("routine/{id}");
        list = list.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .p(px(tokens.space[4]))
                .rounded(px(tokens.radius[2]))
                .bg(if id == selected_id {
                    tokens.selected
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
    list.child(div().flex().flex_wrap().gap(px(tokens.space[2])).children(
        ["all", "running", "paused"].into_iter().map(|value| {
            let label = match value {
                "running" => t("routines.filter.running"),
                "paused" => t("routines.filter.paused"),
                _ => t("common.all"),
            };
            let target = format!("routines?filter={value}");
            Button::new(format!("routine-filter-{value}"))
                .label(label)
                .when(filter == value, |button| button.primary())
                .when(filter != value, |button| button.ghost())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.emit_action(FeatureAction::Navigate(target.clone()), cx)
                }))
        }),
    ))
    .child(action_button(
        t("routines.new"),
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
        .gap(px(tokens.space[6]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(
            t("common.search"),
            t("search.placeholder"),
            tokens,
        ))
        .child(Input::new(input_state(inputs, "query")).id("search-input"))
        .child(input_rpc_button(
            t("common.search"),
            "search",
            json!({"limit": 20}),
            "query",
            inputs,
            cx,
        ))
        .child(
            div()
                .flex()
                .gap(px(tokens.space[4]))
                .child(search_filter_button(t("common.all"), None, inputs, cx))
                .child(search_filter_button(
                    t("search.filter.messages"),
                    Some("message"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.filter.groups"),
                    Some("chat"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.filter.bots"),
                    Some("bot"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.filter.artifacts"),
                    Some("artifact"),
                    inputs,
                    cx,
                ))
                .child(search_filter_button(
                    t("search.filter.routines"),
                    Some("routine"),
                    inputs,
                    cx,
                )),
        )
        .child(card(
            t("common.search"),
            search_result_rows(&result, tokens, cx),
            tokens,
        ))
}

fn search_kind_label(kind: &str) -> String {
    match kind {
        "message" => t("search.filter.messages").to_owned(),
        "chat" => t("search.filter.groups").to_owned(),
        "artifact" => t("search.filter.artifacts").to_owned(),
        "bot" => t("search.filter.bots").to_owned(),
        "routine" => t("search.filter.routines").to_owned(),
        "" => t("common.unknown").to_owned(),
        _ => kind.to_owned(),
    }
}

fn search_result_rows(
    results: &[Value],
    tokens: &Tokens,
    cx: &mut Context<FeaturePage>,
) -> impl IntoElement {
    if results.is_empty() {
        return div()
            .w_full()
            .p(px(tokens.space[6]))
            .text_color(tokens.secondary)
            .child(t("search.no_results"))
            .into_any_element();
    }
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(tokens.space[2]))
        .children(results.iter().enumerate().map(|(index, result)| {
            let kind = string(result, "kind", "");
            let kind_label = search_kind_label(&kind);
            let title = string(result, "title", t("common.unknown"));
            let snippet = string(result, "snippet", t("common.unknown"));
            let target = search_target(result, &kind);
            let mut row = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(tokens.space[5]))
                .py(px(tokens.space[4]))
                .border_b(px(tokens.desktop_layout.divider))
                .border_color(tokens.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .gap(px(tokens.space[2]))
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .child(format!("{kind_label}   {title}")),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(px(tokens.desktop_type[1][0]))
                                .text_color(tokens.secondary)
                                .child(snippet),
                        ),
                );
            if let Some(target) = target {
                row = row.child(div().flex_shrink_0().child(action_button_with_id(
                    format!("search-open-{index}"),
                    t("common.open"),
                    FeatureAction::Navigate(target),
                    cx,
                )));
            }
            row
        }))
        .into_any_element()
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
        "bot" => result
            .get("chat_id")
            .and_then(Value::as_str)
            .filter(|chat_id| !chat_id.is_empty())
            .map(|chat_id| format!("search_bot/{id}?chat_id={chat_id}"))
            .or_else(|| Some(format!("search_bot/{id}"))),
        "routine" => Some(format!("routine/{id}")),
        "artifact" => Some(format!("artifact/{id}")),
        _ => None,
    }
}

fn empty_page(page: &str, tokens: &Tokens, cx: &mut Context<FeaturePage>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens.space[4]))
        .px(px(tokens.space[8]))
        .py(px(tokens.space[6]))
        .child(page_header(page, t("common.unavailable_hint"), tokens))
        .child(action_button(
            t("workbench.title"),
            FeatureAction::Navigate("workbench".into()),
            cx,
        ))
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

fn routine_time_label(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Local).format("%m/%d %H:%M").to_string())
        .unwrap_or_else(|_| t("common.unknown").to_owned())
}

fn routine_trigger(run: &Value) -> &'static str {
    match run.get("trigger").and_then(Value::as_str) {
        Some("schedule") => t("routines.run.scheduled"),
        Some("test") => t("routines.test_run"),
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

fn dashboard_phase_label(phase: &str) -> &'static str {
    match phase {
        "chat" => t("dashboard.phase_chat"),
        "work" => t("dashboard.phase_work"),
        "subagent" => t("dashboard.phase_subagent"),
        "coordinate" => t("dashboard.phase_coordinate"),
        "memory" => t("dashboard.phase_memory"),
        "compact" => t("dashboard.phase_compact"),
        _ => t("dashboard.phase_other"),
    }
}

fn dashboard_bot_label(data: &Value, bot_id: &str) -> String {
    for key in ["all_bots", "bots"] {
        if let Some(bot) = array(data, key)
            .into_iter()
            .find(|bot| bot.get("id").and_then(Value::as_str) == Some(bot_id))
        {
            return string(
                &bot,
                "name",
                &string(&bot, "label", &string(&bot, "display_name", bot_id)),
            );
        }
    }
    bot_id.to_owned()
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
        "{current:.0}% · {} {}{:.0}pt",
        t("dashboard.kpi.vs_prev"),
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
        "{value} · {} {}{:.0}%",
        t("dashboard.kpi.vs_prev"),
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
    fn sparse_heatmap_keeps_nonzero_days_distinct_from_empty_days() {
        let zero: Hsla = gpui_kit::rgb(0xefeff1).into();
        let activity = Tokens::activity_colors();
        let tied = vec![json!(203858), json!(203858), json!(203858)];
        assert_eq!(heatmap_palette_color(0., &tied, zero, &activity), zero);
        let active = heatmap_palette_color(203858., &tied, zero, &activity);
        assert_eq!(active, activity[0]);
        assert_ne!(active, zero);
        assert_eq!(heatmap_palette_color(1., &[], zero, &activity), active);
    }

    #[test]
    fn heatmap_quantiles_use_four_increasing_intensities_of_one_activity_color() {
        let zero: Hsla = gpui_kit::rgb(0x2c2c2e).into();
        let activity = Tokens::activity_colors();
        let thresholds = vec![json!(10), json!(20), json!(30)];
        for (value, expected) in [10., 20., 30., 31.].into_iter().zip(activity) {
            let color = heatmap_palette_color(value, &thresholds, zero, &activity);
            assert_eq!(color, expected);
            assert_ne!(color, zero);
            assert_eq!(color.h, activity[0].h);
            assert_eq!(color.s, activity[0].s);
        }
        assert!(activity.windows(2).all(|pair| pair[0].a < pair[1].a));
        assert_eq!(activity[3].a, 1.);
        assert_eq!(
            heatmap_palette_color(0., &thresholds, zero, &activity),
            zero
        );
    }

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
    fn search_target_bot_uses_optional_chat_hint() {
        assert_eq!(
            search_target(&json!({"id":"bot_1", "chat_id":"chat_1"}), "bot"),
            Some("search_bot/bot_1?chat_id=chat_1".into())
        );
        assert_eq!(
            search_target(&json!({"id":"bot_1"}), "bot"),
            Some("search_bot/bot_1".into())
        );
        assert_eq!(
            search_target(&json!({"id":"bot_1", "chat_id":""}), "bot"),
            Some("search_bot/bot_1".into())
        );
        assert_eq!(search_target(&json!({"chat_id":"chat_1"}), "bot"), None);
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
