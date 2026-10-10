use crate::computer::{Computer, ComputerAction};
use crate::host_storage::HostStore;
use crate::{
    i18n::{state, tr},
    tokens::Tokens,
};
use gpui_kit::component::{
    button::*,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    resizable::*,
    text::TextView,
    *,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use macbot_client_core::{AppState, Client, ClientConfig, ClientEvent, TraceTimeline};
use macbot_client_core::{ScreenClient, ScreenEvent, ScreenHandle};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn diagnostic_error_class(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("broken pipe") || error.contains("os error 32") {
        "broken_pipe"
    } else if error.contains("connection reset") || error.contains("os error 54") {
        "connection_reset"
    } else if error.contains("timed out") || error.contains("timeout") {
        "timeout"
    } else if error.contains("event gap") || error.contains("resync") {
        "event_gap"
    } else if error.contains("closed") {
        "closed"
    } else if error.contains("websocket") {
        "websocket"
    } else if error.is_empty() {
        "none"
    } else {
        "other"
    }
}

#[path = "chat.rs"]
mod chat;
#[path = "shell_features.rs"]
mod shell_features;

gpui_kit::actions!(
    macbot,
    [
        Quit,
        New,
        Search,
        Settings,
        MainBot,
        Workbench,
        Dashboard,
        Skills,
        ToggleSidebar,
        ToggleContext,
        Back,
        Chat1,
        Chat2,
        Chat3,
        Chat4,
        Chat5,
        Chat6,
        Chat7,
        Chat8,
        Chat9
    ]
);

pub struct MacBot {
    local_settings: crate::local_settings::LocalSettings,
    feature_view: Entity<crate::features::FeaturePage>,
    settings_view: Entity<crate::settings_view::SettingsView>,
    trace_view: Entity<crate::trace_view::TraceView>,
    feature_data: Value,
    editor_reload: bool,
    feature_route: String,
    connection_generation: u64,
    search_open_epoch: u64,
    trace_epoch: u64,
    last_cache: std::time::Instant,
    _update_task: Option<Task<()>>,
    update_release: Option<crate::update::Release>,
    update_stage: Option<crate::update::StagedUpdate>,
    computer: Entity<Computer>,
    screen_client: Option<ScreenClient>,
    screen_task: Option<Task<()>>,
    screen_start_task: Option<Task<()>>,
    screen_notice: crate::screen_notice::ScreenNotice,
    screen_bot: String,
    active_endpoint: Option<String>,
    active_node_id: Option<String>,
    active_device_id: Option<String>,
    hosts: Option<HostStore>,
    active_host: Option<String>,
    runtime: tokio::runtime::Runtime,
    client: Option<Client>,
    event_task: Option<Task<()>>,
    state: AppState,
    address: Entity<InputState>,
    password: Entity<InputState>,
    host_name: Entity<InputState>,
    composer: Entity<TextareaState>,
    pending_messages: BTreeMap<String, Value>,
    message_list_cache: chat::MessageListCache,
    question_inputs: BTreeMap<String, Entity<InputState>>,
    image_errors: BTreeMap<String, String>,
    message_following: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    resync_requested: bool,
    connected: bool,
    connecting: bool,
    fixture: bool,
    selected_chat: String,
    page: String,
    context: Vec<String>,
    sidebar_visible: bool,
    context_visible: bool,
    show_completed: bool,
    show_hidden: bool,
    notice: String,
    drafts: BTreeMap<String, String>,
    routines: Vec<Value>,
    templates: Vec<Value>,
    timeline: TraceTimeline,
    trace_target: Value,
    trace_stream: Option<String>,
    thread: Option<Value>,
    reply_to: Option<String>,
    changes_project: Option<String>,
    attachments: Vec<String>,
    message_virtual_scroll: gpui_kit::gpui::ListState,
    image_cache: BTreeMap<String, std::sync::Arc<Image>>,
    image_loading: std::collections::BTreeSet<String>,
}
impl MacBot {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1:7788"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let host_name = cx.new(|cx| InputState::new(window, cx).placeholder("Mac mini"));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(tr("chat.placeholder"))
                .auto_grow(1, 6)
                .submit_on_enter(true)
        });
        let computer = cx.new(|cx| Computer::new(window, cx));
        let feature_view =
            cx.new(|cx| crate::features::FeaturePage::new("workbench", json!({}), window, cx));
        let settings_view = cx.new(|cx| crate::settings_view::SettingsView::new(window, cx));
        let trace_view = cx.new(|cx| crate::trace_view::TraceView::new(window, cx));
        let mut subscriptions =
            vec![
                cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = event {
                        this.send_message(window, cx);
                    }
                    if matches!(event, InputEvent::Change) {
                        this.drafts.insert(
                            this.selected_chat.clone(),
                            this.composer.read(cx).value().to_string(),
                        );
                        cx.notify();
                    }
                }),
            ];
        subscriptions.push(
            cx.subscribe(&computer, |this, _, event: &ComputerAction, cx| {
                this.computer_action(event, cx)
            }),
        );
        subscriptions.push(
            cx.subscribe_in(&feature_view, window, |this, _, event, window, cx| {
                this.feature_action(event, window, cx)
            }),
        );
        subscriptions.push(cx.subscribe_in(
            &settings_view,
            window,
            |this, _, event, window, cx| this.settings_action(event, window, cx),
        ));
        subscriptions.push(
            cx.subscribe_in(&trace_view, window, |this, _, event, window, cx| {
                this.trace_action(event, window, cx)
            }),
        );
        let mut view = Self {
            local_settings: crate::local_settings::load().unwrap_or_default(),
            feature_view,
            settings_view,
            trace_view,
            feature_data: json!({}),
            editor_reload: false,
            feature_route: String::new(),
            connection_generation: 0,
            search_open_epoch: 0,
            trace_epoch: 0,
            last_cache: std::time::Instant::now(),
            _update_task: None,
            update_release: None,
            update_stage: None,
            computer,
            screen_client: None,
            screen_task: None,
            screen_start_task: None,
            screen_notice: crate::screen_notice::ScreenNotice::default(),
            screen_bot: String::new(),
            active_endpoint: None,
            active_node_id: None,
            active_device_id: None,
            hosts: HostStore::load().ok(),
            active_host: None,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("Tokio runtime"),
            client: None,
            event_task: None,
            state: AppState::default(),
            address,
            password,
            host_name,
            composer,
            pending_messages: BTreeMap::new(),
            message_list_cache: chat::MessageListCache::default(),
            question_inputs: BTreeMap::new(),
            image_errors: BTreeMap::new(),
            message_following: true,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
            resync_requested: false,
            connected: false,
            connecting: false,
            fixture: false,
            selected_chat: String::new(),
            page: "connect".into(),
            context: vec![],
            sidebar_visible: true,
            context_visible: true,
            show_completed: false,
            show_hidden: false,
            notice: String::new(),
            drafts: BTreeMap::new(),
            routines: vec![],
            templates: vec![],
            timeline: TraceTimeline::default(),
            trace_target: Value::Null,
            trace_stream: None,
            thread: None,
            reply_to: None,
            changes_project: None,
            attachments: vec![],
            message_virtual_scroll: gpui_kit::gpui::ListState::new(
                0,
                gpui_kit::gpui::ListAlignment::Top,
                px(300.),
            ),
            image_cache: BTreeMap::new(),
            image_loading: std::collections::BTreeSet::new(),
        };
        if std::env::var_os("MACBOT_HOST").is_none()
            && let Some(host) = view
                .hosts
                .as_ref()
                .and_then(|store| store.hosts().first().cloned())
        {
            view.activate_host(&host.id, window, cx);
        }
        if let Ok(endpoint) = std::env::var("MACBOT_HOST") {
            view.address
                .update(cx, |s, cx| s.set_value(endpoint, window, cx));
            let pw = std::env::var("MACBOT_PASSWORD").unwrap_or_default();
            view.password
                .update(cx, |s, cx| s.set_value(pw, window, cx));
            view.connect(cx);
        }
        if std::env::var_os("MACBOT_FIXTURES").is_some() {
            view.load_fixtures(cx);
        }
        match view.local_settings.theme.as_str() {
            "light" => Theme::change(ThemeMode::Light, Some(window), cx),
            "dark" => Theme::change(ThemeMode::Dark, Some(window), cx),
            _ => Theme::sync_system_appearance(Some(window), cx),
        }
        crate::tokens::sync_theme(cx);
        view.focus.focus(window, cx);
        if std::env::var_os("MACBOT_UPDATE_URL").is_some()
            || crate::update::load_update_url().ok().flatten().is_some()
        {
            view.check_update(cx);
            let executor = cx.background_executor().clone();
            view._update_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    executor
                        .timer(std::time::Duration::from_secs(24 * 60 * 60))
                        .await;
                    if this.update(cx, |view, cx| view.check_update(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        let entity = cx.entity().downgrade();
        let handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            if let Some(chat) = response.tag.strip_prefix("macbot.chat.") {
                let chat = chat.to_string();
                let _ = handle.update(cx, |_, window, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |view, cx| view.select_chat(chat, window, cx));
                        window.activate_window();
                    }
                });
                cx.activate(true);
            }
        });
        view
    }
    fn cancel_search_open(&mut self) {
        self.search_open_epoch = self.search_open_epoch.wrapping_add(1);
        if self.notice == tr("search.opening_bot").as_ref() {
            self.notice.clear();
        }
    }
    fn connect(&mut self, cx: &mut Context<Self>) {
        self.cancel_search_open();
        let endpoint = self.address.read(cx).value().to_string();
        let password = self.password.read(cx).value().to_string();
        if endpoint.trim().is_empty() {
            self.notice = tr("error.empty").to_string();
            cx.notify();
            return;
        }
        self.connection_generation = self.connection_generation.wrapping_add(1);
        let generation = self.connection_generation;
        self.close_trace(cx);
        self.close_screen();
        if let Some(client) = self.client.take() {
            let _guard = self.runtime.enter();
            self.runtime.spawn(async move {
                client.close().await;
            });
        }
        self.connected = false;
        self.connecting = true;
        self.active_endpoint = None;
        self.active_node_id = None;
        self.active_device_id = None;
        self.fixture = false;
        self.notice.clear();
        let addresses: Vec<String> = endpoint
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let mut config =
            ClientConfig::new(addresses.first().cloned().unwrap_or_default(), password);
        self.active_host = self.hosts.as_ref().and_then(|store| {
            store
                .hosts()
                .iter()
                .find(|host| host.addresses == addresses)
                .map(|host| host.id.clone())
        });
        config.addresses = addresses;
        if let Some(store) = &self.hosts {
            config.device_id = store.device_id().into();
            if let Some(id) = &self.active_host
                && let Some(host) = store.get(id)
            {
                config.node_id = host.node_id.clone();
            }
        }
        self.active_node_id = config.node_id.clone();
        self.active_device_id = Some(config.device_id.clone());
        self.state.messages.clear();
        self.state.assignments.clear();
        self.state.announcements.clear();
        self.state.approvals.clear();
        self.state.questions.clear();
        self.state = AppState::default();
        self.selected_chat.clear();
        self.context.clear();
        if let Some(record) = self
            .active_host
            .as_ref()
            .and_then(|id| self.hosts.as_ref()?.get(id))
            && let (Some(node), Ok(root)) = (&record.node_id, crate::state_cache::default_root())
            && let Ok(Some(cache)) = crate::state_cache::load(root, &record.id, node)
        {
            config.last_seq = cache.last_seq;
            config.has_cached_state = !cache.state.needs_resync;
            config.node_id = Some(cache.node_id);
            self.active_host = Some(cache.host_id);
            self.state = cache.state;
        }
        self.pending_messages = self
            .active_host
            .as_ref()
            .zip(config.node_id.as_ref())
            .and_then(|(id, node)| crate::outbox::load(id, node).ok())
            .unwrap_or_default();
        for message in self.pending_messages.values_mut() {
            message["in_flight"] = json!(false);
        }
        let _guard = self.runtime.enter();
        let handle = Client::spawn(config);
        self.client = Some(handle.client);
        let mut events = handle.events;
        let executor = cx.background_executor().clone();
        self.event_task = Some(cx.spawn(async move |this, cx| {
            loop {
                for _ in 0..64 {
                    let event = match events.try_recv() {
                        Ok(event) => event,
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
                    };
                    if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
                        eprintln!("client: event bridge received");
                    }
                    if this
                        .update(cx, |view, cx| {
                            if view.connection_generation == generation {
                                view.on_event(event, cx);
                            }
                        })
                        .is_err()
                    {
                        if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
                            eprintln!("client: event bridge entity released");
                        }
                        return;
                    }
                    // Main websocket events mutate the root view from a GPUI
                    // async task. Refresh explicitly so a parked native loop
                    // does not wait for unrelated input to present the change.
                    cx.refresh();
                }
                executor.timer(std::time::Duration::from_millis(50)).await;
            }
        }));
        cx.notify();
    }
    fn rpc(&mut self, method: &str, mut params: Value, cx: &mut Context<Self>) {
        if method == "search" {
            self.cancel_search_open();
        }
        let Some(client) = self.client.as_ref().cloned() else {
            self.notice = tr("error.offline").to_string();
            cx.notify();
            return;
        };
        if method == "chat.send" {
            if params.get("client_request_id").is_none() {
                params["client_request_id"] = json!(uuid::Uuid::new_v4().to_string());
            }
            let id = s(&params, "client_request_id").to_string();
            self.pending_messages
                .insert(id, chat::pending_message(&params));
            self.persist_outbox();
        }
        let generation = self.connection_generation;
        let trace_epoch = self.trace_epoch;
        let method = method.to_owned();
        let params_copy = params.clone();
        let receiver = {
            let _guard = self.runtime.enter();
            client.try_request(method.clone(), params)
        };
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            if this
                .update(cx, |view, cx| {
                    if view.connection_generation != generation {
                        return;
                    }
                    if matches!(method.as_str(), "trace.history" | "trace.subscribe")
                        && view.trace_epoch != trace_epoch
                    {
                        if method == "trace.subscribe"
                            && let Ok(Ok(value)) = &result
                        {
                            view.rpc("trace.unsubscribe", json!({"stream":value["stream"]}), cx);
                        }
                        return;
                    }
                    match result {
                        Ok(Ok(value)) => view.rpc_result(&method, &params_copy, value, cx),
                        Ok(Err(error)) => {
                            view.rpc_failed(&method, &params_copy, error.to_string(), cx);
                        }
                        Err(error) => {
                            view.rpc_failed(&method, &params_copy, error.to_string(), cx);
                        }
                    }
                    cx.notify();
                })
                .is_ok()
            {
                cx.refresh();
            }
        })
        .detach();
    }
    fn on_event(&mut self, event: ClientEvent, cx: &mut Context<Self>) {
        if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
            match &event {
                ClientEvent::Connected { resumed, .. } => {
                    eprintln!("client: connected resumed={resumed}")
                }
                ClientEvent::Bootstrap(value) => eprintln!(
                    "client: bootstrap bots={} chats={} seq={}",
                    arr(value, "bots").len(),
                    arr(value, "chats").len(),
                    value["seq"]
                ),
                ClientEvent::Disconnected { error } => eprintln!(
                    "client: disconnected error_class={}",
                    error
                        .as_deref()
                        .map(diagnostic_error_class)
                        .unwrap_or("none")
                ),
                ClientEvent::TransportError(_) => eprintln!("client: transport error"),
                _ => {}
            }
        }
        let connection_changed = matches!(
            &event,
            ClientEvent::Connected { .. }
                | ClientEvent::Bootstrap(_)
                | ClientEvent::Disconnected { .. }
        );
        match event {
            ClientEvent::Connected {
                hello, endpoint, ..
            } => {
                self.state.hello = Some(hello);
                self.active_endpoint = Some(endpoint);
                self.active_node_id = self
                    .state
                    .hello
                    .as_ref()
                    .and_then(|value| value.get("node_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| self.active_node_id.clone());
                self.connected = true;
                self.connecting = false;
                if self.page == "connect" {
                    self.page = "chat".into();
                }
                self.notice.clear();
                self.remember_host(cx);
                self.rpc("bot.templates", json!({}), cx);
                self.refresh(cx);
                self.retry_outbox(cx);
                if self.page == "computer" && !self.screen_bot.is_empty() {
                    self.open_computer(self.screen_bot.clone(), cx);
                }
                if self.selected_chat.is_empty() {
                    self.select_main(cx);
                }
                if self.context.last().is_some_and(|v| v == "trace") {
                    let mut params = self.trace_target.clone();
                    params["tail"] = json!(true);
                    self.rpc("trace.history", params, cx);
                }
            }
            ClientEvent::Bootstrap(value) => {
                self.resync_requested = false;
                self.state.apply_bootstrap(value);
                self.retry_outbox(cx);
                self.restore_after_bootstrap(cx);
            }
            ClientEvent::Protocol(event) => {
                let d = &event.data;
                match event.event.as_str() {
                    "trace.item" => {
                        if self.trace_stream.as_deref() == Some(s(d, "stream")) {
                            self.timeline.apply_item(d["item"].clone());
                            self.trace_view
                                .update(cx, |v, cx| v.update_data(d.clone(), cx));
                        }
                    }
                    "trace.delta" => {
                        if self.trace_stream.as_deref() == Some(s(d, "stream")) {
                            self.timeline.apply_delta(
                                s(d, "request_id"),
                                s(d, "channel"),
                                s(d, "text"),
                            );
                            self.trace_view
                                .update(cx, |v, cx| v.update_data(json!({"delta":d}), cx));
                        }
                    }
                    "trace.tool_output" if self.trace_stream.as_deref() == Some(s(d, "stream")) => {
                        self.trace_view.update(cx, |view, cx| {
                            view.update_data(json!({"tool_output":d}), cx)
                        });
                    }
                    "routine.run"
                        if self.page == "routine"
                            && s(&d["run"], "routine_id") == s(&self.feature_data, "id") =>
                    {
                        self.rpc(
                            "routine.runs",
                            json!({"routine_id":d["run"]["routine_id"]}),
                            cx,
                        );
                    }
                    _ => {}
                }
                let notification = event.event == "message.created"
                    && event.seq.is_some_and(|seq| seq > self.state.last_seq);
                let message = event.data.get("message").cloned();
                self.state.apply_event(event);
                if notification && let Some(message) = message {
                    self.notify_message(&message, cx);
                }
                if self.screen_client.is_some() {
                    self.sync_screen_request(cx);
                }
            }
            ClientEvent::Disconnected { error } => {
                self.connected = false;
                self.connecting = false;
                self.trace_stream = None;
                self.trace_epoch = self.trace_epoch.wrapping_add(1);
                if let Some(e) = error {
                    self.notice = e;
                }
            }
            ClientEvent::TransportError(error) => {
                self.notice = error;
                self.connecting = false;
            }
        }
        if self.state.needs_resync && !self.resync_requested && self.connected {
            self.resync_requested = true;
            self.rpc("bootstrap", json!({}), cx);
        }
        self.sync_views(cx);
        self.persist_cache(false);
        if connection_changed {
            cx.refresh_windows();
        }
        cx.notify();
    }
    fn rpc_result(&mut self, method: &str, params: &Value, value: Value, cx: &mut Context<Self>) {
        let previous_page = self.page.clone();
        self.feature_result(method, params, &value, cx);
        if let Some(message) = value.get("message") {
            insert(&mut self.state.messages, message, "id");
        }
        if let Some(chat) = value.get("chat").or_else(|| value.get("dm_chat")) {
            insert(&mut self.state.chats, chat, "id");
        }
        if let Some(bot) = value.get("bot") {
            insert(&mut self.state.bots, bot, "id");
        }
        if let Some(project) = value.get("project") {
            insert(&mut self.state.projects, project, "id");
        }
        match method {
            "bootstrap" => {
                self.state.apply_bootstrap(value.clone());
                self.resync_requested = false;
                self.restore_after_bootstrap(cx);
            }
            "chat.history" => {
                for v in arr(&value, "messages") {
                    insert(&mut self.state.messages, v, "id");
                }
            }
            "assignment.list" => {
                for v in arr(&value, "items") {
                    insert(&mut self.state.assignments, v, "id");
                }
            }
            "project.get" => {
                if let Some(v) = value.get("announcement") {
                    insert(&mut self.state.announcements, v, "project_id");
                }
            }
            "approval.list" => {
                for v in arr(&value, "approvals") {
                    insert(&mut self.state.approvals, v, "id");
                }
            }
            "routine.list" => {
                self.routines = arr(&value, "routines").to_vec();
            }
            "bot.templates" => self.templates = arr(&value, "templates").to_vec(),
            "chat.thread" => {
                self.thread = Some(value);
                self.context.push("thread".into());
                self.context_visible = true;
            }
            "trace.history" => {
                if params.get("assignment_id") != self.trace_target.get("assignment_id")
                    || params.get("chat_id") != self.trace_target.get("chat_id")
                {
                    return;
                }
                self.timeline.apply_history(&value);
                let assignment = self
                    .state
                    .assignments
                    .get(s(&self.trace_target, "assignment_id"))
                    .cloned();
                let target = self.trace_target.clone();
                self.trace_view.update(cx, |v, cx| {
                    v.set_timeline(&self.timeline, target, assignment, cx)
                });
                if self.timeline.live && self.trace_stream.is_none() {
                    let mut target = self.trace_target.clone();
                    target["since_aseq"] = json!(self.timeline.last_aseq.unwrap_or(0));
                    self.rpc("trace.subscribe", target, cx);
                }
            }
            "trace.subscribe" => {
                if params.get("assignment_id") != self.trace_target.get("assignment_id")
                    || params.get("chat_id") != self.trace_target.get("chat_id")
                    || self.context.last().is_none_or(|v| v != "trace")
                {
                    self.rpc("trace.unsubscribe", json!({"stream":value["stream"]}), cx);
                    return;
                }
                self.trace_stream = Some(s(&value, "stream").into());
                self.trace_view
                    .update(cx, |v, cx| v.update_data(value.clone(), cx));
                for v in arr(&value, "in_flight") {
                    self.timeline
                        .apply_delta(s(v, "request_id"), "text", s(v, "text"));
                    self.timeline
                        .apply_delta(s(v, "request_id"), "thinking", s(v, "thinking"));
                }
            }
            "chat.send" => {
                self.pending_messages.remove(s(params, "client_request_id"));
                self.persist_outbox();
                self.notice = tr("notice.sent").to_string();
            }
            "project.create" => {
                if let Some(chat) = value.get("chat") {
                    self.selected_chat = s(chat, "id").into();
                    self.page = "chat".into();
                }
            }
            _ => {}
        }
        if self.page != previous_page {
            self.focus_root(cx);
        }
        self.sync_views(cx);
        self.persist_cache(false);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rpc("assignment.list", json!({}), cx);
        self.rpc("approval.list", json!({}), cx);
        self.rpc("routine.list", json!({}), cx);
        self.rpc("provider.list", json!({}), cx);
        self.rpc("settings.get", json!({}), cx);
    }
    fn load_fixtures(&mut self, cx: &mut Context<Self>) {
        let root = std::env::var("MACBOT_FIXTURES").unwrap_or_else(|_| {
            let bundled = std::env::current_exe().ok().and_then(|p| {
                p.parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("Resources/fixtures"))
            });
            bundled
                .filter(|p| p.is_dir())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| {
                    format!(
                        "{}/../../../../protocol/fixtures",
                        env!("CARGO_MANIFEST_DIR")
                    )
                })
        });
        let read = |name: &str| {
            std::fs::read_to_string(std::path::Path::new(&root).join(name))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        };
        let mut value = read("bootstrap.json").or_else(|| read("objects/bootstrap.json"));
        if value.is_none()
            && let Some(hello) = read("objects/hello.json")
        {
            let mut bootstrap = json!({"seq":0,"hello":hello,"settings":read("objects/settings.json"),"pending":{}});
            for (kind, key) in [
                ("bot", "bots"),
                ("chat", "chats"),
                ("project", "projects"),
                ("message", "messages"),
                ("assignment", "assignments"),
                ("announcement", "announcements"),
                ("approval", "approvals"),
                ("question", "questions"),
                ("routine", "routines"),
                ("skill", "skills"),
                ("provider", "providers"),
                ("model", "models"),
            ] {
                bootstrap[key] = json!(
                    read(&format!("objects/{kind}.json"))
                        .into_iter()
                        .collect::<Vec<_>>()
                );
            }
            value = Some(bootstrap);
        }
        if let Some(value) = value {
            self.state.apply_bootstrap(value);
            self.fixture = true;
            self.connected = false;
            self.page = "chat".into();
            self.select_main(cx);
            self.notice = tr("status.fixture").to_string();
            self.routines = self.state.routines.values().cloned().collect();
            self.sync_views(cx);
            cx.notify();
        } else {
            self.notice = tr("connect.fixture_missing").to_string();
            cx.notify();
        }
    }
    fn remember_host(&mut self, cx: &mut Context<Self>) {
        let Some(hello) = self.state.hello.as_ref() else {
            return;
        };
        let name = self.host_name.read(cx).value().to_string();
        let name = if name.trim().is_empty() {
            s(hello, "host_name").to_owned()
        } else {
            name
        };
        let addresses: Vec<String> = self
            .address
            .read(cx)
            .value()
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let password = self.password.read(cx).value().to_string();
        let node = Some(s(hello, "node_id").to_owned());
        let seq = self.state.last_seq;
        let generation = self.connection_generation;
        let task = self.runtime.spawn_blocking(move || {
            HostStore::with_store(|store| {
                let record = store.remember(name, addresses, &password, node, seq)?;
                Ok((store.clone(), record.id))
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                if view.connection_generation != generation {
                    return;
                }
                match result {
                    Ok(Ok((store, id))) => {
                        view.hosts = Some(store);
                        view.active_host = Some(id);
                        view.persist_outbox();
                    }
                    Ok(Err(error)) => view.notice = error.to_string(),
                    Err(error) => view.notice = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn remove_host(&mut self, id: String, cx: &mut Context<Self>) {
        let task = self.runtime.spawn_blocking(move || {
            HostStore::with_store(|store| {
                store.remove(&id)?;
                Ok((store.clone(), id))
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok((store, id))) => {
                        view.hosts = Some(store);
                        if view.active_host.as_deref() == Some(&id) {
                            view.active_host = None;
                        }
                    }
                    Ok(Err(error)) => view.notice = error.to_string(),
                    Err(error) => view.notice = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn activate_host(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(record) = self.hosts.as_ref().and_then(|store| store.get(id)).cloned() else {
            return;
        };
        self.connection_generation = self.connection_generation.wrapping_add(1);
        let generation = self.connection_generation;
        self.connected = false;
        self.close_trace(cx);
        self.close_screen();
        let requested = record.id.clone();
        self.active_host = Some(requested.clone());
        self.connecting = true;
        cx.notify();
        let task = self.runtime.spawn_blocking(move || {
            let store = HostStore::load()?;
            let password = store.password(&record)?;
            Ok::<_, anyhow::Error>((record, password))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |view, window, cx| {
                if view.connection_generation != generation
                    || view.active_host.as_deref() != Some(&requested)
                {
                    return;
                }
                match result {
                    Ok(Ok((record, password))) => {
                        view.address.update(cx, |s, cx| {
                            s.set_value(record.addresses.join(", "), window, cx)
                        });
                        view.host_name
                            .update(cx, |s, cx| s.set_value(record.name, window, cx));
                        view.password
                            .update(cx, |s, cx| s.set_value(password, window, cx));
                        view.connect(cx);
                    }
                    Ok(Err(error)) => {
                        view.connecting = false;
                        view.notice = error.to_string();
                    }
                    Err(error) => {
                        view.connecting = false;
                        view.notice = error.to_string();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn select_main(&mut self, cx: &mut Context<Self>) {
        self.cancel_search_open();
        self.close_trace(cx);
        self.close_screen();
        self.context.clear();
        self.focus_root(cx);
        if let Some(chat) = self.state.chats.values().find(|v| s(v, "kind") == "main") {
            let id = s(chat, "id").to_owned();
            self.selected_chat = id.clone();
            self.page = "chat".into();
            if self.connected {
                self.rpc("chat.history", json!({"chat_id":id}), cx);
            }
        }
    }

    fn restore_after_bootstrap(&mut self, cx: &mut Context<Self>) {
        match bootstrap_chat_selection(&self.state.chats, &self.selected_chat, &self.page) {
            Some((_, true)) => {
                self.selected_chat.clear();
                self.select_main(cx);
            }
            Some((chat_id, _)) => {
                self.selected_chat = chat_id.clone();
                self.rpc("chat.history", json!({"chat_id":chat_id}), cx);
            }
            None => self.selected_chat.clear(),
        }
    }
    fn select_chat(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_search_open();
        self.close_trace(cx);
        self.close_screen();
        self.focus.focus(window, cx);
        self.message_following = true;
        self.selected_chat = id.clone();
        self.page = "chat".into();
        self.context.clear();
        self.reply_to = None;
        self.thread = None;
        self.attachments.clear();
        let draft = self.drafts.get(&id).cloned().unwrap_or_default();
        self.composer
            .update(cx, |input, cx| input.set_value(draft, window, cx));
        self.rpc("chat.history", json!({"chat_id":id}), cx);
        if let Some(chat) = self.state.chats.get(&self.selected_chat) {
            let seq = chat["last_seq"].clone();
            let project = chat["project_id"].clone();
            self.rpc(
                "chat.mark_read",
                json!({"chat_id":self.selected_chat,"seq":seq}),
                cx,
            );
            if !project.is_null() {
                self.rpc("project.get", json!({"project_id":project}), cx);
            }
        }
        cx.notify();
    }
    fn open_computer(&mut self, bot: String, cx: &mut Context<Self>) {
        self.open_computer_with_options(bot, false, cx);
    }
    fn reopen_computer(&mut self, bot: String, cx: &mut Context<Self>) {
        self.open_computer_with_options(bot, true, cx);
    }
    fn open_computer_with_options(
        &mut self,
        bot: String,
        preserve_stream: bool,
        cx: &mut Context<Self>,
    ) {
        let preserve_stream =
            preserve_stream && self.screen_client.is_some() && self.screen_bot == bot;
        let preserved_tab = preserve_stream
            .then(|| self.computer.read(cx).active_tab_id())
            .flatten();
        let previous_screen = self.detach_screen();
        self.close_trace(cx);
        self.context.clear();
        self.computer.update(cx, |screen, cx| {
            if preserve_stream {
                screen.prepare_reconnect(cx);
            } else {
                screen.reset_connection(cx);
            }
        });
        self.screen_bot = bot.clone();
        self.sync_screen_request(cx);
        self.page = "computer".into();
        let addresses = self
            .address
            .read(cx)
            .value()
            .split(',')
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let endpoint = self
            .active_endpoint
            .clone()
            .or_else(|| addresses.first().cloned())
            .unwrap_or_default();
        let mut config = ClientConfig::new(endpoint, self.password.read(cx).value().to_string());
        config.addresses = addresses;
        config.node_id = self.active_node_id.clone();
        if let Some(device_id) = self.active_device_id.clone() {
            config.device_id = device_id;
        }
        let quality = self.computer.read(cx).quality().to_string();
        let generation = self.screen_notice.generation();
        if let Some(previous_screen) = previous_screen {
            self.screen_start_task = Some(cx.spawn(async move |this, cx| {
                let _ = previous_screen.close().await;
                let _ = this.update(cx, |view, cx| {
                    if !view.screen_notice.is_current(generation, &view.page) {
                        return;
                    }
                    view.start_screen_stream(config, bot, quality, preserved_tab, cx);
                });
            }));
        } else {
            self.start_screen_stream(config, bot, quality, preserved_tab, cx);
        }
        cx.notify();
    }
    fn start_screen_stream(
        &mut self,
        config: ClientConfig,
        bot: String,
        quality: String,
        preserved_tab: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let _guard = self.runtime.enter();
        let diagnostics = std::env::var_os("MACBOT_DIAGNOSTICS").is_some();
        let handle = ScreenHandle::spawn(config, bot, quality, preserved_tab);
        self.screen_client = Some(handle.client);
        let generation = self.screen_notice.generation();
        let mut events = handle.events;
        let executor = cx.background_executor().clone();
        if diagnostics {
            eprintln!(
                "client: screen bridge started generation={} page={}",
                generation, self.page
            );
        }
        self.screen_task = Some(cx.spawn(async move |this, cx| {
            loop {
                for _ in 0..64 {
                    let event = match events.try_recv() {
                        Ok(event) => event,
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            if diagnostics {
                                eprintln!("client: screen bridge event channel disconnected");
                            }
                            return;
                        }
                    };
                    if diagnostics {
                        let kind = match &event {
                            ScreenEvent::State(_) => "state",
                            ScreenEvent::Frame(_) => "frame",
                            ScreenEvent::Error(_) => "error",
                            ScreenEvent::Closed => "closed",
                        };
                        eprintln!("client: screen bridge received {kind}");
                    }
                    let applied = match this.update(cx, |view, cx| {
                            if !view.screen_notice.is_current(generation, &view.page) {
                                if diagnostics {
                                    eprintln!(
                                        "client: screen bridge ignored stale event generation={} current={} page={}",
                                        generation,
                                        view.screen_notice.generation(),
                                        view.page
                                    );
                                }
                                return false;
                            }
                            match event {
                                ScreenEvent::State(mut state) => {
                                    if let Some(bot) = view.state.bots.get(&view.screen_bot) {
                                        state["title"] = json!(format!(
                                            "{}{}",
                                            s(bot, "name"),
                                            tr("computer.browser")
                                        ));
                                    }
                                    view.computer
                                        .update(cx, |screen, cx| screen.set_state_in(state, cx));
                                    if diagnostics {
                                        eprintln!("client: screen bridge applied state");
                                    }
                                }
                                ScreenEvent::Frame(frame) => {
                                    let seq = frame.header.seq;
                                    view.computer.update(cx, |screen, cx| {
                                        screen.set_frame(
                                            seq,
                                            frame.header.w,
                                            frame.header.h,
                                            frame.jpeg,
                                            cx,
                                        )
                                    });
                                    if diagnostics {
                                        eprintln!("client: screen bridge applied frame seq={seq}");
                                    }
                                }
                                ScreenEvent::Error(_) => view
                                    .screen_notice
                                    .record(generation, crate::screen_notice::ScreenStatus::Error),
                                ScreenEvent::Closed => view
                                    .screen_notice
                                    .record(generation, crate::screen_notice::ScreenStatus::Closed),
                            }
                            cx.notify();
                            true
                        }) {
                        Ok(applied) => applied,
                        Err(_) => {
                            if diagnostics {
                                eprintln!("client: screen bridge entity released");
                            }
                            return;
                        }
                    };
                    if applied {
                        // The stream is consumed from a GPUI async task. Explicitly
                        // flush the window after applying a frame/state so a parked
                        // native event loop cannot wait for an unrelated input event.
                        cx.refresh();
                    }
                }
                executor.timer(std::time::Duration::from_millis(50)).await;
            }
        }));
        cx.notify();
    }
    fn notify_message(&self, message: &Value, cx: &App) {
        if !self.local_settings.notifications
            || cx.active_window().is_some()
            || s(&message["sender"], "kind") == "user"
        {
            return;
        }
        let chat_id = s(message, "chat_id");
        let Some(chat) = self.state.chats.get(chat_id) else {
            return;
        };
        if chat["muted"] == true {
            return;
        }
        if let Some(bot) = self.state.bots.get(s(&message["sender"], "bot_id"))
            && bot["notifications"] == false
        {
            return;
        }
        let body = if s(message, "fallback_text").is_empty() {
            s(message, "text")
        } else {
            s(message, "fallback_text")
        };
        cx.show_system_notification(SystemNotification {
            tag: format!("macbot.chat.{chat_id}").into(),
            title: s(chat, "title").to_owned().into(),
            body: body.chars().take(120).collect::<String>().into(),
            actions: vec![],
        });
    }
    fn sync_screen_request(&mut self, cx: &mut Context<Self>) {
        let reason = self
            .state
            .messages
            .values()
            .rev()
            .flat_map(|message| arr(message, "blocks"))
            .find(|block| {
                s(block, "type") == "takeover_request"
                    && s(block, "bot_id") == self.screen_bot
                    && s(block, "state") != "done"
            })
            .map(|block| s(block, "reason").to_owned());
        self.computer
            .update(cx, |screen, cx| screen.set_request_reason(reason, cx));
    }
    fn detach_screen(&mut self) -> Option<ScreenClient> {
        self.screen_notice.close();
        self.screen_task = None;
        self.screen_start_task = None;
        self.screen_client.take()
    }
    fn close_screen(&mut self) {
        if let Some(screen) = self.detach_screen() {
            self.runtime.spawn(async move {
                let _ = screen.close().await;
            });
        }
    }
    fn computer_action(&mut self, event: &ComputerAction, cx: &mut Context<Self>) {
        if self.page != "computer" {
            return;
        }
        match event {
            ComputerAction::Close => {
                self.close_screen();
                self.page = "chat".into();
                cx.notify();
            }
            ComputerAction::Takeover => {
                self.rpc("takeover.start", json!({"bot_id":self.screen_bot}), cx)
            }
            ComputerAction::Release => {
                self.computer
                    .update(cx, |screen, cx| screen.set_request_reason(None, cx));
                self.rpc("takeover.release", json!({"bot_id":self.screen_bot}), cx)
            }
            ComputerAction::Quality(_) => {
                let bot = self.screen_bot.clone();
                self.reopen_computer(bot, cx);
            }
            _ => {
                if let Some(screen) = self.screen_client.as_ref().cloned() {
                    let event = event.clone();
                    self.runtime.spawn(async move {
                        match event {
                            ComputerAction::Rendered(seq) => {
                                if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
                                    eprintln!("client: screen ack enqueue seq={seq}");
                                }
                                match screen.ack(seq).await {
                                    Ok(()) => {
                                        if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
                                            eprintln!("client: screen ack queued seq={seq}");
                                        }
                                    }
                                    Err(error) => {
                                        if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
                                            eprintln!(
                                                "client: screen ack queue error seq={} class={}",
                                                seq,
                                                diagnostic_error_class(&error.to_string())
                                            );
                                        }
                                    }
                                }
                            }
                            ComputerAction::SwitchTab(tab) => {
                                let _ = screen.switch_tab(tab).await;
                            }
                            ComputerAction::Input(value) => {
                                let _ = screen
                                    .input(value.get("event").cloned().unwrap_or(value))
                                    .await;
                            }
                            _ => {}
                        }
                    });
                }
            }
        }
    }
    fn close_trace(&mut self, cx: &mut Context<Self>) {
        self.trace_epoch = self.trace_epoch.wrapping_add(1);
        if let Some(stream) = self.trace_stream.take() {
            self.rpc("trace.unsubscribe", json!({"stream":stream}), cx);
        }
    }
    fn open_trace(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.close_trace(cx);
        self.timeline = TraceTimeline::default();
        self.trace_target = if let Some(id) = id {
            json!({"assignment_id":id})
        } else {
            json!({"chat_id":self.selected_chat})
        };
        let mut params = self.trace_target.clone();
        params["tail"] = json!(true);
        self.rpc("trace.history", params, cx);
        self.context.push("trace".into());
        self.context_visible = true;
        cx.notify();
    }
    fn focus_root(&self, cx: &mut Context<Self>) {
        let focus = self.focus.clone();
        let handle = cx.active_window();
        cx.defer(move |cx| {
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, cx| focus.focus(window, cx));
            }
        });
    }
    fn navigate(&mut self, page: &str, cx: &mut Context<Self>) {
        self.cancel_search_open();
        self.close_trace(cx);
        self.close_screen();
        self.context.clear();
        if let Some(data) = self.feature_data.as_object_mut() {
            data.remove("filter");
        }
        if matches!(
            page,
            "new_bot"
                | "new_group"
                | "new"
                | "skills"
                | "workbench"
                | "dashboard"
                | "search"
                | "routines"
        ) {
            self.feature_data = json!({});
            self.editor_reload = false;
        }
        self.page = page.into();
        self.focus_root(cx);
        self.fetch_page(cx);
        cx.notify();
    }
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_search_open();
        self.close_screen();
        if !self.context.is_empty() {
            self.context.pop();
            self.close_trace(cx);
        } else {
            self.page = if self.client.is_some() || self.fixture {
                "chat"
            } else {
                "connect"
            }
            .into();
        }
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let mut list = div()
            .id("chat-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1();
        for chat in self.state.chats.values().filter(|v| s(v, "kind") == "main") {
            list = list.child(self.chat_row(chat, cx));
        }
        list = list.child(
            div()
                .mt_4()
                .mb_1()
                .px_2()
                .text_xs()
                .text_color(t.secondary)
                .child(tr("nav.groups")),
        );
        let mut groups: Vec<_> = self
            .state
            .chats
            .values()
            .filter(|v| s(v, "kind") == "project")
            .collect();
        groups.sort_by_key(|v| {
            (
                !v["pinned"].as_bool().unwrap_or(false),
                s(v, "updated_at").to_string(),
            )
        });
        let mut done = 0;
        for chat in groups {
            let status = self
                .state
                .projects
                .get(s(chat, "project_id"))
                .map(|v| s(v, "status"))
                .unwrap_or("active");
            if status == "archived" {
                continue;
            }
            if status == "done" {
                done += 1;
                if !self.show_completed {
                    continue;
                }
            }
            list = list.child(self.chat_row(chat, cx));
        }
        if !self.state.chats.values().any(|v| s(v, "kind") == "project") {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_xs()
                    .text_color(t.secondary)
                    .child(tr("nav.empty_group")),
            );
        }
        if done > 0 {
            list = list.child(
                Button::new("completed")
                    .ghost()
                    .small()
                    .label(format!("{} {done}", tr("nav.done")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_completed = !this.show_completed;
                        cx.notify();
                    })),
            );
        }
        list = list.child(
            div()
                .mt_4()
                .mb_1()
                .px_2()
                .text_xs()
                .text_color(t.secondary)
                .child(tr("nav.bots")),
        );
        for bot in self.state.bots.values().filter(|v| v["is_main"] != true) {
            if bot["hidden"] == true && !self.show_hidden {
                continue;
            }
            let id = s(bot, "dm_chat_id");
            if let Some(chat) = self.state.chats.get(id) {
                list = list.child(self.chat_row(chat, cx));
            }
        }
        let hidden = self
            .state
            .bots
            .values()
            .filter(|b| b["hidden"] == true)
            .count();
        if hidden > 0 {
            list = list.child(
                Button::new("hidden")
                    .ghost()
                    .small()
                    .label(format!("{} {hidden}", tr("nav.hidden")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_hidden = !this.show_hidden;
                        cx.notify();
                    })),
            );
        }
        let host = self
            .state
            .hello
            .as_ref()
            .map(|v| s(v, "host_name"))
            .filter(|s| !s.is_empty())
            .unwrap_or("Mac Bot")
            .to_string();
        let mut footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .border_t_1()
            .border_color(t.border);
        for (key, icon) in [
            ("workbench", IconName::LayoutDashboard),
            ("dashboard", IconName::LayoutDashboard),
            ("skills", IconName::Star),
            ("settings", IconName::Settings),
        ] {
            let page = key.to_string();
            footer = footer.child(
                Button::new(SharedString::from(format!("nav-{key}")))
                    .ghost()
                    .icon(icon)
                    .label(tr(&format!("nav.{key}")))
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(&page, cx))),
            );
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.sidebar)
            .child(
                div()
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(7.)).rounded_full().bg(if self.connected {
                        t.success
                    } else {
                        t.attention
                    }))
                    .child(
                        Button::new("host-switch")
                            .ghost()
                            .label(host)
                            .on_click(cx.listener(|this, _, _, cx| this.navigate("connect", cx))),
                    )
                    .child(
                        Button::new("new")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip(tr("nav.new"))
                            .on_click(cx.listener(|this, _, _, cx| this.navigate("new", cx))),
                    ),
            )
            .child(
                Button::new("search")
                    .ghost()
                    .icon(IconName::Search)
                    .label(tr("nav.search"))
                    .on_click(cx.listener(|this, _, _, cx| this.navigate("search", cx))),
            )
            .child(list)
            .child(footer)
            .into_any_element()
    }
    fn chat_row(&self, chat: &Value, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let id = s(chat, "id").to_owned();
        let selected = id == self.selected_chat && self.page == "chat";
        let bot = self.state.bots.get(s(chat, "bot_id"));
        let title = s(chat, "title").to_owned();
        let preview = s(&chat["last_message"], "text").to_owned();
        let badge = if chat["unread"].as_u64().unwrap_or(0) > 0 {
            format!(" · {}", chat["unread"])
        } else {
            String::new()
        };
        div()
            .rounded(px(10.))
            .bg(if selected { t.bot } else { t.sidebar })
            .p_1()
            .child(
                Button::new(SharedString::from(id.clone()))
                    .ghost()
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .w_full()
                            .child(bean(bot.unwrap_or(&Value::Null), 32., cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap_1()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().child(title + &badge))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.secondary)
                                            .truncate()
                                            .child(preview),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_chat(id.clone(), window, cx)
                    })),
            )
            .into_any_element()
    }
    fn connection_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let field = |key: &str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().child(tr(key)))
                .child(Input::new(input))
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.window)
            .child(
                div()
                    .flex()
                    .gap_8()
                    .max_w(px(840.))
                    .p_8()
                    .child(
                        div()
                            .w(px(380.))
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr("connect.welcome")),
                            )
                            .child(div().text_color(t.secondary).child(tr("connect.title")))
                            .child(field("connect.address", &self.address))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.secondary)
                                    .child(tr("connect.hint")),
                            )
                            .when(
                                self.hosts
                                    .as_ref()
                                    .is_some_and(|store| !store.hosts().is_empty()),
                                |el| {
                                    el.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(tr("connect.saved"))
                                            .children(
                                                self.hosts.as_ref().unwrap().hosts().iter().map(
                                                    |host| {
                                                        let id = host.id.clone();
                                                        Button::new(SharedString::from(format!(
                                                            "saved-{id}"
                                                        )))
                                                        .ghost()
                                                        .small()
                                                        .label(host.name.clone())
                                                        .on_click(cx.listener(
                                                            move |this, _, window, cx| {
                                                                this.activate_host(&id, window, cx)
                                                            },
                                                        ))
                                                    },
                                                ),
                                            ),
                                    )
                                },
                            )
                            .child(field("connect.password", &self.password))
                            .child(field("connect.name", &self.host_name))
                            .child(
                                Button::new("connect")
                                    .primary()
                                    .label(tr(if self.connecting {
                                        "connect.connecting"
                                    } else {
                                        "connect.action"
                                    }))
                                    .loading(self.connecting)
                                    .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.secondary)
                                    .child(tr("connect.saved")),
                            )
                            .children(
                                self.hosts
                                    .as_ref()
                                    .into_iter()
                                    .flat_map(|store| store.hosts().iter())
                                    .map(|host| {
                                        let id = host.id.clone();
                                        let remove_id = id.clone();
                                        div()
                                            .flex()
                                            .gap_2()
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "host-{id}"
                                                )))
                                                .ghost()
                                                .label(host.name.clone())
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.activate_host(&id, window, cx)
                                                    },
                                                )),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "remove-host-{remove_id}"
                                                )))
                                                .ghost()
                                                .small()
                                                .label(tr("action.remove"))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.remove_host(remove_id.clone(), cx);
                                                })),
                                            )
                                    }),
                            )
                            .child(
                                Button::new("add-host")
                                    .ghost()
                                    .label(tr("connect.add"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.active_host = None;
                                        this.address
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                        this.password
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                        this.host_name
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("fixtures")
                                    .ghost()
                                    .label(tr("connect.fixture"))
                                    .on_click(cx.listener(|this, _, _, cx| this.load_fixtures(cx))),
                            ),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .flex()
                            .flex_col()
                            .gap_5()
                            .pt_8()
                            .child(bean(&json!({"avatar":{"color":0},"is_main":true}), 64., cx))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr("connect.main")),
                            )
                            .child(tr("connect.description"))
                            .child(
                                div()
                                    .mt_6()
                                    .text_color(t.secondary)
                                    .child(tr("connect.team")),
                            )
                            .children(self.templates.iter().map(|template| {
                                let id = s(template, "id").to_string();
                                Button::new(SharedString::from(id.clone()))
                                    .outline()
                                    .label(s(template, "name").to_string())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.rpc(
                                            "bot.create_from_template",
                                            json!({"template_id":id}),
                                            cx,
                                        )
                                    }))
                            })),
                    ),
            )
            .into_any_element()
    }
}
impl Render for MacBot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::env::var_os("MACBOT_DIAGNOSTICS").is_some() {
            eprintln!(
                "client: render page={} connected={} chats={}",
                self.page,
                self.connected,
                self.state.chats.len()
            );
        }
        let t = Tokens::get(cx);
        let center = if self.page == "computer" {
            self.computer.clone().into_any_element()
        } else if self.page == "connect" {
            self.connection_page(cx)
        } else if self.page == "chat" {
            self.chat_page(window, cx)
        } else {
            self.feature_center(window, cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window)
            .text_color(t.primary)
            .text_size(px(14.))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &New, _, cx| this.navigate("new", cx)))
            .on_action(cx.listener(|this, _: &Search, _, cx| this.navigate("search", cx)))
            .on_action(cx.listener(|this, _: &Settings, _, cx| this.navigate("settings", cx)))
            .on_action(cx.listener(|this, _: &MainBot, _, cx| this.select_main(cx)))
            .on_action(cx.listener(|this, _: &Workbench, _, cx| this.navigate("workbench", cx)))
            .on_action(cx.listener(|this, _: &Dashboard, _, cx| this.navigate("dashboard", cx)))
            .on_action(cx.listener(|this, _: &Skills, _, cx| this.navigate("skills", cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleContext, _, cx| {
                this.context_visible = !this.context_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Back, window, cx| this.back(window, cx)))
            .on_action(cx.listener(|this, _: &Chat1, window, cx| this.select_pinned(0, window, cx)))
            .on_action(cx.listener(|this, _: &Chat2, window, cx| this.select_pinned(1, window, cx)))
            .on_action(cx.listener(|this, _: &Chat3, window, cx| this.select_pinned(2, window, cx)))
            .on_action(cx.listener(|this, _: &Chat4, window, cx| this.select_pinned(3, window, cx)))
            .on_action(cx.listener(|this, _: &Chat5, window, cx| this.select_pinned(4, window, cx)))
            .on_action(cx.listener(|this, _: &Chat6, window, cx| this.select_pinned(5, window, cx)))
            .on_action(cx.listener(|this, _: &Chat7, window, cx| this.select_pinned(6, window, cx)))
            .on_action(cx.listener(|this, _: &Chat8, window, cx| this.select_pinned(7, window, cx)))
            .on_action(cx.listener(|this, _: &Chat9, window, cx| this.select_pinned(8, window, cx)))
            .when(!self.notice.is_empty(), |el| {
                el.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(t.sidebar)
                        .text_xs()
                        .text_color(t.secondary)
                        .child(self.notice.clone()),
                )
            })
            .when_some(self.screen_notice.visible_key(&self.page), |el, key| {
                el.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(t.sidebar)
                        .text_xs()
                        .text_color(t.secondary)
                        .child(tr(key)),
                )
            })
            .when(self.update_release.is_some(), |el| {
                el.child(
                    Button::new("update-action")
                        .primary()
                        .label(tr(if self.update_stage.is_some() {
                            "update.install"
                        } else {
                            "update.download"
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.update_stage.is_some() {
                                this.install_update(cx);
                            } else {
                                this.download_update(cx);
                            }
                        })),
                )
            })
            .child(
                h_resizable(
                    if self.context_visible
                        && (self.page == "chat"
                            || self.context.last().is_some_and(|v| v == "trace"))
                    {
                        "main-panes-three"
                    } else {
                        "main-panes-two"
                    },
                )
                .when(self.sidebar_visible, |el| {
                    el.child(
                        resizable_panel()
                            .size(px(280.))
                            .flex_none()
                            .size_range(px(200.)..px(400.))
                            .child(self.sidebar(cx)),
                    )
                })
                .child(
                    resizable_panel()
                        .size_range(px(480.)..px(2000.))
                        .child(center),
                )
                .when(
                    self.context_visible
                        && (self.page == "chat"
                            || self.context.last().is_some_and(|v| v == "trace")),
                    |el| {
                        el.child(
                            resizable_panel()
                                .size(px(340.))
                                .flex_none()
                                .size_range(px(260.)..px(600.))
                                .child(self.context_panel(window, cx)),
                        )
                    },
                ),
            )
    }
}
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn bootstrap_chat_selection(
    chats: &BTreeMap<String, Value>,
    selected_chat: &str,
    page: &str,
) -> Option<(String, bool)> {
    if !selected_chat.is_empty() && chats.contains_key(selected_chat) {
        return Some((selected_chat.to_owned(), false));
    }
    chats
        .values()
        .find(|chat| s(chat, "kind") == "main")
        .map(|chat| (s(chat, "id").to_owned(), page == "chat"))
}

