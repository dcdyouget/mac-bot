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
    scroll::ScrollableElement,
    text::TextView,
    *,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use macbot_client_core::{AppState, Client, ClientConfig, ClientEvent, TraceTimeline};
use macbot_client_core::{ScreenClient, ScreenEvent, ScreenHandle};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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

fn play_bubble_sound() {
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|executable| {
            executable
                .parent()
                .map(|path| path.join("../Resources/Sounds/BubblePop.m4a"))
        })
        .filter(|path| path.is_file());
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/Sounds/BubblePop.m4a");
    let Some(sound) = bundled.or_else(|| source.is_file().then_some(source)) else {
        return;
    };
    let _ = std::process::Command::new("/usr/bin/afplay")
        .arg(sound)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn sync_window_material(window: &mut Window) {
    let appearance = if crate::local_settings::translucent()
        && !crate::local_settings::reduce_transparency_enabled()
    {
        gpui_kit::gpui::WindowBackgroundAppearance::Blurred
    } else {
        gpui_kit::gpui::WindowBackgroundAppearance::Opaque
    };
    window.set_background_appearance(appearance);
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
        CycleFocus,
        CycleFocusBack,
        FocusPrevMessage,
        FocusNextMessage,
        FocusedMessageReply,
        FocusedMessageReference,
        QuestionOption1,
        QuestionOption2,
        QuestionOption3,
        QuestionOption4,
        QuestionOption5,
        QuestionOption6,
        QuestionOption7,
        QuestionOption8,
        QuestionOption9,
        QuestionPrev,
        QuestionNext,
        QuestionOther,
        FocusPending,
        SendMessage,
        EditAnnouncement,
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
    announcement_draft: Entity<TextareaState>,
    pending_messages: BTreeMap<String, Value>,
    message_list_cache: chat::MessageListCache,
    question_inputs: BTreeMap<String, Entity<InputState>>,
    question_selections: BTreeMap<String, std::collections::BTreeSet<usize>>,
    image_errors: BTreeMap<String, String>,
    message_following: bool,
    focus: FocusHandle,
    focus_region: usize,
    focus_message_index: Option<usize>,
    active_question_id: Option<String>,
    expanded_bot_dm_refs: BTreeSet<String>,
    pending_action_handles: BTreeMap<String, FocusHandle>,
    highlighted_message_id: Option<String>,
    pending_chat_jump: Option<(String, String)>,
    focus_messages: FocusHandle,
    focus_context: FocusHandle,
    focus_sidebar: FocusHandle,
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
    quote_message: Option<Value>,
    announcement_conflict: Option<Value>,
    announcement_draft_project: Option<String>,
    announcement_base_version: u64,
    announcement_base_text: String,
    artifact: Value,
    artifact_id: String,
    artifact_project_id: String,
    artifact_versions: Vec<Value>,
    artifact_has_more: bool,
    artifact_versions_loading: bool,
    artifact_selected_version: u64,
    artifact_tab: String,
    artifact_preview_state: crate::artifact_preview::ArtifactPreviewState,
    artifact_content: String,
    artifact_mime: String,
    artifact_image: Option<std::sync::Arc<Image>>,
    home_entries: Vec<Value>,
    home_project_id: String,
    home_path: String,
    home_loading: bool,
    home_error: String,
    steer_inflight: BTreeSet<String>,
    changes_project: Option<String>,
    attachments: Vec<String>,
    message_virtual_scroll: gpui_kit::gpui::ListState,
    image_cache: BTreeMap<String, std::sync::Arc<Image>>,
    image_loading: std::collections::BTreeSet<String>,
    last_notification_sound: Option<std::time::Instant>,
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
        let announcement_draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(tr("context.announcement_placeholder"))
                .auto_grow(3, 10)
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
            announcement_draft,
            pending_messages: BTreeMap::new(),
            message_list_cache: chat::MessageListCache::default(),
            question_inputs: BTreeMap::new(),
            question_selections: BTreeMap::new(),
            image_errors: BTreeMap::new(),
            message_following: true,
            focus: cx.focus_handle(),
            focus_region: 0,
            focus_message_index: None,
            active_question_id: None,
            expanded_bot_dm_refs: BTreeSet::new(),
            pending_action_handles: BTreeMap::new(),
            highlighted_message_id: None,
            pending_chat_jump: None,
            focus_messages: cx.focus_handle(),
            focus_context: cx.focus_handle(),
            focus_sidebar: cx.focus_handle(),
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
            quote_message: None,
            announcement_conflict: None,
            announcement_draft_project: None,
            announcement_base_version: 0,
            announcement_base_text: String::new(),
            artifact: Value::Null,
            artifact_id: String::new(),
            artifact_project_id: String::new(),
            artifact_versions: Vec::new(),
            artifact_has_more: false,
            artifact_versions_loading: false,
            artifact_selected_version: 0,
            artifact_tab: "preview".into(),
            artifact_preview_state: crate::artifact_preview::ArtifactPreviewState::Loading,
            artifact_content: String::new(),
            artifact_mime: String::new(),
            artifact_image: None,
            home_entries: Vec::new(),
            home_project_id: String::new(),
            home_path: String::new(),
            home_loading: false,
            home_error: String::new(),
            steer_inflight: BTreeSet::new(),
            changes_project: None,
            attachments: vec![],
            message_virtual_scroll: gpui_kit::gpui::ListState::new(
                0,
                gpui_kit::gpui::ListAlignment::Top,
                px(300.),
            ),
            image_cache: BTreeMap::new(),
            image_loading: std::collections::BTreeSet::new(),
            last_notification_sound: None,
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
        sync_window_material(window);
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
                            view.rpc_failed(
                                &method,
                                &params_copy,
                                error.to_string(),
                                error.details().cloned(),
                                cx,
                            );
                        }
                        Err(error) => {
                            view.rpc_failed(&method, &params_copy, error.to_string(), None, cx);
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
                if s(params, "chat_id") == self.selected_chat
                    && let Some((target_chat, target_id)) = self.pending_chat_jump.clone()
                    && target_chat == self.selected_chat
                {
                    let target_message = self.state.messages.values().find(|message| {
                        s(message, "chat_id") == target_chat
                            && (s(message, "id") == target_id
                                || arr(message, "blocks").iter().any(|block| {
                                    s(block, "type") == "approval"
                                        && s(block, "approval_id") == target_id
                                }))
                    });
                    if let Some(message_id) =
                        target_message.map(|message| s(message, "id").to_owned())
                    {
                        self.pending_chat_jump = None;
                        self.jump_to_message(&message_id, cx);
                    }
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
            "announcement.update" => {
                if let Some(announcement) = value.get("announcement") {
                    insert(&mut self.state.announcements, announcement, "project_id");
                    self.announcement_base_text = s(announcement, "text").to_owned();
                    self.announcement_base_version = announcement
                        .get("version")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.announcement_base_version);
                }
                self.notice = tr("notice.announcement_saved").to_string();
                self.announcement_conflict = None;
            }
            "question.answer" => {
                if let Some(question) = value.get("question") {
                    let question_id = s(question, "id").to_owned();
                    insert(&mut self.state.questions, question, "id");
                    self.question_selections.remove(&question_id);
                    self.advance_question_after_answer(&question_id, cx);
                }
            }
            "artifact.versions.list" => {
                if s(params, "artifact_id") == self.artifact_id {
                    let incoming = arr(&value, "versions");
                    if params.get("before_version").is_some() {
                        for version in incoming {
                            let version_number = version.get("version").and_then(Value::as_u64);
                            if !self.artifact_versions.iter().any(|known| {
                                known.get("version").and_then(Value::as_u64) == version_number
                            }) {
                                self.artifact_versions.push(version.clone());
                            }
                        }
                    } else {
                        self.artifact_versions = incoming.to_vec();
                    }
                    self.artifact_versions.sort_by_key(|version| {
                        std::cmp::Reverse(
                            version
                                .get("version")
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                        )
                    });
                    self.artifact_has_more = value["has_more"] == true;
                    self.artifact_versions_loading = false;
                    if self.artifact_selected_version == 0 {
                        if let Some(number) = self
                            .artifact_versions
                            .first()
                            .and_then(|version| version.get("version"))
                            .and_then(Value::as_u64)
                        {
                            self.load_artifact_version(number, cx);
                        } else {
                            self.artifact_preview_state =
                                crate::artifact_preview::ArtifactPreviewState::Unsupported(tr(
                                    "artifact.preview.no_history",
                                ));
                        }
                    }
                }
            }
            "artifact.version.get" => {
                if s(params, "artifact_id") == self.artifact_id
                    && params.get("version").and_then(Value::as_u64)
                        == Some(self.artifact_selected_version)
                {
                    let version = value.get("version").cloned().unwrap_or(Value::Null);
                    self.artifact_mime = s(&version, "mime").to_owned();
                    match s(&value, "content_encoding") {
                        "utf8" if is_text_artifact(&version) => {
                            self.artifact_content = s(&value, "content").to_owned();
                            self.artifact_image = None;
                            self.artifact_preview_state =
                                crate::artifact_preview::ArtifactPreviewState::Ready;
                        }
                        "base64" if self.artifact_mime.starts_with("image/") => {
                            match decode_base64(s(&value, "content")) {
                                Ok(bytes) => {
                                    let image_format = match self.artifact_mime.as_str() {
                                        "image/png" => Some(ImageFormat::Png),
                                        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
                                        _ => None,
                                    };
                                    if let Some(format) = image_format {
                                        self.artifact_image = Some(std::sync::Arc::new(
                                            Image::from_bytes(format, bytes),
                                        ));
                                        self.artifact_preview_state =
                                            crate::artifact_preview::ArtifactPreviewState::Ready;
                                    } else {
                                        self.artifact_preview_state =
                                            crate::artifact_preview::ArtifactPreviewState::Unsupported(
                                                tr("artifact.preview.image_format_unsupported"),
                                            );
                                    }
                                }
                                Err(()) => {
                                    self.artifact_preview_state =
                                        crate::artifact_preview::ArtifactPreviewState::Error(tr(
                                            "artifact.preview.error_detail",
                                        ));
                                }
                            }
                        }
                        _ => {
                            self.artifact_image = None;
                            self.artifact_preview_state =
                                crate::artifact_preview::ArtifactPreviewState::Unsupported(tr(
                                    "artifact.preview.unsupported_detail",
                                ));
                        }
                    }
                }
            }
            "message.steer" => {
                if let Some(assignment) = value.get("assignment") {
                    insert(&mut self.state.assignments, assignment, "id");
                }
                self.steer_inflight.remove(&format!(
                    "{}:{}",
                    s(params, "message_id"),
                    s(params, "bot_id")
                ));
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
        self.focus_composer(cx);
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
        self.message_following = true;
        self.focus_message_index = None;
        self.selected_chat = id.clone();
        self.page = "chat".into();
        self.context.clear();
        self.quote_message = None;
        self.attachments.clear();
        let draft = self.drafts.get(&id).cloned().unwrap_or_default();
        self.composer
            .update(cx, |input, cx| input.set_value(draft, window, cx));
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
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
    fn notify_message(&mut self, message: &Value, cx: &App) {
        if !self.local_settings.notifications || s(&message["sender"], "kind") == "user" {
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
        let preferences = crate::local_settings::notification_preferences().unwrap_or_default();
        let blocks = arr(message, "blocks");
        let needs_you = blocks.iter().any(|block| {
            matches!(
                s(block, "type"),
                "question" | "approval" | "takeover_request" | "blocked" | "loop_paused"
            )
        }) || arr(message, "delivery")
            .iter()
            .any(|delivery| s(delivery, "state") == "queued");
        let review = blocks
            .iter()
            .any(|block| s(block, "type") == "review_card" && s(block, "state") == "pending");
        let is_group = s(chat, "kind") == "project";
        let is_dm = matches!(s(chat, "kind"), "direct" | "bot_dm" | "main");
        let mentions_user = arr(message, "mentions")
            .iter()
            .any(|mention| s(mention, "kind") == "user");
        let enabled = (needs_you && preferences.need_you)
            || (review && preferences.review)
            || (is_dm && mentions_user && preferences.dm_mention)
            || (is_group && preferences.all_group);
        if !enabled {
            return;
        }
        let app_is_active = cx.active_window().is_some();
        let current_chat_is_open =
            app_is_active && self.page == "chat" && self.selected_chat == chat_id;
        if current_chat_is_open {
            return;
        }
        if crate::local_settings::sound_enabled()
            && self
                .last_notification_sound
                .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(1))
        {
            play_bubble_sound();
            self.last_notification_sound = Some(std::time::Instant::now());
        }
        if app_is_active {
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
        let origin_chat = self
            .trace_target
            .get("assignment_id")
            .and_then(Value::as_str)
            .and_then(|assignment_id| self.state.assignments.get(assignment_id))
            .map(|assignment| s(assignment, "origin_chat_id").to_owned())
            .or_else(|| {
                self.trace_target
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let mut params = self.trace_target.clone();
        params["tail"] = json!(true);
        self.rpc("trace.history", params, cx);
        if let Some(chat_id) = origin_chat.filter(|chat_id| !chat_id.is_empty()) {
            self.rpc("chat.history", json!({"chat_id":chat_id,"limit":100}), cx);
        }
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
    fn focus_composer(&self, cx: &mut Context<Self>) {
        let composer = self.composer.clone();
        let handle = cx.active_window();
        cx.defer(move |cx| {
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, cx| {
                    composer.update(cx, |input, cx| input.focus(window, cx));
                });
            }
        });
    }
    fn cycle_focus(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != "chat" {
            return;
        }
        self.focus_region = if self
            .composer
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            0
        } else if self.focus_messages.contains_focused(window, cx) {
            1
        } else if self.focus_context.contains_focused(window, cx) {
            2
        } else if self.focus_sidebar.contains_focused(window, cx) {
            3
        } else {
            self.focus_region
        };
        let mut index = self.focus_region;
        for _ in 0..4 {
            index = if reverse {
                (index + 3) % 4
            } else {
                (index + 1) % 4
            };
            match index {
                0 => {
                    self.composer
                        .update(cx, |input, cx| input.focus(window, cx));
                }
                1 => self.focus_messages.focus(window, cx),
                2 => {
                    self.context_visible = true;
                    self.focus_context.focus(window, cx);
                }
                _ => {
                    self.sidebar_visible = true;
                    self.focus_sidebar.focus(window, cx);
                }
            }
            self.focus_region = index;
            cx.notify();
            return;
        }
    }
    fn move_message_focus(&mut self, backwards: bool, window: &Window, cx: &mut Context<Self>) {
        if self.page != "chat" || !self.focus_messages.contains_focused(window, cx) {
            return;
        }
        let count = self.message_list_cache.len();
        if count == 0 {
            self.focus_message_index = None;
            return;
        }
        let current =
            self.focus_message_index
                .unwrap_or(if backwards { count } else { usize::MAX });
        let next = if backwards {
            current.saturating_sub(1).min(count - 1)
        } else if current == usize::MAX {
            0
        } else {
            (current + 1).min(count - 1)
        };
        self.focus_message_index = Some(next);
        self.message_virtual_scroll.scroll_to_reveal_item(next);
        cx.notify();
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

    pub(super) fn open_artifact_preview(
        &mut self,
        artifact: Value,
        project_id: String,
        cx: &mut Context<Self>,
    ) {
        let id = s(&artifact, "artifact_id");
        let id = if id.is_empty() {
            s(&artifact, "id")
        } else {
            id
        };
        self.artifact_id = id.to_owned();
        self.artifact_project_id = project_id;
        self.artifact = artifact;
        self.artifact_versions.clear();
        self.artifact_has_more = false;
        self.artifact_versions_loading = false;
        self.artifact_selected_version = 0;
        self.artifact_tab = "preview".into();
        self.artifact_content.clear();
        self.artifact_mime.clear();
        self.artifact_image = None;
        self.context.clear();
        self.page = "artifact".into();

        match s(&self.artifact, "kind") {
            "url" => {
                self.artifact_preview_state =
                    crate::artifact_preview::ArtifactPreviewState::Unsupported(tr(
                        "artifact.preview.url_external",
                    ));
            }
            "dir" => {
                self.artifact_preview_state =
                    crate::artifact_preview::ArtifactPreviewState::Unsupported(tr(
                        "artifact.preview.remote_home_only",
                    ));
            }
            _ if self.artifact_id.is_empty() => {
                self.artifact_preview_state =
                    crate::artifact_preview::ArtifactPreviewState::Unsupported(tr(
                        "artifact.preview.unsupported_detail",
                    ));
            }
            _ if !self.connected || self.fixture => {
                self.artifact_preview_state =
                    crate::artifact_preview::ArtifactPreviewState::Error(tr("error.offline"));
            }
            _ => {
                self.artifact_preview_state =
                    crate::artifact_preview::ArtifactPreviewState::Loading;
                self.artifact_versions_loading = true;
                self.rpc(
                    "artifact.versions.list",
                    json!({"artifact_id":self.artifact_id,"limit":20}),
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn load_artifact_version(&mut self, version: u64, cx: &mut Context<Self>) {
        if self.artifact_id.is_empty() || version == 0 {
            return;
        }
        self.artifact_selected_version = version;
        self.artifact_content.clear();
        self.artifact_mime.clear();
        self.artifact_image = None;
        self.artifact_preview_state = crate::artifact_preview::ArtifactPreviewState::Loading;
        self.rpc(
            "artifact.version.get",
            json!({"artifact_id":self.artifact_id,"version":version}),
            cx,
        );
        cx.notify();
    }

    fn load_more_artifact_versions(&mut self, cx: &mut Context<Self>) {
        let Some(before_version) = self
            .artifact_versions
            .last()
            .and_then(|version| version.get("version"))
            .and_then(Value::as_u64)
        else {
            return;
        };
        if self.artifact_id.is_empty() || self.artifact_versions_loading {
            return;
        }
        self.artifact_versions_loading = true;
        self.rpc(
            "artifact.versions.list",
            json!({
                "artifact_id":self.artifact_id,
                "limit":20,
                "before_version":before_version,
            }),
            cx,
        );
    }

    fn artifact_preview_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::artifact_preview::{
            ArtifactPreview, ArtifactPreviewFormat, ArtifactPreviewState,
        };

        let tokens = Tokens::get(cx);
        let title = self
            .artifact_versions
            .iter()
            .find(|version| {
                version.get("version").and_then(Value::as_u64)
                    == Some(self.artifact_selected_version)
            })
            .map(|version| s(version, "title"))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| s(&self.artifact, "title"))
            .to_owned();
        let path = self
            .artifact_versions
            .iter()
            .find(|version| {
                version.get("version").and_then(Value::as_u64)
                    == Some(self.artifact_selected_version)
            })
            .map(|version| s(version, "path_or_url"))
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| s(&self.artifact, "path_or_url"))
            .to_owned();
        let updated_at = self
            .artifact_versions
            .iter()
            .find(|version| {
                version.get("version").and_then(Value::as_u64)
                    == Some(self.artifact_selected_version)
            })
            .map(|version| s(version, "created_at"))
            .unwrap_or("")
            .to_owned();
        let author_id = self
            .artifact_versions
            .iter()
            .find(|version| {
                version.get("version").and_then(Value::as_u64)
                    == Some(self.artifact_selected_version)
            })
            .map(|version| s(version, "bot_id"))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| s(&self.artifact, "bot_id"));
        let author = self
            .state
            .bots
            .get(author_id)
            .map(|bot| s(bot, "name"))
            .unwrap_or("")
            .to_owned();

        let tabs = [
            ("preview", "artifact.preview.tabs.preview"),
            ("source", "artifact.preview.tabs.source"),
            ("history", "artifact.preview.tabs.history"),
        ];
        let mut tab_bar = div().flex().items_center().gap_2();
        for (tab, key) in tabs {
            let selected = self.artifact_tab == tab;
            tab_bar = tab_bar.child(
                Button::new(SharedString::from(format!("artifact-tab-{tab}")))
                    .when(selected, |button| button.primary())
                    .when(!selected, |button| button.ghost())
                    .small()
                    .label(tr(key))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.artifact_tab = tab.into();
                        cx.notify();
                    })),
            );
        }

        let history = if self.artifact_tab == "history" {
            let mut list = div()
                .w(px(240.))
                .flex_none()
                .border_r_1()
                .border_color(tokens.border_subtle)
                .p_3()
                .flex()
                .flex_col()
                .gap_1();
            if self.artifact_versions.is_empty() && !self.artifact_versions_loading {
                list = list.child(tr("artifact.preview.no_history"));
            }
            for version in &self.artifact_versions {
                let number = version.get("version").and_then(Value::as_u64).unwrap_or(0);
                let time = s(version, "created_at").to_owned();
                let label = tr("artifact.preview.version")
                    .to_string()
                    .replace("{n}", &number.to_string());
                let selected = number == self.artifact_selected_version;
                list = list.child(
                    Button::new(SharedString::from(format!("artifact-version-{number}")))
                        .when(selected, |button| button.primary())
                        .when(!selected, |button| button.ghost())
                        .small()
                        .label(format!("{label} · {time}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_artifact_version(number, cx);
                        })),
                );
            }
            if self.artifact_has_more {
                list = list.child(
                    Button::new("artifact-history-more")
                        .ghost()
                        .small()
                        .label(tr("artifact.preview.load_more"))
                        .disabled(self.artifact_versions_loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_more_artifact_versions(cx);
                        })),
                );
            }
            if self.artifact_versions_loading {
                list = list.child(tr("artifact.preview.loading"));
            }
            Some(list.into_any_element())
        } else {
            None
        };

        let selected_version = self.artifact_selected_version;
        let version_for_meta = self
            .artifact_versions
            .iter()
            .find(|version| {
                version.get("version").and_then(Value::as_u64) == Some(selected_version)
            })
            .and_then(|version| version.get("version"))
            .and_then(Value::as_u64);

        let preview = if self.artifact_image.is_some() && self.artifact_tab == "preview" {
            let image = self.artifact_image.clone().expect("image checked");
            let mut header = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .p_4()
                .border_b_1()
                .border_color(tokens.border_subtle)
                .bg(tokens.panel)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_lg().child(title.clone()))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .text_xs()
                                .text_color(tokens.secondary)
                                .child(format!("{} {}", author, tr("artifact.provided_by")))
                                .child(div().font_family(tokens.font_mono).child(path.clone()))
                                .child(updated_at.clone())
                                .when_some(version_for_meta, |row, number| {
                                    row.child(
                                        tr("artifact.preview.version")
                                            .to_string()
                                            .replace("{n}", &number.to_string()),
                                    )
                                }),
                        ),
                );
            let path_copy = path.clone();
            header = header.child(
                Button::new("artifact-image-copy-path")
                    .secondary()
                    .small()
                    .label(tr("artifact.preview.copy_path"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path_copy.clone()))
                    }),
            );
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(tokens.chat)
                .child(header)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .p(px(tokens.space[9]))
                        .flex()
                        .justify_center()
                        .child(
                            img(image)
                                .max_w(px(720.))
                                .max_h(px(560.))
                                .object_fit(ObjectFit::Contain),
                        ),
                )
                .into_any_element()
        } else {
            let state = if self.artifact_tab == "source" && self.artifact_image.is_some() {
                ArtifactPreviewState::Unsupported(tr("artifact.preview.source_unsupported"))
            } else {
                self.artifact_preview_state.clone()
            };
            let format = if self.artifact_tab == "source" {
                ArtifactPreviewFormat::PlainText
            } else if self.artifact_mime == "text/markdown"
                || path.ends_with(".md")
                || path.ends_with(".markdown")
            {
                ArtifactPreviewFormat::Markdown
            } else {
                ArtifactPreviewFormat::PlainText
            };
            let copy_text = self.artifact_content.clone();
            let copy_path = path.clone();
            let mut component = ArtifactPreview::new(title.clone(), path.clone())
                .author(author)
                .updated_at(updated_at)
                .content(self.artifact_content.clone(), format)
                .state(state)
                .on_copy_text(move |text, _, cx| {
                    let value = if text.is_empty() {
                        copy_text.clone()
                    } else {
                        text.to_string()
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                })
                .on_copy_path(move |path, _, cx| {
                    let value = if path.is_empty() {
                        copy_path.clone()
                    } else {
                        path.to_string()
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                });
            if let Some(version) = version_for_meta {
                component = component.version(version);
            }
            component.into_any_element()
        };

        let mut center = div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .border_b_1()
                    .border_color(tokens.border_subtle)
                    .bg(tokens.panel)
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                                format!(
                                    "{} › {} › {}",
                                    s(&self.artifact, "project_name"),
                                    tr("group.artifacts"),
                                    title
                                ),
                            ))
                            .child(
                                div()
                                    .font_family(tokens.font_mono)
                                    .text_xs()
                                    .text_color(tokens.secondary)
                                    .child(path.clone()),
                            ),
                    )
                    .child(tab_bar),
            );
        if let Some(history) = history {
            center = center.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(history)
                    .child(preview),
            );
        } else {
            center = center.child(div().flex_1().min_h_0().child(preview));
        }

        if s(&self.artifact, "kind") == "dir" {
            center = center.child(
                div()
                    .px_4()
                    .py_2()
                    .text_xs()
                    .text_color(tokens.secondary)
                    .child(tr("artifact.preview.remote_home_only")),
            );
        }
        center.into_any_element()
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let mut list = div()
            .id("chat-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(t.space[5]))
            .py(px(t.space[4]))
            .flex()
            .flex_col()
            .gap(px(t.space[2]));
        for chat in self.state.chats.values().filter(|v| s(v, "kind") == "main") {
            list = list.child(self.chat_row(chat, cx));
        }
        list = list.child(
            div()
                .mt(px(t.space[6]))
                .mb(px(t.space[2]))
                .px(px(t.space[4]))
                .text_size(px(t.desktop_type[1][0]))
                .text_color(t.secondary)
                .child(
                    tr("sidebar.section.groups").to_string().replace(
                        "{count}",
                        &self
                            .state
                            .chats
                            .values()
                            .filter(|chat| s(chat, "kind") == "project")
                            .count()
                            .to_string(),
                    ),
                ),
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
                    .px(px(t.space[4]))
                    .py(px(t.space[4]))
                    .text_size(px(t.desktop_type[1][0]))
                    .text_color(t.secondary)
                    .child(tr("sidebar.empty_groups")),
            );
        }
        if done > 0 {
            list = list.child(
                Button::new("completed")
                    .ghost()
                    .small()
                    .label(format!("{} {done}", tr("sidebar.completed")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_completed = !this.show_completed;
                        cx.notify();
                    })),
            );
        }
        list = list.child(
            div()
                .mt(px(t.space[6]))
                .mb(px(t.space[2]))
                .px(px(t.space[4]))
                .text_size(px(t.desktop_type[1][0]))
                .text_color(t.secondary)
                .child(
                    tr("sidebar.section.bots").to_string().replace(
                        "{count}",
                        &self
                            .state
                            .bots
                            .values()
                            .filter(|bot| bot["is_main"] != true && bot["hidden"] != true)
                            .count()
                            .to_string(),
                    ),
                ),
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
                    .label(
                        tr("sidebar.hidden_bots")
                            .to_string()
                            .replace("{count}", &hidden.to_string()),
                    )
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
            .gap(px(t.space[2]))
            .p(px(t.space[5]))
            .border_t_1()
            .border_color(t.border);
        for (key, title_key, icon) in [
            ("workbench", "nav.workbench", IconName::LayoutDashboard),
            ("dashboard", "nav.dashboard", IconName::LayoutDashboard),
            ("routines", "nav.routines", IconName::Calendar),
            ("skills", "nav.skills", IconName::Star),
            ("settings", "nav.settings", IconName::Settings),
        ] {
            let page = key.to_string();
            footer = footer.child(
                Button::new(SharedString::from(format!("nav-{key}")))
                    .ghost()
                    .icon(icon)
                    .label(tr(title_key))
                    .on_click(cx.listener(move |this, _, _, cx| this.navigate(&page, cx))),
            );
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(t.sidebar)
            .track_focus(&self.focus_sidebar)
            .child(
                div()
                    .p(px(t.space[5]))
                    .flex()
                    .items_center()
                    .gap(px(t.space[4]))
                    .child(
                        div()
                            .size(px(t.space[4]))
                            .rounded_full()
                            .bg(if self.connected { t.success } else { t.warning }),
                    )
                    .child(
                        Button::new("host-switch")
                            .ghost()
                            .label(host)
                            .tooltip(tr("host.switch.title"))
                            .on_click(cx.listener(|this, _, _, cx| this.navigate("connect", cx))),
                    )
                    .child(
                        Button::new("new")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip(tr("common.new"))
                            .on_click(cx.listener(|this, _, _, cx| this.navigate("new", cx))),
                    ),
            )
            .child(
                Button::new("search")
                    .ghost()
                    .icon(IconName::Search)
                    .label(tr("common.search"))
                    .on_click(cx.listener(|this, _, _, cx| this.navigate("search", cx))),
            )
            .child(list)
            .child(footer)
            .into_any_element()
    }
    fn chat_row(&self, chat: &Value, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let layout = t.desktop_layout;
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
            .rounded(px(t.radius[8]))
            .bg(if selected { t.selected } else { t.sidebar })
            .p(px(t.space[2]))
            .child(
                Button::new(SharedString::from(id.clone()))
                    .ghost()
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space[5]))
                            .w_full()
                            .child(bean(bot.unwrap_or(&Value::Null), layout.avatar_list, cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap(px(t.space[2]))
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(t.desktop_type[3][0]))
                                            .child(title + &badge),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(t.desktop_type[1][0]))
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
    fn connection_page(&self, window_width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let stacked = window_width < px(1120.);
        let field = |key: &str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(t.space[4]))
                .child(div().text_size(px(t.desktop_type[3][0])).child(tr(key)))
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
                    .when(stacked, |el| el.flex_col())
                    .gap(px(t.space[9]))
                    .max_w(px(840.))
                    .w_full()
                    .p(px(t.space[9]))
                    .child(
                        div()
                            .when(stacked, |el| el.w_full())
                            .when(!stacked, |el| el.w(px(380.)))
                            .flex()
                            .flex_col()
                            .gap(px(t.space[7]))
                            .child(
                                div()
                                    .text_size(px(t.desktop_type[7][0]))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr("onboarding.welcome")),
                            )
                            .child(
                                div()
                                    .text_color(t.secondary)
                                    .child(tr("onboarding.connect_subtitle")),
                            )
                            .child(field("host.field.address", &self.address))
                            .child(
                                div()
                                    .text_size(px(t.desktop_type[1][0]))
                                    .text_color(t.secondary)
                                    .child(tr("host.field.address_hint")),
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
                                            .gap(px(t.space[2]))
                                            .child(tr("host.switch.title"))
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
                            .child(field("host.field.password", &self.password))
                            .child(field("host.field.name", &self.host_name))
                            .child(
                                Button::new("connect")
                                    .primary()
                                    .label(tr(if self.connecting {
                                        "connect.connecting"
                                    } else {
                                        "host.connect"
                                    }))
                                    .loading(self.connecting)
                                    .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.secondary)
                                    .child(tr("host.manage")),
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
                                            .gap(px(t.space[4]))
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
                                    .label(tr("host.add"))
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
                            .when(stacked, |el| el.w_full())
                            .when(!stacked, |el| el.w(px(300.)))
                            .flex()
                            .flex_col()
                            .gap(px(t.space[7]))
                            .pt(px(t.space[9]))
                            .child(bean(&json!({"avatar":{"color":0},"is_main":true}), 64., cx))
                            .child(
                                div()
                                    .text_size(px(t.desktop_type[6][0]))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr("onboarding.meet_main")),
                            )
                            .child(tr("onboarding.meet_main_desc"))
                            .child(
                                div()
                                    .mt(px(t.space[8]))
                                    .text_color(t.secondary)
                                    .child(tr("onboarding.footer")),
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
        let layout = t.desktop_layout;
        let window_width = window.bounds().size.width;
        let context_page = self.page == "chat" || self.context.last().is_some_and(|v| v == "trace");
        let context_requested = self.context_visible && context_page;
        let context_overlay = context_requested && window_width < px(1100.);
        let context_inline = context_requested && !context_overlay;
        let sidebar_overlay = self.sidebar_visible && window_width < px(820.);
        let sidebar_inline = self.sidebar_visible && !sidebar_overlay;
        let center = if self.page == "computer" {
            self.computer.clone().into_any_element()
        } else if self.page == "connect" {
            self.connection_page(window_width, cx)
        } else if self.page == "chat" {
            self.chat_page(window, cx)
        } else if self.page == "artifact" {
            self.artifact_preview_page(cx)
        } else {
            self.feature_center(window, cx)
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.window)
            .text_color(t.primary)
            .text_size(px(t.desktop_type[3][0]))
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
            .on_action(
                cx.listener(|this, _: &CycleFocus, window, cx| this.cycle_focus(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CycleFocusBack, window, cx| {
                this.cycle_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPending, window, cx| {
                if this.page == "chat" {
                    this.focus_message_index = None;
                    this.focus_latest_pending_action(window, cx);
                    if this.focus_message_index.is_none() {
                        this.focus_messages.focus(window, cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPrevMessage, window, cx| {
                this.move_message_focus(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusNextMessage, window, cx| {
                this.move_message_focus(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusedMessageReply, window, cx| {
                this.quote_focused_message("reply", window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusedMessageReference, window, cx| {
                    this.quote_focused_message("reference", window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &QuestionOption1, window, cx| {
                this.question_shortcut("option", Some(0), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption2, window, cx| {
                this.question_shortcut("option", Some(1), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption3, window, cx| {
                this.question_shortcut("option", Some(2), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption4, window, cx| {
                this.question_shortcut("option", Some(3), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption5, window, cx| {
                this.question_shortcut("option", Some(4), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption6, window, cx| {
                this.question_shortcut("option", Some(5), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption7, window, cx| {
                this.question_shortcut("option", Some(6), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption8, window, cx| {
                this.question_shortcut("option", Some(7), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOption9, window, cx| {
                this.question_shortcut("option", Some(8), window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionPrev, window, cx| {
                this.question_shortcut("prev", None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionNext, window, cx| {
                this.question_shortcut("next", None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuestionOther, window, cx| {
                this.question_shortcut("other", None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SendMessage, window, cx| {
                if this.page == "chat" {
                    this.send_message(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &EditAnnouncement, window, cx| {
                let project_id = this
                    .state
                    .chats
                    .get(&this.selected_chat)
                    .map(|chat| s(chat, "project_id"))
                    .unwrap_or("")
                    .to_owned();
                if project_id.is_empty() {
                    return;
                }
                if let Some(announcement) = this.state.announcements.get(&project_id) {
                    let draft = s(announcement, "text").to_owned();
                    this.announcement_base_text = draft.clone();
                    this.announcement_base_version = announcement
                        .get("version")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    this.announcement_draft_project = Some(project_id.clone());
                    this.announcement_draft.update(cx, |input, cx| {
                        input.set_value(draft, window, cx);
                        input.focus(window, cx);
                    });
                }
                this.context.clear();
                this.context_visible = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Back, window, cx| {
                let width = window.bounds().size.width;
                if width < px(1100.) && this.context_visible {
                    this.context_visible = false;
                    this.close_trace(cx);
                    cx.notify();
                } else if width < px(820.) && this.sidebar_visible {
                    this.sidebar_visible = false;
                    cx.notify();
                } else {
                    this.back(window, cx);
                }
            }))
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
            .when(
                !self.connected && self.client.is_some() && self.notice.is_empty(),
                |el| {
                    el.child(
                        div()
                            .px(px(t.space[6]))
                            .py(px(t.space[2]))
                            .bg(t.warning_soft)
                            .text_size(px(t.desktop_type[1][0]))
                            .text_color(t.warning)
                            .child(tr("status.disconnected")),
                    )
                },
            )
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
                h_resizable(if context_inline {
                    "main-panes-three"
                } else {
                    "main-panes-two"
                })
                .when(sidebar_inline, |el| {
                    el.child(
                        resizable_panel()
                            .size(px(layout.sidebar_width))
                            .flex_none()
                            .size_range(px(layout.sidebar_min)..px(layout.sidebar_max))
                            .child(self.sidebar(cx)),
                    )
                })
                .child(
                    resizable_panel()
                        .size_range(px(layout.chat_min)..px(2000.))
                        .child(center),
                )
                .when(context_inline, |el| {
                    el.child(
                        resizable_panel()
                            .size(px(layout.panel_width))
                            .flex_none()
                            .size_range(px(layout.panel_min)..px(layout.panel_max))
                            .child(self.context_panel(window, cx)),
                    )
                }),
            )
            .when(sidebar_overlay || context_overlay, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(t.space[0]))
                        .right(px(t.space[0]))
                        .bottom(px(t.space[0]))
                        .left(px(t.space[0]))
                        .bg(t.overlay)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.sidebar_visible = false;
                                this.context_visible = false;
                                cx.notify();
                            }),
                        ),
                )
            })
            .when(context_overlay, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(t.space[0]))
                        .right(px(t.space[0]))
                        .bottom(px(t.space[0]))
                        .w(px(layout.panel_width))
                        .shadow(vec![
                            BoxShadow::new(px(0.), px(t.elevation_offset_y[3]), t.shadow)
                                .blur_radius(px(t.elevation_blur[3]))
                                .spread_radius(px(t.elevation_outline[3])),
                        ])
                        .bg(t.panel)
                        .border_l_1()
                        .border_color(t.border)
                        .child(self.context_panel(window, cx)),
                )
            })
            .when(sidebar_overlay, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(t.space[0]))
                        .left(px(t.space[0]))
                        .bottom(px(t.space[0]))
                        .w(px(layout.sidebar_width))
                        .shadow(vec![
                            BoxShadow::new(px(0.), px(t.elevation_offset_y[3]), t.shadow)
                                .blur_radius(px(t.elevation_blur[3]))
                                .spread_radius(px(t.elevation_outline[3])),
                        ])
                        .bg(t.sidebar)
                        .border_r_1()
                        .border_color(t.border)
                        .child(self.sidebar(cx)),
                )
            })
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

fn is_text_artifact(version: &Value) -> bool {
    let mime = s(version, "mime").to_ascii_lowercase();
    mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-sh"
                | "application/yaml"
        )
}

fn decode_base64(input: &str) -> Result<Vec<u8>, ()> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    let mut padding = false;

    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            padding = true;
            continue;
        }
        if padding {
            return Err(());
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(()),
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    if bits == 6 || (padding && bits == 0 && input.trim_end().ends_with('=')) {
        return Err(());
    }
    Ok(output)
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
    let emoji = s(avatar, "kind") == "emoji";
    let badge_size = size * 0.4;
    let body = div()
        .size(px(size))
        .rounded_full()
        .relative()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.bean(color))
        .shadow(vec![
            BoxShadow::new(px(0.), px(0.), rgba(0x00000014).into())
                .blur_radius(px(2.))
                .inset(),
        ]);
    let body = if emoji {
        body.text_size(px(size * 0.55))
            .text_color(t.primary)
            .child(s(avatar, "emoji").to_string())
    } else {
        body.child(
            div()
                .absolute()
                .top(px(size * 0.4))
                .left(px(size * 0.32))
                .size(px(size * 0.12))
                .h(px(size * 0.18))
                .rounded_full()
                .bg(t.avatar_eye),
        )
        .child(
            div()
                .absolute()
                .top(px(size * 0.4))
                .left(px(size * 0.56))
                .size(px(size * 0.12))
                .h(px(size * 0.18))
                .rounded_full()
                .bg(t.avatar_eye),
        )
    };
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
            .child(
                div()
                    .absolute()
                    .right(px(0.))
                    .bottom(px(0.))
                    .size(px(badge_size))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .child(
                                div()
                                    .absolute()
                                    .left(px(badge_size * 0.24))
                                    .top(px(badge_size * 0.37))
                                    .w(px(badge_size * 0.25))
                                    .h(px(badge_size * 0.22))
                                    .rounded(px(badge_size * 0.07))
                                    .border(px(badge_size * 0.05))
                                    .border_color(t.avatar_glasses)
                                    .bg(rgba(0xffffff38)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(badge_size * 0.51))
                                    .top(px(badge_size * 0.37))
                                    .w(px(badge_size * 0.25))
                                    .h(px(badge_size * 0.22))
                                    .rounded(px(badge_size * 0.07))
                                    .border(px(badge_size * 0.05))
                                    .border_color(t.avatar_glasses)
                                    .bg(rgba(0xffffff38)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(badge_size * 0.49))
                                    .top(px(badge_size * 0.44))
                                    .w(px(badge_size * 0.03))
                                    .h(px(badge_size * 0.05))
                                    .bg(t.avatar_glasses),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(badge_size * 0.12))
                                    .top(px(badge_size * 0.41))
                                    .w(px(badge_size * 0.12))
                                    .h(px(badge_size * 0.05))
                                    .bg(t.avatar_glasses),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(badge_size * 0.76))
                                    .top(px(badge_size * 0.41))
                                    .w(px(badge_size * 0.12))
                                    .h(px(badge_size * 0.05))
                                    .bg(t.avatar_glasses),
                            ),
                    ),
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
