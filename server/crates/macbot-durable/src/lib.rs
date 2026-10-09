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
    #[error("job not found: {0}")]
    JobNotFound(String),
    #[error("job is already terminal: {0}")]
    TerminalJob(String),
    #[error("invalid durable record: {0}")]
    Invalid(String),
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
    inbox: Vec<InboxItem>,
    submissions: HashMap<String, SendMsgReceipt>,
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
            inbox: Vec::new(),
            submissions: HashMap::new(),
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
            commit_seq: self
                .store
                .read_jsonl::<JobCommit>("data/jobs/commits.jsonl")?
                .len() as u64
                + 1,
        };
        self.persist_job(job.clone())?;
        self.jobs.insert(id, job.clone());
        Ok(job)
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.get(id)
    }
    pub fn jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.values()
    }

    /// Commit is write-ahead: the durable commit record is synced before the
    /// snapshot. `unsafe_replay` marks a side effect that cannot be repeated.
    pub fn commit(
        &mut self,
        id: &str,
        status: JobStatus,
        checkpoint: Value,
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
        let seq = self
            .store
            .read_jsonl::<JobCommit>("data/jobs/commits.jsonl")?
            .len() as u64
            + 1;
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
        self.store
            .append_jsonl("data/jobs/commits.jsonl", &JobCommit { job: job.clone() })?;
        self.store
            .write_snapshot(format!("data/jobs/{}.json", id), &job)?;
        self.jobs.insert(id.into(), job.clone());
        Ok(job)
    }

    /// On restart, running jobs are resumable; jobs whose checkpoint represents
    /// a side effect stop in `Suspended` until the orchestrator decides.
    pub fn resume_plan(&mut self) -> Result<Vec<Job>, DurableError> {
        let first_seq = self
            .store
            .read_jsonl::<JobCommit>("data/jobs/commits.jsonl")?
            .len() as u64
            + 1;
        let ids = self
            .jobs
            .values()
            .filter(|job| {
                matches!(job.status, JobStatus::Running | JobStatus::Queued) && job.unsafe_replay
            })
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();
        let mut changed = Vec::with_capacity(ids.len());
        for (next_seq, id) in (first_seq..).zip(ids) {
            validate_id("job", &id)?;
            let current = self
                .jobs
                .get(&id)
                .cloned()
                .ok_or_else(|| DurableError::JobNotFound(id.clone()))?;
            let job = Job {
                status: JobStatus::Suspended,
                commit_seq: next_seq,
                updated_at: now(),
                ..current
            };
            self.store
                .append_jsonl("data/jobs/commits.jsonl", &JobCommit { job: job.clone() })?;
            self.store
                .write_snapshot(format!("data/jobs/{}.json", job.id), &job)?;
            self.jobs.insert(id, job.clone());
            changed.push(job);
        }
        Ok(changed)
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
        if let Some(existing) = self
            .inbox
            .iter()
            .find(|x| x.job_id == job_id && x.message_id == message_id)
        {
            return Ok(existing.clone());
        }
        let item = InboxItem {
            id: format!("in_{}", uuid::Uuid::now_v7()),
            job_id: job_id.into(),
            message_id,
            text: text.into(),
            delivery: Delivery::Queued,
            at: now(),
        };
        self.store.append_jsonl("data/inbox.jsonl", &item)?;
        self.inbox.push(item.clone());
        Ok(item)
    }

    pub fn deliver_next(&mut self, job_id: &str) -> Result<Option<InboxItem>, DurableError> {
        let index = self
            .inbox
            .iter()
            .position(|x| x.job_id == job_id && x.delivery == Delivery::Queued);
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

    fn persist_job(&self, job: Job) -> Result<(), DurableError> {
        validate_id("job", &job.id)?;
        self.store
            .append_jsonl("data/jobs/commits.jsonl", &JobCommit { job: job.clone() })?;
        self.store
            .write_snapshot(format!("data/jobs/{}.json", job.id), &job)?;
        Ok(())
    }

    fn rewrite_inbox(&self) -> Result<(), DurableError> {
        // The inbox is small control-plane state; rewrite it atomically after
        // each delivery transition so a crash cannot duplicate a steer.
        self.store.write_snapshot("data/inbox.json", &self.inbox)?;
        Ok(())
    }

    fn recover(&mut self) -> Result<(), DurableError> {
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
                        self.jobs.insert(job.id.clone(), job);
                    }
                }
            }
        }
        for commit in self
            .store
            .read_jsonl::<JobCommit>("data/jobs/commits.jsonl")?
        {
            let job = commit.job;
            if self
                .jobs
                .get(&job.id)
                .is_none_or(|current| current.commit_seq < job.commit_seq)
            {
                self.jobs.insert(job.id.clone(), job);
            }
        }
        self.inbox = match self
            .store
            .read_snapshot::<Vec<InboxItem>>("data/inbox.json")
        {
            Ok(Some(items)) => items,
            _ => self
                .store
                .read_jsonl("data/inbox.jsonl")
                .unwrap_or_default(),
        };
        for submission in self
            .store
            .read_jsonl::<Submission>("data/submissions.jsonl")?
        {
            self.submissions.insert(submission.key, submission.receipt);
        }
        Ok(())
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
}
