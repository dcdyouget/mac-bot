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
                })?
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

    fn ensure_session(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        if self.sessions.contains_key(bot_id) {
            return Ok(());
        }
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
        self.restore_session(bot_id)?;
        Ok(())
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
        if let Some(session) = self.sessions.get_mut(bot_id) {
            session.state.tabs = restored.tabs;
            session.state.takeover = false;
            session.state.last_activity_ms = epoch_ms();
        }
        Ok(())
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
}
