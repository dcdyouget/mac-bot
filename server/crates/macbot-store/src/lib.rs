//! Crash-safe JSON storage used by `macbotd`.
//!
//! JSONL files are the source of truth.  A record is made durable before its
//! caller publishes the corresponding event, and snapshots are replaced with
//! an fsync+rename sequence.  `Store::open` repairs only an incomplete final
//! JSONL record; a malformed complete record is reported instead of silently
//! losing data.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid JSON in {path} at line {line}: {source}")]
    Json {
        path: PathBuf,
        line: usize,
        source: serde_json::Error,
    },
    #[error("invalid snapshot JSON in {path}: {source}")]
    Snapshot {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("data directory is already locked: {0}")]
    Locked(PathBuf),
    #[error("path escapes store root: {0}")]
    PathEscape(PathBuf),
}

#[derive(Clone)]
pub struct Store {
    root: Arc<PathBuf>,
    lock: Arc<File>,
    write_lock: Arc<Mutex<()>>,
    files: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    event_seq: Arc<Mutex<u64>>,
    event_keys: Arc<Mutex<HashSet<String>>>,
    event_payload_fingerprints: Arc<Mutex<HashSet<u64>>>,
    chat_sequences: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub event: String,
    pub data: Value,
}

impl Store {
    /// Open a store and take the single-writer process lock.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        // Keep the root canonical so `/var` and `/private/var` resolve to the
        // same store on macOS and symlinked store roots cannot change the
        // boundary check later.
        let root = root.canonicalize()?;
        fs::create_dir_all(root.join("data"))?;
        let lock_path = root.join("data/.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        #[cfg(unix)]
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(StoreError::Locked(lock_path));
        }
        let store = Self {
            root: Arc::new(root),
            lock: Arc::new(lock),
            write_lock: Arc::new(Mutex::new(())),
            files: Arc::new(Mutex::new(HashMap::new())),
            event_seq: Arc::new(Mutex::new(0)),
            event_keys: Arc::new(Mutex::new(HashSet::new())),
            event_payload_fingerprints: Arc::new(Mutex::new(HashSet::new())),
            chat_sequences: Arc::new(Mutex::new(())),
        };
        store.repair_jsonl_files()?;
        let seq = store
            .read_jsonl::<Event>("data/events/events.jsonl")?
            .iter()
            .map(|event| event.seq)
            .max()
            .unwrap_or(0);
        *store
            .event_seq
            .lock()
            .expect("event sequence lock poisoned") = seq;
        let events = store.read_jsonl::<Value>("data/events/events.jsonl")?;
        *store.event_keys.lock().expect("event key lock poisoned") = events
            .iter()
            .filter_map(|event| event.get("_operation_key").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        *store
            .event_payload_fingerprints
            .lock()
            .expect("event payload fingerprint lock poisoned") = events
            .iter()
            .filter_map(event_payload_fingerprint_from_wire)
            .collect();
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        self.root.as_ref()
    }

