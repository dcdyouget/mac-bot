//! Persistent multi-host connection metadata for the macOS client.
//!
//! Host metadata is kept in `~/Library/Application Support/MacBot/hosts.json`.
//! Passwords are deliberately kept out of that file and stored in an encrypted
//! local vault. The vault uses a random per-installation key file and AES-GCM;
//! both files are private to the current user (0600) in a private directory
//! (0700).

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const VAULT_FILE: &str = "host-passwords.enc";
const VAULT_KEY_FILE: &str = "host-passwords.key";
const VAULT_LOCK_FILE: &str = "host-passwords.lock";
const VAULT_VERSION: u8 = 1;
const VAULT_AAD: &[u8] = b"macbot-host-password-v1";
const VAULT_KEY_LEN: usize = 32;
const VAULT_NONCE_LEN: usize = 12;
const STORE_FILE: &str = "hosts.json";
const LOCK_FILE: &str = "hosts.lock";

/// Non-secret information needed to reconnect to one Mac Bot host.
///
/// `id` is the local stable identifier used as the encrypted-vault account.
/// The server's `node_id` may be absent before the first successful handshake.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HostRecord {
    pub id: String,
    pub name: String,
    pub node_id: Option<String>,
    pub addresses: Vec<String>,
    pub last_seq: u64,
    pub device_id: String,
}

/// A collection of remembered hosts. The store path is private so callers do
/// not accidentally write credentials or metadata to an arbitrary location.
#[derive(Clone)]
pub struct HostStore {
    path: PathBuf,
    device_id: String,
    hosts: Vec<HostRecord>,
}

impl HostStore {
    /// Load the default store. A missing file means a fresh, empty store.
    pub fn load() -> Result<Self> {
        Self::load_from_path(default_store_path()?)
    }

    /// Load a store from a path. This is useful for tests and does not change
    /// the production default used by [`Self::load`].
    pub fn load_from_path(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let hosts = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Vec<HostRecord>>(&bytes)
                .with_context(|| format!("decode host store {}", path.display()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(error).with_context(|| format!("read host store {}", path.display()));
            }
        };

        let device_id = hosts
            .first()
            .map(|host| host.device_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Ok(Self {
            path,
            device_id,
            hosts,
        })
    }

    /// Serialize a cross-process read/modify/write operation through the
    /// private lock file. The store is loaded after taking the lock, so an
    /// older in-memory snapshot cannot overwrite a newer host record.
    ///
    /// Callers must mutate the supplied store and return from the closure;
    /// the updated metadata is saved after the closure succeeds.
    pub fn with_store<T, F>(mutation: F) -> Result<T>
    where
        F: FnOnce(&mut Self) -> Result<T>,
    {
        Self::with_store_at(default_store_path()?, mutation)
    }

    fn with_store_at<T, F>(path: impl Into<PathBuf>, mutation: F) -> Result<T>
    where
        F: FnOnce(&mut Self) -> Result<T>,
    {
        let path = path.into();
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("host store has no parent directory"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create host store directory {}", parent.display()))?;
        set_mode(parent, 0o700)?;
        let lock_path = parent.join(LOCK_FILE);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .with_context(|| format!("open host store lock {}", lock_path.display()))?;
        set_mode(&lock_path, 0o600)?;
        lock.lock()
            .with_context(|| format!("lock host store {}", lock_path.display()))?;

        let mut store = Self::load_from_path(path)?;
        let result = mutation(&mut store)?;
        store.save()?;
        Ok(result)
    }

    /// Build an empty store at a custom path. Production callers should use
    /// [`Self::load`], but this keeps storage tests isolated from user data.
    #[cfg(test)]
    pub fn new_at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            device_id: Uuid::new_v4().to_string(),
            hosts: Vec::new(),
        }
    }

