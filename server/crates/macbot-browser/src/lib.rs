//! agent-browser sidecar integration.
//!
//! The sidecar is intentionally driven through a small runner trait. Production
//! uses [`ProcessRunner`], while gateway and unit tests can use a fake runner.
//! A Bot owns one named agent-browser session; assignments only own tabs and all
//! actions for one Bot pass through one FIFO queue.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use uuid::Uuid;

pub type BotId = String;
pub type AssignmentId = String;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    Headless,
    HeadlessProfile,
    Attach,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionConfig {
    pub executable: PathBuf,
    pub mode: BrowserMode,
    pub chrome_profile: Option<String>,
    pub idle_timeout_secs: u64,
    /// Optional source profile copied into an app-owned profile before launch.
    /// The source is read only; agent-browser never receives the user's
    /// profile path when `isolated_profile_root` is set.
    pub profile_source: Option<PathBuf>,
    pub isolated_profile_root: Option<PathBuf>,
    /// JSON state containing assignment-to-tab ownership for restart restore.
    pub state_path: Option<PathBuf>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            executable: default_executable(),
            mode: BrowserMode::HeadlessProfile,
            chrome_profile: Some("Default".into()),
            idle_timeout_secs: 15 * 60,
            profile_source: None,
            isolated_profile_root: None,
            state_path: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("browser runner failed: {0}")]
    Runner(String),
    #[error("invalid browser request: {0}")]
    Invalid(String),
    #[error("browser session not found for bot {0}")]
    SessionNotFound(BotId),
    #[error("browser assignment already owns a tab")]
    TabExists,
    #[error("browser takeover is not active")]
    TakeoverRequired,
    #[error("browser lock poisoned")]
    Poisoned,
}

pub trait CliRunner: Send + Sync {
    fn run(&self, executable: &std::path::Path, args: &[String]) -> Result<String, BrowserError>;
}

#[derive(Clone, Default)]
pub struct ProcessRunner;

