//! Small Rust durable runtime primitives.
//!
//! A job checkpoint is committed as an immutable JSONL record first, then its
//! latest snapshot is replaced atomically.  Recovery trusts the commit log and
//! replays records newer than (or missing from) snapshots.  A checkpoint which
//! was in a side-effecting tool is deliberately suspended on recovery so the
//! caller can ask for approval instead of replaying an unsafe operation.

use macbot_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DurableError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("job {job_id} commit {commit_seq} is durable, but its snapshot failed: {source}")]
    SnapshotAfterCommit {
        job_id: String,
        commit_seq: u64,
        #[source]
        source: StoreError,
    },
    #[error("job not found: {0}")]
    JobNotFound(String),
    #[error("job is already terminal: {0}")]
    TerminalJob(String),
    #[error("invalid durable record: {0}")]
    Invalid(String),
    #[error("durable commit sequence exhausted")]
    CommitSequenceExhausted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Waiting,
    Suspended,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub owner: String,
    pub kind: String,
    pub status: JobStatus,
    pub checkpoint: Value,
    pub unsafe_replay: bool,
    pub updated_at: u64,
    pub commit_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobSnapshotMetadata {
    pub len: u64,
    pub modified_nanos: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct JobCommit {
    job: Job,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Queued,
    Delivered,
    Read,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InboxItem {
    pub id: String,
    pub job_id: String,
    pub message_id: String,
    pub text: String,
    pub delivery: Delivery,
    pub at: u64,
}

fn merge_inbox(
    snapshot: Vec<InboxItem>,
    journal: Vec<InboxItem>,
    consumed_ids: &std::collections::HashSet<(String, String)>,
) -> Result<Vec<InboxItem>, DurableError> {
    let mut merged: Vec<InboxItem> =
        Vec::with_capacity(snapshot.len().saturating_add(journal.len()));
    let mut indexes: HashMap<(String, String), usize> = HashMap::new();
    for item in snapshot.into_iter().chain(journal) {
        let key = (item.job_id.clone(), item.message_id.clone());
        if let Some(index) = indexes.get(&key).copied() {
            if merged[index].text != item.text {
                return Err(DurableError::Invalid(format!(
                    "inbox message id {} for job {} was reused with different text",
                    item.message_id, item.job_id
                )));
            }
            // The snapshot is first and carries the latest delivery state.
            // Duplicate journal records can arise when a caller retries after
            // losing an older inbox snapshot; they must not create a second
            // logical delivery.
            if item.delivery == Delivery::Read || consumed_ids.contains(&(item.job_id, item.id)) {
                merged[index].delivery = Delivery::Read;
            }
            continue;
        }
        let mut item = item;
        if consumed_ids.contains(&(item.job_id.clone(), item.id.clone())) {
            item.delivery = Delivery::Read;
        }
        indexes.insert(key, merged.len());
        merged.push(item);
    }
    Ok(merged)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendMsgReceipt {
    pub run_id: String,
    pub call_id: String,
    pub message_id: String,
    pub intent: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Submission {
    key: String,
    receipt: SendMsgReceipt,
}

pub struct DurableRuntime {
    store: Store,
    jobs: HashMap<String, Job>,
    job_metadata: HashMap<String, JobSnapshotMetadata>,
    inbox: Vec<InboxItem>,
    submissions: HashMap<String, SendMsgReceipt>,
    /// The next durable commit sequence.  It is initialized while replaying
    /// the commit log and advanced only after an append succeeds, so normal
    /// commits do not rescan the complete JSONL history.
    next_commit_seq: Option<u64>,
}

impl DurableRuntime {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, DurableError> {
        Self::from_store(Store::open(root)?)
    }

    /// Attach durable state to an already-open store. This is the gateway's
    /// single-writer path: it reuses the existing process lock instead of
    /// attempting to acquire a second `data/.lock` descriptor.
    pub fn from_store(store: Store) -> Result<Self, DurableError> {
        let mut runtime = Self {
            store,
            jobs: HashMap::new(),
            job_metadata: HashMap::new(),
            inbox: Vec::new(),
            submissions: HashMap::new(),
            next_commit_seq: Some(1),
        };
        runtime.recover()?;
        Ok(runtime)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn create_job(
        &mut self,
        owner: impl Into<String>,
        kind: impl Into<String>,
        checkpoint: Value,
    ) -> Result<Job, DurableError> {
        let id = format!("job_{}", uuid::Uuid::now_v7());
        let job = Job {
            id: id.clone(),
            owner: owner.into(),
            kind: kind.into(),
            status: JobStatus::Queued,
            checkpoint,
            unsafe_replay: false,
            updated_at: now(),
            commit_seq: self.next_commit_seq()?,
        };
        self.persist_job(job.clone())?;
        Ok(job)
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.get(id)
    }
    pub fn jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.values()
    }

    pub fn job_snapshot_metadata(&self, id: &str) -> Option<JobSnapshotMetadata> {
        self.job_metadata.get(id).copied()
    }

    /// Commit is write-ahead: the durable commit record is synced before the
    /// snapshot. `unsafe_replay` marks a side effect that cannot be repeated.
    /// `SnapshotAfterCommit` means the WAL and in-memory job already contain
    /// the new state; callers must not treat that error as a rolled-back commit.
    pub fn commit(
        &mut self,
        id: &str,
        status: JobStatus,
        mut checkpoint: Value,
        unsafe_replay: bool,
    ) -> Result<Job, DurableError> {
        validate_id("job", id)?;
        let current = self
            .jobs
            .get(id)
            .ok_or_else(|| DurableError::JobNotFound(id.into()))?;
        if matches!(
            current.status,
            JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled
        ) {
            return Err(DurableError::TerminalJob(id.into()));
        }
        // Engine continuation provenance is one atomic object. Ordinary
        // checkpoints carry it forward; a continuation path may replace it
        // only with a complete new identity, payload, and phase.
        if current.checkpoint.get("continuation").is_some() && !checkpoint.is_object() {
            return Err(DurableError::Invalid(
                "checkpoint must preserve continuation provenance as an object".into(),
            ));
        }
        if let Some(object) = checkpoint.as_object_mut() {
            if !object.contains_key("continuation") {
                if let Some(previous) = current.checkpoint.get("continuation") {
                    object.insert("continuation".into(), previous.clone());
                }
            }
            // Inbox consumption is durable execution metadata. Ordinary model
            // checkpoints carry its complete set forward so a crash between
            // checkpoint commit and inbox acknowledgement cannot duplicate an
            // already injected input.
            if !object.contains_key("runtime_inbox_consumed") {
                if let Some(previous) = current.checkpoint.get("runtime_inbox_consumed") {
                    object.insert("runtime_inbox_consumed".into(), previous.clone());
                }
            }
        }
        if let Some(continuation) = checkpoint.get("continuation") {
            let valid = continuation.get("id").and_then(Value::as_str).is_some()
                && continuation.get("payload").is_some()
                && continuation.get("phase").and_then(Value::as_str).is_some();
            if !valid {
                return Err(DurableError::Invalid(
                    "continuation checkpoint must contain id, payload, and phase".into(),
                ));
            }
        }
        let seq = self.next_commit_seq()?;
        let job = Job {
            id: current.id.clone(),
            owner: current.owner.clone(),
            kind: current.kind.clone(),
            status,
            checkpoint,
            unsafe_replay,
            updated_at: now(),
            commit_seq: seq,
        };
        self.persist_job(job.clone())?;
        Ok(job)
    }

    pub fn commit_with_inbox_input(
        &mut self,
        id: &str,
        status: JobStatus,
        mut checkpoint: Value,
        unsafe_replay: bool,
        inbox_item_id: &str,
    ) -> Result<Job, DurableError> {
        let previous = self
            .jobs
            .get(id)
            .ok_or_else(|| DurableError::JobNotFound(id.into()))?
            .checkpoint
            .get("runtime_inbox_consumed")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut consumed = previous;
        if !consumed
            .iter()
            .any(|value| value.as_str() == Some(inbox_item_id))
        {
            consumed.push(Value::String(inbox_item_id.to_owned()));
        }
        let object = checkpoint.as_object_mut().ok_or_else(|| {
            DurableError::Invalid("checkpoint must be an object when consuming inbox input".into())
        })?;
        object.insert("runtime_inbox_consumed".into(), Value::Array(consumed));
        self.commit(id, status, checkpoint, unsafe_replay)
    }

    /// On restart, running jobs are resumable; jobs whose checkpoint represents
    /// a side effect stop in `Suspended` until the orchestrator decides.
    pub fn resume_plan(&mut self) -> Result<Vec<Job>, DurableError> {
        let mut resumable = self
            .jobs
            .values()
            .filter(|job| matches!(job.status, JobStatus::Running | JobStatus::Queued))
            .cloned()
            .collect::<Vec<_>>();
        // HashMap iteration is deliberately unordered. Keep recovery stable so
        // the backend starts jobs in the same order after every restart.
        resumable.sort_by(|left, right| {
            left.commit_seq
                .cmp(&right.commit_seq)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut plan = Vec::with_capacity(resumable.len());
        for current in resumable {
            if !current.unsafe_replay {
                plan.push(current);
                continue;
            }
            validate_id("job", &current.id)?;
            let seq = self.next_commit_seq()?;
            let job = Job {
                status: JobStatus::Suspended,
                commit_seq: seq,
                updated_at: now(),
                ..current
            };
            self.persist_job(job.clone())?;
            plan.push(job);
        }
        Ok(plan)
    }

    pub fn enqueue_steer(
        &mut self,
        job_id: &str,
        message_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<InboxItem, DurableError> {
        if !self.jobs.contains_key(job_id) {
            return Err(DurableError::JobNotFound(job_id.into()));
        }
        let message_id = message_id.into();
        let text = text.into();
        if let Some(existing) = self
            .inbox
            .iter()
            .find(|x| x.job_id == job_id && x.message_id == message_id)
        {
            if existing.text != text {
                return Err(DurableError::Invalid(format!(
                    "inbox message id {message_id} was reused with different text"
                )));
            }
            return Ok(existing.clone());
        }
        let item = InboxItem {
            id: format!("in_{}", uuid::Uuid::now_v7()),
            job_id: job_id.into(),
            message_id,
            text,
            delivery: Delivery::Queued,
            at: now(),
        };
        self.store.append_jsonl("data/inbox.jsonl", &item)?;
        self.inbox.push(item.clone());
        Ok(item)
    }

    pub fn deliver_next(&mut self, job_id: &str) -> Result<Option<InboxItem>, DurableError> {
        let consumed = self
            .jobs
            .get(job_id)
            .and_then(|job| job.checkpoint.get("runtime_inbox_consumed"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let consumed_ids = consumed
            .iter()
            .filter_map(Value::as_str)
            .collect::<std::collections::HashSet<_>>();
        let mut already_consumed = Vec::new();
        let index = self.inbox.iter().enumerate().find_map(|(candidate, item)| {
            if item.job_id != job_id || item.delivery == Delivery::Read {
                return None;
            }
            if consumed_ids.contains(item.id.as_str()) {
                already_consumed.push(candidate);
                None
            } else {
                Some(candidate)
            }
        });
        if !already_consumed.is_empty() {
            for candidate in already_consumed {
                self.inbox[candidate].delivery = Delivery::Read;
            }
            self.rewrite_inbox()?;
        }
        let Some(index) = index else { return Ok(None) };
        self.inbox[index].delivery = Delivery::Delivered;
        self.rewrite_inbox()?;
        Ok(Some(self.inbox[index].clone()))
    }

    pub fn mark_read(&mut self, item_id: &str) -> Result<Option<InboxItem>, DurableError> {
        let Some(index) = self.inbox.iter().position(|x| x.id == item_id) else {
            return Ok(None);
        };
        self.inbox[index].delivery = Delivery::Read;
        self.rewrite_inbox()?;
        Ok(Some(self.inbox[index].clone()))
    }

    pub fn inbox<'a>(&'a self, job_id: &'a str) -> impl Iterator<Item = &'a InboxItem> + 'a {
        self.inbox.iter().filter(move |x| x.job_id == job_id)
    }

    /// Idempotent send_msg admission. The returned receipt is persisted before
    /// a caller emits the message event, so replaying the same call is safe.
    pub fn send_msg(
        &mut self,
        run_id: &str,
        call_id: &str,
        intent: &str,
        message_id: &str,
        payload: Value,
    ) -> Result<SendMsgReceipt, DurableError> {
        Ok(self
            .send_msg_once(run_id, call_id, intent, message_id, payload)?
            .0)
    }

    /// Idempotent send admission with a creation bit. The execution layer can
    /// use the bit to avoid repeating an external message side effect after a
    /// crash between durable admission and transport delivery.
    pub fn send_msg_once(
        &mut self,
        run_id: &str,
        call_id: &str,
        intent: &str,
        message_id: &str,
        payload: Value,
    ) -> Result<(SendMsgReceipt, bool), DurableError> {
        let key = format!("{run_id}:{call_id}");
        if let Some(receipt) = self.submissions.get(&key) {
            return Ok((receipt.clone(), false));
        }
        let receipt = SendMsgReceipt {
            run_id: run_id.into(),
            call_id: call_id.into(),
            message_id: message_id.into(),
            intent: intent.into(),
            payload,
        };
        self.store.append_jsonl(
            "data/submissions.jsonl",
            &Submission {
                key: key.clone(),
                receipt: receipt.clone(),
            },
        )?;
        self.submissions.insert(key, receipt.clone());
        Ok((receipt, true))
    }

    fn persist_job(&mut self, job: Job) -> Result<(), DurableError> {
        validate_id("job", &job.id)?;
        self.store
            .append_jsonl("data/jobs/commits.jsonl", &JobCommit { job: job.clone() })?;
        self.advance_commit_seq(job.commit_seq);
        // The synced WAL is authoritative even if the derived snapshot cannot
        // be replaced. Never expose the previous (possibly nonterminal) job to
        // a late worker after the commit has already taken effect.
        self.jobs.insert(job.id.clone(), job.clone());
        self.job_metadata.remove(&job.id);
        self.store
            .write_snapshot(format!("data/jobs/{}.json", job.id), &job)
            .map_err(|source| DurableError::SnapshotAfterCommit {
                job_id: job.id.clone(),
                commit_seq: job.commit_seq,
                source,
            })?;
        self.refresh_job_metadata(&job.id);
        Ok(())
    }

    fn rewrite_inbox(&self) -> Result<(), DurableError> {
        // The inbox is small control-plane state; rewrite it atomically after
        // each delivery transition so a crash cannot duplicate a steer.
        self.store.write_snapshot("data/inbox.json", &self.inbox)?;
        Ok(())
    }

    fn recover(&mut self) -> Result<(), DurableError> {
        let mut max_commit_seq = 0;
        if let Ok(entries) = std::fs::read_dir(self.store.root().join("data/jobs")) {
            for entry in entries {
                let path = entry?.path();
                if path.extension().is_some_and(|x| x == "json") {
                    // A torn snapshot is recoverable: the immutable commit
                    // log below remains authoritative.
                    if let Ok(Some(job)) = self
                        .store
                        .read_snapshot::<Job>(path.strip_prefix(self.store.root()).unwrap())
                    {
                        max_commit_seq = max_commit_seq.max(job.commit_seq);
                        self.jobs.insert(job.id.clone(), job.clone());
                        self.refresh_job_metadata(&job.id);
                    }
                }
            }
        }
        for commit in self
            .store
            .read_jsonl::<JobCommit>("data/jobs/commits.jsonl")?
        {
            let job = commit.job;
            max_commit_seq = max_commit_seq.max(job.commit_seq);
            if self
                .jobs
                .get(&job.id)
                .is_none_or(|current| current.commit_seq < job.commit_seq)
            {
                self.job_metadata.remove(&job.id);
                self.jobs.insert(job.id.clone(), job);
            }
        }
        self.next_commit_seq = max_commit_seq.checked_add(1);
        let snapshot = self
            .store
            .read_snapshot::<Vec<InboxItem>>("data/inbox.json")?
            .unwrap_or_default();
        let journal = self.store.read_jsonl::<InboxItem>("data/inbox.jsonl")?;
        let consumed_ids = self
            .jobs
            .values()
            .flat_map(|job| {
                job.checkpoint
                    .get("runtime_inbox_consumed")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str())
                    .map(move |id| (job.id.clone(), id.to_owned()))
            })
            .collect();
        self.inbox = merge_inbox(snapshot, journal, &consumed_ids)?;
        for submission in self
            .store
            .read_jsonl::<Submission>("data/submissions.jsonl")?
        {
            self.submissions.insert(submission.key, submission.receipt);
        }
        Ok(())
    }

    fn next_commit_seq(&self) -> Result<u64, DurableError> {
        self.next_commit_seq
            .ok_or(DurableError::CommitSequenceExhausted)
    }

    fn advance_commit_seq(&mut self, committed_seq: u64) {
        self.next_commit_seq = committed_seq.checked_add(1);
    }

    fn refresh_job_metadata(&mut self, id: &str) {
        let path = self.store.root().join(format!("data/jobs/{id}.json"));
        let Ok(metadata) = std::fs::metadata(path) else {
            self.job_metadata.remove(id);
            return;
        };
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        self.job_metadata.insert(
            id.to_owned(),
            JobSnapshotMetadata {
                len: metadata.len(),
                modified_nanos,
            },
        );
    }
}

fn validate_id(kind: &str, id: &str) -> Result<(), DurableError> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_control())
    {
        return Err(DurableError::Invalid(format!("invalid {kind} id")));
    }
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn recovery_replays_commit_newer_than_snapshot() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 0}))
            .unwrap();
        rt.commit(
            &job.id,
            JobStatus::Running,
            serde_json::json!({"step": 1}),
            false,
        )
        .unwrap();
        // A commit log is the source of truth even if a snapshot is stale.
        std::fs::write(
            dir.path()
                .join("data/jobs/")
                .join(format!("{}.json", job.id)),
            "{\"bad\":true}\n",
        )
        .unwrap();
        drop(rt);
        let rt = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(
            rt.job(&job.id).unwrap().checkpoint,
            serde_json::json!({"step": 1})
        );
    }

    #[test]
    fn continuous_commits_use_the_cached_sequence() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let mut job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 0}))
            .unwrap();
        assert_eq!(job.commit_seq, 1);
        for step in 1..=32 {
            job = rt
                .commit(
                    &job.id,
                    JobStatus::Running,
                    serde_json::json!({"step": step}),
                    false,
                )
                .unwrap();
            assert_eq!(job.commit_seq, step + 1);
            assert_eq!(rt.next_commit_seq, Some(step + 2));
        }
    }

    #[test]
    fn recovery_initializes_sequence_after_a_large_history() {
        let dir = tempdir().unwrap();
        let jobs_dir = dir.path().join("data/jobs");
        std::fs::create_dir_all(&jobs_dir).unwrap();
        let mut history = String::new();
        for seq in 1..=1024 {
            let job = Job {
                id: format!("job_history_{seq}"),
                owner: "bot".into(),
                kind: "demo".into(),
                status: JobStatus::Running,
                checkpoint: serde_json::json!({"step": seq}),
                unsafe_replay: false,
                updated_at: seq,
                commit_seq: seq,
            };
            history.push_str(&serde_json::to_string(&JobCommit { job }).unwrap());
            history.push('\n');
        }
        std::fs::write(jobs_dir.join("commits.jsonl"), history).unwrap();

        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(rt.next_commit_seq, Some(1025));
        let job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 1025}))
            .unwrap();
        assert_eq!(job.commit_seq, 1025);
    }

    #[test]
    fn reopened_runtime_continues_after_the_last_commit() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 0}))
            .unwrap();
        let committed = rt
            .commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"step": 1}),
                false,
            )
            .unwrap();
        let last_seq = committed.commit_seq;
        drop(rt);

        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(rt.next_commit_seq, Some(last_seq + 1));
        let committed = rt
            .commit(
                &job.id,
                JobStatus::Waiting,
                serde_json::json!({"step": 2}),
                false,
            )
            .unwrap();
        assert_eq!(committed.commit_seq, last_seq + 1);
    }

    #[test]
    fn commit_sequence_reports_exhaustion_instead_of_reusing_max() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 0}))
            .unwrap();

        rt.next_commit_seq = Some(u64::MAX - 1);
        let committed = rt
            .commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"step": 1}),
                false,
            )
            .unwrap();
        assert_eq!(committed.commit_seq, u64::MAX - 1);
        assert_eq!(rt.next_commit_seq, Some(u64::MAX));

        let committed = rt
            .commit(
                &job.id,
                JobStatus::Waiting,
                serde_json::json!({"step": 2}),
                false,
            )
            .unwrap();
        assert_eq!(committed.commit_seq, u64::MAX);
        assert_eq!(rt.next_commit_seq, None);
        assert!(matches!(
            rt.commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"step": 3}),
                false,
            ),
            Err(DurableError::CommitSequenceExhausted)
        ));
    }

    #[test]
    fn appended_commit_with_failed_snapshot_is_not_reused_after_reopen() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 0}))
            .unwrap();
        let snapshot = dir
            .path()
            .join("data/jobs")
            .join(format!("{}.json", job.id));
        std::fs::remove_file(&snapshot).unwrap();
        std::fs::create_dir(&snapshot).unwrap();

        assert!(rt
            .commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"step": 1}),
                false,
            )
            .is_err());
        assert_eq!(rt.next_commit_seq, Some(3));
        drop(rt);
        std::fs::remove_dir(&snapshot).unwrap();

        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(
            rt.job(&job.id).unwrap().checkpoint,
            serde_json::json!({"step": 1})
        );
        assert_eq!(rt.next_commit_seq, Some(3));
        let committed = rt
            .commit(
                &job.id,
                JobStatus::Waiting,
                serde_json::json!({"step": 2}),
                false,
            )
            .unwrap();
        assert_eq!(committed.commit_seq, 3);
    }

    #[test]
    fn persisted_new_job_remains_visible_after_snapshot_failure() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = Job {
            id: "job_snapshot_failure".into(),
            owner: "bot".into(),
            kind: "demo".into(),
            status: JobStatus::Queued,
            checkpoint: serde_json::json!({"run_id":"stable"}),
            unsafe_replay: false,
            updated_at: now(),
            commit_seq: 1,
        };
        let snapshot = dir.path().join(format!("data/jobs/{}.json", job.id));
        std::fs::create_dir_all(&snapshot).unwrap();
        assert!(matches!(
            rt.persist_job(job.clone()),
            Err(DurableError::SnapshotAfterCommit { .. })
        ));
        assert_eq!(rt.job(&job.id), Some(&job));
        assert_eq!(rt.next_commit_seq, Some(2));
        std::fs::remove_dir(&snapshot).unwrap();
        drop(rt);
        let recovered = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(recovered.job(&job.id), Some(&job));
    }

    #[test]
    fn wal_append_failure_does_not_change_job_or_sequence() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt.create_job("bot", "demo", serde_json::json!({})).unwrap();
        let wal = dir.path().join("data/jobs/commits.jsonl");
        let saved = dir.path().join("data/jobs/saved.jsonl");
        std::fs::rename(&wal, &saved).unwrap();
        std::fs::create_dir(&wal).unwrap();
        assert!(matches!(
            rt.commit(&job.id, JobStatus::Cancelled, serde_json::json!({}), false),
            Err(DurableError::Store(_))
        ));
        assert_eq!(rt.job(&job.id), Some(&job));
        assert_eq!(rt.next_commit_seq, Some(2));
        std::fs::remove_dir(&wal).unwrap();
        std::fs::rename(&saved, &wal).unwrap();
        drop(rt);
        let recovered = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(recovered.job(&job.id), Some(&job));
    }

    #[test]
    fn failed_snapshot_keeps_committed_terminal_state_in_memory() {
        for status in [JobStatus::Done, JobStatus::Failed, JobStatus::Cancelled] {
            let dir = tempdir().unwrap();
            let mut rt = DurableRuntime::open(dir.path()).unwrap();
            let job = rt.create_job("bot", "demo", serde_json::json!({})).unwrap();
            let snapshot = dir.path().join(format!("data/jobs/{}.json", job.id));
            std::fs::remove_file(&snapshot).unwrap();
            std::fs::create_dir(&snapshot).unwrap();
            assert!(rt
                .commit(
                    &job.id,
                    status.clone(),
                    serde_json::json!({"terminal":true}),
                    false
                )
                .is_err());
            assert_eq!(rt.job(&job.id).unwrap().status, status);
            assert!(rt.job_snapshot_metadata(&job.id).is_none());
            assert!(matches!(
                rt.commit(&job.id, JobStatus::Running, serde_json::json!({}), true),
                Err(DurableError::TerminalJob(_))
            ));
            std::fs::remove_dir(&snapshot).unwrap();
            drop(rt);
            let recovered = DurableRuntime::open(dir.path()).unwrap();
            assert_eq!(recovered.job(&job.id).unwrap().status, status);
        }
    }

    #[test]
    fn failed_snapshot_keeps_unsafe_recovery_suspended_in_memory() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt.create_job("bot", "demo", serde_json::json!({})).unwrap();
        rt.commit(&job.id, JobStatus::Running, serde_json::json!({}), true)
            .unwrap();
        let snapshot = dir.path().join(format!("data/jobs/{}.json", job.id));
        std::fs::remove_file(&snapshot).unwrap();
        std::fs::create_dir(&snapshot).unwrap();
        assert!(rt.resume_plan().is_err());
        assert_eq!(rt.job(&job.id).unwrap().status, JobStatus::Suspended);
        assert!(rt.resume_plan().unwrap().is_empty());
        std::fs::remove_dir(&snapshot).unwrap();
        drop(rt);
        let recovered = DurableRuntime::open(dir.path()).unwrap();
        assert_eq!(recovered.job(&job.id).unwrap().status, JobStatus::Suspended);
    }

    #[test]
    fn unsafe_running_job_is_suspended_after_recovery() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt
            .create_job("bot", "bash", serde_json::json!({"command": "touch x"}))
            .unwrap();
        rt.commit(&job.id, JobStatus::Running, job.checkpoint.clone(), true)
            .unwrap();
        drop(rt);
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let changed = rt.resume_plan().unwrap();
        assert_eq!(changed[0].status, JobStatus::Suspended);
    }

    #[test]
    fn continuation_provenance_is_preserved_and_replaced_atomically() {
        let dir = tempdir().unwrap();
        let mut runtime = DurableRuntime::open(dir.path()).unwrap();
        let job = runtime
            .create_job("bot", "model_run", serde_json::json!({"run_id":"run-c"}))
            .unwrap();
        let entered = runtime.commit(
            &job.id,
            JobStatus::Running,
            serde_json::json!({"run_id":"run-c", "continuation":{"id":"continuation-c","payload":{"kind":"approval","approval_id":"a1"},"phase":"effect_unknown"}}),
            true,
        ).unwrap();
        let preserved = runtime
            .commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"run_id":"run-c", "messages":[{"role":"tool"}]}),
                false,
            )
            .unwrap();
        assert_eq!(preserved.checkpoint["continuation"]["id"], "continuation-c");
        let replaced = runtime
            .commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"run_id":"run-c", "continuation":{"id":"continuation-d","payload":{"kind":"decision","answer":"yes"},"phase":"input_committed"}}),
                false,
            )
            .unwrap();
        let advanced = runtime
            .commit(
                &job.id,
                JobStatus::Running,
                serde_json::json!({"run_id":"run-c", "messages":[{"role":"assistant"}]}),
                false,
            )
            .unwrap();
        assert_eq!(advanced.checkpoint["continuation"]["id"], "continuation-d");
        assert_eq!(
            advanced.checkpoint["continuation"]["payload"]["answer"],
            "yes"
        );
        assert_eq!(
            advanced.checkpoint["continuation"]["phase"],
            "input_committed"
        );
        drop(runtime);

        let mut recovered = DurableRuntime::open(dir.path()).unwrap();
        let plan = recovered.resume_plan().unwrap();
        assert_eq!(plan[0].status, JobStatus::Running);
        let recovered = recovered.job(&job.id).unwrap();
        assert_eq!(recovered.checkpoint["continuation"]["id"], "continuation-d");
        assert_eq!(
            recovered.checkpoint["continuation"]["payload"]["kind"],
            "decision"
        );
        assert!(advanced.commit_seq > replaced.commit_seq);
        assert!(replaced.commit_seq > entered.commit_seq);
    }

    #[test]
    fn resume_plan_returns_safe_work_and_suspends_unsafe_work() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let safe_queued = rt
            .create_job("bot", "dm", serde_json::json!({"run_id": "safe_queued"}))
            .unwrap();
        let safe_running = rt
            .create_job("bot", "dm", serde_json::json!({"run_id": "safe_running"}))
            .unwrap();
        rt.commit(
            &safe_running.id,
            JobStatus::Running,
            safe_running.checkpoint.clone(),
            false,
        )
        .unwrap();
        let unsafe_running = rt
            .create_job(
                "bot",
                "bash",
                serde_json::json!({"run_id": "unsafe_running"}),
            )
            .unwrap();
        rt.commit(
            &unsafe_running.id,
            JobStatus::Running,
            unsafe_running.checkpoint.clone(),
            true,
        )
        .unwrap();
        let waiting = rt
            .create_job("bot", "dm", serde_json::json!({"run_id": "waiting"}))
            .unwrap();
        rt.commit(
            &waiting.id,
            JobStatus::Waiting,
            waiting.checkpoint.clone(),
            false,
        )
        .unwrap();
        drop(rt);

        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let plan = rt.resume_plan().unwrap();
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan.iter()
                .find(|job| job.id == safe_queued.id)
                .unwrap()
                .status,
            JobStatus::Queued
        );
        assert_eq!(
            plan.iter()
                .find(|job| job.id == safe_running.id)
                .unwrap()
                .status,
            JobStatus::Running
        );
        assert_eq!(
            plan.iter()
                .find(|job| job.id == unsafe_running.id)
                .unwrap()
                .status,
            JobStatus::Suspended
        );
        assert!(plan.iter().all(|job| job.id != waiting.id));
        assert_eq!(
            rt.job(&unsafe_running.id).unwrap().status,
            JobStatus::Suspended
        );
    }

    #[test]
    fn terminal_job_cannot_be_overwritten_by_late_worker_commit() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt
            .create_job("bot", "demo", serde_json::json!({"step": 0}))
            .unwrap();
        rt.commit(&job.id, JobStatus::Cancelled, job.checkpoint, false)
            .unwrap();
        assert!(matches!(
            rt.commit(&job.id, JobStatus::Running, serde_json::json!({"step": 1}), true),
            Err(DurableError::TerminalJob(id)) if id == job.id
        ));
        assert_eq!(rt.job(&job.id).unwrap().status, JobStatus::Cancelled);
    }

    #[test]
    fn steer_delivery_is_ordered_and_send_msg_is_idempotent() {
        let dir = tempdir().unwrap();
        let mut rt = DurableRuntime::open(dir.path()).unwrap();
        let job = rt.create_job("bot", "chat", serde_json::json!({})).unwrap();
        let item = rt.enqueue_steer(&job.id, "msg_1", "please stop").unwrap();
        assert_eq!(
            rt.enqueue_steer(&job.id, "msg_1", "please stop").unwrap(),
            item
        );
        assert!(matches!(
            rt.enqueue_steer(&job.id, "msg_1", "different text"),
            Err(DurableError::Invalid(_))
        ));
        assert_eq!(rt.inbox(&job.id).next().unwrap().delivery, Delivery::Queued);
        assert_eq!(
            rt.deliver_next(&job.id).unwrap().unwrap().delivery,
            Delivery::Delivered
        );
        assert_eq!(
            rt.mark_read(&item.id).unwrap().unwrap().delivery,
            Delivery::Read
        );
        let first = rt
            .send_msg("run", "call", "done", "msg", serde_json::json!({"x": 1}))
            .unwrap();
        let second = rt
            .send_msg(
                "run",
                "call",
                "done",
                "msg-other",
                serde_json::json!({"x": 2}),
            )
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn inbox_recovery_merges_journal_appends_after_snapshot() {
        let dir = tempdir().unwrap();
        let (job_id, second) = {
            let mut rt = DurableRuntime::open(dir.path()).unwrap();
            let job = rt.create_job("bot", "chat", serde_json::json!({})).unwrap();
            let first = rt.enqueue_steer(&job.id, "msg_a", "first").unwrap();
            assert_eq!(
                rt.deliver_next(&job.id).unwrap().unwrap().message_id,
                "msg_a"
            );
            rt.mark_read(&first.id).unwrap().unwrap();
            let second = rt.enqueue_steer(&job.id, "msg_b", "second").unwrap();
            (job.id, second)
        };

        let mut reopened = DurableRuntime::open(dir.path()).unwrap();
        let persisted_second = reopened
            .inbox(&job_id)
            .find(|item| item.message_id == "msg_b")
            .unwrap();
        assert_eq!(persisted_second, &second);
        assert_eq!(
            reopened
                .inbox(&job_id)
                .find(|item| item.message_id == "msg_a")
                .unwrap()
                .delivery,
            Delivery::Read
        );
        assert_eq!(
            reopened.deliver_next(&job_id).unwrap().unwrap().message_id,
            "msg_b"
        );
    }

    #[test]
    fn inbox_recovery_deduplicates_logical_input_and_honors_consumed_checkpoint() {
        let dir = tempdir().unwrap();
        let job_id = {
            let mut rt = DurableRuntime::open(dir.path()).unwrap();
            let job = rt.create_job("bot", "chat", serde_json::json!({})).unwrap();
            let first = rt.enqueue_steer(&job.id, "msg_a", "answer").unwrap();
            // Simulate an older queued snapshot whose logical input was
            // consumed under a retry-generated journal id.
            rt.store()
                .write_snapshot("data/inbox.json", &vec![first.clone()])
                .unwrap();
            rt.commit_with_inbox_input(
                &job.id,
                JobStatus::Running,
                serde_json::json!({}),
                false,
                "in_retry_duplicate",
            )
            .unwrap();
            rt.store()
                .append_jsonl(
                    "data/inbox.jsonl",
                    &InboxItem {
                        id: "in_retry_duplicate".into(),
                        job_id: job.id.clone(),
                        message_id: "msg_a".into(),
                        text: "answer".into(),
                        delivery: Delivery::Queued,
                        at: first.at + 1,
                    },
                )
                .unwrap();
            job.id
        };

        let mut reopened = DurableRuntime::open(dir.path()).unwrap();
        let items = reopened.inbox(&job_id).cloned().collect::<Vec<_>>();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message_id, "msg_a");
        assert_eq!(items[0].delivery, Delivery::Read);
        assert_eq!(reopened.deliver_next(&job_id).unwrap(), None);
        assert_eq!(
            reopened.inbox(&job_id).next().unwrap().delivery,
            Delivery::Read
        );
    }

    #[test]
    fn inbox_recovery_propagates_snapshot_and_journal_errors() {
        let snapshot_dir = tempdir().unwrap();
        std::fs::create_dir_all(snapshot_dir.path().join("data")).unwrap();
        std::fs::write(snapshot_dir.path().join("data/inbox.json"), b"not json").unwrap();
        assert!(matches!(
            DurableRuntime::open(snapshot_dir.path()),
            Err(DurableError::Store(_))
        ));

        let journal_dir = tempdir().unwrap();
        std::fs::create_dir_all(journal_dir.path().join("data")).unwrap();
        std::fs::write(journal_dir.path().join("data/inbox.jsonl"), b"not json\n").unwrap();
        assert!(matches!(
            DurableRuntime::open(journal_dir.path()),
            Err(DurableError::Store(_))
        ));
    }
}