    pub fn hosts(&self) -> &[HostRecord] {
        &self.hosts
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn get(&self, id: &str) -> Option<&HostRecord> {
        self.hosts.iter().find(|host| host.id == id)
    }

    /// Persist metadata with an atomic rename and mode 0600.
    pub fn save(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("host store has no parent directory"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create host store directory {}", parent.display()))?;
        set_mode(parent, 0o700)?;

        let temp_path = temporary_path(&self.path);
        let bytes = serde_json::to_vec_pretty(&self.hosts).context("encode host store")?;
        let write_result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp_path)
                .with_context(|| format!("create temporary host store {}", temp_path.display()))?;
            file.write_all(&bytes).context("write host store")?;
            file.sync_all().context("sync host store")?;
            set_mode(&temp_path, 0o600)?;
            fs::rename(&temp_path, &self.path)
                .with_context(|| format!("replace host store {}", self.path.display()))?;
            set_mode(&self.path, 0o600)?;
            sync_directory(parent)
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    /// Remember or update a host, storing its password in the encrypted vault.
    ///
    /// Existing records are matched by server `node_id`, then by their first
    /// address. Password storage succeeds before metadata is written. If the
    /// encrypted-vault storage fails, the metadata file is left untouched.
    pub fn remember(
        &mut self,
        name: impl Into<String>,
        addresses: Vec<String>,
        password: &str,
        node_id: Option<String>,
        last_seq: u64,
    ) -> Result<HostRecord> {
        let name = name.into().trim().to_owned();
        if name.is_empty() {
            return Err(anyhow!("host name cannot be empty"));
        }
        let addresses = addresses
            .into_iter()
            .map(|address| address.trim().to_owned())
            .filter(|address| !address.is_empty())
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(anyhow!("host requires at least one address"));
        }

        let index = node_id
            .as_deref()
            .and_then(|id| {
                self.hosts
                    .iter()
                    .position(|host| host.node_id.as_deref() == Some(id))
            })
            .or_else(|| {
                let first_address = addresses.first()?;
                self.hosts
                    .iter()
                    .position(|host| host.addresses.first() == Some(first_address))
            });
        let id = index
            .map(|index| self.hosts[index].id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // The password never enters HostRecord or the metadata JSON buffer.
        store_password(&id, password.as_bytes())
            .with_context(|| format!("store encrypted password for host {id}"))?;

        let record = HostRecord {
            id: id.clone(),
            name,
            node_id,
            addresses,
            last_seq,
            device_id: self.device_id.clone(),
        };
        let previous_hosts = self.hosts.clone();
        promote_host(&mut self.hosts, record.clone());
        if let Err(error) = self.save() {
            self.hosts = previous_hosts;
            return Err(error).context("persist host metadata after vault update");
        }
        Ok(record)
    }

    /// Read the password for a record from the encrypted local vault.
    pub fn password(&self, record: &HostRecord) -> Result<String> {
        let password = read_password(&record.id)
            .with_context(|| format!("read password for host {}", record.id))?;
        String::from_utf8(password).context("host password is not valid UTF-8")
    }

    /// Remove metadata and its encrypted password. Missing records are a no-op.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let Some(index) = self.hosts.iter().position(|host| host.id == id) else {
            return Ok(());
        };
        delete_password(id)?;
        let removed = self.hosts.remove(index);
        if let Err(error) = self.save() {
            self.hosts.insert(index, removed);
            return Err(error).context("persist host metadata after vault removal");
        }
        Ok(())
    }
}

/// Keep the most recently connected Host first while preserving the relative
/// order of all other saved Hosts. Startup can then reconnect to the same Host
/// without asking the user to choose again.
fn promote_host(hosts: &mut Vec<HostRecord>, record: HostRecord) {
    hosts.retain(|host| host.id != record.id);
    hosts.insert(0, record);
}

fn default_store_path() -> Result<PathBuf> {
    crate::storage_paths::data_file(STORE_FILE)
}

fn default_vault_path() -> Result<PathBuf> {
    Ok(default_store_path()?
        .parent()
        .ok_or_else(|| anyhow!("host store has no parent directory"))?
        .join(VAULT_FILE))
}

#[derive(Default, Deserialize, Serialize)]
struct SecretVault {
    passwords: BTreeMap<String, String>,
}

fn vault_key_path(vault_path: &Path) -> Result<PathBuf> {
    let parent = vault_path
        .parent()
        .ok_or_else(|| anyhow!("password vault has no parent directory"))?;
    Ok(parent.join(VAULT_KEY_FILE))
}

fn vault_lock_path(vault_path: &Path) -> Result<PathBuf> {
    let parent = vault_path
        .parent()
        .ok_or_else(|| anyhow!("password vault has no parent directory"))?;
    Ok(parent.join(VAULT_LOCK_FILE))
}

