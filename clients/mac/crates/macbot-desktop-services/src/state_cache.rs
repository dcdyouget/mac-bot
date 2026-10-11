//! Per-host bootstrap cache for fast reconnects.
//!
//! The cache is disposable UI state. It contains no password and is accepted
//! only when both its host key and the remembered server node match the
//! connection being opened.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use macbot_client_core::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct CachedState {
    pub host_id: String,
    pub node_id: String,
    pub last_seq: u64,
    pub state: AppState,
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheEnvelope {
    host_id: String,
    node_id: String,
    #[serde(default)]
    stamp: u64,
    last_seq: u64,
    state: Value,
}

/// Return the default cache root without creating it.
pub fn default_root() -> Result<PathBuf> {
    crate::storage_paths::data_subdir("cache")
}

/// Return the cache file for a stable HostStore ID. IDs are deliberately
/// restricted to filename-safe characters so this function cannot escape the
/// cache root through a path component.
pub fn path_for(root: impl AsRef<Path>, host_id: &str) -> Result<PathBuf> {
    if host_id.is_empty()
        || !host_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(anyhow!("invalid host id for cache path"));
    }
    Ok(root.as_ref().join(format!("{host_id}.json")))
}

#[cfg(test)]
pub fn save(
    root: impl AsRef<Path>,
    host_id: &str,
    node_id: &str,
    state: &AppState,
) -> Result<PathBuf> {
    save_ordered(root, host_id, node_id, state, cache_stamp()?)
}

/// Save a cache snapshot in captured order. The caller should capture the
/// stamp before handing a snapshot to an asynchronous task. A later snapshot
/// may legitimately have a lower server sequence after a server reset, so
/// ordering is based on the local capture stamp rather than `last_seq`.
pub fn save_ordered(
    root: impl AsRef<Path>,
    host_id: &str,
    node_id: &str,
    state: &AppState,
    stamp: u64,
) -> Result<PathBuf> {
    let path = path_for(root, host_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cache path has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create cache directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;

    // The lock is separate from the atomically replaced cache file, so a
    // writer cannot lose the lock by renaming the cache underneath it.
    let lock_path = path.with_extension("lock");
    let lock_file: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("open state cache lock {}", lock_path.display()))?;
    set_mode(&lock_path, 0o600)?;
    lock_file.lock().context("lock state cache")?;

    // A newer captured snapshot wins even when its server cursor rolled back.
    // Caches from a different node are never allowed to block a new node.
    if let Ok(bytes) = fs::read(&path)
        && let Ok(existing) = serde_json::from_slice::<CacheEnvelope>(&bytes)
        && existing.node_id == node_id
        && existing.stamp > stamp
    {
        return Ok(path);
    }

    let value = state.to_bootstrap_cache();
    let last_seq = value
        .get("seq")
        .and_then(Value::as_u64)
        .unwrap_or(state.last_seq);
    let envelope = CacheEnvelope {
        host_id: host_id.to_owned(),
        node_id: node_id.to_owned(),
        stamp,
        last_seq,
        state: value,
    };
    let bytes = serde_json::to_vec(&envelope).context("encode state cache")?;
    let temporary = temporary_path(&path);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("create temporary cache {}", temporary.display()))?;
        file.write_all(&bytes).context("write state cache")?;
        file.sync_all().context("sync state cache")?;
        set_mode(&temporary, 0o600)?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("replace state cache {}", path.display()))?;
        set_mode(&path, 0o600)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|()| path)
}

pub fn cache_stamp() -> Result<u64> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("cache clock is before Unix epoch")?
        .as_nanos();
    Ok(nanos.try_into().unwrap_or(u64::MAX))
}

/// Load a cache only when it belongs to `host_id` and the remembered node.
/// A missing or stale cache is a normal cold-start result and returns `None`.
pub fn load(
    root: impl AsRef<Path>,
    host_id: &str,
    expected_node_id: &str,
) -> Result<Option<CachedState>> {
    let path = path_for(root, host_id)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read state cache {}", path.display()));
        }
    };
    let envelope: CacheEnvelope = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode state cache {}", path.display()))?;
    if envelope.host_id != host_id || envelope.node_id != expected_node_id {
        return Ok(None);
    }
    let state_seq = envelope
        .state
        .get("seq")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if state_seq != envelope.last_seq {
        return Err(anyhow!("state cache cursor mismatch"));
    }
    let state = AppState::from_bootstrap_cache(envelope.state);
    Ok(Some(CachedState {
        host_id: envelope.host_id,
        node_id: envelope.node_id,
        last_seq: envelope.last_seq,
        state,
    }))
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
        .with_context(|| format!("open cache directory {}", path.display()))?
        .sync_all()
        .context("sync cache directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("macbot-state-cache-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn saves_atomic_private_cache_and_round_trips_state() {
        let root = temp_root("roundtrip");
        let mut state = AppState::default();
        state.apply_bootstrap(json!({"seq": 7, "bots": [{"id":"bot-1","name":"A"}]}));
        let path = save(&root, "host-1", "node-1", &state).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = load(&root, "host-1", "node-1").unwrap().unwrap();
        assert_eq!(loaded.last_seq, 7);
        assert_eq!(loaded.state.bots["bot-1"]["name"], "A");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_other_host_or_node_and_cursor_corruption() {
        let root = temp_root("validation");
        let state = AppState::default();
        let path = save(&root, "host-1", "node-1", &state).unwrap();
        assert!(load(&root, "host-2", "node-1").unwrap().is_none());
        assert!(load(&root, "host-1", "node-2").unwrap().is_none());
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["last_seq"] = json!(42);
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(load(&root, "host-1", "node-1").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn older_async_save_is_skipped_by_capture_stamp() {
        let root = temp_root("ordered-old");
        let mut newer = AppState::default();
        newer.apply_bootstrap(json!({"seq": 7, "bots": [{"id":"new"}]}));
        save_ordered(&root, "host-1", "node-1", &newer, 200).unwrap();

        let mut older = AppState::default();
        older.apply_bootstrap(json!({"seq": 3, "bots": [{"id":"old"}]}));
        save_ordered(&root, "host-1", "node-1", &older, 100).unwrap();

        let loaded = load(&root, "host-1", "node-1").unwrap().unwrap();
        assert_eq!(loaded.last_seq, 7);
        assert!(loaded.state.bots.contains_key("new"));
        assert!(!loaded.state.bots.contains_key("old"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn newer_stamp_can_commit_a_legal_server_cursor_rollback() {
        let root = temp_root("ordered-rollback");
        let mut old_server_state = AppState::default();
        old_server_state.apply_bootstrap(json!({"seq": 200, "bots": [{"id":"old"}]}));
        save_ordered(&root, "host-1", "node-1", &old_server_state, 100).unwrap();

        let mut reset_state = AppState::default();
        reset_state.apply_bootstrap(json!({"seq": 4, "bots": [{"id":"reset"}]}));
        save_ordered(&root, "host-1", "node-1", &reset_state, 200).unwrap();

        let loaded = load(&root, "host-1", "node-1").unwrap().unwrap();
        assert_eq!(loaded.last_seq, 4);
        assert!(loaded.state.bots.contains_key("reset"));
        assert!(!loaded.state.bots.contains_key("old"));
        let _ = fs::remove_dir_all(root);
    }
}
