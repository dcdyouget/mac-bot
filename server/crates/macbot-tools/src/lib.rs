//! Local tools exposed to model runs. Tool failures are values (`is_error`) so
//! the model can fix an argument without losing its durable run.

use async_trait::async_trait;
use base64::Engine;
use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};
use thiserror::Error;
use tokio::{
    fs,
    process::{Child, Command},
    sync::Mutex,
    time::{timeout, Duration},
};

pub const MAX_OUTPUT_LINES: usize = 2_000;
pub const MAX_OUTPUT_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Risk {
    Read,
    Write,
    Exec,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Part {
    Text { text: String },
    Image { data: String, mime: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Part>,
    pub details: Value,
    pub is_error: bool,
}
impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Part::Text { text: text.into() }],
            details: Value::Null,
            is_error: false,
        }
    }
    pub fn error(error: impl std::fmt::Display) -> Self {
        Self {
            content: vec![Part::Text {
                text: error.to_string(),
            }],
            details: Value::Null,
            is_error: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub run_id: String,
    pub output_dir: PathBuf,
    pub env: HashMap<String, String>,
}
impl ToolContext {
    pub fn new(
        cwd: impl Into<PathBuf>,
        run_id: impl Into<String>,
        output_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            run_id: run_id.into(),
            output_dir: output_dir.into(),
            env: HashMap::new(),
        }
    }
    fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let raw = Path::new(path);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.cwd.join(raw)
        };
        let cwd = self
            .cwd
            .canonicalize()
            .map_err(|_| ToolError::PathEscape(candidate.clone()))?;
        let resolved = canonicalize_with_missing(&candidate)?;
        if !resolved.starts_with(&cwd) {
            return Err(ToolError::PathEscape(candidate));
        }
        Ok(candidate)
    }
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    Args(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("path escapes working directory: {0}")]
    PathEscape(PathBuf),
    #[error("command failed: {0}")]
    Command(String),
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    fn risk(&self, args: &Value) -> Risk;
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult;
}

#[derive(Clone, Default)]
pub struct FileMutationQueue {
    locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}
impl FileMutationQueue {
    /// Create a queue that can be shared by all mutation tools in one run.
    pub fn new() -> Self {
        Self::default()
    }

    async fn lock_for(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[derive(Clone)]
pub struct ReadTool;
#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Read a text file with bounded output."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let result: Result<ToolResult, ToolError> = async {
            let path = ctx.resolve(
                args.get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Args("path is required".into()))?,
            )?;
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1) as usize;
            if offset == 0 {
                return Err(ToolError::Args("offset is 1-indexed".into()));
            }
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|x| x as usize);
            if let Some(mime) = image_mime(&path) {
                let data = base64::engine::general_purpose::STANDARD.encode(fs::read(&path).await?);
                return Ok(ToolResult {
                    content: vec![Part::Image {
                        data,
                        mime: mime.into(),
                    }],
                    details: json!({"path":path,"mime":mime,"truncated":false}),
                    is_error: false,
                });
            }
            let bytes = fs::read(&path).await?;
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.split('\n').collect();
            if offset > lines.len() {
                return Err(ToolError::Args(format!(
                    "offset {offset} is beyond {} lines",
                    lines.len()
                )));
            }
            let end = limit
                .map(|n| (offset - 1 + n).min(lines.len()))
                .unwrap_or(lines.len());
            let mut result = bounded_text(ctx, &lines[(offset - 1)..end].join("\n"), false).await?;
            result.details["path"] = json!(path);
            if result.details["truncated"] == true {
                if let Some(Part::Text { text }) = result.content.first_mut() {
                    text.push_str(&format!(
                        "\n\n[use offset={} to continue]",
                        offset + end - offset
                    ));
                }
            }
            Ok(result)
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

#[derive(Clone, Default)]
pub struct WriteTool {
    queue: FileMutationQueue,
}
impl WriteTool {
    pub fn new(queue: FileMutationQueue) -> Self {
        Self { queue }
    }

