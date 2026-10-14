//! Project-owned encrypted credential storage.
//!
//! Credentials are stored under the configured MacBot data home, encrypted
//! with a local 256-bit key. Both files are private to the service account;
//! credential values never enter provider snapshots, RPC replies, or logs.

use crate::{Error, Result, SecretStore};
use ring::{aead, rand::SecureRandom};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

const MASTER_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const FILE_HEADER: &[u8; 4] = b"MCB1";
const KEY_FILE: &str = ".provider-secrets-key";

pub struct FileSecrets {
    directory: PathBuf,
    key: [u8; MASTER_KEY_BYTES],
}

fn storage_error(_: std::io::Error) -> Error {
    // OS errors can include paths or other environment details. Keep them out
    // of client replies and service logs.
    Error::Secret("encrypted credential storage operation failed".into())
}

fn crypto_error() -> Error {
    Error::Secret("encrypted credential could not be opened".into())
}

impl FileSecrets {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        let absolute = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()
                .map_err(storage_error)?
                .join(directory)
        };
        fs::create_dir_all(&absolute).map_err(storage_error)?;
        let metadata = fs::symlink_metadata(&absolute).map_err(storage_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Secret(
                "credential directory must be a real directory".into(),
            ));
        }
        let directory = fs::canonicalize(&absolute).map_err(storage_error)?;
        if directory
            .ancestors()
            .any(|parent| parent.join(".git").exists())
        {
            return Err(Error::Secret(
                "credentials must be stored outside a Git checkout".into(),
            ));
        }
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(storage_error)?;
        let key = load_or_create_master_key(&directory)?;
        Ok(Self { directory, key })
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || id.len() > 256 {
            return Err(Error::Secret("invalid credential identifier".into()));
        }
        // Hex encoding is reversible and collision-free, while keeping an
        // identifier from becoming a path component.
        let name = id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(self.directory.join(format!("{name}.enc")))
    }

    fn cipher(&self) -> Result<aead::LessSafeKey> {
        let key =
            aead::UnboundKey::new(&aead::AES_256_GCM, &self.key).map_err(|_| crypto_error())?;
        Ok(aead::LessSafeKey::new(key))
    }
}

impl SecretStore for FileSecrets {
    fn get(&self, id: &str) -> Result<Option<String>> {
        let path = self.path(id)?;
        let encrypted = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                #[cfg(unix)]
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                    .map_err(storage_error)?;
                fs::read(path).map_err(storage_error)?
            }
            Ok(_) => {
                return Err(Error::Secret(
                    "credential record must be a regular file".into(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        if encrypted.len() < FILE_HEADER.len() + NONCE_BYTES + aead::AES_256_GCM.tag_len()
            || &encrypted[..FILE_HEADER.len()] != FILE_HEADER
        {
            return Err(crypto_error());
        }
        let nonce_start = FILE_HEADER.len();
        let nonce_end = nonce_start + NONCE_BYTES;
        let nonce = aead::Nonce::try_assume_unique_for_key(&encrypted[nonce_start..nonce_end])
            .map_err(|_| crypto_error())?;
        let mut plaintext = encrypted[nonce_end..].to_vec();
        let opened = self
            .cipher()?
            .open_in_place(nonce, aead::Aad::from(id.as_bytes()), &mut plaintext)
            .map_err(|_| crypto_error())?;
        String::from_utf8(opened.to_vec())
            .map(Some)
            .map_err(|_| crypto_error())
    }

    fn set(&self, id: &str, secret: &str) -> Result<()> {
        let path = self.path(id)?;
        let mut nonce_bytes = [0u8; NONCE_BYTES];
        ring::rand::SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| crypto_error())?;
        let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);
        let mut ciphertext = secret.as_bytes().to_vec();
        ciphertext.reserve(aead::AES_256_GCM.tag_len());
        self.cipher()?
            .seal_in_place_append_tag(nonce, aead::Aad::from(id.as_bytes()), &mut ciphertext)
            .map_err(|_| crypto_error())?;
        let mut record = Vec::with_capacity(FILE_HEADER.len() + NONCE_BYTES + ciphertext.len());
        record.extend_from_slice(FILE_HEADER);
        record.extend_from_slice(&nonce_bytes);
        record.extend_from_slice(&ciphertext);
        write_atomic(&self.directory, &path, &record)
    }

    fn delete(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.path(id)?) {
            Ok(()) => sync_directory(&self.directory),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage_error(error)),
        }
    }
}