    /// Allocate durable message positions shared by every writer of a chat.
    /// An update to an existing message keeps its original position, including
    /// a streaming placeholder replaced by its final response. A caller may
    /// supply ordered legacy history to initialize a chat during migration.
    pub fn sequence_chat_messages(
        &self,
        chat_id: &str,
        messages: &[Value],
    ) -> Result<Vec<Value>, StoreError> {
        let component = chat_component(chat_id)?;
        let relative = format!("data/chats/{component}/sequences.jsonl");
        let _guard = self
            .chat_sequences
            .lock()
            .expect("chat sequence lock poisoned");
        let records = self.read_jsonl::<Value>(&relative)?;
        let mut positions = HashMap::new();
        let mut maximum = 0u64;
        for record in records {
            if let (Some(id), Some(seq)) = (
                record.get("message_id").and_then(Value::as_str),
                record.get("seq").and_then(Value::as_u64),
            ) {
                maximum = maximum.max(seq);
                positions.entry(id.to_owned()).or_insert(seq);
            }
        }
        let mut output = Vec::with_capacity(messages.len());
        for message in messages {
            let id = message
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "message id is required")
                })?;
            let seq = match positions.get(id).copied() {
                Some(seq) => seq,
                None => {
                    maximum = maximum.checked_add(1).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "chat sequence exhausted")
                    })?;
                    self.append_jsonl(
                        &relative,
                        &serde_json::json!({"message_id":id,"seq":maximum}),
                    )?;
                    positions.insert(id.to_owned(), maximum);
                    maximum
                }
            };
            let mut message = message.clone();
            message
                .as_object_mut()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "message must be an object")
                })?
                .insert("seq".into(), Value::from(seq));
            output.push(message);
        }
        Ok(output)
    }

    pub fn last_chat_sequence(&self, chat_id: &str) -> Result<u64, StoreError> {
        let component = chat_component(chat_id)?;
        Ok(self
            .read_jsonl::<Value>(format!("data/chats/{component}/sequences.jsonl"))?
            .iter()
            .filter_map(|record| record.get("seq").and_then(Value::as_u64))
            .max()
            .unwrap_or(0))
    }

    fn resolve(&self, relative: impl AsRef<Path>) -> Result<PathBuf, StoreError> {
        let relative = relative.as_ref();
        if relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(StoreError::PathEscape(relative.to_path_buf()));
        }
        let path = if relative.is_absolute() {
            relative.to_path_buf()
        } else {
            self.root.join(relative)
        };
        let canonical = canonicalize_with_missing_tail(&path).map_err(StoreError::Io)?;
        if !canonical.starts_with(self.root.as_path()) {
            return Err(StoreError::PathEscape(path));
        }
        Ok(path)
    }

    fn file_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.files.lock().expect("store lock poisoned");
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Append one complete JSON value and sync it to disk.
    pub fn append_jsonl<T: Serialize>(
        &self,
        relative: impl AsRef<Path>,
        value: &T,
    ) -> Result<(), StoreError> {
        let path = self.resolve(relative)?;
        let lock = self.file_lock(&path);
        let _file_guard = lock.lock().expect("file lock poisoned");
        let _write_guard = self.write_lock.lock().expect("store lock poisoned");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(value).map_err(|e| StoreError::Json {
            path: path.clone(),
            line: 0,
            source: e,
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }

    /// Read JSONL records, repairing an incomplete final line if needed.
    pub fn read_jsonl<T: DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<Vec<T>, StoreError> {
        let path = self.resolve(relative)?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        let lock = self.file_lock(&path);
        let _guard = lock.lock().expect("file lock poisoned");
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        let complete_len = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        if complete_len < bytes.len() {
            let file = OpenOptions::new().write(true).open(&path)?;
            file.set_len(complete_len as u64)?;
            file.sync_data()?;
            bytes.truncate(complete_len);
        }
        let mut records = Vec::new();
        for (idx, line) in bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .enumerate()
        {
            records.push(
                serde_json::from_slice(line).map_err(|source| StoreError::Json {
                    path: path.clone(),
                    line: idx + 1,
                    source,
                })?,
            );
        }
        Ok(records)
    }

    /// Atomically replace a JSON snapshot.  The temporary file is in the same
    /// directory so rename is atomic on the filesystem used by macOS.
    pub fn write_snapshot<T: Serialize>(
        &self,
        relative: impl AsRef<Path>,
        value: &T,
    ) -> Result<(), StoreError> {
        let path = self.resolve(relative)?;
        let lock = self.file_lock(&path);
        let _file_guard = lock.lock().expect("file lock poisoned");
        let _write_guard = self.write_lock.lock().expect("store lock poisoned");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let bytes = serde_json::to_vec_pretty(value).map_err(|source| StoreError::Snapshot {
            path: path.clone(),
            source,
        })?;
        let result = (|| {
            let mut f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
            f.write_all(&bytes)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
            fs::rename(&tmp, &path)?;
            if let Some(parent) = path.parent() {
                File::open(parent)?.sync_all()?;
            }
            Ok::<(), io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map_err(StoreError::Io)
    }

    pub fn read_snapshot<T: DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> Result<Option<T>, StoreError> {
        let path = self.resolve(relative)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| StoreError::Snapshot { path, source })
    }

    pub fn last_event_seq(&self) -> Result<u64, StoreError> {
        Ok(*self.event_seq.lock().expect("event sequence lock poisoned"))
    }

    /// Append a globally sequenced event. The event is synced before return.
    pub fn append_event(&self, event: impl Into<String>, data: Value) -> Result<Event, StoreError> {
        Ok(self
            .append_event_inner(None, event.into(), data)?
            .expect("unkeyed append always writes"))
    }

    /// Append an operation's event exactly once, including across restart.
    /// The receipt lives in the same synced JSONL row as the event, avoiding
    /// a separate receipt snapshot's crash window. It is internal metadata;
    /// `events_since` exposes only the ordinary Event fields.
    pub fn append_event_once(
        &self,
        key: &str,
        event: impl Into<String>,
        data: Value,
    ) -> Result<Option<Event>, StoreError> {
        self.append_event_inner(Some(key), event.into(), data)
    }

    /// Fast in-memory receipt lookup. The event log remains authoritative.
    pub fn has_event_key(&self, key: &str) -> bool {
        self.event_keys
            .lock()
            .expect("event key lock poisoned")
            .contains(key)
    }

    /// Conservative payload lookup. A hit only means that an exact durable
    /// comparison may be useful; callers must verify the event log before
    /// suppressing an append.
    pub fn event_payload_might_contain(&self, event: &str, data: &Value) -> bool {
        self.event_payload_fingerprints
            .lock()
            .expect("event payload fingerprint lock poisoned")
            .contains(&event_payload_fingerprint(event, data))
    }

    fn append_event_inner(
        &self,
        key: Option<&str>,
        event: String,
        data: Value,
    ) -> Result<Option<Event>, StoreError> {
        // Serialize sequence allocation and append together. Calling
        // `append_jsonl` here would invert the file/store lock order, so this
        // method performs the small append inline.
        let path = self.resolve("data/events/events.jsonl")?;
        let file_lock = self.file_lock(&path);
        let _file_guard = file_lock.lock().expect("file lock poisoned");
        let _write_guard = self.write_lock.lock().expect("store lock poisoned");
        let mut keys = self.event_keys.lock().expect("event key lock poisoned");
        if key.is_some_and(|key| keys.contains(key)) {
            return Ok(None);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut next_seq = self.event_seq.lock().expect("event sequence lock poisoned");
        let seq = *next_seq + 1;
        let record = Event { seq, event, data };
        let mut raw = serde_json::to_value(&record).map_err(|source| StoreError::Json {
            path: path.clone(),
            line: 0,
            source,
        })?;
        if let Some(key) = key {
            raw["_operation_key"] = Value::String(key.to_owned());
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(
            &serde_json::to_vec(&raw).map_err(|source| StoreError::Json {
                path: path.clone(),
                line: 0,
                source,
            })?,
        )?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        *next_seq = seq;
        if let Some(key) = key {
            keys.insert(key.to_owned());
        }
        self.event_payload_fingerprints
            .lock()
            .expect("event payload fingerprint lock poisoned")
            .insert(event_payload_fingerprint(&record.event, &record.data));
        Ok(Some(record))
    }

    pub fn events_since(&self, seq: u64) -> Result<Vec<Event>, StoreError> {
        Ok(self
            .read_jsonl::<Event>("data/events/events.jsonl")?
            .into_iter()
            .filter(|e| e.seq > seq)
            .collect())
    }

    fn repair_jsonl_files(&self) -> Result<(), StoreError> {
        fn visit(dir: &Path, store: &Store) -> Result<(), StoreError> {
            for entry in fs::read_dir(dir)? {
                let path = entry?.path();
                if path.is_dir() {
                    visit(&path, store)?;
                } else if path.extension().is_some_and(|x| x == "jsonl") {
                    let rel = path.strip_prefix(store.root()).unwrap_or(&path);
                    let _: Vec<Value> = store.read_jsonl(rel)?;
                }
            }
            Ok(())
        }
        visit(&self.root.join("data"), self)
    }

    /// Keep the lock file alive for the lifetime of the store.
    pub fn is_locked(&self) -> bool {
        self.lock.metadata().is_ok()
    }
}

fn event_payload_fingerprint(event: &str, data: &Value) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    event.hash(&mut hasher);
    hash_event_value(data, &mut hasher);
    hasher.finish()
}

fn hash_event_value(value: &Value, hasher: &mut impl Hasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(value) => {
            1u8.hash(hasher);
            value.hash(hasher);
        }
        Value::Number(value) => {
            2u8.hash(hasher);
            value.to_string().hash(hasher);
        }
        Value::String(value) => {
            3u8.hash(hasher);
            value.hash(hasher);
        }
        Value::Array(values) => {
            4u8.hash(hasher);
            values.len().hash(hasher);
            for value in values {
                hash_event_value(value, hasher);
            }
        }
        Value::Object(values) => {
            5u8.hash(hasher);
            values.len().hash(hasher);
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                key.hash(hasher);
                hash_event_value(&values[key], hasher);
            }
        }
    }
}

fn event_payload_fingerprint_from_wire(value: &Value) -> Option<u64> {
    Some(event_payload_fingerprint(
        value.get("event")?.as_str()?,
        value.get("data")?,
    ))
}

/// Canonicalize the existing ancestor of a path and append the missing tail.
/// This catches symlink escapes even when the final file has not been created.
fn chat_component(chat_id: &str) -> Result<String, StoreError> {
    if chat_id.is_empty() || chat_id == "." || chat_id == ".." || chat_id.contains(['/', '\\']) {
        return Err(StoreError::PathEscape(PathBuf::from(chat_id)));
    }
    Ok(chat_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect())
}

fn canonicalize_with_missing_tail(path: &Path) -> io::Result<PathBuf> {
    let mut probe = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        match fs::canonicalize(&probe) {
            Ok(mut canonical) => {
                for component in tail.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&probe) {
                    // Another writer may create an ordinary ancestor between
                    // canonicalize and metadata. Retry instead of turning a
                    // valid concurrent first append into an ENOENT failure.
                    Ok(metadata) if !metadata.file_type().is_symlink() => continue,
                    Ok(_) => return Err(error), // A dangling symlink is not a missing tail.
                    Err(metadata_error) if metadata_error.kind() != io::ErrorKind::NotFound => {
                        return Err(metadata_error);
                    }
                    Err(_) => {}
                }
                let Some(name) = probe.file_name() else {
                    return Err(error);
                };
                tail.push(name.to_os_string());
                let Some(parent) = probe.parent() else {
                    return Err(error);
                };
                probe = parent.to_path_buf();
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn operation_event_receipt_is_atomic_concurrent_and_survives_reopen() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let threads = (0..16)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .append_event_once(
                            "rpc:request:assignment.created",
                            "assignment.created",
                            serde_json::json!({"assignment":{"id":"task"}}),
                        )
                        .unwrap()
                        .is_some()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .filter(|written| *written)
                .count(),
            1
        );
        assert_eq!(store.last_event_seq().unwrap(), 1);
        let wire = serde_json::to_value(store.events_since(0).unwrap()[0].clone()).unwrap();
        assert!(wire.get("_operation_key").is_none());
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert!(store
            .append_event_once(
                "rpc:request:assignment.created",
                "assignment.created",
                Value::Null,
            )
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .append_event("message.created", Value::Null)
                .unwrap()
                .seq,
            2
        );
        assert_eq!(store.events_since(0).unwrap().len(), 2);
    }

    #[test]
    fn event_indexes_distinguish_new_payloads_and_rebuild_after_reopen() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let data = serde_json::json!({"message":{"id":"m1"}});
        store.append_event("message.created", data.clone()).unwrap();
        assert!(store.event_payload_might_contain("message.created", &data));
        assert!(store.event_payload_might_contain(
            "message.created",
            &serde_json::json!({"message":{"id":"m1"}})
        ));
        assert!(!store.event_payload_might_contain(
            "message.created",
            &serde_json::json!({"message":{"id":"m2"}})
        ));
        assert!(!store.has_event_key("repair:m1"));
        store
            .append_event_once("repair:m1", "message.created", serde_json::json!({"x":1}))
            .unwrap();
        assert!(store.has_event_key("repair:m1"));
        drop(store);

        let store = Store::open(dir.path()).unwrap();
        assert!(store.has_event_key("repair:m1"));
        assert!(store.event_payload_might_contain("message.created", &data));
        assert!(store.event_payload_might_contain("message.created", &serde_json::json!({"x":1})));
    }

    #[test]
    fn torn_event_does_not_restore_an_uncommitted_operation_receipt() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .append_event_once("first", "message.created", Value::Null)
            .unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.path().join("data/events/events.jsonl"))
            .unwrap();
        file.write_all(br#"{"seq":2,"_operation_key":"torn""#)
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert!(!store.has_event_key("torn"));
        assert_eq!(
            store
                .append_event_once("torn", "message.created", Value::Null)
                .unwrap()
                .unwrap()
                .seq,
            2
        );
        assert!(store
            .append_event_once("first", "message.created", Value::Null)
            .unwrap()
            .is_none());
    }

    #[test]
    fn chat_sequences_are_shared_and_updates_keep_their_position_after_recovery() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let rows = store
            .sequence_chat_messages(
                "chat",
                &[
                    serde_json::json!({"id":"user-1","seq":1}),
                    serde_json::json!({"id":"bot-1","seq":1,"streaming":true}),
                ],
            )
            .unwrap();
        assert_eq!(rows[0]["seq"], 1);
        assert_eq!(rows[1]["seq"], 2);
        let updated = store
            .clone()
            .sequence_chat_messages(
                "chat",
                &[
                    serde_json::json!({"id":"bot-1","seq":1,"streaming":false}),
                    serde_json::json!({"id":"user-2"}),
                ],
            )
            .unwrap();
        assert_eq!(updated[0]["seq"], 2);
        assert_eq!(updated[1]["seq"], 3);
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let rows = store
            .sequence_chat_messages(
                "chat",
                &[
                    serde_json::json!({"id":"bot-1"}),
                    serde_json::json!({"id":"bot-2"}),
                ],
            )
            .unwrap();
        assert_eq!(rows[0]["seq"], 2);
        assert_eq!(rows[1]["seq"], 4);
        assert_eq!(store.last_chat_sequence("chat").unwrap(), 4);
    }

    #[test]
    fn chat_sequence_recovery_repairs_a_torn_reservation_without_reusing_committed_positions() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .sequence_chat_messages("chat", &[serde_json::json!({"id":"reserved"})])
            .unwrap();
        let path = dir.path().join("data/chats/chat/sequences.jsonl");
        let mut log = OpenOptions::new().append(true).open(&path).unwrap();
        log.write_all(br#"{"message_id":"torn","seq":2"#).unwrap();
        log.sync_all().unwrap();
        drop(log);
        drop(store);

        let store = Store::open(dir.path()).unwrap();
        let messages = store
            .sequence_chat_messages(
                "chat",
                &[
                    serde_json::json!({"id":"reserved"}),
                    serde_json::json!({"id":"next"}),
                ],
            )
            .unwrap();
        assert_eq!(messages[0]["seq"], 1);
        assert_eq!(messages[1]["seq"], 2);
        assert_eq!(store.last_chat_sequence("chat").unwrap(), 2);
        assert_eq!(
            store
                .read_jsonl::<Value>("data/chats/chat/sequences.jsonl")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn concurrent_chat_sequence_allocations_are_unique() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let threads = (0..12)
            .map(|index| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .sequence_chat_messages(
                            "chat",
                            &[serde_json::json!({"id":format!("message-{index}")})],
                        )
                        .unwrap()[0]["seq"]
                        .as_u64()
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let mut sequences = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        sequences.sort_unstable();
        assert_eq!(sequences, (1..=12).collect::<Vec<_>>());
    }

    #[test]
    fn truncates_incomplete_last_line_and_keeps_previous_records() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .append_jsonl("data/log.jsonl", &serde_json::json!({"n": 1}))
            .unwrap();
        let path = dir.path().join("data/log.jsonl");
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(br#"{"n": 2"#).unwrap();
        drop(f);
        let records: Vec<Value> = store.read_jsonl("data/log.jsonl").unwrap();
        assert_eq!(records, vec![serde_json::json!({"n": 1})]);
        assert_eq!(fs::read_to_string(path).unwrap(), "{\"n\":1}\n");
    }

    #[test]
    fn snapshot_replace_is_readable() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .write_snapshot("data/state.json", &serde_json::json!({"version": 2}))
            .unwrap();
        assert_eq!(
            store.read_snapshot::<Value>("data/state.json").unwrap(),
            Some(serde_json::json!({"version": 2}))
        );
    }

    #[test]
    fn large_snapshot_round_trips_from_complete_file() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let values = (0..4096)
            .map(|index| serde_json::json!({"index":index,"text":"snapshot"}))
            .collect::<Vec<_>>();
        store.write_snapshot("data/large.json", &values).unwrap();
        let restored = store
            .read_snapshot::<Vec<Value>>("data/large.json")
            .unwrap()
            .unwrap();
        assert_eq!(restored.len(), values.len());
        assert_eq!(restored[4095], values[4095]);
    }

    #[test]
    fn events_are_monotonic() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(
            store.append_event("a", serde_json::json!({})).unwrap().seq,
            1
        );
        assert_eq!(
            store.append_event("b", serde_json::json!({})).unwrap().seq,
            2
        );
        assert_eq!(store.events_since(1).unwrap().len(), 1);
    }

    #[test]
    fn concurrent_event_appends_have_unique_sequences() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut threads = Vec::new();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        for index in 0..8 {
            let store = store.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                (0..16)
                    .map(|offset| {
                        store
                            .append_event(
                                "test",
                                serde_json::json!({"index": index, "offset": offset}),
                            )
                            .unwrap()
                            .seq
                    })
                    .collect::<Vec<_>>()
            }));
        }
        let mut seqs = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=128).collect::<Vec<_>>());
        assert_eq!(store.last_event_seq().unwrap(), 128);
    }

    #[test]
    fn rejects_parent_traversal_and_symlink_escape() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(matches!(
            store.append_jsonl("data/../outside.jsonl", &serde_json::json!({})),
            Err(StoreError::PathEscape(_))
        ));
        let outside = tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), dir.path().join("data/link")).unwrap();
        #[cfg(unix)]
        assert!(matches!(
            store.append_jsonl("data/link/escape.jsonl", &serde_json::json!({})),
            Err(StoreError::PathEscape(_))
        ));
    }
}
