//! Per-host bootstrap cache for fast reconnects.
//!
//! The cache is disposable UI state. It contains no password and is accepted
//! only when both its host key and the remembered server node match the
//! connection being opened.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use macbot_client_core::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const CACHE_DIR: &str = "Library/Application Support/MacBot/cache";

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
    last_seq: u64,
    state: Value,
}

/// Return the default cache root without creating it.
pub fn default_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(CACHE_DIR))
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

pub fn save(
    root: impl AsRef<Path>,
    host_id: &str,
    node_id: &str,
    state: &AppState,
) -> Result<PathBuf> {
    let path = path_for(root, host_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cache path has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create cache directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;

    let value = state.to_bootstrap_cache();
    let last_seq = value
        .get("seq")
        .and_then(Value::as_u64)
        .unwrap_or(state.last_seq);
    let envelope = CacheEnvelope {
        host_id: host_id.to_owned(),
        node_id: node_id.to_owned(),
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
}