fn with_encrypted_vault_at<T, F>(path: &Path, mutation: F) -> Result<T>
where
    F: FnOnce(&mut SecretVault) -> Result<T>,
{
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("password vault has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create password vault directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;

    let lock_path = vault_lock_path(path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("open password vault lock {}", lock_path.display()))?;
    set_mode(&lock_path, 0o600)?;
    lock.lock()
        .with_context(|| format!("lock password vault {}", lock_path.display()))?;

    let key_path = vault_key_path(path)?;
    let key = load_or_create_vault_key(&key_path)?;
    let mut vault = load_encrypted_vault(path, &key)?;
    let result = mutation(&mut vault)?;
    save_encrypted_vault(path, &key, &vault)?;
    Ok(result)
}

fn load_or_create_vault_key(path: &Path) -> Result<[u8; VAULT_KEY_LEN]> {
    match fs::read(path) {
        Ok(bytes) => {
            ensure_private_regular_file(path)?;
            let key: [u8; VAULT_KEY_LEN] = bytes
                .try_into()
                .map_err(|_| anyhow!("password vault key has an invalid length"))?;
            Ok(key)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut key = [0; VAULT_KEY_LEN];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| anyhow!("generate random password vault key"))?;
            write_private_atomic(path, &key)?;
            Ok(key)
        }
        Err(error) => {
            Err(error).with_context(|| format!("read password vault key {}", path.display()))
        }
    }
}

fn load_encrypted_vault(path: &Path, key: &[u8; VAULT_KEY_LEN]) -> Result<SecretVault> {
    let bytes = match fs::read(path) {
        Ok(bytes) => {
            ensure_private_regular_file(path)?;
            bytes
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SecretVault::default()),
        Err(error) => {
            return Err(error).with_context(|| format!("read password vault {}", path.display()));
        }
    };
    let plaintext = decrypt_vault(key, &bytes)?;
    serde_json::from_slice(&plaintext)
        .with_context(|| format!("decode encrypted password vault {}", path.display()))
}

fn save_encrypted_vault(path: &Path, key: &[u8; VAULT_KEY_LEN], vault: &SecretVault) -> Result<()> {
    let plaintext = serde_json::to_vec(vault).context("encode password vault")?;
    let ciphertext = encrypt_vault(key, &plaintext)?;
    write_private_atomic(path, &ciphertext)
}

fn encrypt_vault(key: &[u8; VAULT_KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce_bytes = [0; VAULT_NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| anyhow!("generate random password vault nonce"))?;

    let cipher_key = UnboundKey::new(&AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| anyhow!("initialize password vault encryption"))?;
    let mut ciphertext = plaintext.to_vec();
    cipher_key
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(VAULT_AAD),
            &mut ciphertext,
        )
        .map_err(|_| anyhow!("encrypt password vault"))?;

    let mut envelope = Vec::with_capacity(1 + VAULT_NONCE_LEN + ciphertext.len());
    envelope.push(VAULT_VERSION);
    envelope.extend_from_slice(&nonce_bytes);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn decrypt_vault(key: &[u8; VAULT_KEY_LEN], envelope: &[u8]) -> Result<Vec<u8>> {
    if envelope.len() < 1 + VAULT_NONCE_LEN + AES_256_GCM.tag_len() {
        return Err(anyhow!("password vault ciphertext is truncated"));
    }
    if envelope[0] != VAULT_VERSION {
        return Err(anyhow!(
            "unsupported password vault version {}",
            envelope[0]
        ));
    }
    let nonce_bytes: [u8; VAULT_NONCE_LEN] = envelope[1..1 + VAULT_NONCE_LEN]
        .try_into()
        .expect("nonce slice length is checked");
    let cipher_key = UnboundKey::new(&AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| anyhow!("initialize password vault decryption"))?;
    let mut ciphertext = envelope[1 + VAULT_NONCE_LEN..].to_vec();
    let plaintext = cipher_key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(VAULT_AAD),
            &mut ciphertext,
        )
        .map_err(|_| anyhow!("password vault authentication failed"))?;
    Ok(plaintext.to_vec())
}

fn ensure_private_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect private file {}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(anyhow!(
            "private file is not a regular file: {}",
            path.display()
        ));
    }
    set_mode(path, 0o600)
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("private file has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create private file directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;

    let temp_path = temporary_path(path);
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)
            .with_context(|| format!("create private temporary file {}", temp_path.display()))?;
        file.write_all(bytes).context("write private file")?;
        file.sync_all().context("sync private file")?;
        set_mode(&temp_path, 0o600)?;
        fs::rename(&temp_path, path)
            .with_context(|| format!("replace private file {}", path.display()))?;
        set_mode(path, 0o600)?;
        sync_directory(parent)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn set_file_password_at(path: &Path, account: &str, password: &[u8]) -> Result<()> {
    let password =
        String::from_utf8(password.to_vec()).context("host password is not valid UTF-8")?;
    with_encrypted_vault_at(path, |vault| {
        vault.passwords.insert(account.to_owned(), password);
        Ok(())
    })
}

