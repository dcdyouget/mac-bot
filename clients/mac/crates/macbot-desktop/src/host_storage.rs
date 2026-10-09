//! Persistent multi-host connection metadata for the macOS client.
//!
//! Host metadata is kept in `~/Library/Application Support/MacBot/hosts.json`.
//! Passwords are deliberately kept out of that file and stored in the macOS
//! login keychain as generic passwords. A keychain failure is returned to the
//! caller; there is no implicit plaintext fallback. Developers may explicitly
//! opt into the private, 0600 file backend with `MACBOT_SECRET_BACKEND=file`.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use core_foundation::{
    base::{CFGetTypeID, CFRelease, CFType, CFTypeRef, TCFType},
    data::CFData,
    dictionary::{CFDictionary, CFMutableDictionary},
    string::{CFString, CFStringRef},
};
use security_framework::base::Error as SecurityError;
use security_framework_sys::{
    base::{errSecDuplicateItem, errSecItemNotFound, errSecSuccess},
    item::{
        kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecReturnData,
        kSecUseAuthenticationUI, kSecValueData,
    },
    keychain_item::{SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecUseAuthenticationUIFail: CFStringRef;
}

const KEYCHAIN_SERVICE: &str = "bot.mac.desktop.host-password";
const SECRET_BACKEND_ENV: &str = "MACBOT_SECRET_BACKEND";
const DEVELOPMENT_SECRET_FILE: &str = "development-secrets.json";
const STORE_DIR: &str = "Library/Application Support/MacBot";
const STORE_FILE: &str = "hosts.json";

/// Non-secret information needed to reconnect to one Mac Bot host.
///
/// `id` is the local stable identifier used as the keychain account. The
/// server's `node_id` may be absent before the first successful handshake.
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

    /// Remember or update a host, storing its password in the login keychain.
    ///
    /// Existing records are matched by server `node_id`, then by their first
    /// address. Password storage succeeds before metadata is written. If the
    /// keychain rejects the operation, the JSON file is left untouched.
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

        // The password never enters HostRecord or the serialized buffer.
        store_password(&id, password.as_bytes())
            .with_context(|| format!("store password for host {id}"))?;

        let record = HostRecord {
            id: id.clone(),
            name,
            node_id,
            addresses,
            last_seq,
            device_id: self.device_id.clone(),
        };
        let old = index.map(|index| self.hosts[index].clone());
        match index {
            Some(index) => self.hosts[index] = record.clone(),
            None => self.hosts.push(record.clone()),
        }
        if let Err(error) = self.save() {
            match old {
                Some(previous) => self.hosts[index.expect("existing index")] = previous,
                None => {
                    self.hosts.retain(|host| host.id != id);
                }
            }
            return Err(error).context("persist host metadata after keychain update");
        }
        Ok(record)
    }

    /// Read the password for a record from the macOS login keychain.
    pub fn password(&self, record: &HostRecord) -> Result<String> {
        let password = read_password(&record.id)
            .with_context(|| format!("read password for host {}", record.id))?;
        String::from_utf8(password).context("host password is not valid UTF-8")
    }

    /// Remove metadata and its keychain password. Missing records are a no-op.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let Some(index) = self.hosts.iter().position(|host| host.id == id) else {
            return Ok(());
        };
        delete_password(id)?;
        let removed = self.hosts.remove(index);
        if let Err(error) = self.save() {
            self.hosts.insert(index, removed);
            return Err(error).context("persist host metadata after keychain removal");
        }
        Ok(())
    }
}

fn default_store_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(STORE_DIR).join(STORE_FILE))
}

fn default_secret_path() -> Result<PathBuf> {
    Ok(default_store_path()?
        .parent()
        .ok_or_else(|| anyhow!("host store has no parent directory"))?
        .join(DEVELOPMENT_SECRET_FILE))
}

fn file_backend_enabled() -> bool {
    file_backend_requested(std::env::var(SECRET_BACKEND_ENV).ok().as_deref())
}

fn file_backend_requested(value: Option<&str>) -> bool {
    value == Some("file")
}

#[derive(Default, Deserialize, Serialize)]
struct DevelopmentSecrets {
    passwords: BTreeMap<String, String>,
}