    /// Use the supplied queue to serialize writes with other mutation tools.
    pub fn with_queue(queue: FileMutationQueue) -> Self {
        Self::new(queue)
    }
}
#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Write a text file, creating parent directories."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["path","content"],"properties":{"path":{"type":"string"},"content":{"type":"string"}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Write
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let result: Result<ToolResult, ToolError> = async {
            let path = ctx.resolve(
                args.get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Args("path is required".into()))?,
            )?;
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Args("content is required".into()))?;
            let lock = self.queue.lock_for(&path).await;
            let _guard = lock.lock().await;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&path, content).await?;
            Ok(ToolResult::text(format!(
                "wrote {} bytes to {}",
                content.len(),
                path.display()
            )))
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

#[derive(Clone, Default)]
pub struct EditTool {
    queue: FileMutationQueue,
}
impl EditTool {
    pub fn new(queue: FileMutationQueue) -> Self {
        Self { queue }
    }

    /// Use the supplied queue to serialize edits with other mutation tools.
    pub fn with_queue(queue: FileMutationQueue) -> Self {
        Self::new(queue)
    }
}
#[derive(Deserialize)]
struct Edit {
    #[serde(rename = "oldText")]
    old_text: String,
    #[serde(rename = "newText")]
    new_text: String,
}
#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        "Apply unique, non-overlapping text replacements."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["path","edits"],"properties":{"path":{"type":"string"},"edits":{"type":"array","items":{"type":"object","required":["oldText","newText"]}}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Write
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let result: Result<ToolResult, ToolError> = async {
            let path = ctx.resolve(
                args.get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::Args("path is required".into()))?,
            )?;
            let edits: Vec<Edit> = serde_json::from_value(
                args.get("edits")
                    .cloned()
                    .ok_or_else(|| ToolError::Args("edits is required".into()))?,
            )
            .map_err(|e| ToolError::Args(e.to_string()))?;
            if edits.is_empty() {
                return Err(ToolError::Args("edits cannot be empty".into()));
            }
            let lock = self.queue.lock_for(&path).await;
            let _guard = lock.lock().await;
            let original = fs::read_to_string(&path).await?;
            let mut ranges = Vec::new();
            for edit in &edits {
                let mut it = original.match_indices(&edit.old_text);
                let Some((start, _)) = it.next() else {
                    return Err(ToolError::Args(format!(
                        "oldText not found: {}",
                        edit.old_text
                    )));
                };
                if it.next().is_some() {
                    return Err(ToolError::Args(format!(
                        "oldText is not unique: {}",
                        edit.old_text
                    )));
                }
                let end = start + edit.old_text.len();
                if ranges
                    .iter()
                    .any(|(a, b): &(usize, usize)| start < *b && end > *a)
                {
                    return Err(ToolError::Args("edits overlap".into()));
                }
                ranges.push((start, end));
            }
            let mut result = original;
            let mut indexed: Vec<_> = edits.into_iter().zip(ranges).collect();
            indexed.sort_by_key(|(_, (start, _))| std::cmp::Reverse(*start));
            for (edit, (start, end)) in indexed {
                result.replace_range(start..end, &edit.new_text);
            }
            fs::write(&path, result).await?;
            Ok(ToolResult::text(format!("edited {}", path.display())))
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

