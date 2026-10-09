//! Periodic cleanup for uploaded files.
//!
//! Uploads are referenced by protocol file blocks (`root: "upload"`).  The
//! reference scan is deliberately conservative: malformed or unreadable
//! durable state returns an error, so the scheduler keeps the files instead of
//! guessing that they are unused.

use macbot_store::{Store, StoreError};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs, io,
    time::{Duration, SystemTime},
};
use thiserror::Error;

/// Uploads remain available for one day after their last filesystem change.
pub const UPLOAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Error)]
pub enum HousekeepingError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("filesystem: {0}")]
    Io(#[from] io::Error),
}

/// Delete unreferenced upload files older than [`UPLOAD_TTL`].
///
/// `store` must be the process' existing Store handle.  This function never
/// opens the store (and therefore never takes a second process lock), and its
/// `now` argument makes expiry behavior deterministic in tests.
pub fn cleanup_uploads(store: &Store, now: SystemTime) -> Result<usize, HousekeepingError> {
    let referenced = referenced_uploads(store)?;
    let uploads = store.root().join("uploads");
    if !uploads.exists() {
        return Ok(0);
    }

    let mut removed = 0;
    for entry in fs::read_dir(uploads)? {
        let entry = entry?;
        // Uploads are single files named by their upload id.  Do not follow
        // symlinks or recursively touch a directory placed beside them.
        if !entry.file_type()?.is_file() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if referenced.contains(&id) {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        let expired = now
            .duration_since(modified)
            .is_ok_and(|age| age >= UPLOAD_TTL);
        if !expired {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            // A concurrent upload cleanup or operator may have removed it.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(removed)
}

fn referenced_uploads(store: &Store) -> Result<HashSet<String>, HousekeepingError> {
    let mut referenced = HashSet::new();
    let chats = store.root().join("data/chats");
    if chats.exists() {
        for entry in fs::read_dir(chats)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let relative = entry
                .path()
                .join("messages.jsonl")
                .strip_prefix(store.root())
                .expect("chat path is below store root")
                .to_path_buf();
            for message in store.read_jsonl::<Value>(relative)? {
                collect_references(&message, &mut referenced);
            }
        }
    }

    if let Some(snapshot) = store.read_snapshot::<Value>("data/orchestrator/state.json")? {
        collect_references(&snapshot, &mut referenced);
    }
    Ok(referenced)
}

fn collect_references(value: &Value, referenced: &mut HashSet<String>) {
    match value {
        Value::Object(object) => {
            if object.get("root").and_then(Value::as_str) == Some("upload") {
                if let Some(root_id) = object.get("root_id").and_then(Value::as_str) {
                    referenced.insert(root_id.to_owned());
                }
            }
            for child in object.values() {
                collect_references(child, referenced);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_references(child, referenced);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn upload(store: &Store, id: &str, bytes: &[u8]) {
        let path = store.root().join("uploads").join(id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn removes_expired_unreferenced_and_keeps_referenced() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        upload(&store, "old", b"old");
        upload(&store, "kept", b"kept");
        store
            .append_jsonl(
                "data/chats/dm/messages.jsonl",
                &json!({"blocks":[{"root":"upload","root_id":"kept"}]}),
            )
            .unwrap();
        store
            .write_snapshot(
                "data/orchestrator/state.json",
                &json!({"artifact":{"file":{"root":"upload","root_id":"missing"}}}),
            )
            .unwrap();

        let now = SystemTime::now() + UPLOAD_TTL + Duration::from_secs(1);
        assert_eq!(cleanup_uploads(&store, now).unwrap(), 1);
        assert!(!dir.path().join("uploads/old").exists());
        assert!(dir.path().join("uploads/kept").exists());
    }

    #[test]
    fn fresh_upload_is_retained() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        upload(&store, "fresh", b"fresh");
        assert_eq!(cleanup_uploads(&store, SystemTime::now()).unwrap(), 0);
        assert!(dir.path().join("uploads/fresh").exists());
    }
}