impl CliRunner for ProcessRunner {
    fn run(&self, executable: &std::path::Path, args: &[String]) -> Result<String, BrowserError> {
        let output = std::process::Command::new(executable)
            .args(args)
            .output()
            .map_err(|e| BrowserError::Runner(e.to_string()))?;
        if !output.status.success() {
            return Err(BrowserError::Runner(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserTab {
    pub tab_id: String,
    pub assignment_id: AssignmentId,
    pub title: String,
    pub url: String,
    pub active: bool,
}

/// Metadata returned by `agent-browser tab list`.  This is deliberately kept
/// private: the assignment ownership in [`BrowserTab`] must never come from
/// the sidecar, only from our durable state.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SidecarTab {
    tab_id: String,
    title: String,
    url: String,
    active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSessionState {
    pub bot_id: BotId,
    pub session: String,
    pub tabs: Vec<BrowserTab>,
    pub takeover: bool,
    pub last_activity_ms: u64,
}

#[derive(Clone, Debug)]
pub struct BrowserAction {
    pub assignment_id: AssignmentId,
    pub operation: String,
    pub args: Vec<String>,
}

struct Session {
    state: BrowserSessionState,
    config: SessionConfig,
    queue: VecDeque<BrowserAction>,
    active: bool,
    screen_active: bool,
    last_activity: SystemTime,
}

pub struct BrowserManager<R: CliRunner = ProcessRunner> {
    runner: Arc<R>,
    config: SessionConfig,
    bot_configs: HashMap<BotId, SessionConfig>,
    sessions: HashMap<BotId, Session>,
}

impl<R: CliRunner> BrowserManager<R> {
    pub fn new(config: SessionConfig, runner: Arc<R>) -> Self {
        Self {
            runner,
            config,
            bot_configs: HashMap::new(),
            sessions: HashMap::new(),
        }
    }

    /// Set the mode/profile for one Bot. Existing active sessions retain their
    /// process; a new session (or the next idle restart) uses this config.
    pub fn set_bot_config(
        &mut self,
        bot_id: &str,
        config: SessionConfig,
    ) -> Result<(), BrowserError> {
        if let Some(session) = self.sessions.get_mut(bot_id) {
            if session.active {
                return Err(BrowserError::Invalid("browser session is busy".into()));
            }
            session.config = config.clone();
        }
        self.bot_configs.insert(bot_id.to_owned(), config);
        Ok(())
    }

    pub fn bot_config(&self, bot_id: &str) -> SessionConfig {
        self.bot_configs
            .get(bot_id)
            .cloned()
            .unwrap_or_else(|| self.config.clone())
    }

    pub fn session(&mut self, bot_id: &str) -> &BrowserSessionState {
        let session = self
            .sessions
            .entry(bot_id.to_owned())
            .or_insert_with(|| Session {
                state: BrowserSessionState {
                    bot_id: bot_id.into(),
                    session: format!("macbot-{bot_id}"),
                    tabs: Vec::new(),
                    takeover: false,
                    last_activity_ms: 0,
                },
                config: self
                    .bot_configs
                    .get(bot_id)
                    .cloned()
                    .unwrap_or_else(|| self.config.clone()),
                queue: VecDeque::new(),
                active: false,
                screen_active: false,
                last_activity: SystemTime::now(),
            });
        &session.state
    }

    pub fn open_tab(
        &mut self,
        bot_id: &str,
        assignment_id: &str,
        url: &str,
    ) -> Result<BrowserTab, BrowserError> {
        self.ensure_session(bot_id)?;
        let config = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?
            .config
            .clone();
        // `open` navigates the session's active tab and can therefore reuse a
        // tab owned by another assignment. `tab new` is the agent-browser
        // primitive that creates an isolated, stable tab id.
        let args = self.global_args(&config, bot_id, "tab", &["new".into(), url.to_string()]);
        let session = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        if session
            .state
            .tabs
            .iter()
            .any(|t| t.assignment_id == assignment_id)
        {
            return Err(BrowserError::TabExists);
        }
        let tab_id = Uuid::now_v7().to_string();
        let output = self.runner.run(&config.executable, &args)?;
        let value = serde_json::from_str::<Value>(&output).ok();
        let tab_id = value
            .as_ref()
            .and_then(|v| {
                v.pointer("/data/tabId")
                    .or_else(|| v.pointer("/data/tab_id"))
                    .or_else(|| v.get("tab_id"))
                    .or_else(|| v.get("id"))
                    .or_else(|| v.pointer("/data/targetId"))
                    .or_else(|| v.get("targetId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or(tab_id);
        let tab = BrowserTab {
            tab_id,
            assignment_id: assignment_id.into(),
            title: value
                .as_ref()
                .and_then(|v| v.pointer("/data/title").and_then(Value::as_str))
                .unwrap_or_default()
                .into(),
            url: value
                .as_ref()
                .and_then(|v| v.pointer("/data/url").and_then(Value::as_str))
                .unwrap_or(url)
                .into(),
            active: session.state.tabs.is_empty(),
        };
        session.state.tabs.push(tab.clone());
        session.last_activity = SystemTime::now();
        session.state.last_activity_ms = epoch_ms();
        self.persist_session(bot_id)?;
        Ok(tab)
    }

    pub fn enqueue(
        &mut self,
        bot_id: &str,
        action: BrowserAction,
    ) -> Result<Option<Value>, BrowserError> {
        self.ensure_session(bot_id)?;
        let session = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        session.queue.push_back(action);
        session.last_activity = SystemTime::now();
        session.state.last_activity_ms = epoch_ms();
        if session.active {
            return Ok(None);
        }
        self.run_next(bot_id)
    }

    pub fn run_next(&mut self, bot_id: &str) -> Result<Option<Value>, BrowserError> {
        let action = {
            let s = self
                .sessions
                .get_mut(bot_id)
                .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
            let Some(a) = s.queue.pop_front() else {
                s.active = false;
                return Ok(None);
            };
            s.active = true;
            a
        };
        let tab = {
            let s = self
                .sessions
                .get(bot_id)
                .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
            s.state
                .tabs
                .iter()
                .find(|t| t.assignment_id == action.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    BrowserError::Invalid(format!("no tab for assignment {}", action.assignment_id))
                })
        };
        let tab = match tab {
            Ok(tab) => tab,
            Err(error) => {
                if let Some(session) = self.sessions.get_mut(bot_id) {
                    session.active = false;
                }
                return Err(error);
            }
        };
        // `tab <id>` is a standalone command in agent-browser; actions are
        // issued by the following command against that session's active tab.
        let config = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?
            .config
            .clone();
        let select = self.global_args(&config, bot_id, "tab", std::slice::from_ref(&tab.tab_id));
        if let Err(error) = self.runner.run(&config.executable, &select) {
            if let Some(s) = self.sessions.get_mut(bot_id) {
                s.active = false;
            }
            return Err(error);
        }
        let args = self.global_args(&config, bot_id, &action.operation, &action.args);
        let out = self.runner.run(&config.executable, &args);
        if let Some(s) = self.sessions.get_mut(bot_id) {
            s.active = false;
            s.last_activity = SystemTime::now();
            s.state.last_activity_ms = epoch_ms();
        }
        self.persist_session(bot_id)?;
        out.map(|v| serde_json::from_str(&v).unwrap_or(Value::String(v)))
            .map(Some)
    }

    pub fn close_idle(&mut self, now: SystemTime) -> Result<Vec<BotId>, BrowserError> {
        let mut closed = Vec::new();
        let ids = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                !s.active
                    && !s.screen_active
                    && now.duration_since(s.last_activity).unwrap_or_default()
                        >= Duration::from_secs(s.config.idle_timeout_secs)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            let config = self
                .sessions
                .get(&id)
                .map(|session| session.config.clone())
                .unwrap_or_else(|| self.config.clone());
            let args = self.global_args(&config, &id, "close", &[]);
            self.runner.run(&config.executable, &args)?;
            self.sessions.remove(&id);
            closed.push(id);
        }
        Ok(closed)
    }

    pub fn takeover_start(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        self.ensure_session(bot_id)?;
        let s = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        s.state.takeover = true;
        s.last_activity = SystemTime::now();
        self.persist_session(bot_id)
    }
    pub fn takeover_release(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        let s = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        s.state.takeover = false;
        s.last_activity = SystemTime::now();
        self.persist_session(bot_id)
    }
    pub fn input(
        &mut self,
        bot_id: &str,
        assignment_id: &str,
        event: ScreenInput,
    ) -> Result<Option<Value>, BrowserError> {
        let s = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        if !s.state.takeover {
            return Err(BrowserError::TakeoverRequired);
        };
        let (operation, args) = match event {
            ScreenInput::Mouse {
                action,
                x,
                y,
                button,
                ..
            } => match action.as_str() {
                "move" => (
                    "mouse".into(),
                    vec!["move".into(), x.to_string(), y.to_string()],
                ),
                "down" | "up" => ("mouse".into(), vec![action, button]),
                "click" => {
                    self.enqueue(
                        bot_id,
                        BrowserAction {
                            assignment_id: assignment_id.into(),
                            operation: "mouse".into(),
                            args: vec!["move".into(), x.to_string(), y.to_string()],
                        },
                    )?;
                    self.enqueue(
                        bot_id,
                        BrowserAction {
                            assignment_id: assignment_id.into(),
                            operation: "mouse".into(),
                            args: vec!["down".into(), button.clone()],
                        },
                    )?;
                    ("mouse".into(), vec!["up".into(), button])
                }
                _ => {
                    return Err(BrowserError::Invalid(format!(
                        "unsupported mouse action {action}"
                    )))
                }
            },
            ScreenInput::Wheel { dx, dy, .. } => (
                "mouse".into(),
                vec!["wheel".into(), dy.to_string(), dx.to_string()],
            ),
            ScreenInput::Key {
                action, key, text, ..
            } => match action.as_str() {
                "down" | "up" | "press" => ("press".into(), vec![key]),
                "type" => ("keyboard".into(), vec!["type".into(), text.unwrap_or(key)]),
                _ => {
                    return Err(BrowserError::Invalid(format!(
                        "unsupported key action {action}"
                    )))
                }
            },
            ScreenInput::Touch { action, points } => {
                let point = points
                    .first()
                    .ok_or_else(|| BrowserError::Invalid("touch event has no points".into()))?;
                let command = match action.as_str() {
                    "start" => "down",
                    "end" => "up",
                    "move" => "move",
                    _ => {
                        return Err(BrowserError::Invalid(format!(
                            "unsupported touch action {action}"
                        )))
                    }
                };
                if command == "down" {
                    self.enqueue(
                        bot_id,
                        BrowserAction {
                            assignment_id: assignment_id.into(),
                            operation: "mouse".into(),
                            args: vec!["move".into(), point.x.to_string(), point.y.to_string()],
                        },
                    )?;
                }
                (
                    "mouse".into(),
                    if command == "move" {
                        vec![command.into(), point.x.to_string(), point.y.to_string()]
                    } else {
                        vec![command.into(), "left".into()]
                    },
                )
            }
        };
        self.enqueue(
            bot_id,
            BrowserAction {
                assignment_id: assignment_id.into(),
                operation,
                args,
            },
        )
    }

    /// Capture the active tab as a JPEG/PNG according to the sidecar's
    /// configured output. The path is temporary and is removed after read.
    pub fn screenshot(&mut self, bot_id: &str, tab_id: &str) -> Result<Vec<u8>, BrowserError> {
        self.ensure_session(bot_id)?;
        let config = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?
            .config
            .clone();
        let select = self.global_args(&config, bot_id, "tab", &[tab_id.to_owned()]);
        self.runner.run(&config.executable, &select)?;
        let path = std::env::temp_dir().join(format!("macbot-screen-{}.jpg", Uuid::now_v7()));
        let path_arg = path.to_string_lossy().into_owned();
        let args = self.global_args(
            &config,
            bot_id,
            "screenshot",
            &[path_arg, "--screenshot-format".into(), "jpeg".into()],
        );
        self.runner.run(&config.executable, &args)?;
        let bytes = std::fs::read(&path).map_err(|e| BrowserError::Runner(e.to_string()));
        let _ = std::fs::remove_file(path);
        bytes
    }

    /// Enable the sidecar's session-scoped WebSocket screencast. The caller
    /// must prove assignment ownership first; stream lifecycle is Bot-wide,
    /// while frames remain scoped by the gateway connection.
    pub fn stream_enable_for_assignment(
        &mut self,
        bot_id: &str,
        assignment_id: &str,
        port: Option<u16>,
    ) -> Result<Value, BrowserError> {
        self.ensure_session(bot_id)?;
        let _ = self.tab_for_assignment(bot_id, assignment_id, None)?;
        self.stream_command(bot_id, "enable", port)
    }

    pub fn stream_disable_for_assignment(
        &mut self,
        bot_id: &str,
        assignment_id: &str,
    ) -> Result<Value, BrowserError> {
        self.ensure_session(bot_id)?;
        let _ = self.tab_for_assignment(bot_id, assignment_id, None)?;
        self.stream_command(bot_id, "disable", None)
    }

    pub fn stream_status_for_assignment(
        &mut self,
        bot_id: &str,
        assignment_id: &str,
    ) -> Result<Value, BrowserError> {
        self.ensure_session(bot_id)?;
        let _ = self.tab_for_assignment(bot_id, assignment_id, None)?;
        self.stream_command(bot_id, "status", None)
    }
    pub fn state(&self, bot_id: &str) -> Result<BrowserSessionState, BrowserError> {
        self.sessions
            .get(bot_id)
            .map(|s| s.state.clone())
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))
    }

    /// Restore a persisted browser session for a screen connection. A screen
    /// client may reconnect after the idle reaper removed the in-memory
    /// session, so it must use the same restore path as browser actions.
    pub fn ensure_session_for_screen(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        self.ensure_session(bot_id)
    }

    /// Keep the browser session alive while at least one gateway screen
    /// connection is attached. This is separate from `active`, which tracks a
    /// single CLI action and may return to false while the screen remains open.
    pub fn set_screen_active(&mut self, bot_id: &str, active: bool) -> Result<(), BrowserError> {
        if active {
            self.ensure_session(bot_id)?;
        }
        let session = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        session.screen_active = active;
        if active {
            session.last_activity = SystemTime::now();
            session.state.last_activity_ms = epoch_ms();
        }
        self.persist_session(bot_id)
    }

    /// Return only tabs owned by one assignment.  Callers should use this
    /// instead of exposing the complete Bot session to a model run.
    pub fn tabs_for_assignment(
        &self,
        bot_id: &str,
        assignment_id: &str,
    ) -> Result<Vec<BrowserTab>, BrowserError> {
        Ok(self
            .state(bot_id)?
            .tabs
            .into_iter()
            .filter(|tab| tab.assignment_id == assignment_id)
            .collect())
    }

    pub fn tab_for_assignment(
        &self,
        bot_id: &str,
        assignment_id: &str,
        tab_id: Option<&str>,
    ) -> Result<BrowserTab, BrowserError> {
        self.tabs_for_assignment(bot_id, assignment_id)?
            .into_iter()
            .find(|tab| tab_id.is_none_or(|id| id == tab.tab_id))
            .ok_or_else(|| {
                BrowserError::Invalid(format!("no browser tab for assignment {assignment_id}"))
            })
    }

    /// Dynamic driver state for screen clients and takeover indicators.
    pub fn driver_for_assignment(
        &self,
        bot_id: &str,
        assignment_id: Option<&str>,
    ) -> Result<&'static str, BrowserError> {
        let state = self.state(bot_id)?;
        if state.takeover {
            return Ok("user");
        }
        if assignment_id.is_some_and(|id| state.tabs.iter().any(|tab| tab.assignment_id == id)) {
            Ok("bot")
        } else {
            Ok("idle")
        }
    }

    pub fn switch_tab(&mut self, bot_id: &str, assignment_id: &str) -> Result<(), BrowserError> {
        let session = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        let tab = session
            .state
            .tabs
            .iter()
            .find(|tab| tab.assignment_id == assignment_id)
            .ok_or_else(|| {
                BrowserError::Invalid(format!("no tab for assignment {assignment_id}"))
            })?;
        let config = session.config.clone();
        let args = self.global_args(&config, bot_id, "tab", std::slice::from_ref(&tab.tab_id));
        self.runner.run(&config.executable, &args)?;
        Ok(())
    }

    /// Persist metadata observed from the screencast sidecar. The sidecar's
    /// URL event is authoritative for the selected tab, and keeping it in the
    /// session snapshot prevents the next screen state refresh from reverting
    /// to the URL captured when the tab was opened.
    pub fn update_tab_url(
        &mut self,
        bot_id: &str,
        tab_id: &str,
        url: &str,
    ) -> Result<BrowserTab, BrowserError> {
        let session = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        let tab = session
            .state
            .tabs
            .iter_mut()
            .find(|tab| tab.tab_id == tab_id)
            .ok_or_else(|| BrowserError::Invalid(format!("no browser tab {tab_id}")))?;
        tab.url = url.to_owned();
        let tab = tab.clone();
        session.last_activity = SystemTime::now();
        session.state.last_activity_ms = epoch_ms();
        self.persist_session(bot_id)?;
        Ok(tab)
    }

    fn ensure_session(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        if self.sessions.contains_key(bot_id) {
            return Ok(());
        }
        let result = (|| {
            self.session(bot_id);
            let config = self
                .sessions
                .get(bot_id)
                .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?
                .config
                .clone();
            let config = self.prepare_config(bot_id, config)?;
            if let Some(session) = self.sessions.get_mut(bot_id) {
                session.config = config.clone();
            }
            let args = self.global_args(&config, bot_id, "session", &["info".into()]);
            let _ = self.runner.run(&config.executable, &args);
            self.restore_session(bot_id)
        })();
        if result.is_err() {
            // Do not leave a half-restored session that would make a later
            // retry return early from the `contains_key` fast path.
            self.sessions.remove(bot_id);
        }
        result
    }

    fn stream_command(
        &self,
        bot_id: &str,
        action: &str,
        port: Option<u16>,
    ) -> Result<Value, BrowserError> {
        let session = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        let mut args = vec![action.to_owned()];
        if action == "enable" {
            if let Some(port) = port {
                args.extend(["--port".into(), port.to_string()]);
            }
        }
        let output = match self.runner.run(
            &session.config.executable,
            &self.global_args(&session.config, bot_id, "stream", &args),
        ) {
            Ok(output) => output,
            Err(error)
                if action == "enable"
                    && error
                        .to_string()
                        .to_ascii_lowercase()
                        .contains("already enabled") =>
            {
                self.runner.run(
                    &session.config.executable,
                    &self.global_args(&session.config, bot_id, "stream", &["status".into()]),
                )?
            }
            Err(error) => return Err(error),
        };
        serde_json::from_str(&output).map_err(|error| BrowserError::Runner(error.to_string()))
    }
    fn global_args(
        &self,
        config: &SessionConfig,
        bot_id: &str,
        command: &str,
        args: &[String],
    ) -> Vec<String> {
        let mut out = vec![
            "--session".into(),
            format!("macbot-{bot_id}"),
            "--json".into(),
        ];
        match config.mode {
            BrowserMode::Headless => {}
            BrowserMode::HeadlessProfile => {
                out.push("--restore".into());
                if let Some(p) = &config.chrome_profile {
                    out.push("--profile".into());
                    out.push(p.clone());
                }
            }
            BrowserMode::Attach => out.push("--auto-connect".into()),
        }
        if !command.is_empty() {
            out.push(command.into());
        }
        out.extend(args.iter().cloned());
        out
    }

    fn prepare_config(
        &self,
        bot_id: &str,
        mut config: SessionConfig,
    ) -> Result<SessionConfig, BrowserError> {
        // Only profile-backed headless sessions need an isolated on-disk copy.
        // Attach uses the running browser's session and must not copy its data.
        if !matches!(config.mode, BrowserMode::HeadlessProfile) {
            config.chrome_profile = None;
            config.profile_source = None;
            config.isolated_profile_root = None;
            return Ok(config);
        }
        let Some(root) = config.isolated_profile_root.clone() else {
            return Ok(config);
        };
        let destination = root.join(bot_id);
        if !destination.exists() {
            if let Some(source) = config
                .profile_source
                .clone()
                .or_else(|| default_chrome_profile(config.chrome_profile.as_deref()))
            {
                copy_profile(&source, &destination)?;
            } else {
                std::fs::create_dir_all(&destination)
                    .map_err(|error| BrowserError::Runner(error.to_string()))?;
            }
        }
        config.chrome_profile = Some(destination.to_string_lossy().into_owned());
        Ok(config)
    }

    fn restore_session(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        let Some(path) = self
            .sessions
            .get(bot_id)
            .and_then(|session| session.config.state_path.clone())
        else {
            return Ok(());
        };
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(());
        };
        let Ok(restored) = serde_json::from_slice::<BrowserSessionState>(&bytes) else {
            return Ok(());
        };
        let config = self
            .sessions
            .get(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?
            .config
            .clone();
        let real_tabs = self.list_sidecar_tabs(bot_id, &config)?;
        let mut created_tab_ids = Vec::new();
        let result = (|| {
            let mut tabs = Vec::with_capacity(restored.tabs.len());
            for saved in restored.tabs {
                if let Some(real) = real_tabs
                    .iter()
                    .find(|tab| tab.tab_id == saved.tab_id && tab.url == saved.url)
                {
                    // A tab id alone is not an ownership proof: agent-browser can
                    // reuse the short t<N> id after a daemon restart.  Require
                    // the URL to match before retaining the assignment binding.
                    let mut tab = saved;
                    tab.title = if real.title.is_empty() {
                        tab.title
                    } else {
                        real.title.clone()
                    };
                    tab.url = real.url.clone();
                    tab.active = real.active;
                    tabs.push(tab);
                    continue;
                }

                // Attach mode is a user's existing browser.  A closed user tab is
                // intentionally not recreated or navigated by the service.  The
                // assignment can explicitly call browser.open again if needed.
                if config.mode == BrowserMode::Attach {
                    continue;
                }
                if saved.url.is_empty() {
                    return Err(BrowserError::Invalid(format!(
                        "cannot restore assignment {} without a tab URL",
                        saved.assignment_id
                    )));
                }
                let args =
                    self.global_args(&config, bot_id, "tab", &["new".into(), saved.url.clone()]);
                let output = self.runner.run(&config.executable, &args)?;
                let real = parse_tab_new_response(&output, &saved.url)?;
                created_tab_ids.push(real.tab_id.clone());
                tabs.push(BrowserTab {
                    tab_id: real.tab_id,
                    assignment_id: saved.assignment_id,
                    title: if real.title.is_empty() {
                        saved.title
                    } else {
                        real.title
                    },
                    url: real.url,
                    active: real.active || tabs.is_empty(),
                });
            }
            if let Some(session) = self.sessions.get_mut(bot_id) {
                session.state.tabs = tabs;
                session.state.takeover = false;
                session.state.last_activity_ms = epoch_ms();
            }
            self.persist_session(bot_id)
        })();
        if let Err(error) = result {
            return Err(self.rollback_created_tabs(bot_id, &config, &created_tab_ids, error));
        }
        Ok(())
    }

    fn rollback_created_tabs(
        &self,
        bot_id: &str,
        config: &SessionConfig,
        created_tab_ids: &[String],
        original: BrowserError,
    ) -> BrowserError {
        let mut cleanup_errors = Vec::new();
        for tab_id in created_tab_ids.iter().rev() {
            let args = self.global_args(config, bot_id, "tab", &["close".into(), tab_id.clone()]);
            if let Err(error) = self.runner.run(&config.executable, &args) {
                cleanup_errors.push(format!("{tab_id}: {error}"));
            }
        }
        if cleanup_errors.is_empty() {
            original
        } else {
            BrowserError::Runner(format!(
                "{original}; failed to clean up restored tabs: {}",
                cleanup_errors.join(", ")
            ))
        }
    }

    fn list_sidecar_tabs(
        &self,
        bot_id: &str,
        config: &SessionConfig,
    ) -> Result<Vec<SidecarTab>, BrowserError> {
        let args = self.global_args(config, bot_id, "tab", &["list".into()]);
        let output = self.runner.run(&config.executable, &args)?;
        parse_tab_list_response(&output)
    }

    fn persist_session(&self, bot_id: &str) -> Result<(), BrowserError> {
        let Some(session) = self.sessions.get(bot_id) else {
            return Ok(());
        };
        let Some(path) = session.config.state_path.clone() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| BrowserError::Runner(error.to_string()))?;
        }
        let bytes = serde_json::to_vec(&session.state)
            .map_err(|error| BrowserError::Runner(error.to_string()))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|error| BrowserError::Runner(error.to_string()))?;
        std::fs::rename(tmp, path).map_err(|error| BrowserError::Runner(error.to_string()))?;
        Ok(())
    }
}

fn default_chrome_profile(profile: Option<&str>) -> Option<PathBuf> {
    let profile = profile?;
    let path = PathBuf::from(profile);
    if path.is_absolute() {
        return Some(path);
    }
    dirs_home().map(|home| {
        home.join("Library/Application Support/Google/Chrome")
            .join(profile)
    })
}

fn parse_tab_list_response(output: &str) -> Result<Vec<SidecarTab>, BrowserError> {
    let value: Value = serde_json::from_str(output)
        .map_err(|error| BrowserError::Runner(format!("invalid tab list JSON: {error}")))?;
    let tabs = value
        .pointer("/data/tabs")
        .or_else(|| value.get("tabs"))
        .and_then(Value::as_array)
        .ok_or_else(|| BrowserError::Runner("tab list response missing data.tabs".into()))?;
    tabs.iter().map(parse_sidecar_tab).collect()
}

fn parse_sidecar_tab(value: &Value) -> Result<SidecarTab, BrowserError> {
    let tab_id = value
        .get("tabId")
        .or_else(|| value.get("tab_id"))
        .or_else(|| value.get("targetId"))
        .or_else(|| value.get("target_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| BrowserError::Runner("tab list item missing tab id".into()))?;
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| BrowserError::Runner("tab list item missing URL".into()))?;
    Ok(SidecarTab {
        tab_id: tab_id.into(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        url: url.into(),
        active: value
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_tab_new_response(output: &str, requested_url: &str) -> Result<SidecarTab, BrowserError> {
    let value: Value = serde_json::from_str(output)
        .map_err(|error| BrowserError::Runner(format!("invalid tab new JSON: {error}")))?;
    let data = value.get("data").unwrap_or(&value);
    let tab_id = data
        .get("tabId")
        .or_else(|| data.get("tab_id"))
        .or_else(|| data.get("targetId"))
        .or_else(|| data.get("target_id"))
        .or_else(|| data.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| BrowserError::Runner("tab new response missing tab id".into()))?;
    Ok(SidecarTab {
        tab_id: tab_id.into(),
        title: data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        url: data
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or(requested_url)
            .into(),
        active: data.get("active").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn default_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("MACBOT_BROWSER_BIN") {
        return PathBuf::from(path);
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(candidate) = current.parent().map(|parent| parent.join("agent-browser")) {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("agent-browser")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn copy_profile(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), BrowserError> {
    if !source.exists() {
        std::fs::create_dir_all(destination)
            .map_err(|error| BrowserError::Runner(error.to_string()))?;
        return Ok(());
    }
    std::fs::create_dir_all(destination)
        .map_err(|error| BrowserError::Runner(error.to_string()))?;
    for entry in
        std::fs::read_dir(source).map_err(|error| BrowserError::Runner(error.to_string()))?
    {
        let entry = entry.map_err(|error| BrowserError::Runner(error.to_string()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("Singleton")
            || matches!(name.as_ref(), "Cache" | "Code Cache" | "GPUCache")
        {
            continue;
        }
        let target = destination.join(entry.file_name());
        let ty = entry
            .file_type()
            .map_err(|error| BrowserError::Runner(error.to_string()))?;
        if ty.is_dir() {
            copy_profile(&entry.path(), &target)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), target)
                .map_err(|error| BrowserError::Runner(error.to_string()))?;
        }
    }
    Ok(())
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameHeader {
    pub seq: u64,
    pub tab_id: String,
    pub w: u32,
    pub h: u32,
    pub ts: u64,
    pub url: String,
}
#[derive(Clone, Debug)]
pub struct ScreenFrame {
    pub header: FrameHeader,
    pub jpeg: Vec<u8>,
}

pub fn encode_screen_frame(frame: &ScreenFrame) -> Result<Vec<u8>, BrowserError> {
    let header =
        serde_json::to_vec(&frame.header).map_err(|e| BrowserError::Invalid(e.to_string()))?;
    if header.len() > u32::MAX as usize {
        return Err(BrowserError::Invalid(
            "screen frame header too large".into(),
        ));
    }
    let mut out = Vec::with_capacity(4 + header.len() + frame.jpeg.len());
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&frame.jpeg);
    Ok(out)
}

/// One in-flight frame per screen connection. New frames replace an unacked
/// frame, so a slow phone never creates an unbounded queue.
#[derive(Default)]
pub struct FrameBroker {
    in_flight: Option<ScreenFrame>,
    latest: Option<ScreenFrame>,
    next_seq: u64,
}
impl FrameBroker {
    pub fn offer(&mut self, mut frame: ScreenFrame) -> bool {
        self.next_seq += 1;
        frame.header.seq = self.next_seq;
        if self.in_flight.is_none() {
            self.in_flight = Some(frame);
            true
        } else {
            self.latest = Some(frame);
            false
        }
    }
    pub fn ack(&mut self, seq: u64) -> Option<ScreenFrame> {
        if self.in_flight.as_ref().is_some_and(|f| f.header.seq == seq) {
            self.in_flight = None;
            if let Some(f) = self.latest.take() {
                self.in_flight = Some(f.clone());
                Some(f)
            } else {
                None
            }
        } else {
            None
        }
    }
    pub fn in_flight_seq(&self) -> Option<u64> {
        self.in_flight.as_ref().map(|f| f.header.seq)
    }
    pub fn pending(&self) -> bool {
        self.latest.is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScreenInput {
    Mouse {
        action: String,
        x: f64,
        y: f64,
        button: String,
        click_count: u8,
    },
    Wheel {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Key {
        action: String,
        key: String,
        code: String,
        text: Option<String>,
        modifiers: Vec<String>,
    },
    Touch {
        action: String,
        points: Vec<Point>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl CliRunner for Fake {
        fn run(&self, _: &std::path::Path, args: &[String]) -> Result<String, BrowserError> {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.windows(2).any(|window| window == ["tab", "list"]) {
                return Ok(r#"{"success":true,"data":{"tabs":[]}}"#.into());
            }
            if args.windows(2).any(|window| window == ["tab", "new"]) {
                let calls = self.calls.lock().unwrap().len();
                let url = args.last().cloned().unwrap_or_default();
                return Ok(format!(
                    r#"{{"success":true,"data":{{"tabId":"fake-{calls}","url":"{url}"}}}}"#
                ));
            }
            Ok("{}".into())
        }
    }
    struct RealShapeFake;
    impl CliRunner for RealShapeFake {
        fn run(&self, _: &std::path::Path, args: &[String]) -> Result<String, BrowserError> {
            if args.windows(2).any(|window| window == ["tab", "new"]) {
                return Ok(r#"{"success":true,"data":{"tabId":"t9","targetId":"cdp-9","title":"Demo","url":"data:text/html,demo"}}"#.into());
            }
            Ok("{}".into())
        }
    }
    struct ReconcileFake {
        calls: Mutex<Vec<Vec<String>>>,
        tabs: String,
        new_tab: String,
    }
    impl CliRunner for ReconcileFake {
        fn run(&self, _: &std::path::Path, args: &[String]) -> Result<String, BrowserError> {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.windows(2).any(|window| window == ["tab", "list"]) {
                return Ok(self.tabs.clone());
            }
            if args.windows(2).any(|window| window == ["tab", "new"]) {
                return Ok(self.new_tab.clone());
            }
            Ok("{}".into())
        }
    }
    struct RetryFake {
        calls: Mutex<Vec<Vec<String>>>,
        first_list: Mutex<bool>,
    }
    impl CliRunner for RetryFake {
        fn run(&self, _: &std::path::Path, args: &[String]) -> Result<String, BrowserError> {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.windows(2).any(|window| window == ["tab", "list"]) {
                if *self.first_list.lock().unwrap() {
                    *self.first_list.lock().unwrap() = false;
                    return Ok("{}".into());
                }
                return Ok(r#"{"success":true,"data":{"tabs":[]}}"#.into());
            }
            if args.windows(2).any(|window| window == ["tab", "new"]) {
                return Ok(
                    r#"{"success":true,"data":{"tabId":"retry-t1","url":"https://retry.example"}}"#
                        .into(),
                );
            }
            Ok("{}".into())
        }
    }
    struct PartialRestoreFake {
        calls: Mutex<Vec<Vec<String>>>,
        new_count: Mutex<usize>,
        fail_second_new: Mutex<bool>,
        fail_close: bool,
    }
    impl CliRunner for PartialRestoreFake {
        fn run(&self, _: &std::path::Path, args: &[String]) -> Result<String, BrowserError> {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.windows(2).any(|window| window == ["tab", "list"]) {
                return Ok(r#"{"success":true,"data":{"tabs":[]}}"#.into());
            }
            if args.windows(2).any(|window| window == ["tab", "new"]) {
                let mut count = self.new_count.lock().unwrap();
                *count += 1;
                if *count == 2 && *self.fail_second_new.lock().unwrap() {
                    *self.fail_second_new.lock().unwrap() = false;
                    return Err(BrowserError::Runner("new failed".into()));
                }
                return Ok(format!(
                    r#"{{"success":true,"data":{{"tabId":"partial-t{count}","url":"{}"}}}}"#,
                    args.last().cloned().unwrap_or_default()
                ));
            }
            if args.windows(2).any(|window| window == ["tab", "close"]) {
                if self.fail_close {
                    return Err(BrowserError::Runner("close failed".into()));
                }
                return Ok(r#"{"success":true}"#.into());
            }
            Ok("{}".into())
        }
    }

    fn seed_state(path: &std::path::Path, tab_id: &str, url: &str) {
        let state = BrowserSessionState {
            bot_id: "bot".into(),
            session: "macbot-bot".into(),
            tabs: vec![BrowserTab {
                tab_id: tab_id.into(),
                assignment_id: "assignment".into(),
                title: "Saved".into(),
                url: url.into(),
                active: true,
            }],
            takeover: true,
            last_activity_ms: 1,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    }

    fn reconcile_config(path: PathBuf, mode: BrowserMode) -> SessionConfig {
        SessionConfig {
            mode,
            state_path: Some(path),
            ..Default::default()
        }
    }

    #[test]
    fn restore_keeps_current_tab_without_reopening() {
        let root =
            std::env::temp_dir().join(format!("macbot-browser-reconcile-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        seed_state(&path, "t1", "https://keep.example");
        let fake = Arc::new(ReconcileFake {
            calls: Mutex::new(Vec::new()),
            tabs: r#"{"success":true,"data":{"tabs":[{"tabId":"t1","title":"Live","url":"https://keep.example","active":true}]}}"#.into(),
            new_tab: r#"{"success":true,"data":{"tabId":"t9","url":"https://keep.example"}}"#.into(),
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake.clone());
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Headless))
            .unwrap();
        browser.ensure_session_for_screen("bot").unwrap();
        let tab = browser.state("bot").unwrap().tabs.remove(0);
        assert_eq!(tab.tab_id, "t1");
        assert_eq!(tab.title, "Live");
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| { call.windows(2).any(|window| window == ["tab", "new"]) }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_reopens_missing_assignment_tab() {
        let root =
            std::env::temp_dir().join(format!("macbot-browser-reconcile-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        seed_state(&path, "t1", "https://missing.example");
        let fake = Arc::new(ReconcileFake {
            calls: Mutex::new(Vec::new()),
            tabs: r#"{"success":true,"data":{"tabs":[]}}"#.into(),
            new_tab: r#"{"success":true,"data":{"tabId":"t9","url":"https://missing.example"}}"#
                .into(),
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake.clone());
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Headless))
            .unwrap();
        browser.ensure_session_for_screen("bot").unwrap();
        let tab = browser.state("bot").unwrap().tabs.remove(0);
        assert_eq!(tab.tab_id, "t9");
        assert_eq!(tab.assignment_id, "assignment");
        assert_eq!(tab.url, "https://missing.example");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_reopens_reused_tab_id_when_url_changed() {
        let root =
            std::env::temp_dir().join(format!("macbot-browser-reconcile-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        seed_state(&path, "t1", "https://expected.example");
        let fake = Arc::new(ReconcileFake {
            calls: Mutex::new(Vec::new()),
            tabs: r#"{"success":true,"data":{"tabs":[{"tabId":"t1","title":"Other","url":"https://other.example","active":true}]}}"#.into(),
            new_tab: r#"{"success":true,"data":{"tabId":"t9","url":"https://expected.example"}}"#.into(),
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Headless))
            .unwrap();
        browser.ensure_session_for_screen("bot").unwrap();
        let tab = browser.state("bot").unwrap().tabs.remove(0);
        assert_eq!(tab.tab_id, "t9");
        assert_eq!(tab.url, "https://expected.example");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn attach_restore_does_not_reopen_missing_user_tab() {
        let root = std::env::temp_dir().join(format!("macbot-browser-attach-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        seed_state(&path, "t1", "https://user.example");
        let fake = Arc::new(ReconcileFake {
            calls: Mutex::new(Vec::new()),
            tabs: r#"{"success":true,"data":{"tabs":[]}}"#.into(),
            new_tab: r#"{"success":true,"data":{"tabId":"t9","url":"https://user.example"}}"#
                .into(),
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake.clone());
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Attach))
            .unwrap();
        browser.ensure_session_for_screen("bot").unwrap();
        assert!(browser.state("bot").unwrap().tabs.is_empty());
        assert!(!fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| { call.windows(2).any(|window| window == ["tab", "new"]) }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_restore_removes_half_created_session_for_retry() {
        let root = std::env::temp_dir().join(format!("macbot-browser-retry-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        seed_state(&path, "t1", "https://retry.example");
        let fake = Arc::new(RetryFake {
            calls: Mutex::new(Vec::new()),
            first_list: Mutex::new(true),
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Headless))
            .unwrap();
        assert!(browser.ensure_session_for_screen("bot").is_err());
        assert!(browser.state("bot").is_err());
        browser.ensure_session_for_screen("bot").unwrap();
        assert_eq!(browser.state("bot").unwrap().tabs[0].tab_id, "retry-t1");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn partial_restore_closes_created_tabs_and_preserves_state_for_retry() {
        let root = std::env::temp_dir().join(format!("macbot-browser-partial-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        let original = BrowserSessionState {
            bot_id: "bot".into(),
            session: "macbot-bot".into(),
            tabs: vec![
                BrowserTab {
                    tab_id: "old-1".into(),
                    assignment_id: "assignment-1".into(),
                    title: "Old 1".into(),
                    url: "https://one.example".into(),
                    active: true,
                },
                BrowserTab {
                    tab_id: "old-2".into(),
                    assignment_id: "assignment-2".into(),
                    title: "Old 2".into(),
                    url: "https://two.example".into(),
                    active: false,
                },
            ],
            takeover: false,
            last_activity_ms: 1,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let fake = Arc::new(PartialRestoreFake {
            calls: Mutex::new(Vec::new()),
            new_count: Mutex::new(0),
            fail_second_new: Mutex::new(true),
            fail_close: false,
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake.clone());
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Headless))
            .unwrap();
        assert!(browser.ensure_session_for_screen("bot").is_err());
        let saved: BrowserSessionState =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved, original);
        assert!(fake.calls.lock().unwrap().iter().any(|call| {
            call.windows(2).any(|window| window == ["tab", "close"])
                && call.last().is_some_and(|id| id == "partial-t1")
        }));

        browser.ensure_session_for_screen("bot").unwrap();
        let restored = browser.state("bot").unwrap();
        assert_eq!(restored.tabs.len(), 2);
        assert_eq!(restored.tabs[0].assignment_id, "assignment-1");
        assert_eq!(restored.tabs[1].assignment_id, "assignment-2");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn partial_restore_reports_cleanup_failure_with_original_error() {
        let root = std::env::temp_dir().join(format!("macbot-browser-cleanup-{}", Uuid::now_v7()));
        let path = root.join("state.json");
        let original = BrowserSessionState {
            bot_id: "bot".into(),
            session: "macbot-bot".into(),
            tabs: vec![
                BrowserTab {
                    tab_id: "old-1".into(),
                    assignment_id: "assignment-1".into(),
                    title: "Old 1".into(),
                    url: "https://one.example".into(),
                    active: true,
                },
                BrowserTab {
                    tab_id: "old-2".into(),
                    assignment_id: "assignment-2".into(),
                    title: "Old 2".into(),
                    url: "https://two.example".into(),
                    active: false,
                },
            ],
            takeover: false,
            last_activity_ms: 1,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let fake = Arc::new(PartialRestoreFake {
            calls: Mutex::new(Vec::new()),
            new_count: Mutex::new(0),
            fail_second_new: Mutex::new(true),
            fail_close: true,
        });
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        browser
            .set_bot_config("bot", reconcile_config(path.clone(), BrowserMode::Headless))
            .unwrap();
        let error = browser.ensure_session_for_screen("bot").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("new failed"));
        assert!(message.contains("failed to clean up restored tabs"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cli_uses_bot_session_and_json() {
        let fake = Arc::new(Fake::default());
        let mut b = BrowserManager::new(SessionConfig::default(), fake.clone());
        b.open_tab("bot-1", "a-1", "https://example.com").unwrap();
        let calls = fake.calls.lock().unwrap();
        assert!(calls[0]
            .windows(2)
            .any(|w| w == ["--session", "macbot-bot-1"]));
        assert!(calls[0].contains(&"--json".into()));
        assert!(calls
            .iter()
            .any(|call| { call.windows(2).any(|window| window == ["tab", "new"]) }));
    }
    #[test]
    fn parses_agent_browser_tab_new_shape() {
        let mut browser = BrowserManager::new(SessionConfig::default(), Arc::new(RealShapeFake));
        let tab = browser
            .open_tab("bot", "assignment", "https://example.com")
            .unwrap();
        assert_eq!(tab.tab_id, "t9");
        assert_eq!(tab.title, "Demo");
        assert_eq!(tab.url, "data:text/html,demo");
        assert!(browser
            .stream_enable_for_assignment("bot", "other", Some(1234))
            .is_err());
    }
    #[test]
    fn actions_for_one_bot_are_fifo() {
        let fake = Arc::new(Fake::default());
        let mut b = BrowserManager::new(
            SessionConfig {
                mode: BrowserMode::Headless,
                ..Default::default()
            },
            fake.clone(),
        );
        b.open_tab("b", "a", "https://x").unwrap();
        b.enqueue(
            "b",
            BrowserAction {
                assignment_id: "a".into(),
                operation: "snapshot".into(),
                args: vec![],
            },
        )
        .unwrap();
        b.enqueue(
            "b",
            BrowserAction {
                assignment_id: "a".into(),
                operation: "click".into(),
                args: vec!["@e1".into()],
            },
        )
        .unwrap();
        let calls = fake.calls.lock().unwrap();
        assert!(calls.iter().any(|x| x.contains(&"snapshot".into())));
        drop(calls);
        let calls = fake.calls.lock().unwrap();
        assert!(calls.iter().any(|x| x.contains(&"click".into())));
    }
    #[test]
    fn broker_only_releases_latest_after_ack() {
        let h = |n| FrameHeader {
            seq: 0,
            tab_id: "t".into(),
            w: 1,
            h: 1,
            ts: n,
            url: "u".into(),
        };
        let mut b = FrameBroker::default();
        assert!(b.offer(ScreenFrame {
            header: h(1),
            jpeg: vec![1]
        }));
        assert!(!b.offer(ScreenFrame {
            header: h(2),
            jpeg: vec![2]
        }));
        let f = b.ack(1).unwrap();
        assert_eq!(f.jpeg, vec![2]);
        assert!(b.in_flight_seq().is_some());
        assert!(b.ack(99).is_none());
    }

    #[test]
    fn attach_and_plain_headless_do_not_copy_profile_data() {
        let root = std::env::temp_dir().join(format!("macbot-browser-no-copy-{}", Uuid::now_v7()));
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("Cookies"), b"private-test-data").unwrap();
        for mode in [BrowserMode::Attach, BrowserMode::Headless] {
            let attach = matches!(mode, BrowserMode::Attach);
            let fake = Arc::new(Fake::default());
            let mut browser = BrowserManager::new(SessionConfig::default(), fake.clone());
            browser
                .set_bot_config(
                    "bot",
                    SessionConfig {
                        mode,
                        chrome_profile: Some(source.to_string_lossy().into_owned()),
                        profile_source: Some(source.clone()),
                        isolated_profile_root: Some(root.join("isolated")),
                        ..Default::default()
                    },
                )
                .unwrap();
            browser
                .open_tab("bot", "assignment", "https://example.com")
                .unwrap();
            assert!(!root.join("isolated").exists());
            let calls = fake.calls.lock().unwrap();
            assert!(calls.iter().all(|args| !args.contains(&"--profile".into())));
            assert_eq!(
                calls
                    .iter()
                    .any(|args| args.contains(&"--auto-connect".into())),
                attach
            );
        }
        assert_eq!(
            std::fs::read(source.join("Cookies")).unwrap(),
            b"private-test-data"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bot_profile_is_copied_and_assignment_tabs_restore() {
        let root = std::env::temp_dir().join(format!("macbot-browser-test-{}", Uuid::now_v7()));
        let source = root.join("chrome/Default");
        let isolated = root.join("profiles");
        let state = root.join("state/bot.json");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("Preferences"), b"{}").unwrap();
        let config = SessionConfig {
            mode: BrowserMode::HeadlessProfile,
            chrome_profile: Some("Default".into()),
            profile_source: Some(source.clone()),
            isolated_profile_root: Some(isolated.clone()),
            state_path: Some(state.clone()),
            ..Default::default()
        };
        let fake = Arc::new(Fake::default());
        let mut first = BrowserManager::new(SessionConfig::default(), fake.clone());
        first.set_bot_config("bot", config.clone()).unwrap();
        first
            .open_tab("bot", "assignment-1", "https://example.com")
            .unwrap();
        assert!(isolated.join("bot/Preferences").exists());
        assert!(state.exists());
        drop(first);

        let mut restored = BrowserManager::new(SessionConfig::default(), fake);
        restored.set_bot_config("bot", config).unwrap();
        restored
            .open_tab("bot", "assignment-2", "https://example.org")
            .unwrap();
        let tabs = restored.state("bot").unwrap().tabs;
        assert!(tabs.iter().any(|tab| tab.assignment_id == "assignment-1"));
        assert!(tabs.iter().any(|tab| tab.assignment_id == "assignment-2"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn screen_reconnect_restores_idle_evicted_session() {
        let root = std::env::temp_dir().join(format!("macbot-browser-screen-{}", Uuid::now_v7()));
        let state = root.join("state/bot.json");
        let config = SessionConfig {
            state_path: Some(state.clone()),
            ..Default::default()
        };
        let fake = Arc::new(Fake::default());
        let mut browser = BrowserManager::new(SessionConfig::default(), fake.clone());
        browser.set_bot_config("bot", config.clone()).unwrap();
        let tab = browser
            .open_tab("bot", "assignment", "https://example.com")
            .unwrap();
        browser
            .close_idle(SystemTime::now() + Duration::from_secs(901))
            .unwrap();
        assert!(browser.state("bot").is_err());

        browser.ensure_session_for_screen("bot").unwrap();
        assert_ne!(browser.state("bot").unwrap().tabs[0].tab_id, tab.tab_id);
        assert_eq!(
            browser.state("bot").unwrap().tabs[0].assignment_id,
            "assignment"
        );
        assert_eq!(
            browser.state("bot").unwrap().tabs[0].url,
            "https://example.com"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn screen_active_session_is_not_idle_evicted_until_last_screen_closes() {
        let root = std::env::temp_dir().join(format!("macbot-browser-screen-{}", Uuid::now_v7()));
        let config = SessionConfig {
            state_path: Some(root.join("state/bot.json")),
            ..Default::default()
        };
        let fake = Arc::new(Fake::default());
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        browser.set_bot_config("bot", config).unwrap();
        browser
            .open_tab("bot", "assignment", "https://example.com")
            .unwrap();
        browser.set_screen_active("bot", true).unwrap();
        browser
            .close_idle(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();
        assert!(browser.state("bot").is_ok());

        browser.set_screen_active("bot", false).unwrap();
        browser
            .close_idle(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();
        assert!(browser.state("bot").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_assignment_tab_does_not_leave_session_busy() {
        let fake = Arc::new(Fake::default());
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        assert!(browser
            .enqueue(
                "bot",
                BrowserAction {
                    assignment_id: "missing".into(),
                    operation: "snapshot".into(),
                    args: vec![],
                }
            )
            .is_err());
        browser
            .set_bot_config("bot", SessionConfig::default())
            .unwrap();
        browser
            .open_tab("bot", "valid", "https://valid.example")
            .unwrap();
        assert!(browser
            .enqueue(
                "bot",
                BrowserAction {
                    assignment_id: "valid".into(),
                    operation: "snapshot".into(),
                    args: vec![],
                }
            )
            .unwrap()
            .is_some());
    }

    #[test]
    fn assignment_tab_lookup_never_crosses_private_tabs() {
        let fake = Arc::new(Fake::default());
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        browser
            .open_tab("bot", "assignment-a", "https://a")
            .unwrap();
        browser
            .open_tab("bot", "assignment-b", "https://b")
            .unwrap();
        let a = browser.tabs_for_assignment("bot", "assignment-a").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].assignment_id, "assignment-a");
        assert!(browser
            .tab_for_assignment("bot", "assignment-a", Some("missing"))
            .is_err());
        assert_eq!(
            browser
                .driver_for_assignment("bot", Some("assignment-a"))
                .unwrap(),
            "bot"
        );
    }

    #[test]
    fn takeover_and_sidecar_url_metadata_are_visible_and_persisted() {
        let fake = Arc::new(Fake::default());
        let mut browser = BrowserManager::new(SessionConfig::default(), fake);
        let tab = browser
            .open_tab("bot", "assignment", "https://old.example")
            .unwrap();
        assert_eq!(
            browser
                .driver_for_assignment("bot", Some("assignment"))
                .unwrap(),
            "bot"
        );
        browser.takeover_start("bot").unwrap();
        assert_eq!(
            browser
                .driver_for_assignment("bot", Some("assignment"))
                .unwrap(),
            "user"
        );
        let updated = browser
            .update_tab_url("bot", &tab.tab_id, "https://new.example")
            .unwrap();
        assert_eq!(updated.url, "https://new.example");
        assert_eq!(
            browser.state("bot").unwrap().tabs[0].url,
            "https://new.example"
        );
        browser.takeover_release("bot").unwrap();
        assert_eq!(
            browser
                .driver_for_assignment("bot", Some("assignment"))
                .unwrap(),
            "bot"
        );
    }
}