#[derive(Clone)]
pub struct LsTool;
#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }
    fn description(&self) -> &str {
        "List a directory, including hidden files."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"limit":{"type":"integer"}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let result: Result<ToolResult, ToolError> = async {
            let path = ctx.resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize;
            let mut dir = fs::read_dir(&path).await?;
            let mut entries = Vec::new();
            while let Some(entry) = dir.next_entry().await? {
                let mut name = entry.file_name().to_string_lossy().to_string();
                if entry.file_type().await?.is_dir() {
                    name.push('/');
                }
                entries.push(name);
                if entries.len() >= limit {
                    break;
                }
            }
            entries.sort();
            bounded_text(ctx, &entries.join("\n"), false).await
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

#[derive(Clone)]
pub struct FindTool;
#[async_trait]
impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }
    fn description(&self) -> &str {
        "Find files by glob pattern."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string"},"path":{"type":"string"},"limit":{"type":"integer"}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let result: Result<ToolResult, ToolError> = async {
            let pattern = args
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Args("pattern is required".into()))?;
            let path = ctx.resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(1000) as usize;
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(pattern).map_err(|e| ToolError::Args(e.to_string()))?);
            let set = builder
                .build()
                .map_err(|e| ToolError::Args(e.to_string()))?;
            let mut out = Vec::new();
            for entry in ignored_walk(&path).filter_map(Result::ok) {
                if !entry.path().is_file() {
                    continue;
                }
                let rel = entry.path().strip_prefix(&path).unwrap_or(entry.path());
                if set.is_match(rel) || set.is_match(Path::new(entry.file_name())) {
                    out.push(entry.path().display().to_string());
                    if out.len() >= limit {
                        break;
                    }
                }
            }
            bounded_text(ctx, &out.join("\n"), false).await
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

#[derive(Clone)]
pub struct GrepTool;
#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search text files with a regular expression."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["pattern"],"properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"},"ignoreCase":{"type":"boolean"},"literal":{"type":"boolean"},"context":{"type":"integer"},"limit":{"type":"integer"}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let result: Result<ToolResult, ToolError> = async {
            let pattern = args
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Args("pattern is required".into()))?;
            let regex = if args
                .get("literal")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                regex::escape(pattern)
            } else {
                pattern.into()
            };
            let re = RegexBuilder::new(&regex)
                .case_insensitive(
                    args.get("ignoreCase")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .build()
                .map_err(|e| ToolError::Args(e.to_string()))?;
            let path = ctx.resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
            let glob = args
                .get("glob")
                .and_then(Value::as_str)
                .map(|g| Glob::new(g).map(|x| x.compile_matcher()))
                .transpose()
                .map_err(|e| ToolError::Args(e.to_string()))?;
            let context = args.get("context").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
            let mut out = Vec::new();
            for entry in ignored_walk(&path).filter_map(Result::ok) {
                if !entry.path().is_file() {
                    continue;
                }
                let p = entry.path();
                if let Some(g) = &glob {
                    if !g.is_match(Path::new(p.file_name().unwrap_or_default())) {
                        continue;
                    }
                }
                let Ok(text) = fs::read_to_string(p).await else {
                    continue;
                };
                let lines: Vec<_> = text.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if re.is_match(line) {
                        let start = i.saturating_sub(context);
                        let end = (i + context + 1).min(lines.len());
                        for (n, line) in lines.iter().enumerate().take(end).skip(start) {
                            out.push(format!("{}:{}:{}", p.display(), n + 1, line));
                            if out.len() >= limit {
                                break;
                            }
                        }
                    }
                    if out.len() >= limit {
                        break;
                    }
                }
                if out.len() >= limit {
                    break;
                }
            }
            bounded_text(ctx, &out.join("\n"), false).await
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BashDetails {
    pub exit_code: Option<i32>,
    pub wall_time_seconds: f64,
    pub truncated: bool,
    pub full_output_path: Option<String>,
}

#[derive(Clone, Default)]
pub struct BashJobManager {
    jobs: Arc<Mutex<HashMap<String, BackgroundJob>>>,
}

#[derive(Clone)]
struct BackgroundJob {
    pid: u32,
    output: Arc<Mutex<Vec<u8>>>,
    status: Arc<Mutex<Option<i32>>>,
}