fn insert(map: &mut BTreeMap<String, Value>, value: &Value, key: &str) {
    let id = s(value, key);
    if !id.is_empty() {
        map.insert(id.to_string(), value.clone());
    }
}

fn bean(bot: &Value, size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let avatar = &bot["avatar"];
    let color = avatar["color"].as_u64().unwrap_or(0);
    let body = div()
        .size(px(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(Tokens::bean(color))
        .text_color(t.user_text)
        .child(if s(avatar, "kind") == "emoji" {
            s(avatar, "emoji").to_string()
        } else {
            "● ●".into()
        });
    div()
        .relative()
        .flex_none()
        .child(body)
        .when(bot["is_main"] == true, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(-8.))
                    .right(px(0.))
                    .text_size(px(14.))
                    .child("♛"),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod bootstrap_navigation_tests {
    use super::bootstrap_chat_selection;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn chats() -> BTreeMap<String, serde_json::Value> {
        BTreeMap::from([
            ("chat_main".into(), json!({"id":"chat_main", "kind":"main"})),
            ("chat_bot".into(), json!({"id":"chat_bot", "kind":"dm"})),
        ])
    }

    #[test]
    fn bootstrap_keeps_valid_chat_without_navigation_on_feature_page() {
        assert_eq!(
            bootstrap_chat_selection(&chats(), "chat_bot", "skills"),
            Some(("chat_bot".into(), false))
        );
    }

    #[test]
    fn bootstrap_replaces_missing_chat_without_leaving_feature_page() {
        assert_eq!(
            bootstrap_chat_selection(&chats(), "stale", "skills"),
            Some(("chat_main".into(), false))
        );
    }

    #[test]
    fn bootstrap_replaces_missing_chat_and_navigates_only_from_chat_page() {
        assert_eq!(
            bootstrap_chat_selection(&chats(), "stale", "chat"),
            Some(("chat_main".into(), true))
        );
    }

    #[test]
    fn bootstrap_without_main_chat_clears_selection() {
        assert_eq!(
            bootstrap_chat_selection(&BTreeMap::new(), "stale", "skills"),
            None
        );
    }
}