fn file_password_at(path: &Path, account: &str) -> Result<Vec<u8>> {
    with_encrypted_vault_at(path, |vault| {
        vault
            .passwords
            .get(account)
            .cloned()
            .ok_or_else(|| anyhow!("password not found for host {account}"))
            .map(String::into_bytes)
    })
}

fn delete_file_password_at(path: &Path, account: &str) -> Result<()> {
    with_encrypted_vault_at(path, |vault| {
        vault.passwords.remove(account);
        Ok(())
    })
}

fn store_password(account: &str, password: &[u8]) -> Result<()> {
    set_file_password_at(&default_vault_path()?, account, password)
}

fn read_password(account: &str) -> Result<Vec<u8>> {
    file_password_at(&default_vault_path()?, account)
}

fn delete_password(account: &str) -> Result<()> {
    delete_file_password_at(&default_vault_path()?, account)
}

fn temporary_path(path: &Path) -> PathBuf {
    let suffix = format!("{}.{}.tmp", std::process::id(), Uuid::new_v4());
    path.with_file_name(format!(
        ".{}.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(STORE_FILE),
        suffix
    ))
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("set permissions {:o} on {}", mode, path.display()))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("sync directory {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_records_never_contain_password() {
        let record = HostRecord {
            id: "host-id".into(),
            name: "Mac mini".into(),
            node_id: Some("node-1".into()),
            addresses: vec!["127.0.0.1:7789".into()],
            last_seq: 4,
            device_id: "device-1".into(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("password"));
        assert!(json.contains("node_id"));
    }

    #[test]
    fn promoting_a_host_remembers_the_most_recent_connection_first() {
        let host = |id: &str| HostRecord {
            id: id.into(),
            name: id.into(),
            node_id: Some(format!("node-{id}")),
            addresses: vec![format!("{id}:7788")],
            last_seq: 0,
            device_id: "device-1".into(),
        };
        let mut hosts = vec![host("one"), host("two"), host("three")];

        promote_host(&mut hosts, host("two"));

        assert_eq!(
            hosts
                .iter()
                .map(|host| host.id.as_str())
                .collect::<Vec<_>>(),
            ["two", "one", "three"]
        );
        assert_eq!(hosts.iter().filter(|host| host.id == "two").count(), 1);
    }

    #[test]
    fn store_uses_private_mode_and_round_trips_metadata() {
        let root = std::env::temp_dir().join(format!("macbot-host-store-{}", Uuid::new_v4()));
        let path = root.join("hosts.json");
        let store = HostStore::new_at(&path);
        let record = HostRecord {
            id: "host-id".into(),
            name: "Mac mini".into(),
            node_id: None,
            addresses: vec!["127.0.0.1:7789".into()],
            last_seq: 9,
            device_id: store.device_id.clone(),
        };
        let store = HostStore {
            hosts: vec![record.clone()],
            ..store
        };
        store.save().unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let loaded = HostStore::load_from_path(&path).unwrap();
        assert_eq!(loaded.hosts(), &[record]);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&root);
    }

    #[test]
    fn locked_mutations_keep_records_from_concurrent_snapshots() {
        let root = std::env::temp_dir().join(format!("macbot-host-lock-{}", Uuid::new_v4()));
        let path = root.join("hosts.json");
        HostStore::new_at(&path).save().unwrap();
        // Keep an old snapshot alive while both mutations run.  The
        // with_store API must never use this stale value for its write.
        let stale_snapshot = HostStore::load_from_path(&path).unwrap();

        let first_path = path.clone();
        let first = std::thread::spawn(move || {
            HostStore::with_store_at(&first_path, |store| {
                std::thread::sleep(std::time::Duration::from_millis(25));
                let device_id = store.device_id.clone();
                store.hosts.push(HostRecord {
                    id: "host-a".into(),
                    name: "A".into(),
                    node_id: Some("node-a".into()),
                    addresses: vec!["127.0.0.1:7788".into()],
                    last_seq: 1,
                    device_id,
                });
                Ok(())
            })
        });
        let second_path = path.clone();
        let second = std::thread::spawn(move || {
            HostStore::with_store_at(&second_path, |store| {
                let device_id = store.device_id.clone();
                store.hosts.push(HostRecord {
                    id: "host-b".into(),
                    name: "B".into(),
                    node_id: Some("node-b".into()),
                    addresses: vec!["127.0.0.1:7789".into()],
                    last_seq: 2,
                    device_id,
                });
                Ok(())
            })
        });
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        let final_store = HostStore::load_from_path(&path).unwrap();
        assert_eq!(final_store.hosts().len(), 2);
        assert!(stale_snapshot.hosts().is_empty());
        assert!(final_store.get("host-a").is_some());
        assert!(final_store.get("host-b").is_some());
        let lock_path = root.join(LOCK_FILE);
        assert_eq!(
            fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn vault_ciphertext_round_trips_with_version_and_fresh_nonce() {
        let key = [9; VAULT_KEY_LEN];
        let plaintext = br#"{"passwords":{"host-id":"correct horse battery staple"}}"#;
        let first = encrypt_vault(&key, plaintext).unwrap();
        let second = encrypt_vault(&key, plaintext).unwrap();

        assert_eq!(first[0], VAULT_VERSION);
        assert_ne!(
            &first[1..1 + VAULT_NONCE_LEN],
            &second[1..1 + VAULT_NONCE_LEN]
        );
        assert_ne!(first, second);
        assert!(
            !first
                .windows(b"correct horse battery staple".len())
                .any(|window| window == b"correct horse battery staple")
        );
        assert_eq!(decrypt_vault(&key, &first).unwrap(), plaintext);
    }

    #[test]
    fn vault_rejects_tampering_wrong_key_and_unknown_version() {
        let key = [5; VAULT_KEY_LEN];
        let mut ciphertext = encrypt_vault(&key, b"secret").unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0x80;
        assert!(decrypt_vault(&key, &ciphertext).is_err());

        let ciphertext = encrypt_vault(&key, b"secret").unwrap();
        assert!(decrypt_vault(&[6; VAULT_KEY_LEN], &ciphertext).is_err());
        assert!(decrypt_vault(&key, &[0; 1 + VAULT_NONCE_LEN]).is_err());

        let mut future_version = encrypt_vault(&key, b"secret").unwrap();
        future_version[0] = VAULT_VERSION + 1;
        assert!(decrypt_vault(&key, &future_version).is_err());
    }

    #[test]
    fn encrypted_password_vault_round_trips_updates_and_deletes_privately() {
        let root = std::env::temp_dir().join(format!("macbot-secret-vault-{}", Uuid::new_v4()));
        let path = root.join(VAULT_FILE);
        let key_path = vault_key_path(&path).unwrap();
        let lock_path = vault_lock_path(&path).unwrap();

        set_file_password_at(&path, "host-id", b"first-password").unwrap();
        assert_eq!(
            file_password_at(&path, "host-id").unwrap(),
            b"first-password"
        );
        set_file_password_at(&path, "host-id", b"updated-password").unwrap();
        assert_eq!(
            file_password_at(&path, "host-id").unwrap(),
            b"updated-password"
        );

        assert_eq!(path.file_name().unwrap(), VAULT_FILE);
        assert_eq!(key_path, root.join(VAULT_KEY_FILE));
        assert_eq!(lock_path, root.join(VAULT_LOCK_FILE));
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for private_path in [&path, &key_path, &lock_path] {
            assert_eq!(
                fs::metadata(private_path).unwrap().permissions().mode() & 0o777,
                0o600,
                "{} must be private",
                private_path.display()
            );
        }
        let ciphertext = fs::read(&path).unwrap();
        assert_eq!(ciphertext[0], VAULT_VERSION);
        assert!(
            !ciphertext
                .windows(b"updated-password".len())
                .any(|window| window == b"updated-password")
        );
        assert_eq!(fs::read(&key_path).unwrap().len(), VAULT_KEY_LEN);

        delete_file_password_at(&path, "host-id").unwrap();
        assert!(file_password_at(&path, "host-id").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_existing_vault_key_is_not_replaced() {
        let root = std::env::temp_dir().join(format!("macbot-secret-key-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let key_path = root.join(VAULT_KEY_FILE);
        fs::write(&key_path, [7; 4]).unwrap();

        assert!(load_or_create_vault_key(&key_path).is_err());
        assert_eq!(fs::read(&key_path).unwrap(), [7; 4]);
        let _ = fs::remove_dir_all(&root);
    }
}