fn load_or_create_master_key(directory: &Path) -> Result<[u8; MASTER_KEY_BYTES]> {
    let path = directory.join(KEY_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            #[cfg(unix)]
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(storage_error)?;
            let bytes = fs::read(path).map_err(storage_error)?;
            bytes.try_into().map_err(|_| crypto_error())
        }
        Ok(_) => Err(Error::Secret(
            "credential key must be a regular file".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut key = [0u8; MASTER_KEY_BYTES];
            ring::rand::SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| crypto_error())?;
            let temporary = directory.join(format!(".provider-key-{}.tmp", uuid::Uuid::now_v7()));
            let result = (|| {
                write_new_private(&temporary, &key)?;
                match fs::hard_link(&temporary, &path) {
                    Ok(()) => sync_directory(directory)?,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(storage_error(error)),
                }
                Ok(())
            })();
            let _ = fs::remove_file(&temporary);
            result?;
            load_or_create_master_key(directory)
        }
        Err(error) => Err(storage_error(error)),
    }
}

fn write_new_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(storage_error)?;
    file.write_all(bytes).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)
}

fn write_atomic(directory: &Path, destination: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = directory.join(format!(".provider-secret-{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        write_new_private(&temporary, bytes)?;
        fs::rename(&temporary, destination).map_err(storage_error)?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_directory(directory: &Path) -> Result<()> {
    fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_error)
}

pub fn configured_secret_store(home: impl AsRef<Path>) -> Result<Arc<dyn SecretStore>> {
    let home = home.as_ref();
    let directory = match std::env::var("MACBOT_SECRET_BACKEND").as_deref() {
        Ok("file") | Err(std::env::VarError::NotPresent) => {
            match std::env::var_os("MACBOT_SECRET_DIR") {
                Some(directory) => PathBuf::from(directory),
                None => home.join("data/provider-secrets"),
            }
        }
        Ok("keychain") => {
            return Err(Error::Secret(
                "Keychain credential storage is no longer supported".into(),
            ))
        }
        _ => return Err(Error::Secret("unsupported MACBOT_SECRET_BACKEND".into())),
    };
    Ok(Arc::new(FileSecrets::new(directory)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn encrypted_files_survive_reopen_and_replace_without_leaking_plaintext() {
        let directory = tempdir().unwrap();
        let store = FileSecrets::new(directory.path().join("credentials")).unwrap();
        let id = "../provider/namespaced";
        let secret = "fake-provider-token-do-not-persist-in-plaintext";
        assert_eq!(store.get(id).unwrap(), None);
        store.set(id, secret).unwrap();
        store.set(id, "replacement-fake-token").unwrap();
        let reopened = FileSecrets::new(directory.path().join("credentials")).unwrap();
        assert_eq!(
            reopened.get(id).unwrap().as_deref(),
            Some("replacement-fake-token")
        );
        let files = fs::read_dir(directory.path().join("credentials"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|path| {
            !fs::read(path)
                .unwrap()
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
        }));
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&reopened.directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert!(files
                .iter()
                .all(|path| { fs::metadata(path).unwrap().permissions().mode() & 0o777 == 0o600 }));
        }
        reopened.delete(id).unwrap();
        reopened.delete(id).unwrap();
        assert_eq!(store.get(id).unwrap(), None);
    }

    #[test]
    fn encrypted_records_bind_the_provider_id_and_reject_tampering() {
        let directory = tempdir().unwrap();
        let store = FileSecrets::new(directory.path().join("credentials")).unwrap();
        store.set("provider-a", "secret-a").unwrap();
        let path = store.path("provider-a").unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::write(store.path("provider-b").unwrap(), &bytes).unwrap();
        assert!(store.get("provider-b").is_err());
        let mut bytes = bytes;
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(store.get("provider-a").is_err());
    }

    #[test]
    fn encrypted_store_rejects_checkouts_and_symlink_reads() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join(".git"), "gitdir: fake").unwrap();
        assert!(FileSecrets::new(directory.path().join("keys")).is_err());
        fs::remove_file(directory.path().join(".git")).unwrap();
        let store = FileSecrets::new(directory.path().join("keys")).unwrap();
        #[cfg(unix)]
        {
            let target = directory.path().join("outside");
            fs::write(&target, b"not an encrypted credential").unwrap();
            std::os::unix::fs::symlink(&target, store.path("p").unwrap()).unwrap();
            assert!(store.get("p").is_err());
            store.set("p", "replacement").unwrap();
            assert_eq!(fs::read(target).unwrap(), b"not an encrypted credential");
        }
    }
}
