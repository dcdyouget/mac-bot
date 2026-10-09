//! Durable, per-host queue for idempotent chat sends.
//!
//! The outbox contains only retry metadata and the original `chat.send`
//! parameters. Credentials are rejected before serialization and are never
//! written to this store. A node mismatch is treated as a cold start so a
//! queue from another server cannot be replayed accidentally.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type PendingMessages = BTreeMap<String, Value>;

#[derive(Debug, Deserialize, Serialize)]
struct OutboxEnvelope {
    host_id: String,
    node_id: String,
    pending_messages: PendingMessages,
}

/// Return the default outbox root without creating it.
pub fn default_root() -> Result<PathBuf> {
    crate::storage_paths::data_subdir("outbox")
}

/// Return the host-scoped outbox file below `root`.
pub fn path_for(root: impl AsRef<Path>, host_id: &str) -> Result<PathBuf> {
    validate_component(host_id, "host id")?;
    Ok(root.as_ref().join(format!("{host_id}.json")))
}

/// Load pending messages from the default `MacBot/outbox` directory.
///
/// Missing files and a different server node both mean an empty queue.
pub fn load(host_id: &str, node_id: &str) -> Result<PendingMessages> {
    load_at(default_root()?, host_id, node_id)
}

/// Save pending messages in the default `MacBot/outbox` directory.
pub fn save(host_id: &str, node_id: &str, pending: &PendingMessages) -> Result<()> {
    save_at(default_root()?, host_id, node_id, pending)
}

/// Testable variant of [`load`] that reads from a caller-provided root.
pub fn load_at(root: impl AsRef<Path>, host_id: &str, node_id: &str) -> Result<PendingMessages> {
    let path = path_for(root, host_id)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read outbox {}", path.display()));
        }
    };
    let envelope: OutboxEnvelope = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode outbox {}", path.display()))?;
    if envelope.host_id != host_id || envelope.node_id != node_id {
        return Ok(BTreeMap::new());
    }
    validate_pending(&envelope.pending_messages)?;
    Ok(envelope.pending_messages)
}

/// Testable variant of [`save`] that writes below a caller-provided root.
pub fn save_at(
    root: impl AsRef<Path>,
    host_id: &str,
    node_id: &str,
    pending: &PendingMessages,
) -> Result<()> {
    let path = path_for(root, host_id)?;
    validate_pending(pending)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("outbox path has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create outbox directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;

    let envelope = OutboxEnvelope {
        host_id: host_id.to_owned(),
        node_id: node_id.to_owned(),
        pending_messages: pending.clone(),
    };
    let bytes = serde_json::to_vec(&envelope).context("encode outbox")?;
    let temporary = temporary_path(&path);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("create temporary outbox {}", temporary.display()))?;
        file.write_all(&bytes).context("write outbox")?;
        file.sync_all().context("sync outbox")?;
        set_mode(&temporary, 0o600)?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("replace outbox {}", path.display()))?;
        set_mode(&path, 0o600)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn validate_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(anyhow!("invalid {label} for outbox path"));
    }
    Ok(())
}

fn validate_pending(pending: &PendingMessages) -> Result<()> {
    for (id, value) in pending {
        if id.is_empty() {
            return Err(anyhow!("outbox entry id cannot be empty"));
        }
        reject_credentials(value, id)?;
    }
    Ok(())
}

fn reject_credentials(value: &Value, entry_id: &str) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key.to_ascii_lowercase();
                if matches!(
                    normalized.as_str(),
                    "password"
                        | "passphrase"
                        | "token"
                        | "access_token"
                        | "refresh_token"
                        | "api_key"
                        | "apikey"
                        | "secret"
                        | "authorization"
                        | "bearer"
                ) {
                    return Err(anyhow!(
                        "outbox entry {entry_id} contains credential field {key}"
                    ));
                }
                reject_credentials(child, entry_id)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                reject_credentials(child, entry_id)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    path.with_extension(format!("json.tmp-{}-{nonce}", std::process::id()))
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("open outbox directory {}", path.display()))?
        .sync_all()
        .context("sync outbox directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "macbot-outbox-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn restart_round_trip_retains_original_retry_params_and_request_id() {
        let root = temp_root("roundtrip");
        let mut pending = PendingMessages::new();
        pending.insert(
            "request-1".into(),
            json!({
                "retry_params": {
                    "chat_id": "chat-1",
                    "text": "hello",
                    "mentions": [],
                    "client_request_id": "stable-request-id"
                },
                "send_status": "queued",
                "chat_id": "chat-1"
            }),
        );
        save_at(&root, "host-1", "node-1", &pending).unwrap();

        // A second load models a process restart and must preserve the exact
        // idempotency key used by the first attempt.
        let restored = load_at(&root, "host-1", "node-1").unwrap();
        assert_eq!(restored, pending);
        assert_eq!(
            restored["request-1"]["retry_params"]["client_request_id"],
            "stable-request-id"
        );
        let path = path_for(&root, "host-1").unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn node_mismatch_is_ignored_and_successful_send_is_removed() {
        let root = temp_root("node-and-delete");
        let mut pending = PendingMessages::new();
        pending.insert(
            "request-1".into(),
            json!({"retry_params":{"chat_id":"chat-1","client_request_id":"stable"},"send_status":"queued","chat_id":"chat-1"}),
        );
        save_at(&root, "host-1", "node-1", &pending).unwrap();
        assert!(load_at(&root, "host-1", "node-2").unwrap().is_empty());

        pending.remove("request-1");
        save_at(&root, "host-1", "node-1", &pending).unwrap();
        assert!(load_at(&root, "host-1", "node-1").unwrap().is_empty());
        let disk = fs::read_to_string(path_for(&root, "host-1").unwrap()).unwrap();
        assert!(!disk.contains("stable"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn credentials_are_rejected_before_disk_write() {
        let root = temp_root("credentials");
        let mut pending = PendingMessages::new();
        pending.insert(
            "request-1".into(),
            json!({"retry_params":{"chat_id":"chat-1","password":"must-not-persist"}}),
        );
        assert!(save_at(&root, "host-1", "node-1", &pending).is_err());
        assert!(!path_for(&root, "host-1").unwrap().exists());
        let _ = fs::remove_dir_all(root);
    }
}