impl BashJobManager {
    pub async fn start(
        &self,
        command: &str,
        cwd: &Path,
        env: &HashMap<String, String>,
    ) -> Result<String, ToolError> {
        let child = shell_command(command, cwd, env).spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| ToolError::Command("background process has no pid".into()))?;
        #[cfg(unix)]
        unsafe {
            // Establish the group from the parent as well as in pre_exec;
            // this closes the race before the shell creates descendants.
            libc::setpgid(pid as libc::pid_t, pid as libc::pid_t);
        }
        let id = format!("bash_{}", uuid::Uuid::now_v7());
        let output = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(None));
        let job = BackgroundJob {
            pid,
            output: output.clone(),
            status: status.clone(),
        };
        self.jobs.lock().await.insert(id.clone(), job);
        tokio::spawn(async move {
            if let Ok(result) = child.wait_with_output().await {
                let mut bytes = result.stdout;
                bytes.extend_from_slice(&result.stderr);
                *output.lock().await = bytes;
                *status.lock().await = result.status.code();
            } else {
                *status.lock().await = Some(-1);
            }
        });
        Ok(id)
    }

    pub async fn status(&self, id: &str) -> Option<(Option<i32>, Vec<u8>)> {
        let job = self.jobs.lock().await.get(id).cloned()?;
        let status = *job.status.lock().await;
        let output = job.output.lock().await.clone();
        Some((status, output))
    }

    pub async fn output(&self, id: &str) -> Option<Vec<u8>> {
        self.status(id).await.map(|(_, output)| output)
    }

    pub async fn kill(&self, id: &str) -> Result<bool, ToolError> {
        let Some(job) = self.jobs.lock().await.get(id).cloned() else {
            return Ok(false);
        };
        #[cfg(unix)]
        unsafe {
            libc::kill(-(job.pid as i32), libc::SIGTERM);
            // Escalate in the same process group so a shell waiting on a
            // descendant cannot leave an orphaned background task.
            libc::kill(-(job.pid as i32), libc::SIGKILL);
            libc::kill(job.pid as i32, libc::SIGKILL);
        }
        *job.status.lock().await = Some(-libc::SIGKILL);
        Ok(true)
    }

    /// Stop every background process owned by this run manager. The gateway
    /// should call this when a durable run ends; callers that deliberately
    /// register a long-lived service can retain the manager without calling it.
    pub async fn cleanup(&self) -> Result<usize, ToolError> {
        let ids = self.jobs.lock().await.keys().cloned().collect::<Vec<_>>();
        let mut killed = 0;
        for id in ids {
            if self.kill(&id).await? {
                killed += 1;
            }
        }
        Ok(killed)
    }
}

#[derive(Clone, Default)]
pub struct BashTool {
    pub jobs: BashJobManager,
}
impl BashTool {
    pub fn with_jobs(jobs: BashJobManager) -> Self {
        Self { jobs }
    }
}
#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Execute a zsh command with sanitized environment and bounded output."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["command"],"properties":{"command":{"type":"string"},"timeout":{"type":"number"},"background":{"type":"boolean"},"cwd":{"type":"string"}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Exec
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let started = Instant::now();
        let result: Result<ToolResult, ToolError> = async {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Args("command is required".into()))?;
            let cwd = if let Some(cwd) = args.get("cwd").and_then(Value::as_str) {
                ctx.resolve(cwd)?
            } else {
                ctx.cwd.clone()
            };
            if args
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let job_id = self.jobs.start(command, &cwd, &ctx.env).await?;
                return Ok(ToolResult {
                    content: vec![Part::Text {
                        text: format!("started background job {job_id}"),
                    }],
                    details: json!({"job_id": job_id}),
                    is_error: false,
                });
            }
            let secs = args.get("timeout").and_then(Value::as_f64).unwrap_or(300.0);
            let child = shell_command(command, &cwd, &ctx.env).spawn()?;
            let output = run_child(child, Duration::from_secs_f64(secs)).await?;
            let mut combined = output.stdout;
            combined.extend_from_slice(&output.stderr);
            let cut = truncate_tail(&combined);
            let full_path = if cut.truncated {
                let dir = output_dir(ctx);
                fs::create_dir_all(&dir).await?;
                let path = dir.join(format!("bash-{}.log", uuid::Uuid::now_v7()));
                fs::write(&path, &combined).await?;
                Some(path.display().to_string())
            } else {
                None
            };
            let details = BashDetails {
                exit_code: output.status,
                wall_time_seconds: started.elapsed().as_secs_f64(),
                truncated: cut.truncated,
                full_output_path: full_path.clone(),
            };
            let mut text = String::from_utf8_lossy(&cut.bytes).to_string();
            if cut.truncated {
                text.push_str(&format!(
                    "\n\n[output truncated; full output: {}]",
                    full_path.unwrap_or_default()
                ));
            }
            Ok(ToolResult {
                content: vec![Part::Text { text }],
                details: serde_json::to_value(details).unwrap(),
                is_error: output.status.unwrap_or(-1) != 0,
            })
        }
        .await;
        result.unwrap_or_else(ToolResult::error)
    }
}