fn load_development_secrets(path: &Path) -> Result<DevelopmentSecrets> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("decode development secrets {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DevelopmentSecrets::default()),
        Err(error) => {
            Err(error).with_context(|| format!("read development secrets {}", path.display()))
        }
    }
}

fn save_development_secrets(path: &Path, secrets: &DevelopmentSecrets) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("development secrets has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create development secrets directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;

    let temp_path = temporary_path(path);
    let bytes = serde_json::to_vec_pretty(secrets).context("encode development secrets")?;
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp_path)
            .with_context(|| {
                format!(
                    "create temporary development secrets {}",
                    temp_path.display()
                )
            })?;
        file.write_all(&bytes)
            .context("write development secrets")?;
        file.sync_all().context("sync development secrets")?;
        set_mode(&temp_path, 0o600)?;
        fs::rename(&temp_path, path)
            .with_context(|| format!("replace development secrets {}", path.display()))?;
        set_mode(path, 0o600)?;
        sync_directory(parent)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn set_file_password_at(path: &Path, account: &str, password: &[u8]) -> Result<()> {
    let mut secrets = load_development_secrets(path)?;
    let password =
        String::from_utf8(password.to_vec()).context("development password is not valid UTF-8")?;
    secrets.passwords.insert(account.to_owned(), password);
    save_development_secrets(path, &secrets)
}

fn file_password_at(path: &Path, account: &str) -> Result<Vec<u8>> {
    let secrets = load_development_secrets(path)?;
    secrets
        .passwords
        .get(account)
        .cloned()
        .ok_or_else(|| anyhow!("development password not found for host {account}"))
        .map(String::into_bytes)
}

fn delete_file_password_at(path: &Path, account: &str) -> Result<()> {
    let mut secrets = load_development_secrets(path)?;
    if secrets.passwords.remove(account).is_some() {
        save_development_secrets(path, &secrets)?;
    }
    Ok(())
}

fn store_password(account: &str, password: &[u8]) -> Result<()> {
    if file_backend_enabled() {
        set_file_password_at(&default_secret_path()?, account, password)
    } else {
        set_keychain_password(account, password)
    }
}

fn read_password(account: &str) -> Result<Vec<u8>> {
    if file_backend_enabled() {
        file_password_at(&default_secret_path()?, account)
    } else {
        keychain_password(account)
    }
}

fn delete_password(account: &str) -> Result<()> {
    if file_backend_enabled() {
        delete_file_password_at(&default_secret_path()?, account)
    } else {
        delete_keychain_password(account)
    }
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

fn set_keychain_password(account: &str, password: &[u8]) -> Result<()> {
    // Use the modern SecItem APIs directly. The authentication UI value is
    // deliberately noninteractive: a locked/protected item returns an error instead
    // of synchronously presenting a Keychain dialog on the GPUI thread.
    let add = keychain_attributes(account, Some(password), false);
    let status = unsafe { SecItemAdd(add.as_concrete_TypeRef(), std::ptr::null_mut()) };
    if status == errSecDuplicateItem {
        let search = keychain_attributes(account, None, false);
        let update = keychain_update(password);
        let status =
            unsafe { SecItemUpdate(search.as_concrete_TypeRef(), update.as_concrete_TypeRef()) };
        security_status(status)
    } else {
        security_status(status)
    }
}

fn keychain_password(account: &str) -> Result<Vec<u8>> {
    let query = keychain_attributes(account, None, true);
    let mut result: CFTypeRef = std::ptr::null();
    security_status(unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) })?;
    if result.is_null() {
        return Err(anyhow!("Keychain returned no password data"));
    }
    if unsafe { CFGetTypeID(result) } != CFData::type_id() {
        unsafe {
            CFRelease(result);
        }
        return Err(anyhow!("Keychain returned a non-data password item"));
    }
    let data = unsafe { CFData::wrap_under_create_rule(result as _) };
    Ok(data.bytes().to_vec())
}

fn keychain_update(password: &[u8]) -> CFDictionary<CFType, CFType> {
    let mut update = CFMutableDictionary::<CFType, CFType>::from_CFType_pairs(&[]);
    let key = unsafe { CFString::wrap_under_get_rule(kSecValueData) }.into_CFType();
    let value = CFData::from_buffer(password).into_CFType();
    update.add(&key, &value);
    update.to_immutable()
}

