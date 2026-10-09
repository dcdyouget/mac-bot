use super::*;
use crate::features::FeatureAction;
use crate::settings_view::SettingsAction;
use crate::trace_view::TraceAction;
use chrono::{Duration, Utc};

impl MacBot {
    pub(super) fn sync_views(&mut self, cx: &mut Context<Self>) {
        let data = self.page_data();
        self.feature_view
            .update(cx, |v, cx| v.update_data(data, cx));
        let data = json!({"local":self.local_settings,"settings":self.state.settings,"providers":self.state.providers.values().collect::<Vec<_>>(),"models":self.state.models.values().collect::<Vec<_>>()});
        self.settings_view
            .update(cx, |v, cx| v.update_data(data, cx));
    }
    fn page_data(&self) -> Value {
        let mut data = self.feature_data.clone();
        data["all_bots"] = json!(self.state.bots.values().collect::<Vec<_>>());
        if self.page != "workbench" {
            data["bots"] = data["all_bots"].clone();
        }
        data["projects"] = json!(self.state.projects.values().collect::<Vec<_>>());
        data["skills"] = json!(self.state.skills.values().collect::<Vec<_>>());
        data["routines"] = json!(self.routines);
        data["models"] = json!(self.state.models.values().collect::<Vec<_>>());
        if matches!(self.page.as_str(), "bot" | "bot_settings")
            && let Some(bot) = self
                .state
                .chats
                .get(&self.selected_chat)
                .and_then(|chat| self.state.bots.get(s(chat, "bot_id")))
        {
            for (k, v) in bot.as_object().into_iter().flatten() {
                data[k] = v.clone();
            }
            data["selected"] = bot.clone();
        }
        if self.page.starts_with("routine")
            && let Some(chat) = self.state.chats.get(&self.selected_chat)
        {
            data["bot_id"] = chat["bot_id"].clone();
        }
        data
    }
    pub(super) fn feature_center(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.page == "settings" {
            self.settings_view
                .update(cx, |view, cx| view.sync_inputs(window, cx));
            return self.settings_view.clone().into_any_element();
        }
        if self.page == "new" {
            let mut menu = div()
                .size_full()
                .p_6()
                .flex()
                .flex_col()
                .gap_4()
                .child(tr("nav.new"));
            for (id, key) in [
                ("new_bot", "nav.new_bot"),
                ("new_group", "nav.new_group"),
                ("routines", "nav.new_routine"),
            ] {
                let id = id.to_string();
                menu = menu.child(
                    Button::new(SharedString::from(id.clone()))
                        .outline()
                        .label(tr(key))
                        .on_click(cx.listener(move |this, _, _, cx| this.navigate(&id, cx))),
                );
            }
            return menu.into_any_element();
        }
        let page = if self.page == "new_bot" {
            "bot"
        } else {
            self.page.as_str()
        }
        .to_string();
        if self.feature_view.read(cx).page != page {
            let data = self.page_data();
            self.feature_view
                .update(cx, |v, cx| v.set_page(page, data, window, cx));
        }
        if self.editor_reload {
            if let Some(selected) = self
                .feature_data
                .get("selected")
                .filter(|v| v.is_object())
                .cloned()
            {
                self.feature_view
                    .update(cx, |v, cx| v.load_selected(selected, window, cx));
            }
            self.editor_reload = false;
        }
        self.feature_view.clone().into_any_element()
    }
    pub(super) fn fetch_page(&mut self, cx: &mut Context<Self>) {
        if self.fixture {
            return;
        }
        match self.page.split('/').next().unwrap_or("") {
            "settings" => {
                self.rpc("provider.list", json!({}), cx);
                self.rpc("settings.get", json!({}), cx);
            }
            "workbench" => self.rpc("workbench.get", json!({}), cx),
            "skills" | "skill" => self.rpc("skill.list", json!({}), cx),
            "routine" | "routines" => self.rpc("routine.list", json!({}), cx),
            "dashboard" => {
                let now = Utc::now();
                let from = (now - Duration::days(30)).to_rfc3339();
                let to = now.to_rfc3339();
                self.feature_data["from"] = json!(from);
                self.feature_data["to"] = json!(to);
                self.rpc("usage.summary", json!({"from":from,"to":to}), cx);
                self.rpc("usage.heatmap",json!({"mode":"calendar","from":(now-Duration::days(370)).to_rfc3339(),"to":to,"metric":"tokens"}),cx);
                self.rpc("usage.timeseries",json!({"from":from,"to":to,"granularity":"auto","dimension":"model","metric":"tokens","split_io":true}),cx);
                self.rpc(
                    "usage.breakdown",
                    json!({"from":from,"to":to,"dimension":"model"}),
                    cx,
                );
            }
            _ => {}
        }
        self.sync_views(cx);
    }
    pub(super) fn feature_result(
        &mut self,
        method: &str,
        params: &Value,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        match method {
            "workbench.get" => {
                for (k, v) in value.as_object().into_iter().flatten() {
                    self.feature_data[k] = v.clone();
                }
            }
            "usage.summary" => {
                self.feature_data["current"] = value["current"].clone();
                self.feature_data["previous"] = value["previous"].clone();
            }
            "usage.heatmap" => self.feature_data["heatmap"] = value.clone(),
            "usage.timeseries" => self.feature_data["timeseries"] = value.clone(),
            "usage.breakdown" => self.feature_data["breakdown"] = value.clone(),
            "search" => self.feature_data["results"] = value["results"].clone(),
            "provider.list" => {
                self.state.providers.clear();
                for v in arr(value, "providers") {
                    insert(&mut self.state.providers, v, "id");
                }
                self.state.models.clear();
                for v in arr(value, "models") {
                    insert(&mut self.state.models, v, "ref");
                }
            }
            "settings.get" | "settings.update" => {
                self.state.settings = Some(value["settings"].clone())
            }
            "skill.list" => {
                self.state.skills.clear();
                for v in arr(value, "skills") {
                    insert(&mut self.state.skills, v, "name");
                }
            }
            "skill.import" => {
                for skill in arr(value, "skills") {
                    insert(&mut self.state.skills, skill, "name");
                }
                self.page = "skills".into();
            }
            "skill.get" => {
                self.editor_reload = true;
                self.feature_data["selected"] = value["skill"].clone();
                self.feature_data["content"] = value["skill"]["content"].clone();
            }
            "routine.runs" => self.feature_data["runs"] = value["runs"].clone(),
            "provider.test" => {
                self.notice = if value["ok"] == true {
                    format!(
                        "{} · {} ms",
                        tr("settings.connection_ok"),
                        value["latency_ms"]
                    )
                } else {
                    format!(
                        "{}：{}",
                        tr("settings.connection_failed"),
                        s(value, "error")
                    )
                }
            }
            "model.refresh" => {
                for model in arr(value, "models") {
                    insert(&mut self.state.models, model, "ref");
                }
            }
            _ => {}
        }
        for (kind, key) in [
            ("provider", "id"),
            ("model", "ref"),
            ("skill", "name"),
            ("routine", "id"),
        ] {
            if let Some(item) = value.get(kind) {
                match kind {
                    "provider" => insert(&mut self.state.providers, item, key),
                    "model" => insert(&mut self.state.models, item, key),
                    "skill" => insert(&mut self.state.skills, item, key),
                    "routine" => insert(&mut self.state.routines, item, key),
                    _ => {}
                }
            }
        }
        if method.ends_with(".delete") {
            match method {
                "bot.delete" => {
                    if let Some(bot) = self.state.bots.remove(s(params, "bot_id")) {
                        self.state.chats.remove(s(&bot, "dm_chat_id"));
                    }
                    self.select_main(cx);
                }
                "provider.delete" => {
                    self.state.providers.remove(s(params, "provider_id"));
                }
                "model.delete" => {
                    self.state.models.remove(s(params, "ref"));
                }
                "skill.delete" => {
                    self.state.skills.remove(s(params, "name"));
                }
                "routine.delete" => {
                    self.state.routines.remove(s(params, "routine_id"));
                }
                _ => {}
            }
        }
        if method.starts_with("routine.") {
            self.routines = self.state.routines.values().cloned().collect();
        }
        if (method == "bot.create" || method == "bot.create_from_template")
            && let Some(chat) = value.get("dm_chat")
        {
            self.selected_chat = s(chat, "id").into();
            self.page = "chat".into();
        }
        if method == "project.create" {
            self.notice = tr("notice.created").to_string();
        }
        if (method.starts_with("approval.")
            || method == "question.answer"
            || method.starts_with("assignment.")
            || method.starts_with("project.")
            || method.starts_with("takeover."))
            && self.page == "workbench"
        {
            self.rpc("workbench.get", json!({}), cx);
        }
    }
    pub(super) fn feature_action(
        &mut self,
        event: &FeatureAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FeatureAction::Rpc { method, params } => self.rpc(method, params.clone(), cx),
            FeatureAction::Trace(id) => self.open_trace(Some(id.clone()), cx),
            FeatureAction::Computer(id) => self.open_computer(id.clone(), cx),
            FeatureAction::Toast(text) => self.notice = text.clone(),
            FeatureAction::Navigate(target) => {
                let (page, query) = target.split_once('?').unwrap_or((target, ""));
                if page == "usage/export.csv" {
                    self.export_usage(cx);
                    return;
                }
                if let Some(upload_id) = page.strip_prefix("skill/import/upload/") {
                    if upload_id.trim().is_empty() {
                        self.upload_skill(window, cx);
                    } else {
                        self.rpc(
                            "skill.import",
                            json!({"source":{"kind":"upload","upload_id":upload_id}}),
                            cx,
                        );
                    }
                    return;
                }
                if let Some(id) = page.strip_prefix("question/") {
                    if let Some(question) = self.state.questions.get(id).cloned() {
                        self.select_chat(s(&question, "chat_id").into(), window, cx);
                        self.notice = tr("block.question").into();
                    }
                    return;
                }
                if let Some(target) = page.strip_prefix("assignment/") {
                    let (id, action) = target.split_once('/').unwrap_or((target, "detail"));
                    if let Some(assignment) = self.state.assignments.get(id).cloned() {
                        if action == "cancel" {
                            self.rpc("assignment.stop", json!({"assignment_id":id}), cx);
                        } else if action == "retry" {
                            self.select_chat(s(&assignment, "origin_chat_id").into(), window, cx);
                            self.composer.update(cx, |input, cx| {
                                input.set_value(
                                    s(&assignment, "instruction").to_string(),
                                    window,
                                    cx,
                                )
                            });
                            self.composer.focus_handle(cx).focus(window, cx);
                        } else {
                            self.open_trace(Some(id.into()), cx);
                        }
                    }
                    return;
                }
                if let Some(target) = page.strip_prefix("project/") {
                    let (id, action) = target.split_once('/').unwrap_or((target, "detail"));
                    if let Some(chat) = self
                        .state
                        .chats
                        .values()
                        .find(|chat| s(chat, "project_id") == id)
                        .cloned()
                    {
                        self.select_chat(s(&chat, "id").into(), window, cx);
                        if action == "changes" {
                            self.changes_project = Some(id.into());
                            self.notice = tr("notice.changes").into();
                            self.composer.focus_handle(cx).focus(window, cx);
                        } else {
                            self.context.clear();
                            self.context_visible = true;
                        }
                    }
                    return;
                }
                if let Some(name) = page.strip_prefix("skill/")
                    && name != "preview"
                {
                    self.page = "skill".into();
                    self.rpc("skill.get", json!({"name":name}), cx);
                    let data = self.page_data();
                    self.feature_view
                        .update(cx, |v, cx| v.set_page("skill", data, window, cx));
                    return;
                }
                if let Some(id) = page.strip_prefix("bot/") {
                    if let Some(bot) = self.state.bots.get(id) {
                        self.selected_chat = s(bot, "dm_chat_id").into();
                        self.navigate("bot_settings", cx);
                    }
                    return;
                }
                if let Some(id) = page.strip_prefix("routine/") {
                    self.feature_data = if id == "new" {
                        json!({})
                    } else {
                        self.state
                            .routines
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| json!({}))
                    };
                    self.navigate("routine", cx);
                    if id != "new" {
                        self.rpc("routine.runs", json!({"routine_id":id}), cx);
                    }
                    return;
                }
                if let Some(id) = page.strip_prefix("artifact/") {
                    let artifact = self
                        .state
                        .announcements
                        .values()
                        .flat_map(|ann| arr(ann, "artifacts"))
                        .find(|artifact| s(artifact, "id") == id)
                        .cloned()
                        .or_else(|| {
                            arr(&self.feature_data, "results")
                                .iter()
                                .find(|item| s(item, "id") == id)
                                .cloned()
                        });
                    if let Some(artifact) = artifact {
                        if let Some(file) = artifact.get("file").filter(|file| file.is_object()) {
                            self.download_file(file.clone(), cx);
                        } else {
                            let location = s(&artifact, "path_or_url");
                            if location.starts_with("https://") || location.starts_with("http://") {
                                cx.open_url(location);
                            } else if !location.is_empty() {
                                let project_id = s(&artifact, "project_id");
                                let home = self
                                    .state
                                    .projects
                                    .get(project_id)
                                    .map(|p| s(p, "home_path"))
                                    .unwrap_or("");
                                let path = if home.is_empty() {
                                    location
                                } else {
                                    location.strip_prefix(home).unwrap_or(location)
                                }
                                .trim_start_matches('/');
                                self.download_file(
                                    json!({"root":"project","root_id":project_id,"path":path}),
                                    cx,
                                );
                            } else {
                                self.notice = tr("file.unavailable").into();
                            }
                        }
                    } else {
                        self.notice = tr("file.unavailable").into();
                    }
                    return;
                }
                if let Some(id) = page.strip_prefix("chat/") {
                    self.select_chat(id.into(), window, cx);
                    return;
                }
                self.navigate(page, cx);
                if !query.is_empty() {
                    self.feature_data["filter"] = json!(query);
                    self.sync_views(cx);
                }
            }
        }
        cx.notify();
    }
    pub(super) fn settings_action(
        &mut self,
        event: &SettingsAction,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsAction::Rpc { method, params } => self.rpc(method, params.clone(), cx),
            SettingsAction::Notice(text) => {
                self.notice = text.clone();
                cx.notify();
            }
            SettingsAction::UpdateUrl(url) => {
                self.notice = match crate::update::save_update_url(url) {
                    Ok(()) => tr("settings.saved").to_string(),
                    Err(e) => e.to_string(),
                };
                cx.notify();
            }
            SettingsAction::Local { key, value } => {
                let mut local = self.local_settings.clone();
                match key.as_str() {
                    "theme" => {
                        if let Some(theme) = value.as_str() {
                            local.theme = theme.into();
                        }
                    }
                    "notifications" => {
                        local.notifications = value.as_bool().unwrap_or(false);
                    }
                    "launch_at_login" => {
                        let enabled = value.as_bool().unwrap_or(false);
                        if let Err(error) = crate::local_settings::set_launch_at_login(enabled) {
                            self.notice = error.to_string();
                            self.sync_views(cx);
                            return;
                        }
                        local.launch_at_login = enabled;
                    }
                    _ => return,
                }
                match crate::local_settings::save(&local) {
                    Ok(()) => {
                        self.local_settings = local;
                        self.notice = tr("settings.saved").into();
                    }
                    Err(error) => self.notice = error.to_string(),
                }
                self.sync_views(cx);
                cx.notify();
            }
            SettingsAction::HostConnections => self.navigate("connect", cx),
            SettingsAction::CheckUpdate => self.check_update(cx),
            SettingsAction::Theme(mode) => {
                if mode == "system" {
                    Theme::sync_system_appearance(Some(_window), cx);
                } else {
                    Theme::change(
                        if mode == "dark" {
                            ThemeMode::Dark
                        } else {
                            ThemeMode::Light
                        },
                        None,
                        cx,
                    );
                }
                crate::tokens::sync_theme(cx);
                cx.notify();
            }
        }
    }
    pub(super) fn trace_action(
        &mut self,
        event: &TraceAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TraceAction::Rpc { method, params } => self.rpc(method, params.clone(), cx),
            TraceAction::Computer { bot_id } => self.open_computer(bot_id.clone(), cx),
            TraceAction::Mention { bot_id } => {
                let name = self
                    .state
                    .bots
                    .get(bot_id)
                    .map(|v| s(v, "name"))
                    .unwrap_or(bot_id)
                    .to_string();
                self.composer
                    .update(cx, |v, cx| v.set_value(format!("@{name} "), window, cx));
                self.composer.read(cx).focus_handle(cx).focus(window, cx);
            }
            TraceAction::Jump { chat_id, .. } => self.select_chat(chat_id.clone(), window, cx),
            TraceAction::Close => {
                self.context.pop();
                self.close_trace(cx);
            }
            TraceAction::DownloadOutput { file, .. } => {
                if let Some(file) = file {
                    self.download_file(file.clone(), cx);
                } else {
                    self.notice = tr("trace.output_unavailable").to_string();
                }
            }
        }
        cx.notify();
    }
    pub(super) fn persist_cache(&mut self, force: bool) {
        if self.fixture || (!force && self.last_cache.elapsed().as_secs() < 5) {
            return;
        }
        if let (Some(id), Some(node), Ok(root)) = (
            self.active_host.clone(),
            self.state
                .hello
                .as_ref()
                .map(|v| s(v, "node_id").to_string()),
            crate::state_cache::default_root(),
        ) {
            let state = self.state.clone();
            self.runtime.spawn(async move {
                let _ = crate::state_cache::save(root, &id, &node, &state);
            });
            self.last_cache = std::time::Instant::now();
        }
    }
    fn export_usage(&mut self, cx: &mut Context<Self>) {
        let rows = arr(&self.feature_data["breakdown"], "rows");
        let mut csv = String::from("key,label,input_tokens,output_tokens,cost,requests\n");
        let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
        for row in rows {
            let usage = &row["usage"];
            csv.push_str(&format!(
                "{},{},{},{},{},{}\n",
                quote(s(row, "key")),
                quote(s(row, "label")),
                usage["input_tokens"].as_u64().unwrap_or(0),
                usage["output_tokens"].as_u64().unwrap_or(0),
                usage["cost"],
                usage["requests"].as_u64().unwrap_or(0)
            ));
        }
        let path = std::env::temp_dir().join(format!(
            "MacBot-usage-{}.csv",
            Utc::now().format("%Y%m%d-%H%M%S")
        ));
        match std::fs::write(&path, csv) {
            Ok(()) => {
                let _ = std::process::Command::new("open").arg(path).spawn();
            }
            Err(error) => self.notice = error.to_string(),
        }
        cx.notify();
    }
    pub(super) fn check_update(&mut self, cx: &mut Context<Self>) {
        let version = crate::update::current_version();
        match crate::update::UpdateClient::from_env(&version) {
            Ok(Some(_)) => {}
            Ok(None) => {
                self.notice = tr("update.disabled").to_string();
                cx.notify();
                return;
            }
            Err(e) => {
                self.notice = e.to_string();
                cx.notify();
                return;
            }
        };
        let task = self
            .runtime
            .spawn(async move { crate::update::check_for_update(&version).await });
        self.notice = tr("update.checking").to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(release))) => {
                        view.notice = format!("{} {}", tr("update.available"), release.version);
                        view.update_release = Some(release);
                    }
                    Ok(Ok(None)) => {
                        view.update_release = None;
                        view.notice = tr("update.current").to_string();
                    }
                    Ok(Err(e)) => view.notice = e.to_string(),
                    Err(e) => view.notice = e.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn download_update(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.update_release.clone() else {
            return;
        };
        let client = match crate::update::UpdateClient::from_env(&crate::update::current_version())
        {
            Ok(Some(client)) => client,
            _ => return,
        };
        self.notice = tr("update.downloading").to_string();
        let task = self
            .runtime
            .spawn(async move { client.download_and_stage(&release).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok(stage)) => {
                        view.update_stage = Some(stage);
                        view.notice = tr("update.ready").to_string();
                    }
                    Ok(Err(e)) => view.notice = e.to_string(),
                    Err(e) => view.notice = e.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(stage) = self.update_stage.as_ref() else {
            return;
        };
        if let Some(script) = &stage.installer {
            match std::process::Command::new("/bin/zsh").arg(script).spawn() {
                Ok(_) => {
                    self.persist_cache(true);
                    cx.quit();
                }
                Err(e) => {
                    self.notice = e.to_string();
                    cx.notify();
                }
            }
        } else {
            let _ = std::process::Command::new("open")
                .arg(&stage.artifact)
                .spawn();
        }
    }

    pub(super) fn select_pinned(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut chats: Vec<_> = self
            .state
            .chats
            .values()
            .filter(|chat| s(chat, "kind") == "project" && chat["pinned"] == true)
            .collect();
        chats.sort_by_key(|chat| s(chat, "updated_at").to_owned());
        if let Some(chat) = chats.get(index) {
            self.select_chat(s(chat, "id").to_string(), window, cx);
        }
    }
    pub(super) fn persist_outbox(&mut self) {
        if let (Some(id), Some(hello)) = (&self.active_host, &self.state.hello)
            && let Err(error) = crate::outbox::save(id, s(hello, "node_id"), &self.pending_messages)
        {
            self.notice = error.to_string();
        }
    }
    pub(super) fn retry_outbox(&mut self, cx: &mut Context<Self>) {
        let requests = self
            .pending_messages
            .values()
            .filter(|message| {
                message["send_status"] == "queued"
                    && message["in_flight"] != true
                    && self.state.chats.contains_key(s(message, "chat_id"))
            })
            .map(|message| message["retry_params"].clone())
            .collect::<Vec<_>>();
        for params in requests {
            self.rpc("chat.send", params, cx);
        }
    }
    pub(super) fn rpc_failed(
        &mut self,
        method: &str,
        params: &Value,
        error: String,
        cx: &mut Context<Self>,
    ) {
        self.notice = error.clone();
        if method == "chat.send" {
            if let Some(message) = self
                .pending_messages
                .get_mut(s(params, "client_request_id"))
            {
                message["send_status"] = json!("failed");
                message["error"] = json!(error);
                message["in_flight"] = json!(false);
            }
            self.persist_outbox();
        }
        if method == "bootstrap" {
            self.resync_requested = false;
        }
        cx.notify();
    }
}
