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
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("agent-browser"),
            mode: BrowserMode::HeadlessProfile,
            chrome_profile: Some("Default".into()),
            idle_timeout_secs: 15 * 60,
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
    queue: VecDeque<BrowserAction>,
    active: bool,
    last_activity: SystemTime,
}

pub struct BrowserManager<R: CliRunner = ProcessRunner> {
    runner: Arc<R>,
    config: SessionConfig,
    sessions: HashMap<BotId, Session>,
}

impl<R: CliRunner> BrowserManager<R> {
    pub fn new(config: SessionConfig, runner: Arc<R>) -> Self {
        Self {
            runner,
            config,
            sessions: HashMap::new(),
        }
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
        let args = self.global_args(bot_id, "open", &[url.to_string()]);
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
        let output = self.runner.run(&self.config.executable, &args)?;
        let tab_id = serde_json::from_str::<Value>(&output)
            .ok()
            .and_then(|v| {
                v.get("tab_id")
                    .or_else(|| v.get("id"))
                    .or_else(|| v.get("targetId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or(tab_id);
        let tab = BrowserTab {
            tab_id,
            assignment_id: assignment_id.into(),
            title: String::new(),
            url: url.into(),
            active: session.state.tabs.is_empty(),
        };
        session.state.tabs.push(tab.clone());
        session.last_activity = SystemTime::now();
        session.state.last_activity_ms = epoch_ms();
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
        let select = self.global_args(bot_id, "tab", std::slice::from_ref(&tab.tab_id));
        if let Err(error) = self.runner.run(&self.config.executable, &select) {
            if let Some(s) = self.sessions.get_mut(bot_id) {
                s.active = false;
            }
            return Err(error);
        }
        let args = self.global_args(bot_id, &action.operation, &action.args);
        let out = self.runner.run(&self.config.executable, &args);
        if let Some(s) = self.sessions.get_mut(bot_id) {
            s.active = false;
            s.last_activity = SystemTime::now();
            s.state.last_activity_ms = epoch_ms();
        }
        out.map(|v| serde_json::from_str(&v).unwrap_or(Value::String(v)))
            .map(Some)
    }

    pub fn close_idle(&mut self, now: SystemTime) -> Result<Vec<BotId>, BrowserError> {
        let mut closed = Vec::new();
        let timeout = Duration::from_secs(self.config.idle_timeout_secs);
        let ids = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                !s.active && now.duration_since(s.last_activity).unwrap_or_default() >= timeout
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            let args = self.global_args(&id, "close", &[]);
            self.runner.run(&self.config.executable, &args)?;
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
        Ok(())
    }
    pub fn takeover_release(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        let s = self
            .sessions
            .get_mut(bot_id)
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))?;
        s.state.takeover = false;
        s.last_activity = SystemTime::now();
        Ok(())
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
        let select = self.global_args(bot_id, "tab", &[tab_id.to_owned()]);
        self.runner.run(&self.config.executable, &select)?;
        let path = std::env::temp_dir().join(format!("macbot-screen-{}.jpg", Uuid::now_v7()));
        let path_arg = path.to_string_lossy().into_owned();
        let args = self.global_args(
            bot_id,
            "screenshot",
            &[path_arg, "--screenshot-format".into(), "jpeg".into()],
        );
        self.runner.run(&self.config.executable, &args)?;
        let bytes = std::fs::read(&path).map_err(|e| BrowserError::Runner(e.to_string()));
        let _ = std::fs::remove_file(path);
        bytes
    }
    pub fn state(&self, bot_id: &str) -> Result<BrowserSessionState, BrowserError> {
        self.sessions
            .get(bot_id)
            .map(|s| s.state.clone())
            .ok_or_else(|| BrowserError::SessionNotFound(bot_id.into()))
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
        let args = self.global_args(bot_id, "tab", std::slice::from_ref(&tab.tab_id));
        self.runner.run(&self.config.executable, &args)?;
        Ok(())
    }

    fn ensure_session(&mut self, bot_id: &str) -> Result<(), BrowserError> {
        if self.sessions.contains_key(bot_id) {
            return Ok(());
        }
        self.session(bot_id);
        let args = self.global_args(bot_id, "session", &["info".into()]);
        let _ = self.runner.run(&self.config.executable, &args);
        Ok(())
    }
    fn global_args(&self, bot_id: &str, command: &str, args: &[String]) -> Vec<String> {
        let mut out = vec![
            "--session".into(),
            format!("macbot-{bot_id}"),
            "--json".into(),
        ];
        match self.config.mode {
            BrowserMode::Headless => {}
            BrowserMode::HeadlessProfile => {
                out.push("--restore".into());
                if let Some(p) = &self.config.chrome_profile {
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
        assert!(calls.iter().any(|call| call.contains(&"open".into())));
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
}