fn shell_command(command: &str, cwd: &Path, extra_env: &HashMap<String, String>) -> Command {
    let mut cmd = Command::new("/bin/zsh");
    cmd.arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "PATH", "HOME", "USER", "SHELL", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE",
    ] {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    for (key, value) in extra_env {
        if !key.starts_with("API_") && !key.ends_with("_API_KEY") && !key.ends_with("_TOKEN") {
            cmd.env(key, value);
        }
    }
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
}

async fn run_child(child: Child, duration: Duration) -> Result<ChildOutput, ToolError> {
    let pid = child.id();
    let wait = child.wait_with_output();
    tokio::pin!(wait);
    match timeout(duration, &mut wait).await {
        Ok(Ok(output)) => Ok(ChildOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            status: output.status.code(),
        }),
        Ok(Err(e)) => Err(ToolError::Io(e)),
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = wait.await;
            Err(ToolError::Command("command timed out".into()))
        }
    }
}
struct ChildOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
}

#[derive(Clone, Default)]
pub struct BashJobTool {
    pub jobs: BashJobManager,
}
#[async_trait]
impl Tool for BashJobTool {
    fn name(&self) -> &str {
        "bash_job"
    }
    fn description(&self) -> &str {
        "Inspect or kill a background bash job."
    }
    fn schema(&self) -> Value {
        json!({"type":"object","required":["job_id","action"],"properties":{"job_id":{"type":"string"},"action":{"enum":["status","output","kill"]}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Exec
    }
    async fn call(&self, ctx: &ToolContext, args: Value) -> ToolResult {
        let Some(id) = args.get("job_id").and_then(Value::as_str) else {
            return ToolResult::error("job_id is required");
        };
        match args.get("action").and_then(Value::as_str) {
            Some("kill") => match self.jobs.kill(id).await {
                Ok(true) => ToolResult::text("kill requested"),
                Ok(false) => ToolResult::error("job not found"),
                Err(e) => ToolResult::error(e),
            },
            Some("output") => match self.jobs.output(id).await {
                Some(bytes) => bounded_text(ctx, &String::from_utf8_lossy(&bytes), true)
                    .await
                    .unwrap_or_else(ToolResult::error),
                None => ToolResult::error("job not found"),
            },
            Some("status") => self
                .jobs
                .status(id)
                .await
                .map(|(status, _)| ToolResult {
                    content: vec![Part::Text {
                        text: format!(
                            "status: {}",
                            status.map_or("running".into(), |s| s.to_string())
                        ),
                    }],
                    details: json!({"exit_code": status}),
                    is_error: false,
                })
                .unwrap_or_else(|| ToolResult::error("job not found")),
            _ => ToolResult::error("action must be status, output, or kill"),
        }
    }
}

struct Cut {
    bytes: Vec<u8>,
    truncated: bool,
}
fn truncate_head(text: &str) -> Cut {
    let bytes = text.as_bytes();
    if bytes.len() <= MAX_OUTPUT_BYTES && text.lines().count() <= MAX_OUTPUT_LINES {
        return Cut {
            bytes: bytes.to_vec(),
            truncated: false,
        };
    }
    let mut end = 0;
    for (lines, line) in text.split_inclusive('\n').enumerate() {
        if lines >= MAX_OUTPUT_LINES || end + line.len() > MAX_OUTPUT_BYTES {
            break;
        }
        end += line.len();
    }
    Cut {
        bytes: bytes[..end].to_vec(),
        truncated: true,
    }
}
fn truncate_tail(bytes: &[u8]) -> Cut {
    let text = String::from_utf8_lossy(bytes);
    if bytes.len() <= MAX_OUTPUT_BYTES && text.lines().count() <= MAX_OUTPUT_LINES {
        return Cut {
            bytes: bytes.to_vec(),
            truncated: false,
        };
    }
    let original_start = bytes.len().saturating_sub(MAX_OUTPUT_BYTES);
    let mut start = original_start;
    while start < bytes.len() && bytes[start] != b'\n' {
        start += 1;
    }
    if start == bytes.len() {
        // A single line can exceed the byte limit; keep its tail rather than
        // returning an empty result.
        start = original_start;
    }
    let mut out = bytes[start.min(bytes.len())..].to_vec();
    if String::from_utf8_lossy(&out).lines().count() > MAX_OUTPUT_LINES {
        let output_text = String::from_utf8_lossy(&out);
        let keep: Vec<_> = output_text.lines().rev().take(MAX_OUTPUT_LINES).collect();
        out = keep
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
    }
    Cut {
        bytes: out,
        truncated: true,
    }
}

fn canonicalize_with_missing(path: &Path) -> Result<PathBuf, ToolError> {
    if path.exists() {
        return path.canonicalize().map_err(ToolError::Io);
    }
    let mut suffix = Vec::new();
    let mut ancestor = path.to_path_buf();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return Err(ToolError::PathEscape(path.to_path_buf()));
        };
        suffix.push(name.to_os_string());
        ancestor.pop();
    }
    let mut resolved = ancestor.canonicalize().map_err(ToolError::Io)?;
    for name in suffix.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn output_dir(ctx: &ToolContext) -> PathBuf {
    let safe = ctx
        .run_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    ctx.output_dir.join(safe)
}

async fn bounded_text(ctx: &ToolContext, text: &str, tail: bool) -> Result<ToolResult, ToolError> {
    let cut = if tail {
        truncate_tail(text.as_bytes())
    } else {
        truncate_head(text)
    };
    let full_path = if cut.truncated {
        let dir = output_dir(ctx);
        fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("output-{}.log", uuid::Uuid::now_v7()));
        fs::write(&path, text).await?;
        Some(path)
    } else {
        None
    };
    let mut visible = String::from_utf8_lossy(&cut.bytes).into_owned();
    if let Some(path) = &full_path {
        visible.push_str(&format!(
            "\n\n[output truncated; full output: {}]",
            path.display()
        ));
    }
    Ok(ToolResult {
        content: vec![Part::Text { text: visible }],
        details: json!({
            "truncated": cut.truncated,
            "full_output_path": full_path.as_ref().map(|path| path.display().to_string()),
        }),
        is_error: false,
    })
}

fn ignored_walk(path: &Path) -> ignore::Walk {
    WalkBuilder::new(path)
        .hidden(false)
        .add_custom_ignore_filename(".gitignore")
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
}

pub fn default_tools() -> Vec<Arc<dyn Tool>> {
    let jobs = BashJobManager::default();
    let mutations = FileMutationQueue::new();
    vec![
        Arc::new(ReadTool),
        Arc::new(WriteTool::new(mutations.clone())),
        Arc::new(EditTool::new(mutations)),
        Arc::new(LsTool),
        Arc::new(FindTool),
        Arc::new(GrepTool),
        Arc::new(BashTool::with_jobs(jobs.clone())),
        Arc::new(BashJobTool { jobs }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[tokio::test]
    async fn edit_requires_unique_replacements() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "a a").await.unwrap();
        let ctx = ToolContext::new(dir.path(), "run", dir.path().join("runs"));
        assert!(
            EditTool::default()
                .call(
                    &ctx,
                    json!({"path":"a.txt","edits":[{"oldText":"a","newText":"b"}]})
                )
                .await
                .is_error
        );
    }
    #[tokio::test]
    async fn bash_sanitizes_api_keys_and_returns_error_value() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "run", dir.path().join("runs"));
        let result = BashTool::default()
            .call(
                &ctx,
                json!({"command":"printf '%s' \"${SECRET_API_KEY-unset}\"; exit 3"}),
            )
            .await;
        assert!(result.is_error);
        assert!(matches!(&result.content[0], Part::Text { text } if text.contains("unset")));
    }
    #[tokio::test]
    async fn write_and_read_work() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new(dir.path(), "run", dir.path().join("runs"));
        let mutations = FileMutationQueue::new();
        assert!(
            !WriteTool::new(mutations)
                .call(&ctx, json!({"path":"nested/a","content":"hello"}))
                .await
                .is_error
        );
        let result = ReadTool.call(&ctx, json!({"path":"nested/a"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn read_returns_supported_images_as_base64_parts() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pixel.PNG");
        fs::write(&path, b"fake-png").await.unwrap();
        let result = ReadTool
            .call(
                &ToolContext::new(dir.path(), "run", dir.path().join("runs")),
                json!({"path":"pixel.PNG"}),
            )
            .await;
        assert!(!result.is_error);
        assert!(
            matches!(&result.content[0], Part::Image { mime, data } if mime == "image/png" && base64::engine::general_purpose::STANDARD.decode(data).unwrap() == b"fake-png")
        );
    }

    #[tokio::test]
    async fn find_and_grep_respect_gitignore() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored/\nsecret.txt\n")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("ignored"))
            .await
            .unwrap();
        fs::write(dir.path().join("visible.txt"), "needle")
            .await
            .unwrap();
        fs::write(dir.path().join("secret.txt"), "needle")
            .await
            .unwrap();
        fs::write(dir.path().join("ignored/hidden.txt"), "needle")
            .await
            .unwrap();
        let ctx = ToolContext::new(dir.path(), "run", dir.path().join("runs"));
        let found = FindTool.call(&ctx, json!({"pattern":"**/*.txt"})).await;
        let found_text = match &found.content[0] {
            Part::Text { text } => text,
            _ => panic!("find must return text"),
        };
        assert!(found_text.contains("visible.txt"));
        assert!(!found_text.contains("hidden.txt"));
        assert!(!found_text.contains("secret.txt"));
        let grep = GrepTool.call(&ctx, json!({"pattern":"needle"})).await;
        let grep_text = match &grep.content[0] {
            Part::Text { text } => text,
            _ => panic!("grep must return text"),
        };
        assert!(grep_text.contains("visible.txt"));
        assert!(!grep_text.contains("hidden.txt"));
        assert!(!grep_text.contains("secret.txt"));
    }

    #[tokio::test]
    async fn truncated_read_and_bash_save_complete_output_under_run() {
        let dir = tempdir().unwrap();
        let content = (0..2_100)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("large.txt"), &content)
            .await
            .unwrap();
        let ctx = ToolContext::new(dir.path(), "run-output", dir.path().join("runs"));
        let read = ReadTool.call(&ctx, json!({"path":"large.txt"})).await;
        assert_eq!(read.details["truncated"], true);
        let read_path = read.details["full_output_path"].as_str().unwrap();
        assert!(read_path.contains("runs/run-output"));
        assert_eq!(fs::read_to_string(read_path).await.unwrap(), content);

        let bash = BashTool::default()
            .call(
                &ctx,
                json!({"command":"for i in $(seq 1 2100); do echo line-$i; done"}),
            )
            .await;
        assert_eq!(bash.details["truncated"], true);
        let bash_path = bash.details["full_output_path"].as_str().unwrap();
        assert!(bash_path.contains("runs/run-output"));
        assert!(fs::read_to_string(bash_path)
            .await
            .unwrap()
            .contains("line-2100"));
    }

    #[tokio::test]
    async fn bash_job_kill_ends_the_process_group() {
        let manager = BashJobManager::default();
        let dir = tempdir().unwrap();
        let job_id = manager
            .start("sleep 30 & wait", dir.path(), &HashMap::new())
            .await
            .unwrap();
        assert!(manager.kill(&job_id).await.unwrap());
        for _ in 0..40 {
            if manager.status(&job_id).await.unwrap().0.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("process group did not terminate after kill");
    }
}