fn delete_keychain_password(account: &str) -> Result<()> {
    let query = keychain_attributes(account, None, false);
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
    if status == errSecItemNotFound {
        Ok(())
    } else {
        security_status(status)
    }
}

fn keychain_attributes(
    account: &str,
    password: Option<&[u8]>,
    return_data: bool,
) -> CFDictionary<CFType, CFType> {
    let mut query = CFMutableDictionary::<CFType, CFType>::from_CFType_pairs(&[]);
    unsafe {
        add_static_string(&mut query, kSecClass, kSecClassGenericPassword);
        add_string(&mut query, kSecAttrService, KEYCHAIN_SERVICE);
    }
    if !account.is_empty() {
        unsafe {
            add_string(&mut query, kSecAttrAccount, account);
        }
    }
    if let Some(password) = password {
        let key = unsafe { CFString::wrap_under_get_rule(kSecValueData) }.into_CFType();
        let value = CFData::from_buffer(password).into_CFType();
        query.add(&key, &value);
    }
    if return_data {
        unsafe {
            add_static_boolean_true(&mut query, kSecReturnData);
        }
    }
    // Bind Apple's exported constant, which is absent from security-framework-sys 2.17.
    unsafe {
        add_static_string(
            &mut query,
            kSecUseAuthenticationUI,
            kSecUseAuthenticationUIFail,
        );
    }
    query.to_immutable()
}

fn add_static_string(
    query: &mut CFMutableDictionary<CFType, CFType>,
    key_ref: CFStringRef,
    value_ref: CFStringRef,
) {
    let key = unsafe { CFString::wrap_under_get_rule(key_ref) }.into_CFType();
    let value = unsafe { CFString::wrap_under_get_rule(value_ref) }.into_CFType();
    query.add(&key, &value);
}

fn add_string(query: &mut CFMutableDictionary<CFType, CFType>, key_ref: CFStringRef, value: &str) {
    let key = unsafe { CFString::wrap_under_get_rule(key_ref) }.into_CFType();
    let value = CFString::from(value).into_CFType();
    query.add(&key, &value);
}

fn add_static_boolean_true(query: &mut CFMutableDictionary<CFType, CFType>, key_ref: CFStringRef) {
    let key = unsafe { CFString::wrap_under_get_rule(key_ref) }.into_CFType();
    let value = core_foundation::boolean::CFBoolean::true_value().into_CFType();
    query.add(&key, &value);
}

fn security_status(status: i32) -> Result<()> {
    if status == errSecSuccess {
        Ok(())
    } else {
        Err(SecurityError::from_code(status).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_queries_disable_authentication_ui() {
        let query = keychain_attributes("host-test", None, false);
        let key = unsafe { CFString::wrap_under_get_rule(kSecUseAuthenticationUI) }.into_CFType();
        let expected =
            unsafe { CFString::wrap_under_get_rule(kSecUseAuthenticationUIFail) }.into_CFType();
        assert_eq!(*query.find(&key).unwrap(), expected);
        let return_key = unsafe { CFString::wrap_under_get_rule(kSecReturnData) }.into_CFType();
        assert!(query.find(&return_key).is_none());
    }

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
    fn development_file_backend_round_trips_with_private_permissions() {
        let root = std::env::temp_dir().join(format!("macbot-dev-secrets-{}", Uuid::new_v4()));
        let path = root.join("development-secrets.json");

        set_file_password_at(&path, "host-id", b"dev-password").unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(file_password_at(&path, "host-id").unwrap(), b"dev-password");

        set_file_password_at(&path, "host-id", b"updated-password").unwrap();
        assert_eq!(
            file_password_at(&path, "host-id").unwrap(),
            b"updated-password"
        );
        delete_file_password_at(&path, "host-id").unwrap();
        assert!(file_password_at(&path, "host-id").is_err());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&root);
    }

    #[test]
    fn file_backend_requires_explicit_development_switch() {
        assert!(file_backend_requested(Some("file")));
        assert!(!file_backend_requested(None));
        assert!(!file_backend_requested(Some("keychain")));
        assert!(!file_backend_requested(Some("FILE")));
    }
}
